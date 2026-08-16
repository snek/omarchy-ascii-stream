#!/bin/bash
set -euo pipefail

HOST_KEY="${HOST_KEY:-/data/host_key}"
VIDEO_PATH="${VIDEO_PATH:-/clips/video.mp4}"
DATA_DIR="$(dirname "$HOST_KEY")"

mkdir -p "$DATA_DIR"

if [ ! -f "$VIDEO_PATH" ]; then
    echo "[quattro] no video at ${VIDEO_PATH}" >&2
    echo "[quattro] drop an .mp4 or .webm into ./clips and set VIDEO_PATH, then restart." >&2
    exit 1
fi

if [ ! -f "$HOST_KEY" ]; then
    echo "[quattro] generating host key at ${HOST_KEY}"
    ssh-keygen -q -t ed25519 -f "$HOST_KEY" -N "" -C "quattro"
else
    echo "[quattro] using existing host key at ${HOST_KEY}"
fi

# Drop root if the image started as uid 0 (volume perms are then fixed first).
if [ "$(id -u)" -eq 0 ]; then
    chown -R watch:watch "$DATA_DIR"
    # Bounding-set / inh-caps need CAP_SETPCAP, which we drop in Compose.
    exec setpriv --reuid=watch --regid=watch --init-groups -- "$@"
fi

exec "$@"
