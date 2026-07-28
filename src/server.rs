use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::config::{Config, EndpointConfig};
use crate::protocol::{decode_binding_request, encode_binding_response};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EndpointSlot {
    ip: usize,
    port: usize,
}

impl EndpointSlot {
    const PRIMARY: Self = Self { ip: 0, port: 0 };
    const ALTERNATE_PORT: Self = Self { ip: 0, port: 1 };
    const ALTERNATE_IP: Self = Self { ip: 1, port: 0 };
    const ALTERNATE_IP_PORT: Self = Self { ip: 1, port: 1 };
}

#[derive(Debug)]
struct BoundEndpoint {
    socket: Arc<UdpSocket>,
    advertise: SocketAddr,
    slot: EndpointSlot,
}

#[derive(Debug, Default)]
struct Stats {
    udp_requests: AtomicU64,
    udp_responses: AtomicU64,
    tcp_requests: AtomicU64,
    tcp_responses: AtomicU64,
    invalid_packets: AtomicU64,
    io_errors: AtomicU64,
}

pub struct Server {
    config: Config,
}

impl Server {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    pub async fn run(self) -> Result<()> {
        let stats = Arc::new(Stats::default());
        let endpoints = Arc::new(bind_udp_endpoints(&self.config).await?);
        let mut tasks = JoinSet::new();

        for endpoint in endpoints.iter() {
            let socket = endpoint.socket.clone();
            let slot = endpoint.slot;
            let endpoints = endpoints.clone();
            let stats = stats.clone();
            let software = self.config.server.software.clone();
            let max_packet_size = self.config.server.max_packet_size;
            tasks.spawn(async move {
                udp_loop(socket, slot, endpoints, stats, software, max_packet_size).await
            });
        }

        for bind_addr in &self.config.tcp.bind {
            let listener = TcpListener::bind(bind_addr)
                .await
                .with_context(|| format!("failed to bind TCP endpoint {bind_addr}"))?;
            info!(address = %listener.local_addr()?, "TCP STUN endpoint listening");
            let stats = stats.clone();
            let software = self.config.server.software.clone();
            let max_packet_size = self.config.server.max_packet_size;
            tasks.spawn(async move { tcp_loop(listener, stats, software, max_packet_size).await });
        }

        if self.config.server.stats_interval_seconds > 0 {
            let stats = stats.clone();
            let interval = Duration::from_secs(self.config.server.stats_interval_seconds);
            tasks.spawn(async move {
                stats_loop(stats, interval).await;
                Ok(())
            });
        }

        if !self.config.has_alternate_ip() {
            warn!(
                "single-IP mode: address discovery and change-port work, but EasyTier change-IP NAT classification requires two public IP addresses"
            );
        } else if !self.config.has_complete_endpoint_matrix() {
            info!(
                "EasyTier three-endpoint mode: current EasyTier NAT classification is supported; change-IP-only requests fall back to the receiving endpoint"
            );
        }

        tokio::select! {
            result = wait_for_task_failure(&mut tasks) => result,
            signal = shutdown_signal() => {
                signal?;
                info!("shutdown requested");
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                Ok(())
            }
        }
    }
}

async fn bind_udp_endpoints(config: &Config) -> Result<Vec<BoundEndpoint>> {
    let configured = [
        (EndpointSlot::PRIMARY, Some(&config.udp.primary)),
        (
            EndpointSlot::ALTERNATE_PORT,
            Some(&config.udp.alternate_port),
        ),
        (EndpointSlot::ALTERNATE_IP, config.udp.alternate_ip.as_ref()),
        (
            EndpointSlot::ALTERNATE_IP_PORT,
            config.udp.alternate_ip_port.as_ref(),
        ),
    ];
    let mut endpoints = Vec::with_capacity(4);
    for (slot, endpoint) in configured {
        if let Some(endpoint) = endpoint {
            endpoints.push(bind_udp_endpoint(slot, endpoint).await?);
        }
    }
    Ok(endpoints)
}

