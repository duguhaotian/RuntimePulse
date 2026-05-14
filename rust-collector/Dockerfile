FROM ubuntu:24.04

WORKDIR /app
# Development image path: run `cargo build` in rust-collector before `docker compose build`.
# This keeps local validation fast while the production collector image pipeline is still being defined.
COPY target/debug/runtimepulse-collector /usr/local/bin/runtimepulse-collector

ENV INGEST_URL=http://runtimepulse-query-api:8081/api/ingest/batch
ENV RUNTIMEPULSE_COLLECTOR_NODE_ID=rust-node-a
ENV RUNTIMEPULSE_COLLECTOR_CLUSTER_ID=cluster-prod
ENV RUNTIMEPULSE_COLLECTOR_INTERVAL_MS=5000
ENV RUNTIMEPULSE_COLLECTOR_PLUGINS=none
ENV RUNTIMEPULSE_COLLECTOR_SCOPE=outlet

CMD ["runtimepulse-collector"]
