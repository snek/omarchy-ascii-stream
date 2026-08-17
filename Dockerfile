# syntax=docker/dockerfile:1
FROM ghcr.io/astral-sh/uv:0.11.11 AS uv

FROM python:3.13-slim

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

COPY --from=uv /uv /usr/local/bin/uv

WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN set -ex \
    && uv sync --frozen --no-dev --no-install-project

COPY ascii_ssh.py entrypoint.sh ./
RUN set -ex \
    && chmod +x ./entrypoint.sh \
    && chown -R watch:watch /app

ENV PATH="/app/.venv/bin:$PATH" \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    VIDEO_PATH=/clips/video.mp4 \
    HOST=0.0.0.0 \
    PORT=2222 \
    HOST_KEY=/data/host_key

EXPOSE 2222
ENTRYPOINT ["tini", "-v", "--", "/app/entrypoint.sh"]
CMD ["python", "ascii_ssh.py"]
