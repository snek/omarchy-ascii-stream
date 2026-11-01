# bootleg SSH server

Date: 2026-10-31

Replace the Python SSH server with one Rust binary, `bootleg`. The project name quattro goes away in the server, the image, the docs, and the banner. Viewers still connect with any SSH client, still see truecolor block art, and each login still plays the clip from the first frame on its own clock.

The one-shot boot player is a second crate, specified in [2026-10-31-bootleg-play-design.md](2026-10-31-bootleg-play-design.md). This spec does not build it and does not link it.

## Goals

- More simultaneous viewers inside the existing 1 GiB container limit and whatever bandwidth the host has.
- Steady per-session frame timing while other sessions render.
- A fast first frame the first time a terminal size appears.

Boot decode stays on ffmpeg. Each viewer has a private playhead. The clip stays a separate file.

## Repository

The repo is a Cargo workspace. This spec adds the `server` member only.

```
Cargo.toml       workspace
server/          package bootleg, binary bootleg
```

`README.md` and `DEPLOY.md` call the service, image, binary, and install path bootleg. The documented install path is `/opt/bootleg`. The git remote URL stays as it is.

## End state

`ascii_ssh.py`, `pyproject.toml`, `uv.lock`, and `tests/` are gone. `README.md` and `DEPLOY.md` describe `cargo test` and the `bootleg` binary. The Compose service name is `bootleg`. The published port, volume mounts, capability drop, read-only root, 1 GiB memory cap, and healthcheck stay.

## Process

One process. At startup it decodes the clip once and keeps the pixels in a single shared movie. Each accepted SSH session is its own task:

1. Read the PTY size, or use 80×24 when the client sends no PTY.
2. Choose a grid and keep it for the life of that login. Ignore later window-change requests.
3. Send the banner and frame 0.
4. Start that session's clock only after frame 0 has been written.
5. On each tick, write the frame for "now". If a write ran long, skip the indexes that were missed and send the current one. Do not queue a backlog.
6. Loop to frame 0 when that session reaches the end of the clip.

A slow connection delays only its own task. It does not change another session's index.

Sessions share the decoded pixels and the rendered ANSI for a terminal size. The second login at the same size reuses those bytes and still starts at frame 0.

Rendering runs on Tokio's blocking pool. SSH writes stay on the async runtime. The runtime is multi-threaded.

## Modules

All of these live under `server/src/`:

| Module | Responsibility |
|---|---|
| `movie` | Probe the file, run ffmpeg, own the packed RGB frames |
| `render` | Subsample one frame to a grid and emit ANSI |
| `cache` | Share rendered frames, fill each size once, enforce the byte budget |
| `session` | Grid, banner, per-session clock, quit, terminal restore |
| `ssh` | russh server: no auth, one stream channel, refuse everything else |
| `main` | Read env, decode, listen until SIGINT or SIGTERM |

## Decode

Environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `VIDEO_PATH` | `/clips/video.mp4` | Clip path |
| `HOST` | `0.0.0.0` | Bind address |
| `PORT` | `2222` | Bind port |
| `FPS` | `0` | Playback rate. `0` uses the source rate |
| `GRID_COLS` | `160` | Decode width in cells |
| `HOST_KEY` | `./host_key` | OpenSSH ed25519 private key |
| `ANSI_CACHE_MIB` | `512` | Retained rendered-ANSI budget |

ffprobe supplies width, height, and `r_frame_rate` (`num/den`). The frame count is the number of raw frames actually read.

`ref_rows = max(1, round_half_even((src_height / src_width) * GRID_COLS / 2) * 2)`.

ffmpeg scales with `scale={ref_cols}:{ref_rows}:flags=area` and writes `rgb24` to stdout. Frames are packed RGB24, `ref_cols * ref_rows * 3` bytes, stored once. On a short read, stop. If ffmpeg does not exit after stdout ends, terminate it, wait 3 seconds, then kill it. Exit the process if the probe fails, ffmpeg fails, or zero frames come back.

Startup logs, then listen:

```
[bootleg] decoding {path} ...
[bootleg] {n} frames @ {fps:.2f}fps ref {cols}x{rows}
[bootleg] listening on {host}:{port}
```