async fn bind_udp_endpoint(slot: EndpointSlot, config: &EndpointConfig) -> Result<BoundEndpoint> {
    let socket = UdpSocket::bind(config.bind)
        .await
        .with_context(|| format!("failed to bind UDP endpoint {}", config.bind))?;
    let local_addr = socket.local_addr()?;
    let advertise = config.advertise.unwrap_or(local_addr);
    info!(
        bind = %local_addr,
        advertise = %advertise,
        ip_group = slot.ip,
        port_group = slot.port,
        "UDP STUN endpoint listening"
    );
    Ok(BoundEndpoint {
        socket: Arc::new(socket),
        advertise,
        slot,
    })
}

async fn udp_loop(
    socket: Arc<UdpSocket>,
    current_slot: EndpointSlot,
    endpoints: Arc<Vec<BoundEndpoint>>,
    stats: Arc<Stats>,
    software: String,
    max_packet_size: usize,
) -> Result<()> {
    let mut buffer = vec![0_u8; max_packet_size];
    loop {
        let (length, peer_addr) = socket.recv_from(&mut buffer).await?;
        stats.udp_requests.fetch_add(1, Ordering::Relaxed);
        let request = match decode_binding_request(&buffer[..length]) {
            Ok(Some(request)) => request,
            Ok(None) => {
                stats.invalid_packets.fetch_add(1, Ordering::Relaxed);
                debug!(%peer_addr, "ignored non-Binding STUN packet");
                continue;
            }
            Err(error) => {
                stats.invalid_packets.fetch_add(1, Ordering::Relaxed);
                debug!(%peer_addr, %error, "ignored malformed STUN packet");
                continue;
            }
        };

        let requested_slot =
            requested_response_slot(current_slot, request.change_ip, request.change_port);
        let response_endpoint = endpoint_for_slot(&endpoints, requested_slot)
            .or_else(|| {
                endpoint_for_slot(
                    &endpoints,
                    EndpointSlot {
                        ip: current_slot.ip,
                        port: current_slot.port ^ usize::from(request.change_port),
                    },
                )
            })
            .unwrap_or_else(|| endpoint_for_slot(&endpoints, current_slot).unwrap());
        let has_second_ip = endpoint_for_slot(&endpoints, EndpointSlot::ALTERNATE_IP).is_some()
            || endpoint_for_slot(&endpoints, EndpointSlot::ALTERNATE_IP_PORT).is_some();
        let other_endpoint = endpoint_for_slot(
            &endpoints,
            EndpointSlot {
                ip: current_slot.ip ^ usize::from(has_second_ip),
                port: current_slot.port ^ 1,
            },
        )
        .or_else(|| {
            endpoint_for_slot(
                &endpoints,
                EndpointSlot {
                    ip: current_slot.ip,
                    port: current_slot.port ^ 1,
                },
            )
        })
        .unwrap_or(response_endpoint);

        let response = match encode_binding_response(
            &request,
            peer_addr,
            response_endpoint.advertise,
            other_endpoint.advertise,
            &software,
        ) {
            Ok(response) => response,
            Err(error) => {
                stats.invalid_packets.fetch_add(1, Ordering::Relaxed);
                warn!(%peer_addr, %error, "failed to encode STUN response");
                continue;
            }
        };

        match response_endpoint.socket.send_to(&response, peer_addr).await {
            Ok(_) => {
                stats.udp_responses.fetch_add(1, Ordering::Relaxed);
                debug!(
                    %peer_addr,
                    origin = %response_endpoint.advertise,
                    other = %other_endpoint.advertise,
                    change_ip = request.change_ip,
                    change_port = request.change_port,
                    "answered UDP Binding request"
                );
            }
            Err(error) => {
                stats.io_errors.fetch_add(1, Ordering::Relaxed);
                warn!(%peer_addr, %error, "failed to send UDP STUN response");
            }
        }
    }
}

