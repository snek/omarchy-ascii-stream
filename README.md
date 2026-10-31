# bootleg

Stream a video as **truecolor solid-block art** over **anonymous SSH**.

Anyone with an SSH client can watch — no account, no password, no key.
There is no shell. Clients only receive ANSI frames.

```bash
ssh -p 2222 -o StrictHostKeyChecking=no your.vps.example
```

`q`, `Ctrl+C`, or `Ctrl+D` leaves. A truecolor terminal (most modern ones)
looks best.

## How it works

On boot the container decodes the clip **once** into memory, then serves
per-client block ANSI (`▀` / `█`) sized to that viewer's terminal. Each
session starts from the beginning of the clip and loops.

Inspired by [ascii.theater](https://ascii.theater).

## Deploy on a VPS

You need Docker Compose and a video file you have the right to stream.

```bash
git clone https://github.com/snek/omarchy-ascii-stream.git
cd omarchy-ascii-stream
cp .env.example .env
cp /path/to/your-clip.mp4 clips/video.mp4
docker compose up -d --build
```

Open the published port on the VPS firewall (default **2222/tcp**):

```bash
# Ubuntu / Debian
sudo ufw allow 2222/tcp
sudo ufw reload
```

Then, from anywhere:

```bash
ssh -p 2222 -o StrictHostKeyChecking=no your.vps.example
```

### Admin SSH vs viewer SSH

The VPS's own `sshd` already owns port 22. Leave it alone. Viewers use
`-p 2222` (or whatever you set as `SSH_PUBLIC_PORT`).

If you really want `ssh watch.example.com` with no `-p`, bind a **second
public IP** to 22, or move admin SSH off 22 first. Do not publish this
service on 22 if that is how you log in.

### Cloudflare / HTTP proxies

This is raw SSH (TCP), not HTTP. Cloudflare orange-cloud, tunnels, and
Access cannot sit in front of anonymous public SSH. Use a DNS-only record
and a firewall port open to the world.

## Configuration

Copy `.env.example` to `.env`. Compose reads it automatically.

| Variable | Default | Meaning |
|---|---|---|
| `SSH_PUBLIC_PORT` | `2222` | Host port published to the internet |
| `VIDEO_PATH` | `/clips/video.mp4` | Clip path **inside** the container (`./clips` is mounted at `/clips`) |
| `FPS` | `0` | Playback rate. `0` = source fps |
| `GRID_COLS` | `160` | Decode width in cells. Higher is sharper and uses more RAM |
| `ANSI_CACHE_MIB` | `512` | Retained rendered-ANSI budget, in MiB |

Changing the video or `GRID_COLS` requires a restart (`docker compose up -d`).
The whole clip is decoded at startup.

```bash
# example: a differently named file
echo 'VIDEO_PATH=/clips/trailer.webm' >> .env
docker compose up -d
```

## Tests

```bash
cargo test -p bootleg
```

## Local test

```bash
cp .env.example .env
cp your-clip.mp4 clips/video.mp4
docker compose up --build
ssh -p 2222 -o StrictHostKeyChecking=no 127.0.0.1
```

## Security model

- Authentication is **deliberately off** (`begin_auth() -> False`).
- No shell, no exec, no SFTP, no port forwarding — only streamed text.
- The process drops to an unprivileged user after generating the host key.
- Capabilities are dropped; the rootfs is read-only.
- Treat this as a public read-only kiosk. Do not put secrets in `clips/`
  or bake them into the image.
- The SSH host key lives in `./data/` and is created on first boot.
  **Never commit it.** Replacing `data/host_key` will make clients warn
  about a changed host key.

The whole video is held in RAM. A short clip at `GRID_COLS=160` is
typically tens of megabytes. A long high-resolution source at a large
grid can exhaust memory — keep clips short or lower `GRID_COLS`.
Compose sets a 1 GiB memory limit by default.

## Layout

```
server/              SSH server
Dockerfile           image build
docker-compose.yml   VPS / local deploy
entrypoint.sh        host-key bootstrap, drop root
clips/               your video (not in git)
data/                persisted host key (not in git)
```

## License

MIT. See [LICENSE](LICENSE).