## Grid

```
cols = min(max(10, saturating_sub(term_cols, 1)), ref_cols)
aspect_rows = max(4, round_half_even((ref_rows as f64 / ref_cols as f64) * 2.0 * cols as f64))
rows = min(aspect_rows, max(4, saturating_sub(term_rows, 1)))
```

`saturating_sub` stops at 0, so a reported size of 0 or 1 still yields at least 10×4. `round_half_even` is Python 3 `round()` for the `.5` cases in the grid, the reference height, and the sample step.

The last column is left unused so a terminal that ignores wrap-off cannot insert a blank line.

## Render

Output for the same pixels, grid, and terminal size matches the Python server.

The stored frame is `ref_rows` by `ref_cols`. Sample that, with `dec_h = max(2, rows * 2)`:

```
step_y = max(1, round_half_even(ref_rows / dec_h))
step_x = max(1, round_half_even(ref_cols / cols))
```

Take every `step_y` row and every `step_x` column, then keep the top-left `dec_h` by `cols`. If that is short, pad the bottom and right by repeating the edge pixel, then crop to `dec_h` by `cols`.

Pair pixel rows `(0,1), (2,3), …`. If the sampled height is odd, duplicate the last top row as the missing bottom row.

For each run of identical cells in a character row:

- Equal top and bottom RGB: `\x1b[49m\x1b[38;2;R;G;Bm` plus `█` repeated for the run.
- Otherwise: `\x1b[38;2;Rt;Gt;Btm\x1b[48;2;Rb;Gb;Bbm` plus `▀` repeated for the run.

Append `\x1b[0m` to each character row. Join character rows with `\x1b[E`. Each stored frame is an immutable byte buffer with no per-session copy.

## Cache

The cache key is the integer pair `(cols, rows)`. Each frame slot is filled by at most one render. Waiters on that slot share the result. Slots publish as they finish, so a session can use frame *k* before the rest of that size exists.

Frame 0 is rendered and sent before the clock starts. The rest of an admitted size fills in the background, one frame at a time for that size. Different sizes may render in parallel. The fill belongs to the size: it continues after the login that triggered it disconnects. A session that reaches an empty slot renders it through that same slot, and the background fill skips slots that are already full.

`ANSI_CACHE_MIB` counts retained rendered bytes only. The decoded movie is not charged to it. The first size is always stored, even when it alone exceeds the budget. Once a size is admitted, its whole clip is kept, even if the finished bytes cross the budget. A later new size is admitted only when retained bytes are still under the budget. Otherwise that login snaps to a stored size and does not render its exact grid.

Nearest size: smallest `|Δcols|`, then smallest `|Δrows|`, then the candidate with more cells, then the one stored earlier. The snapped pair is the grid that login uses for the whole session. The banner still reports the source pixel size, the source frame count, and the playback fps.

## Clock

`t0` is a monotonic instant taken after frame 0 is written. `last` starts at `0`, so frame 0 is not sent again until the clip wraps.

```
idx = floor(elapsed_seconds * fps) mod frame_count
```

When `idx != last`, write `\x1b[H` plus that frame and set `last = idx`. Sleep until `t0 + (floor(elapsed_seconds * fps) + 1) / fps`. A non-positive delay yields once and the loop reads the clock again.

Each session has its own `t0`. Missed indexes are not written.

## SSH

russh. Accept the `none` authentication method for any username, so the client gets no password prompt. Accept a shell request and a PTY request. That shell is the stream: no process is spawned. Refuse exec, subsystems (including SFTP), TCP forwarding, agent forwarding, and X11.

On start, write one banner, then `\x1b[H` and frame 0. `{fps}` uses Python 3's `{fps:.0f}` rounding. The banner is this single byte string (no extra newline between the reset and the `\r\n`):

`\x1b[?1049h\x1b[2J\x1b[?25l\x1b[?7l\x1b[0m\r\n\x1b[38;2;255;255;255mbootleg  ·  {width}x{height}  ·  {n} frames @ {fps}fps  ·  'q' quits\x1b[0m\r\n\r\n`

