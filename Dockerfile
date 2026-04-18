FROM --platform=linux/amd64 rust:1.88-bookworm AS builder
WORKDIR /app

# Copy workspace config
COPY Cargo.lock ./
COPY docker/Cargo.toml ./Cargo.toml

# Copy source and embedded assets
COPY crates/common crates/common
COPY crates/controller crates/controller
COPY dashboard dashboard

# Build — no cargo-chef so Depot cannot cache a stub binary
RUN cargo build --release --bin kumaushi-controller && \
    ls -lh /app/target/release/kumaushi-controller

FROM --platform=linux/amd64 debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/kumaushi-controller /usr/local/bin/kumaushi-controller

VOLUME ["/data"]
ENV KUMAUSHI_DB=/data/kumaushi.db
EXPOSE 3000
CMD ["/usr/local/bin/kumaushi-controller"]
