FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock* rust-toolchain.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home stun
COPY --from=builder /src/target/release/easytier-stun /usr/local/bin/easytier-stun
COPY config.example.toml /etc/easytier-stun/config.toml
USER stun
EXPOSE 3478/udp 3479/udp 3478/tcp
ENTRYPOINT ["/usr/local/bin/easytier-stun"]
CMD ["--config", "/etc/easytier-stun/config.toml"]
