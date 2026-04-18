FROM rust:1.88-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

# ── Planner: generate recipe from controller + common only ────────────────────
FROM chef AS planner
COPY Cargo.lock ./
# Minimal workspace that excludes sensor-node (requires ESP-IDF)
COPY docker/Cargo.toml ./Cargo.toml
COPY crates/common crates/common
COPY crates/controller crates/controller
RUN cargo chef prepare --recipe-path recipe.json

# ── Builder ───────────────────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
COPY docker/Cargo.toml ./Cargo.toml
COPY Cargo.lock ./
COPY crates/common crates/common
COPY crates/controller crates/controller
RUN cargo chef cook --release --recipe-path recipe.json
RUN cargo build --release --bin kumaushi-controller

# ── Runtime ───────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/kumaushi-controller /usr/local/bin/kumaushi-controller

VOLUME ["/data"]
ENV KUMAUSHI_DB=/data/kumaushi.db
EXPOSE 3000

CMD ["kumaushi-controller"]