Close the session when incoming bytes contain `\x03`, `\x04`, `\x1b[99;5u`, `\x1b[67;5u`, or `\x1b[3;5u`, or when the chunk starts with `q`. Also close on break, channel EOF, and INT, TERM, QUIT, or HUP. Any other input, including an empty read, leaves the session up.

On the way out, including after a session error, write `\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l` when the channel is still open, then close it.

## Failures

Exit before listen, with a non-zero status, when the clip is missing, ffprobe or ffmpeg fails, zero frames are decoded, the host key is missing, or the port will not bind. `entrypoint.sh` still creates the host key on first boot and still exits when the clip is absent.

A failure inside one session writes `\r\n\x1b[0mstream error: {message}\x1b[0m\r\n` when the channel is open, then restores the terminal and closes that channel. Other sessions keep running. A disconnect during a write ends that task and leaves no session-local frame queue behind.

## Deploy

Multi-stage image. The builder compiles the workspace package `bootleg` in release (`lto` on, one codegen unit). The runtime image contains that binary only. The runtime image is Debian slim with ffmpeg, `ssh-keygen`, `setpriv`, tini, and netcat. User `watch` is uid 1000. `ENTRYPOINT` remains `tini -- /app/entrypoint.sh`. `CMD` is `/app/bootleg`. The image sets `VIDEO_PATH=/clips/video.mp4`, `HOST=0.0.0.0`, `PORT=2222`, and `HOST_KEY=/data/host_key`.

`entrypoint.sh` keeps its current behavior: require the clip, generate an ed25519 key at `HOST_KEY` when absent, and `setpriv` down to `watch` when started as root. Its log lines use the `[bootleg]` prefix.

Compose keeps image `bootleg:0.1.0`, env passthrough, `./clips` and `./data` mounts, `read_only`, `/tmp` tmpfs, `cap_drop: ALL` plus `SETUID`, `SETGID`, and `CHOWN`, `no-new-privileges`, `mem_limit: 1g`, the json-file log caps, and the `nc -z 127.0.0.1 2222` healthcheck. The service name is `bootleg`.

`.env.example` gains `ANSI_CACHE_MIB=512`.

## Tests

`cargo test -p bootleg` replaces pytest.

Always run, with in-memory frames:

- A solid color is full blocks only, with one SGR run per solid row, and `rows - 1` cursor-next-line sequences.
- Stacked even/odd colors are half blocks only.
- Cell count equals `cols * rows` across cols 8–24, rows 4–12, and RGB channels 0–255 (proptest, 25 cases).
- A known RGB triple is emitted as `R;G;B` in the SGR sequence and reads back as the same triple.
- Edge-pad produces exactly the requested height and width.
- Two grids do not share a buffer. Frame index wraps with `mod frame_count`.
- Grid clamp: a tiny terminal is at least 10×4; a huge terminal stops at `ref_cols` and at least 4 rows.
- Quit matcher: `q`, `quit`, ETX, EOT, the three CSI-u sequences, break, INT, and EOF close; empty input and `x` do not.
- Clock: a late wake returns the current index and the list of indexes to send contains only that index.

Cache, in memory:

- Two waiters on one empty slot cause one render.
- After the budget is exhausted, the next new size snaps to the nearest stored size and does not grow the cache.

When `ffmpeg` and `ffprobe` are on `PATH`, decode a generated `testsrc` of 64×36 at 8 fps (4 frames) with `GRID_COLS=32`. Expect source 64×36, even `ref_rows`, at least 3 frames, and a 16×8 render whose cell count is 128. Skip otherwise.

When `ffmpeg` and `ssh` are on `PATH`, start the server on an ephemeral port with a generated host key and that clip. An OpenSSH client, any username, no password, `StrictHostKeyChecking=no`, terminal 40×16, must receive the banner (`bootleg`, the source size) and truecolor blocks (`\x1b[38;2;` and `█` or `▀`). A second concurrent client must receive its own banner and blocks. `q` ends the client. Skip otherwise.

## Out of scope

- The `bootleg-play` crate, its export command, and local one-shot playback.
- A shared playhead across viewers.
- Replacing ffmpeg with an in-process decoder.
- Changing the grid when the terminal is resized.
- A fixed cap on connection count. Limits are memory, the ANSI budget, file descriptors, and the network.
- Renaming the git remote.
