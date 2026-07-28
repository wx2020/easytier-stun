# EasyTier STUN

An independently deployable STUN server built with the same core stack used by
EasyTier: Rust, Tokio, `stun_codec`, `bytecodec`, Clap, and Tracing.

It implements the behavior consumed by `wx2020/easytier`:

- RFC 5389 UDP and TCP Binding
- RFC 5780 `CHANGE-REQUEST`, `RESPONSE-ORIGIN`, and `OTHER-ADDRESS`
- EasyTier-compatible `CHANGED-ADDRESS`
- IPv4 and IPv6 socket addresses
- two-port single-IP mode and full two-IP/four-endpoint mode

This is STUN, not TURN. It discovers NAT mappings and helps EasyTier classify
NAT behavior; it does not relay EasyTier traffic.

## Requirements

- Rust stable with Rust 2024 support
- UDP 3478 and 3479 exposed
- optional TCP 3478 for TCP mapping detection
- two public IP addresses for complete change-IP NAT classification

With one public IP, Binding and change-port requests work, but EasyTier cannot
distinguish every cone NAT type.

## Build and run

```bash
cp config.example.toml config.toml
cargo build --release
./target/release/easytier-stun --config config.toml
```

Validate configuration without binding sockets:

```bash
./target/release/easytier-stun --config config.toml --check-config
```

Set every `advertise` value when binding a wildcard/private address or using
one-to-one NAT. Advertised addresses must be reachable from clients.

## EasyTier configuration

Point EasyTier at independently reachable endpoints:

```text
stun.example.com:3478
stun.example.com:3479
```

For a private build, place these in its UDP STUN server list or publish them in
the TXT record consumed by that build. Configure the TCP STUN list with
`stun.example.com:3478` when TCP mapping detection is required.

Accurate production NAT classification uses four UDP endpoints:

```text
IP A, port 3478  primary
IP A, port 3479  alternate_port
IP B, port 3478  alternate_ip
IP B, port 3479  alternate_ip_port
```

All four addresses must reach the same server instance.

## Docker

Host networking is intentional because STUN responses must originate from the
configured address and port.

```bash
cp config.example.toml config.toml
docker compose up -d --build
```

## systemd

```bash
sudo install -m 0755 target/release/easytier-stun /usr/local/bin/
sudo useradd --system --no-create-home easytier-stun
sudo install -d -o easytier-stun -g easytier-stun /etc/easytier-stun
sudo install -m 0644 config.toml /etc/easytier-stun/config.toml
sudo install -m 0644 deploy/easytier-stun.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now easytier-stun
```

## Security

- Expose only UDP 3478/3479 and optional TCP 3478.
- Apply provider-level packets-per-second limits to public deployments.
- The server accepts only Binding requests and caps input size.
- Do not place it behind a UDP proxy that rewrites response source ports.

## License

LGPL-3.0-only, matching EasyTier core. The compatibility attribute codec is
derived from EasyTier's STUN codec extension.
