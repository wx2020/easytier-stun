use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub udp: UdpConfig,
    #[serde(default)]
    pub tcp: TcpConfig,
    #[serde(default)]
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UdpConfig {
    pub primary: EndpointConfig,
    pub alternate_port: EndpointConfig,
    pub alternate_ip: Option<EndpointConfig>,
    pub alternate_ip_port: Option<EndpointConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    pub bind: SocketAddr,
    pub advertise: Option<SocketAddr>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TcpConfig {
    pub bind: Vec<SocketAddr>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub software: String,
    pub max_packet_size: usize,
    pub stats_interval_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            software: format!("easytier-stun/{}", env!("CARGO_PKG_VERSION")),
            max_packet_size: 2048,
            stats_interval_seconds: 60,
        }
    }
}

impl Config {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !(20..=u16::MAX as usize + 20).contains(&self.server.max_packet_size) {
            bail!("server.max_packet_size must be between 20 and 65555");
        }
        if self.udp.alternate_ip.is_some() && self.udp.alternate_ip_port.is_none() {
            bail!("udp.alternate_ip requires udp.alternate_ip_port");
        }

        let endpoints = [
            Some(&self.udp.primary),
            Some(&self.udp.alternate_port),
            self.udp.alternate_ip.as_ref(),
            self.udp.alternate_ip_port.as_ref(),
        ];
        let binds = endpoints
            .into_iter()
            .flatten()
            .map(|endpoint| endpoint.bind)
            .collect::<Vec<_>>();
        for (index, addr) in binds.iter().enumerate() {
            if binds[..index].contains(addr) {
                bail!("duplicate UDP bind address: {addr}");
            }
        }
        Ok(())
    }

    pub fn has_alternate_ip(&self) -> bool {
        self.udp.alternate_ip.is_some() || self.udp.alternate_ip_port.is_some()
    }

    pub fn has_complete_endpoint_matrix(&self) -> bool {
        self.udp.alternate_ip.is_some() && self.udp.alternate_ip_port.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_half_configured_alternate_ip() {
        let config: Config = toml::from_str(
            r#"
            [udp.primary]
            bind = "127.0.0.1:3478"

            [udp.alternate_port]
            bind = "127.0.0.1:3479"

            [udp.alternate_ip]
            bind = "127.0.0.2:3478"
            "#,
        )
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn accepts_easytier_three_endpoint_mode() {
        let config: Config = toml::from_str(
            r#"
            [udp.primary]
            bind = "127.0.0.1:3478"

            [udp.alternate_port]
            bind = "127.0.0.1:3479"

            [udp.alternate_ip_port]
            bind = "127.0.0.2:3479"
            "#,
        )
        .unwrap();

        assert!(config.validate().is_ok());
        assert!(config.has_alternate_ip());
        assert!(!config.has_complete_endpoint_matrix());
    }
}
