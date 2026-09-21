FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin nugitea --bin nugitea-storaged

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/nugitea /usr/local/bin/nugitea
COPY --from=builder /build/target/release/nugitea-storaged /usr/local/bin/nugitea-storaged