fn endpoint_for_slot(endpoints: &[BoundEndpoint], slot: EndpointSlot) -> Option<&BoundEndpoint> {
    endpoints.iter().find(|endpoint| endpoint.slot == slot)
}

fn requested_response_slot(
    current_slot: EndpointSlot,
    change_ip: bool,
    change_port: bool,
) -> EndpointSlot {
    EndpointSlot {
        ip: current_slot.ip ^ usize::from(change_ip),
        port: current_slot.port ^ usize::from(change_port),
    }
}

async fn tcp_loop(
    listener: TcpListener,
    stats: Arc<Stats>,
    software: String,
    max_packet_size: usize,
) -> Result<()> {
    loop {
        let (stream, peer_addr) = listener.accept().await.context("TCP accept failed")?;
        let stats = stats.clone();
        let software = software.clone();
        tokio::spawn(async move {
            if let Err(error) =
                handle_tcp_connection(stream, peer_addr, stats.clone(), software, max_packet_size)
                    .await
            {
                stats.io_errors.fetch_add(1, Ordering::Relaxed);
                debug!(%peer_addr, %error, "TCP STUN connection closed with error");
            }
        });
    }
}

async fn handle_tcp_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    stats: Arc<Stats>,
    software: String,
    max_packet_size: usize,
) -> Result<()> {
    let local_addr = stream.local_addr()?;
    loop {
        let mut header = [0_u8; 20];
        match stream.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error).context("failed to read TCP STUN header"),
        }
        let total_length = 20 + u16::from_be_bytes([header[2], header[3]]) as usize;
        if total_length > max_packet_size {
            bail!("TCP STUN message exceeds configured packet limit");
        }
        let mut packet = vec![0_u8; total_length];
        packet[..20].copy_from_slice(&header);
        stream
            .read_exact(&mut packet[20..])
            .await
            .context("failed to read TCP STUN body")?;
        stats.tcp_requests.fetch_add(1, Ordering::Relaxed);

        let Some(request) = decode_binding_request(&packet).context("invalid TCP STUN message")?
        else {
            stats.invalid_packets.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let response =
            encode_binding_response(&request, peer_addr, local_addr, local_addr, &software)
                .context("failed to encode TCP STUN response")?;
        stream.write_all(&response).await?;
        stats.tcp_responses.fetch_add(1, Ordering::Relaxed);
    }
}

async fn stats_loop(stats: Arc<Stats>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        info!(
            udp_requests = stats.udp_requests.load(Ordering::Relaxed),
            udp_responses = stats.udp_responses.load(Ordering::Relaxed),
            tcp_requests = stats.tcp_requests.load(Ordering::Relaxed),
            tcp_responses = stats.tcp_responses.load(Ordering::Relaxed),
            invalid_packets = stats.invalid_packets.load(Ordering::Relaxed),
            io_errors = stats.io_errors.load(Ordering::Relaxed),
            "STUN server statistics"
        );
    }
}

async fn wait_for_task_failure(tasks: &mut JoinSet<Result<()>>) -> Result<()> {
    match tasks.join_next().await {
        Some(Ok(Ok(()))) => bail!("server task stopped unexpectedly"),
        Some(Ok(Err(error))) => Err(error),
        Some(Err(error)) => Err(error).context("server task panicked"),
        None => bail!("no server tasks are running"),
    }
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easytier_change_ip_and_port_selects_third_endpoint() {
        assert_eq!(
            requested_response_slot(EndpointSlot::PRIMARY, true, true),
            EndpointSlot::ALTERNATE_IP_PORT
        );
    }

    #[test]
    fn easytier_change_port_selects_second_endpoint() {
        assert_eq!(
            requested_response_slot(EndpointSlot::PRIMARY, false, true),
            EndpointSlot::ALTERNATE_PORT
        );
    }
}
