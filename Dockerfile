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
# Build dependencies first (cached unless Cargo.lock changes)
RUN cargo chef cook --release --recipe-path recipe.json
# Copy app source AFTER dependency build — changes here only recompile app code
COPY crates/common crates/common
COPY crates/controller crates/controller
COPY dashboard dashboard
RUN cargo build --release --bin kumaushi-controller

FROM --platform=linux/amd64 debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/kumaushi-controller /usr/local/bin/kumaushi-controller

VOLUME ["/data"]
ENV KUMAUSHI_DB=/data/kumaushi.db
EXPOSE 3000
CMD ["/usr/local/bin/kumaushi-controller"]
