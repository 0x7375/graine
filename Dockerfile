# syntax=docker/dockerfile:1
FROM node:22-bookworm-slim AS frontend
WORKDIR /build/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1-bookworm AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo build --locked --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates coreutils \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /var/lib/graine
COPY --from=backend /build/target/release/graine /usr/local/bin/graine
COPY --from=frontend /build/web/dist/ /opt/graine/web/
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/graine"]
CMD ["--config", "/etc/graine/config.toml"]
