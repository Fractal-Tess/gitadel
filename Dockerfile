FROM oven/bun:1.4.2 AS frontend-dependencies
WORKDIR /build/frontend
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile

FROM node:24.22.2-trixie-slim AS node

FROM rust:1.98.1-slim-trixie AS chef
RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential cmake libdav1d-dev libssl-dev perl pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked --version 0.1.78
WORKDIR /build
COPY --from=frontend-dependencies /usr/local/bin/bun /usr/local/bin/bun
COPY --from=node /usr/local/bin/node /usr/local/bin/node

# The recipe only changes with manifests and the lockfile, so the dependency
# layer below stays cached across source and frontend changes.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS backend
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --locked --package gitadel --bin gitadel --recipe-path recipe.json
COPY Cargo.toml Cargo.lock CHANGELOG.md ./
COPY crates/ ./crates/
COPY proto/ ./proto/
COPY api/ ./api/
COPY scripts/ ./scripts/
COPY frontend/ ./frontend/
COPY --from=frontend-dependencies /build/frontend/node_modules ./frontend/node_modules/
# The offline Rust exporter, SDK drift check and real TypeScript check must
# succeed before bundling. The release build verifies the resulting fingerprint.
RUN bun run --cwd frontend build
RUN cargo build --release --locked --package gitadel --bin gitadel \
    && strip target/release/gitadel

FROM debian:trixie-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl fonts-dejavu-core libdav1d7 libgcc-s1 libssl3t64 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --shell /usr/sbin/nologin gitadel \
    && install --directory --owner gitadel --group gitadel /data
COPY --from=backend /build/target/release/gitadel /usr/local/bin/gitadel

USER gitadel
WORKDIR /data
VOLUME ["/data"]
EXPOSE 3000 2222
ENV \
    GITADEL_PUBLIC_URL=http://localhost:3000 \
    GITADEL_BIND=0.0.0.0:3000 \
    GITADEL_DATABASE_URL=sqlite:///data/gitadel.db?mode=rwc \
    GITADEL_REPOSITORY_ROOT=/data/repositories \
    GITADEL_LFS_ROOT=/data/lfs \
    GITADEL_REGISTRY_ROOT=/data/registry \
    GITADEL_SSH_BIND=0.0.0.0:2222 \
    GITADEL_SSH_HOST_KEY=/data/ssh-host-ed25519
ENTRYPOINT ["gitadel"]
