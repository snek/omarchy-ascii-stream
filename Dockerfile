# syntax=docker/dockerfile:1
FROM ghcr.io/astral-sh/uv:0.11.11 AS uv

FROM python:3.13-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ffmpeg \
        openssh-client \
        netcat-openbsd \
        util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 1000 --home /app --shell /usr/sbin/nologin watch

COPY --from=uv /uv /usr/local/bin/uv

WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev --no-install-project

COPY ascii_ssh.py entrypoint.sh ./
RUN chmod +x ./entrypoint.sh \
    && chown -R watch:watch /app

ENV PATH="/app/.venv/bin:$PATH" \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    VIDEO_PATH=/clips/video.mp4 \
    HOST=0.0.0.0 \
    PORT=2222 \
    HOST_KEY=/data/host_key

EXPOSE 2222
ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["python", "ascii_ssh.py"]
