# EasyTier STUN

[English](README.md)

一个可独立部署的 STUN 服务器，采用与 EasyTier 核心相同的技术栈构建：
Rust、Tokio、`stun_codec`、`bytecodec`、Clap 和 Tracing。

本项目实现了 `wx2020/easytier` 使用的 STUN 行为：

- RFC 5389 UDP 和 TCP Binding
- RFC 5780 `CHANGE-REQUEST`、`RESPONSE-ORIGIN` 和 `OTHER-ADDRESS`
- 与 EasyTier 兼容的 `CHANGED-ADDRESS`
- IPv4 和 IPv6 套接字地址
- 双端口单 IP 模式、EasyTier 三端点模式和完整四端点模式

本项目提供的是 STUN，而不是 TURN。它用于发现 NAT 映射并帮助 EasyTier
判断 NAT 类型，不会中继 EasyTier 流量。

## 运行要求

- 支持 Rust 2024 的稳定版 Rust
- 开放 UDP 3478 和 3479 端口
- 可选开放 TCP 3478，用于 TCP 映射检测
- 如需 EasyTier 执行变更 IP 的 NAT 类型判断，需要两个公网 IP

只有一个公网 IP 时，Binding 和变更端口请求仍可正常工作，但 EasyTier
无法区分所有锥形 NAT 类型。

## 构建和运行

```bash
cp config.example.toml config.toml
cargo build --release
./target/release/easytier-stun --config config.toml
```

可以在不绑定套接字的情况下验证配置：

```bash
./target/release/easytier-stun --config config.toml --check-config
```

当监听通配地址、私有地址或服务器位于一对一 NAT 后方时，请设置所有
`advertise` 值。公布的地址必须能被客户端访问。

## EasyTier 配置

将 EasyTier 指向可独立访问的端点：

```text
stun.example.com:3478
stun.example.com:3479
```

对于私有构建，可将这些地址加入其 UDP STUN 服务器列表，或发布到该构建
读取的 TXT 记录中。如需 TCP 映射检测，请将 `stun.example.com:3478`
加入 TCP STUN 服务器列表。

当前 EasyTier 的 NAT 类型判断需要三个 UDP 端点：

```text
IP A，端口 3478  primary
IP A，端口 3479  alternate_port
IP B，端口 3479  alternate_ip_port
```

如需完整的四端点 RFC 矩阵，请增加 `IP B:3478` 作为 `alternate_ip`。
所有配置的地址都必须到达同一个服务器实例。

## Docker

使用主机网络是有意为之，因为 STUN 响应必须从配置的地址和端口发出。

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

## 安全建议

- 只开放 UDP 3478/3479 和可选的 TCP 3478。
- 对公网部署应用云服务商级别的每秒数据包限制。
- 服务器只接受 Binding 请求，并限制输入数据大小。
- 不要将服务器放在会改写响应源端口的 UDP 代理后方。

## 许可证

本项目使用 `LGPL-3.0-only`，与 EasyTier 核心一致。兼容属性编解码器派生自
EasyTier 的 STUN 编解码扩展。
