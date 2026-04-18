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
RUN cargo build --release --bin kumaushi-controller

FROM --platform=linux/amd64 debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
ARG CACHEBUST=1
COPY --from=builder /app/target/release/kumaushi-controller /usr/local/bin/kumaushi-controller

VOLUME ["/data"]
ENV KUMAUSHI_DB=/data/kumaushi.db
EXPOSE 3000

RUN apt-get update && apt-get install -y --no-install-recommends bash && rm -rf /var/lib/apt/lists/*
CMD ["/bin/bash", "-c", "echo 'BINARY START' >&2; /usr/local/bin/kumaushi-controller; echo \"EXIT: $?\" >&2; sleep 3600"]
