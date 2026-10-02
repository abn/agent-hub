# syntax=docker/dockerfile:1

FROM rust:1.97.1-bookworm AS builder

RUN apt-get update \
 && apt-get install --yes --no-install-recommends clang libclang-dev cmake \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
# The build script stamps the binary with the version and the commit it was
# built from, so it must be in the image context too. Without it the crate does
# not compile: the storage payload reads the value with env!.
COPY build.rs ./
COPY vendor ./vendor
COPY src ./src
COPY web ./web
# The served skill guide is embedded into the binary at build time.
COPY assets ./assets

# The image context excludes .git, so the commit is passed in at build time.
# With no argument the row reports "unknown" rather than inventing a value.
ARG GIT_COMMIT=
ENV GIT_COMMIT=${GIT_COMMIT}

# The image only serves, so it leaves out the client: the stdio proxy and the
# one-shot calls run on the agents' machines, not here.
RUN cargo build --release --locked --no-default-features

RUN mkdir -p /data && chown 65532:65532 /data

FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder /build/target/release/agent-hub /usr/local/bin/agent-hub
COPY --from=builder --chown=65532:65532 /data /data

USER 65532:65532

ENV HUB_DATA_DIR=/data
ENV HUB_BIND=0.0.0.0:8080

EXPOSE 8080

# The runtime is distroless: no shell and no curl, so the check is the binary's
# own `health` subcommand, which GETs /readyz. Exec form, because there is no
# shell to parse a string one.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/agent-hub", "health"]

ENTRYPOINT ["/usr/local/bin/agent-hub"]
