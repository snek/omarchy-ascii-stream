# Deploy quattro on a fresh DigitalOcean VPS

End-to-end steps from an empty DigitalOcean account to viewers running:

```bash
ssh -p 2222 -o StrictHostKeyChecking=no YOUR_DROPLET_IP
```

This service is **anonymous public SSH**. It never opens a shell. DigitalOcean's own `sshd` stays on port **22** for you; viewers use **2222**.

## 0. What you need

- A DigitalOcean account
- An SSH key already added under **Settings → Security → SSH keys**
- A video file you have the right to stream (`.mp4` or `.webm`)
- A laptop with `ssh` and `scp` (or `rsync`)

**Size the Droplet for RAM, not CPU.** The container decodes the whole clip into memory at boot and Compose caps it at **1 GiB**. A 1 GB Droplet will OOM.

| Droplet | When to use |
|---|---|
| **2 GB RAM / 1 vCPU** (Basic) | Short clip, default `GRID_COLS=160` |
| **4 GB RAM / 2 vCPU** | Longer clip, or you raise `GRID_COLS` |

25 GB disk is enough. Pick the region closest to your viewers.

## 1. Create the Droplet

1. [Create a Droplet](https://cloud.digitalocean.com/droplets/new).
2. **Image** — either:
   - **Marketplace → Docker** (Ubuntu with Docker Engine + Compose already installed), or
   - **Ubuntu 24.04 LTS** (you install Docker in step 4).
3. **Plan** — Basic, **2 GB RAM** or larger (see table above).
4. **Region** — closest to the audience.
5. **Authentication** — SSH key. Do not use a password.
6. **Hostname** — e.g. `quattro`.
7. **IPv6** — optional; enable if you want AAAA later.
8. Create the Droplet. Copy its public IPv4.

Do **not** enable a managed database, App Platform, or a load balancer. This is a raw TCP SSH service on one box.

## 2. Cloud Firewall (do this before you go public)

DigitalOcean [Cloud Firewalls](https://docs.digitalocean.com/products/networking/firewalls/) sit in front of the Droplet and do not fight with Docker the way UFW does. Prefer this over UFW. Use **one** firewall layer, not both.

1. **Networking → Firewalls → Create Firewall**.
2. Name it `quattro`.
3. **Inbound rules** (delete the defaults, then add):

   | Type | Protocol | Port | Sources |
   |---|---|---|---|
   | SSH (you) | TCP | 22 | **your IP** (or `All IPv4` if your IP is dynamic) |
   | Custom (viewers) | TCP | 2222 | All IPv4, and All IPv6 if you enabled IPv6 |

4. **Outbound rules** — leave the defaults (allow all). The image must reach Docker Hub / GitHub / Ubuntu mirrors to build.
5. **Apply to Droplets** — attach `quattro`.
6. Save.

Until this firewall is attached, a brand-new Droplet is reachable on every port. Attach it as soon as the Droplet exists.

## 3. First login

From your laptop:

```bash
ssh root@YOUR_DROPLET_IP
```

Update the box:

```bash
apt-get update && apt-get upgrade -y
```

If you used the **Docker 1-Click** image, skip to step 5.

Optional but recommended: create a sudo user and disable password SSH. Root + key-only is acceptable for a single-purpose kiosk.

```bash
adduser --disabled-password --gecos "" deploy
usermod -aG sudo deploy
rsync --archive --chown=deploy:deploy ~/.ssh /home/deploy
```

Then log in as `deploy` and prefix the remaining commands with `sudo` where needed. The rest of this guide assumes you stayed on `root` (DigitalOcean default).

## 4. Install Docker (Ubuntu image only)

Skip this section if you created the Droplet from the **Docker** Marketplace image.

```bash
apt-get install -y ca-certificates curl
install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
chmod a+r /etc/apt/keyrings/docker.asc

. /etc/os-release
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu ${UBUNTU_CODENAME:-$VERSION_CODENAME} stable" \
  > /etc/apt/sources.list.d/docker.list

apt-get update
apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
systemctl enable --now docker
docker compose version
```

## 5. Clone the repo

```bash
mkdir -p /opt
git clone https://github.com/snek/omarchy-ascii-stream.git /opt/quattro
cd /opt/quattro
```

## 6. Copy the video onto the Droplet

On **your laptop** (not the Droplet):

```bash
scp /path/to/your-clip.mp4 root@YOUR_DROPLET_IP:/opt/quattro/clips/video.mp4
```

Or, if you are already on the Droplet and the file is elsewhere:

```bash
# example: you uploaded to /root/clip.mp4
cp /root/clip.mp4 /opt/quattro/clips/video.mp4
```

The filename `video.mp4` matches the default `VIDEO_PATH`. A different name is fine if you set `VIDEO_PATH` in the next step (path is **inside** the container, so `./clips/trailer.webm` on the host is `/clips/trailer.webm`).

Confirm it landed:

```bash
ls -lh /opt/quattro/clips/
```

## 7. Configure

```bash
cd /opt/quattro
cp .env.example .env
```

Edit `.env` only if you need to. Defaults:

```bash
SSH_PUBLIC_PORT=2222
VIDEO_PATH=/clips/video.mp4
FPS=0
GRID_COLS=160
```

- `FPS=0` uses the source frame rate.
- Raise `GRID_COLS` for a sharper image; it uses more RAM. Stay at 160 until you know the clip fits in the 1 GiB container limit.

Do **not** set `SSH_PUBLIC_PORT=22`. That is your admin SSH.

## 8. Start the stack

```bash
cd /opt/quattro
docker compose up -d --build
docker compose ps
docker compose logs -f
```

On first boot you should see:

- `generating host key at /data/host_key` (or `using existing…`)
- `decoding /clips/video.mp4 …`
- `N frames @ … fps`
- `listening on 0.0.0.0:2222`

`Ctrl+C` leaves the log follow; the container keeps running.

The host key is written to `/opt/quattro/data/` and survives `docker compose down` / rebuilds. Do not commit or copy that directory into git.

If the container exits immediately:

```bash
docker compose logs --tail=80
```

The usual cause is a missing clip (`no video at /clips/video.mp4`). Fix the file or `VIDEO_PATH`, then `docker compose up -d`.

## 9. Verify

**On the Droplet:**

```bash
ss -lntp | grep 2222
docker compose ps    # Status should be healthy after ~25s
```

**On your laptop:**

```bash
ssh -p 2222 -o StrictHostKeyChecking=no root@YOUR_DROPLET_IP
```

Any username works (`root`, `watch`, `anonymous`). There is no password prompt. You should get a full-terminal block stream. `q` or `Ctrl+C` disconnects.

Confirm you can still administer the box:

```bash
ssh root@YOUR_DROPLET_IP
```

That session is real SSH on port 22. The port-2222 session is not.

## 10. Optional: DNS

A hostname is nicer than a raw IP. This is **DNS only**.

1. In your DNS host, add an **A** record: `watch.example.com` → Droplet IPv4.
2. Add **AAAA** only if you enabled IPv6 on the Droplet.
3. If the zone is on Cloudflare, set the record to **DNS only** (grey cloud). Orange-cloud, tunnels, and Access cannot proxy anonymous SSH.

Wait for the record to resolve, then:

```bash
ssh -p 2222 -o StrictHostKeyChecking=no watch.example.com
```

Do not put this service on port 22 just to drop `-p`. Move admin SSH first, or use a second public IP.

## 11. Day-2 operations

```bash
cd /opt/quattro

# logs
docker compose logs -f --tail=100

# replace the clip
scp new.mp4 root@YOUR_DROPLET_IP:/opt/quattro/clips/video.mp4
docker compose up -d --force-recreate

# change GRID_COLS / FPS
nano .env
docker compose up -d

# update the app
git pull
docker compose up -d --build

# stop
docker compose down
```

Replacing `data/host_key` makes every returning client warn about a changed host key. Leave it alone unless you are wiping the box.

## 12. Troubleshooting

| Symptom | Check |
|---|---|
| `Connection refused` | `docker compose ps`, Cloud Firewall inbound **2222**, `ss -lntp \| grep 2222` |
| `Connection timed out` | Firewall / your own network blocking 2222. DigitalOcean firewall must allow it from `All IPv4`. |
| Password prompt on 2222 | You hit **sshd on 22**, not quattro. Use `-p 2222`. |
| Container restart loop | `docker compose logs` — almost always a missing `VIDEO_PATH`. |
| `stream error` / killed | Clip + `GRID_COLS` exceeded `mem_limit` (1g). Shorten the video, lower `GRID_COLS`, or raise `mem_limit` in `docker-compose.yml` **and** the Droplet RAM. |
| Build cannot pull images | Cloud Firewall outbound must allow HTTPS. |
| UFW + Docker both installed | They fight over iptables. Remove UFW (`ufw disable`) and use only the Cloud Firewall. |

## 13. Security model (read once)

- Port **2222** accepts **anyone**, with no key and no password. That is the product.
- Port **22** is how **you** get in. Restrict it to your IP in the Cloud Firewall if you can.
- The process drops to uid 1000, drops capabilities, and runs on a read-only rootfs. Clients cannot get a shell, SFTP, or forwarding.
- Treat `clips/` as public. Do not put secrets there.
- Do not publish quattro on port 22 while that is still your admin SSH.

You are done when a machine that is not the Droplet can run the `ssh -p 2222` command in the header and see the clip.
