# syntax=docker/dockerfile:1.7
# Imperial Battlecode images, from the repository root. Two targets:
#
#   docker build -f deploy/Dockerfile --target worker      -t ibc-worker .
#   docker build -f deploy/Dockerfile --target coordinator -t ibc-coordinator .
#
#   docker run --rm -e IBC_COORDINATOR=https://coordinator.example \
#              -e IBC_REGISTRATION_TOKEN=... -v ibc-cache:/var/cache/ibc-worker ibc-worker
#   docker run -d -p 8080:8080 -e IBC_ADMIN_TOKEN=... -v ibc-data:/data ibc-coordinator
#
# deploy/docker-compose.yml runs both on one box. The tag should name the patch (engine version + ruleset
# hash), because a worker only serves the patch of the runner it ships: `ibc-worker:0.1.0-00d2cae8a4f4c61b`.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# rust-toolchain.toml pins the compiler and lists the wasm targets; rustup installs them on first use.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p ibc-cli -p ibc-worker -p ibc-coordinator \
 && cargo build --release --locked -p ibc-engine-wasm --target wasm32-unknown-unknown \
 && mkdir -p /out/bin /out/cache /out/data /out/engine \
 && cp target/release/ibc target/release/ibc-worker target/release/ibc-coordinator /out/bin/ \
 && cp target/wasm32-unknown-unknown/release/ibc_engine_wasm.wasm /out/engine/

# The static sites the coordinator serves: the replay viewer (with the engine build) and, when the
# checkout has one, the web front end (`web/`). Either may be absent; the coordinator then serves the API.
FROM node:22-bookworm AS sites
WORKDIR /src
COPY . .
COPY --from=build /out/engine/ibc_engine_wasm.wasm /tmp/ibc_engine_wasm.wasm
RUN mkdir -p /out/viewer /out/web \
 && if [ -f viewer/package.json ]; then \
      cd viewer && npm ci --no-audit --no-fund \
      && IBC_ENGINE_WASM=/tmp/ibc_engine_wasm.wasm npm run build && cp -r dist/. /out/viewer/ && cd ..; \
    fi \
 && if [ -f web/package.json ]; then \
      cd web && npm ci --no-audit --no-fund && npm run build && cp -r dist/. /out/web/ && cd ..; \
    fi

# distroless/cc: glibc, libgcc and CA certificates; no shell, no package manager. Runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot AS worker
COPY --from=build /out/bin/ibc /out/bin/ibc-worker /usr/local/bin/
COPY --from=build --chown=65532:65532 /out/cache /var/cache/ibc-worker
ENV IBC_RUNNER=/usr/local/bin/ibc
VOLUME /var/cache/ibc-worker
# SIGTERM (docker stop, a spot reclaim relayed by the orchestrator) drains: finish running matches, lease no more.
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/ibc-worker", "--cache", "/var/cache/ibc-worker"]

# One process: API, job queue, ranked scheduler. Everything it keeps is in /data (back it up with
# `ibc-coordinator backup`, docs/DEPLOY.md). Set IBC_ADMIN_TOKEN and the registration tokens at run time.
# The built viewer and web front end are put at IBC_VIEWER_DIR / IBC_WEB_DIR: the coordinator serves the
# web front end at / and the viewer at /viewer/.
FROM gcr.io/distroless/cc-debian12:nonroot AS coordinator
COPY --from=build /out/bin/ibc-coordinator /usr/local/bin/
COPY --from=sites /out/viewer /srv/viewer
COPY --from=sites /out/web /srv/web
COPY --from=build --chown=65532:65532 /out/data /data
ENV IBC_DATA=/data \
    IBC_LISTEN=0.0.0.0:8080 \
    IBC_VIEWER_DIR=/srv/viewer \
    IBC_WEB_DIR=/srv/web
VOLUME /data
EXPOSE 8080
STOPSIGNAL SIGINT
ENTRYPOINT ["/usr/local/bin/ibc-coordinator"]
