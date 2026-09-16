# syntax=docker/dockerfile:1

FROM rust:1.97.1-bookworm AS builder

RUN apt-get update \
 && apt-get install --yes --no-install-recommends clang libclang-dev cmake \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY src ./src

RUN cargo build --release --locked

RUN mkdir -p /data && chown 65532:65532 /data

FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder /build/target/release/agent-hub /usr/local/bin/agent-hub
COPY --from=builder --chown=65532:65532 /data /data

USER 65532:65532

ENV HUB_DATA_DIR=/data
ENV HUB_BIND=0.0.0.0:8080

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/agent-hub"]
