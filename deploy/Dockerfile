# syntax=docker/dockerfile:1.7
# Imperial Battlecode worker image: the `ibc` runner and the `ibc-worker` supervisor, nothing else.
#
#   docker build -f deploy/Dockerfile -t ibc-worker .            # from the repository root
#   docker run --rm -e IBC_COORDINATOR=https://coordinator.example \
#              -e IBC_REGISTRATION_TOKEN=... -v ibc-cache:/var/cache/ibc-worker ibc-worker
#
# The tag should name the patch (engine version + ruleset hash), because a worker only serves the patch
# of the runner it ships: `ibc-worker:0.1.0-00d2cae8a4f4c61b`.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p ibc-cli -p ibc-worker \
 && mkdir -p /out/bin /out/cache \
 && cp target/release/ibc target/release/ibc-worker /out/bin/

# distroless/cc: glibc, libgcc and CA certificates; no shell, no package manager. Runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot AS worker
COPY --from=build /out/bin/ibc /out/bin/ibc-worker /usr/local/bin/
COPY --from=build --chown=65532:65532 /out/cache /var/cache/ibc-worker
ENV IBC_RUNNER=/usr/local/bin/ibc
VOLUME /var/cache/ibc-worker
# SIGTERM (docker stop, a spot reclaim relayed by the orchestrator) drains: finish running matches, lease no more.
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/ibc-worker", "--cache", "/var/cache/ibc-worker"]
