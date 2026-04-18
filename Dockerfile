FROM --platform=linux/amd64 rust:1.88-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

FROM --platform=linux/amd64 chef AS planner
COPY Cargo.lock ./
COPY docker/Cargo.toml ./Cargo.toml
COPY crates/common crates/common
COPY crates/controller crates/controller
COPY dashboard dashboard
RUN cargo chef prepare --recipe-path recipe.json

FROM --platform=linux/amd64 chef AS builder
COPY --from=planner /app/recipe.json recipe.json
COPY docker/Cargo.toml ./Cargo.toml
COPY Cargo.lock ./
COPY crates/common crates/common
COPY crates/controller crates/controller
COPY dashboard dashboard
RUN cargo chef cook --release --recipe-path recipe.json
# REBUILD arg busts the cargo build cache when source changes
ARG REBUILD=1
RUN cargo build --release --bin kumaushi-controller && \
    ls -la /app/target/release/kumaushi-controller

FROM --platform=linux/amd64 debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates bash && rm -rf /var/lib/apt/lists/*
# REBUILD ensures binary is never cached from a stale builder stage
ARG REBUILD=1
COPY --from=builder /app/target/release/kumaushi-controller /usr/local/bin/kumaushi-controller
RUN ls -la /usr/local/bin/kumaushi-controller

VOLUME ["/data"]
ENV KUMAUSHI_DB=/data/kumaushi.db
EXPOSE 3000

CMD ["/usr/local/bin/kumaushi-controller"]
