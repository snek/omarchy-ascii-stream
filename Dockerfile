# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server ./server
COPY player ./player
RUN cargo build --release -p bootleg

FROM debian:bookworm-slim
RUN set -ex \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        ffmpeg \
        openssh-client \
        netcat-openbsd \
        tini \
        util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 1000 --home /app --shell /usr/sbin/nologin watch
WORKDIR /app
COPY --from=build /src/target/release/bootleg /app/bootleg
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh /app/bootleg \
    && chown -R watch:watch /app
ENV VIDEO_PATH=/clips/video.mp4 \
    HOST=0.0.0.0 \
    PORT=2222 \
    HOST_KEY=/data/host_key
EXPOSE 2222
ENTRYPOINT ["tini", "-v", "--", "/app/entrypoint.sh"]
CMD ["/app/bootleg"]
