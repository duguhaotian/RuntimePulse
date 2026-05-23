FROM rust:1-bookworm AS build

WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --locked

FROM ubuntu:24.04

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/debug/runtimepulse-collector /usr/local/bin/runtimepulse-collector

ENV INGEST_URL=http://runtimepulse-query-api:8081/api/ingest/batch
ENV RUNTIMEPULSE_COLLECTOR_NODE_ID=rust-node-a
ENV RUNTIMEPULSE_COLLECTOR_CLUSTER_ID=runtimepulse-local
ENV RUNTIMEPULSE_COLLECTOR_INTERVAL_MS=5000
ENV RUNTIMEPULSE_COLLECTOR_PLUGINS=none
ENV RUNTIMEPULSE_COLLECTOR_SCOPE=outlet

CMD ["runtimepulse-collector"]
