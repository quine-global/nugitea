FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .

# search_island (the file-browser's realtime search box) is a separate
# wasm32 crate, built with wasm-pack before the native binaries -- the app
# tier embeds its output via include_bytes! at compile time. Prebuilt
# installer, not `cargo install wasm-pack`, to avoid several more minutes
# on an already-slow from-scratch Rust build.
RUN rustup target add wasm32-unknown-unknown \
    && curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh
RUN cd search_island && wasm-pack build --release --target web -- --no-default-features --features hydrate

RUN cargo build --release --bin nugitea --bin nugitea-storaged

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/nugitea /usr/local/bin/nugitea
COPY --from=builder /build/target/release/nugitea-storaged /usr/local/bin/nugitea-storaged
