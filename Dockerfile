FROM oven/bun:1.3.13 AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY frontend/ ./
RUN bun run build

FROM rust:1.97.1-slim-trixie AS backend
RUN apt-get update \
    && apt-get install --yes --no-install-recommends build-essential cmake perl pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ ./src/
COPY --from=frontend /build/frontend/build ./frontend/build/
RUN cargo build --release --locked \
    && strip target/release/gitadel

FROM debian:trixie-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 --shell /usr/sbin/nologin gitadel \
    && install --directory --owner gitadel --group gitadel /data
COPY --from=backend /build/target/release/gitadel /usr/local/bin/gitadel

USER gitadel
WORKDIR /data
VOLUME ["/data"]
EXPOSE 3000 2222
ENV GITADEL_PUBLIC_URL=http://localhost:3000
ENTRYPOINT ["gitadel"]
CMD ["--bind", "0.0.0.0:3000", "--database-url", "sqlite:///data/gitadel.db?mode=rwc", "--repository-root", "/data/repositories", "--lfs-root", "/data/lfs", "--ssh-bind", "0.0.0.0:2222", "--ssh-host-key", "/data/ssh-host-ed25519"]
