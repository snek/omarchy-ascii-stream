# bootleg-play

Date: 2026-10-31

`bootleg-play` is a second workspace crate. It plays one clip in the local terminal and then exits. The clip is baked into a copy of that binary so the target machine needs no ffmpeg and no video file beside it. The crate does not link the SSH server, the ANSI cache, or Tokio.

The server is specified in [2026-10-31-bootleg-server-design.md](2026-10-31-bootleg-server-design.md). The two crates do not share a library. This crate owns its own copy of the grid and render rules below, which match the server on purpose.

## Goals

- One executable a boot script can run, including an Ubuntu startup unit on tty1.
- Play the clip once from frame 0 and exit.
- Build the player once, then bake a new clip by appending bytes. The compiler does not run per clip.

## Repository

Workspace member:

```
player/          package bootleg-play, binary bootleg-play
```

The server image does not contain this binary. Export runs on a machine that has `bootleg-play` and ffmpeg.

## Modes

Open `current_exe()` and read the footer described below.

- A valid footer selects play mode. Arguments are ignored.
- Otherwise, `bootleg-play export …` selects export mode.
- Otherwise print usage to stderr and exit 2.

Usage line:

```
bootleg-play export --video CLIP -o PATH [--fps N] [--grid-cols N]
```

## Export

`--video` and `-o` are required. `--fps` defaults to `0` (source rate). `--grid-cols` defaults to `160`.

Probe and decode match the server:

- ffprobe supplies width, height, and `r_frame_rate` (`num/den`).
- `ref_rows = max(1, round_half_even((src_height / src_width) * grid_cols / 2) * 2)`.
- ffmpeg scales with `scale={ref_cols}:{ref_rows}:flags=area` and writes `rgb24` to stdout.
- The frame count is the number of raw frames read. On a short read, stop. If ffmpeg does not exit, terminate it, wait 3 seconds, then kill it.

Refuse to write `-o` when that path is `current_exe()`, and exit 1. On success, copy `current_exe()` to `-o`, append the packed frames and the footer, mark the file executable, and exit 0. A missing clip, a probe failure, an ffmpeg failure, or zero frames exits 1.

## Footer

Little-endian, 52 bytes, sitting at the end of the file. Both magic fields are the eight bytes `BOOTLEG1`.

| Field | Type |
|---|---|
| magic | 8 bytes |
| src_width | u32 |
| src_height | u32 |
| ref_cols | u32 |
| ref_rows | u32 |
| fps | f64 |
| frame_count | u32 |
| payload_len | u64 |
| magic | 8 bytes |

The payload is `frame_count` frames of packed RGB24, `ref_cols * ref_rows * 3` bytes each, immediately before the footer. A footer is valid only when both magics match, `fps > 0`, `ref_cols > 0`, `ref_rows > 0`, `frame_count > 0`, `payload_len` equals `frame_count * ref_cols * ref_rows * 3`, and the file is long enough to hold the payload plus the footer. The unexported binary fails this check.

## Grid

Read the terminal size with `TIOCGWINSZ` on stdout. The grid uses the baked `ref_cols` and `ref_rows`:

```
cols = min(max(10, saturating_sub(term_cols, 1)), ref_cols)
aspect_rows = max(4, round_half_even((ref_rows as f64 / ref_cols as f64) * 2.0 * cols as f64))
rows = min(aspect_rows, max(4, saturating_sub(term_rows, 1)))
```

`saturating_sub` stops at 0. `round_half_even` is Python 3 `round()`.

## Render

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

Append `\x1b[0m` to each character row. Join character rows with `\x1b[E`.

## Playback

Stdout must be a terminal. If it is not, write a one-line error to stderr and exit 1.

Put the terminal in raw mode with no echo. A drop guard restores termios and writes `\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l` on every exit path.

On start, write `\x1b[?1049h\x1b[2J\x1b[?25l\x1b[?7l\x1b[0m`, then `\x1b[H` and frame 0. There is no status banner.

`t0` is a monotonic instant taken after frame 0 is written. `last` starts at `0`.

```
idx = floor(elapsed_seconds * fps)
```

When `idx >= frame_count`, restore the terminal and exit 0. When `idx != last` and `idx < frame_count`, write `\x1b[H` plus that frame and set `last = idx`. Sleep until `t0 + (floor(elapsed_seconds * fps) + 1) / fps`. A non-positive delay yields once. The index does not wrap.

`q` at the start of a stdin chunk, bytes `\x03`, `\x04`, `\x1b[99;5u`, `\x1b[67;5u`, or `\x1b[3;5u`, plus SIGINT and SIGTERM, set a quit flag. The loop restores the terminal and exits 0. A finished clip also exits 0.

## systemd example

`docs/examples/bootleg-play.tty1.service` is an example, not installed by the build. It is a oneshot that finishes before getty takes tty1:

```
[Unit]
Description=Play a bootleg clip once on tty1
Before=getty@tty1.service
After=systemd-vconsole-setup.service

[Service]
Type=oneshot
ExecStart=/opt/bootleg-intro
StandardInput=tty
StandardOutput=tty
TTYPath=/dev/tty1
TTYReset=yes
TTYVHangup=yes

[Install]
WantedBy=multi-user.target
```

`/opt/bootleg-intro` is a placeholder for the exported file. The root README links to this unit and shows the export command.

## Tests

`cargo test -p bootleg-play`.

Always run, with in-memory frames:

- Footer round-trip: write a stub plus payload, read it back, and reject a stub that has no footer.
- A solid color is full blocks only. Stacked even/odd colors are half blocks only. Cell count equals `cols * rows`.
- Frame 0 is sent before the clock starts. On-time wakes then send `1 .. frame_count` (end excluded) and stop. A late wake sends only the index for "now", or nothing when that index is past the last frame. The index does not wrap.
- Export refuses an `-o` path equal to `current_exe()`.

When `ffmpeg` and `ffprobe` are on `PATH`, export a generated `testsrc` of 64×36 at 8 fps (4 frames) with `--grid-cols 32`. The output footer reports source 64×36, an even `ref_rows`, and at least 3 frames. Skip otherwise.

When those tools and a PTY are available, run the exported file under a PTY. The process exits 0, and the captured output contains `\x1b[38;2;` and `█` or `▀`. Skip otherwise.

## Out of scope

- SSH, authentication, and listening on a port.
- Looping the clip.
- A status banner.
- Sharing Rust code with the server crate.
- Baking `bootleg-play` into the server image.
- Installing the systemd unit.
