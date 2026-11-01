# bootleg-play Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a separate `bootleg-play` binary that bakes a clip into a copy of itself, plays that clip once in the local terminal, and exits.

**Architecture:** The player crate owns its own grid and renderer. `export` appends raw RGB frames and a 52-byte footer to a copy of the executable. On startup a valid footer selects play mode and ignores argv. There is no SSH code and no shared library with `server`.

**Tech Stack:** Rust edition 2021, libc, signal-hook, ffmpeg/ffprobe at export time, portable-pty for the integration test.

## Global Constraints

- Package path `player/`. Binary name `bootleg-play`. Do not link russh, tokio, or the `server` crate.
- Usage line, exit 2 when there is no footer and the args are not `export`: `bootleg-play export --video CLIP -o PATH [--fps N] [--grid-cols N]`
- `--video` and `-o` are required. `--fps` defaults to `0` (source rate). `--grid-cols` defaults to `160`.
- A valid footer selects play mode and argv is ignored. Refuse `-o` equal to `current_exe()` with exit 1.
- Footer is 52 little-endian bytes at EOF. Both magics are the eight bytes `BOOTLEG1`. Fields in order: magic, `src_width` u32, `src_height` u32, `ref_cols` u32, `ref_rows` u32, `fps` f64, `frame_count` u32, `payload_len` u64, magic.
- Payload is `frame_count` frames of packed RGB24, `ref_cols * ref_rows * 3` bytes each, immediately before the footer. Valid only when both magics match, `fps > 0`, `ref_cols > 0`, `ref_rows > 0`, `frame_count > 0`, `payload_len` equals `frame_count * ref_cols * ref_rows * 3`, and the file is long enough.
- `ref_rows = max(1, round_half_even((src_height / src_width) * grid_cols / 2) * 2)`.
- Grid and render rules match the server spec: sample the stored frame, full block `█` when the pixel pair matches, half block `▀` otherwise, rows joined with `\x1b[E`.
- Play setup bytes: `\x1b[?1049h\x1b[2J\x1b[?25l\x1b[?7l\x1b[0m` then `\x1b[H` and frame 0. No banner.
- Restore bytes: `\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l`, plus the original termios, on every exit path.
- Clock starts after frame 0. `idx = floor(elapsed_seconds * fps)`. When `idx >= frame_count`, exit. Do not wrap. A late wake writes only the index for now.
- `q` at the start of a chunk, `\x03`, `\x04`, `\x1b[99;5u`, `\x1b[67;5u`, `\x1b[3;5u`, SIGINT, and SIGTERM exit 0 after restore. A finished clip exits 0. No terminal exits 1.
- Example unit path: `docs/examples/bootleg-play.tty1.service`. It is not installed. `ExecStart=/opt/bootleg-intro` is the documented placeholder path for the exported file.
- The server image must still build after this crate joins the workspace, so the Dockerfile copies `player/` as well.

---

### Task 1: Crate and renderer

**Files:**
- Modify: `Cargo.toml`
- Create: `player/Cargo.toml`
- Create: `player/src/lib.rs`
- Create: `player/src/render.rs`

**Interfaces:**
- Consumes: workspace `Cargo.toml` if the server plan already created it
- Produces:
  - `pub fn round_half_even(value: f64) -> i64`
  - `pub fn grid(term_cols: u32, term_rows: u32, ref_cols: u32, ref_rows: u32) -> (u32, u32)`
  - `pub struct Frame { pub width: usize, pub height: usize, pub rgb: Vec<u8> }`
  - `pub fn render_frame(frame: &Frame, cols: usize, rows: usize) -> Vec<u8>`

- [ ] **Step 1: Write the failing test**

If `Cargo.toml` does not exist, create it with `members = ["player"]`. If it exists and `members` is `["server"]`, change it to `["server", "player"]`. Keep `resolver = "2"` and the release profile:

```toml
[workspace]
resolver = "2"
members = ["server", "player"]

[profile.release]
lto = true
codegen-units = 1
```

Create `player/Cargo.toml`:

```toml
[package]
name = "bootleg-play"
version = "0.1.0"
edition = "2021"
license = "MIT"
publish = false

[dependencies]
libc = "0.2"
signal-hook = "0.3"

[dev-dependencies]
proptest = "1"
portable-pty = "0.9"
```

Create `player/src/lib.rs` with `pub mod render;`.

Create `player/src/render.rs` with this test module and no functions yet:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::Config as ProptestConfig;

    fn solid(h: usize, w: usize, rgb: [u8; 3]) -> Frame {
        let mut raw = vec![0u8; h * w * 3];
        for px in raw.chunks_exact_mut(3) { px.copy_from_slice(&rgb); }
        Frame { width: w, height: h, rgb: raw }
    }

    fn stacked(h: usize, w: usize, top: [u8; 3], bot: [u8; 3]) -> Frame {
        let mut frame = solid(h, w, bot);
        for y in (0..h).step_by(2) {
            for x in 0..w {
                let i = (y * w + x) * 3;
                frame.rgb[i..i + 3].copy_from_slice(&top);
            }
        }
        frame
    }

    #[test]
    fn half_even_ties_go_to_even() {
        assert_eq!(round_half_even(0.5), 0);
        assert_eq!(round_half_even(1.5), 2);
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(4.5), 4);
    }

    #[test]
    fn exact_rows() {
        assert_eq!(
            render_frame(&solid(2, 2, [10, 20, 30]), 2, 1),
            "\u{1b}[49m\u{1b}[38;2;10;20;30m██\u{1b}[0m".as_bytes()
        );
        assert_eq!(
            render_frame(&stacked(2, 2, [255, 0, 0], [0, 0, 255]), 2, 1),
            "\u{1b}[38;2;255;0;0m\u{1b}[48;2;0;0;255m▀▀\u{1b}[0m".as_bytes()
        );
    }

    #[test]
    fn aspect_uses_half_even_on_4_5() {
        assert_eq!(grid(16, 100, 40, 6), (15, 4));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(25))]
        #[test]
        fn cell_count_matches_grid(cols in 8usize..25, rows in 4usize..13, r in 0u8..=255, g in 0u8..=255, b in 0u8..=255) {
            let out = render_frame(&solid(rows * 2, cols, [r, g, b]), cols, rows);
            let full = "█".as_bytes();
            let half = "▀".as_bytes();
            let cells = out.windows(full.len()).filter(|w| *w == full).count()
                + out.windows(half.len()).filter(|w| *w == half).count();
            prop_assert_eq!(cells, cols * rows);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg-play --lib render -- --test-threads=8`

Expected: FAIL to compile. The functions are missing.

- [ ] **Step 3: Write minimal implementation**

Put this implementation above the tests. `grid` uses `max(10).min(ref_cols)`, not `clamp`, because `ref_cols` may be below 10.

```rust
#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

pub fn round_half_even(value: f64) -> i64 {
    let floor = value.floor();
    let frac = value - floor;
    if (frac - 0.5).abs() <= 1e-9 {
        let down = floor as i64;
        if down % 2 == 0 { down } else { down + 1 }
    } else if frac > 0.5 {
        floor as i64 + 1
    } else {
        floor as i64
    }
}

pub fn grid(term_cols: u32, term_rows: u32, ref_cols: u32, ref_rows: u32) -> (u32, u32) {
    let cols = term_cols.saturating_sub(1).max(10).min(ref_cols);
    let aspect = round_half_even((ref_rows as f64 / ref_cols as f64) * 2.0 * cols as f64).max(0) as u32;
    let rows = aspect.max(4).min(term_rows.saturating_sub(1).max(4));
    (cols, rows)
}

pub fn render_frame(frame: &Frame, cols: usize, rows: usize) -> Vec<u8> {
    let dec_h = (rows * 2).max(2);
    let step_y = round_half_even(frame.height as f64 / dec_h as f64).max(1) as usize;
    let step_x = round_half_even(frame.width as f64 / cols as f64).max(1) as usize;
    let mut ys = Vec::new();
    let mut y = 0;
    while y < frame.height {
        ys.push(y);
        y += step_y;
    }
    ys.truncate(dec_h);
    let mut xs = Vec::new();
    let mut x = 0;
    while x < frame.width {
        xs.push(x);
        x += step_x;
    }
    xs.truncate(cols);
    if ys.is_empty() || xs.is_empty() {
        return Vec::new();
    }
    let last_y = *ys.last().unwrap();
    let last_x = *xs.last().unwrap();
    while ys.len() < dec_h { ys.push(last_y); }
    while xs.len() < cols { xs.push(last_x); }
    let mut pixels = vec![[0u8; 3]; dec_h * cols];
    for (iy, sy) in ys.iter().enumerate() {
        for (ix, sx) in xs.iter().enumerate() {
            let i = (*sy * frame.width + *sx) * 3;
            pixels[iy * cols + ix] = [frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2]];
        }
    }
    let mut char_rows = dec_h / 2 + usize::from(dec_h % 2 == 1);
    let mut lines = Vec::with_capacity(char_rows);
    for r in 0..char_rows {
        let top_y = (r * 2).min(dec_h - 1);
        let bot_y = if r * 2 + 1 < dec_h { r * 2 + 1 } else { top_y };
        let mut line = Vec::new();
        let mut run_start = 0;
        for x in 0..=cols {
            let split = x == cols
                || pixels[top_y * cols + x] != pixels[top_y * cols + run_start]
                || pixels[bot_y * cols + x] != pixels[bot_y * cols + run_start];
            if x > run_start && split {
                let top = pixels[top_y * cols + run_start];
                let bot = pixels[bot_y * cols + run_start];
                let run = x - run_start;
                if top == bot {
                    line.extend(format!("\u{1b}[49m\u{1b}[38;2;{};{};{}m", top[0], top[1], top[2]).into_bytes());
                    for _ in 0..run { line.extend("█".as_bytes()); }
                } else {
                    line.extend(format!(
                        "\u{1b}[38;2;{};{};{}m\u{1b}[48;2;{};{};{}m",
                        top[0], top[1], top[2], bot[0], bot[1], bot[2]
                    ).into_bytes());
                    for _ in 0..run { line.extend("▀".as_bytes()); }
                }
                run_start = x;
            }
        }
        line.extend(b"\x1b[0m");
        lines.push(line);
    }
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 { out.extend(b"\x1b[E"); }
        out.extend(line);
    }
    out
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg-play --lib render -- --test-threads=8`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock player/Cargo.toml player/src/lib.rs player/src/render.rs
git commit -m "Add the bootleg-play crate and its block renderer."
```

---

### Task 2: Footer

**Files:**
- Create: `player/src/footer.rs`
- Modify: `player/src/lib.rs`

**Interfaces:**
- Consumes: nothing from the renderer
- Produces:
  - `pub const FOOTER_LEN: usize` = 52
  - `pub struct Meta { pub src_width: u32, pub src_height: u32, pub ref_cols: u32, pub ref_rows: u32, pub fps: f64, pub frame_count: u32 }`
  - `pub fn append(stub: &[u8], frames: &[u8], meta: &Meta) -> Vec<u8>`
  - `pub fn read(file: &[u8]) -> Option<(Meta, std::ops::Range<usize>)>` where the range is the payload inside `file`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_reject_a_plain_stub() {
        let stub = b"not-a-real-elf-but-long-enough";
        let payload = vec![7u8; 2 * 2 * 2 * 3];
        let meta = Meta { src_width: 64, src_height: 36, ref_cols: 2, ref_rows: 2, fps: 8.0, frame_count: 2 };
        let file = append(stub, &payload, &meta);
        let (got, range) = read(&file).unwrap();
        assert_eq!(got.frame_count, 2);
        assert_eq!(got.fps, 8.0);
        assert_eq!(&file[range], payload.as_slice());
        assert!(read(stub).is_none());
    }

    #[test]
    fn reject_a_short_payload_length() {
        let mut file = append(b"stub", &[0u8; 12], &Meta {
            src_width: 1, src_height: 1, ref_cols: 2, ref_rows: 2, fps: 8.0, frame_count: 2,
        });
        let len_at = file.len() - 8 - 8;
        file[len_at..len_at + 8].copy_from_slice(&1u64.to_le_bytes());
        assert!(read(&file).is_none());
    }
}
```

Add `pub mod footer;` to `player/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg-play --lib footer -- --test-threads=8`

Expected: FAIL to compile.

- [ ] **Step 3: Write minimal implementation**

```rust
pub const FOOTER_LEN: usize = 52;
const MAGIC: &[u8; 8] = b"BOOTLEG1";

#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub src_width: u32,
    pub src_height: u32,
    pub ref_cols: u32,
    pub ref_rows: u32,
    pub fps: f64,
    pub frame_count: u32,
}

pub fn append(stub: &[u8], frames: &[u8], meta: &Meta) -> Vec<u8> {
    let mut out = Vec::with_capacity(stub.len() + frames.len() + FOOTER_LEN);
    out.extend_from_slice(stub);
    out.extend_from_slice(frames);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&meta.src_width.to_le_bytes());
    out.extend_from_slice(&meta.src_height.to_le_bytes());
    out.extend_from_slice(&meta.ref_cols.to_le_bytes());
    out.extend_from_slice(&meta.ref_rows.to_le_bytes());
    out.extend_from_slice(&meta.fps.to_le_bytes());
    out.extend_from_slice(&meta.frame_count.to_le_bytes());
    out.extend_from_slice(&(frames.len() as u64).to_le_bytes());
    out.extend_from_slice(MAGIC);
    out
}

pub fn read(file: &[u8]) -> Option<(Meta, std::ops::Range<usize>)> {
    if file.len() < FOOTER_LEN { return None; }
    let start = file.len() - FOOTER_LEN;
    let f = &file[start..];
    if &f[0..8] != MAGIC || &f[44..52] != MAGIC { return None; }
    let src_width = u32::from_le_bytes(f[8..12].try_into().ok()?);
    let src_height = u32::from_le_bytes(f[12..16].try_into().ok()?);
    let ref_cols = u32::from_le_bytes(f[16..20].try_into().ok()?);
    let ref_rows = u32::from_le_bytes(f[20..24].try_into().ok()?);
    let fps = f64::from_le_bytes(f[24..32].try_into().ok()?);
    let frame_count = u32::from_le_bytes(f[32..36].try_into().ok()?);
    let payload_len = u64::from_le_bytes(f[36..44].try_into().ok()?) as usize;
    if fps <= 0.0 || ref_cols == 0 || ref_rows == 0 || frame_count == 0 { return None; }
    let expect = frame_count as usize * ref_cols as usize * ref_rows as usize * 3;
    if payload_len != expect || start < payload_len { return None; }
    let payload = start - payload_len..start;
    Some((Meta { src_width, src_height, ref_cols, ref_rows, fps, frame_count }, payload))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg-play --lib footer -- --test-threads=8`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add player/src/footer.rs player/src/lib.rs
git commit -m "Append and read the bootleg-play frame footer."
```

---

### Task 3: One-shot clock

**Files:**
- Create: `player/src/clock.rs`
- Modify: `player/src/lib.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub enum Tick { Wait, Show(usize), Done }`
  - `pub fn tick(elapsed_secs: f64, fps: f64, frame_count: usize, last: usize) -> Tick`

`last` starts at 0 because frame 0 was already shown.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_time_plays_each_remaining_frame_then_stops() {
        let fps = 8.0;
        let n = 4;
        assert!(matches!(tick(0.0, fps, n, 0), Tick::Wait));
        assert!(matches!(tick(1.0 / fps, fps, n, 0), Tick::Show(1)));
        assert!(matches!(tick(2.0 / fps, fps, n, 1), Tick::Show(2)));
        assert!(matches!(tick(3.0 / fps, fps, n, 2), Tick::Show(3)));
        assert!(matches!(tick(4.0 / fps, fps, n, 3), Tick::Done));
    }

    #[test]
    fn late_wake_shows_only_now() {
        assert!(matches!(tick(3.2 / 8.0, 8.0, 10, 0), Tick::Show(3)));
    }

    #[test]
    fn does_not_wrap() {
        assert!(matches!(tick(5.0, 8.0, 4, 3), Tick::Done));
    }
}
```

Add `pub mod clock;`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg-play --lib clock -- --test-threads=8`

Expected: FAIL to compile.

- [ ] **Step 3: Write minimal implementation**

```rust
pub enum Tick { Wait, Show(usize), Done }

pub fn tick(elapsed_secs: f64, fps: f64, frame_count: usize, last: usize) -> Tick {
    let idx = (elapsed_secs * fps).floor() as usize;
    if idx >= frame_count { Tick::Done }
    else if idx != last { Tick::Show(idx) }
    else { Tick::Wait }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg-play --lib clock -- --test-threads=8`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add player/src/clock.rs player/src/lib.rs
git commit -m "Stop bootleg-play after the last frame."
```

---

### Task 4: Export and local playback

**Files:**
- Create: `player/src/decode.rs`
- Create: `player/src/play.rs`
- Create: `player/src/main.rs`
- Create: `player/tests/export.rs`
- Modify: `player/src/lib.rs`

**Interfaces:**
- Consumes: `Frame`, `render_frame`, `grid`, `Meta`, `append`, `read`, `tick`
- Produces:
  - `pub async` is not used. `pub fn decode(path: &Path, grid_cols: u32, fps_override: f64) -> Result<Decoded, String>`
  - `pub struct Decoded { pub meta: Meta, pub frames: Vec<Frame> }`
  - `pub fn export(video: &Path, out: &Path, fps: f64, grid_cols: u32) -> Result<(), String>`
  - `pub fn play(meta: &Meta, frames: &[Frame]) -> i32`
  - `main` dispatches footer vs export vs usage

- [ ] **Step 1: Write the failing test**

Create `player/tests/export.rs`:

```rust
use std::path::PathBuf;
use std::process::Command;

#[test]
fn export_writes_a_footer_for_a_generated_clip() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skip: ffmpeg is required");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bootleg-play-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mp4");
    let out = dir.join("play-once");
    assert!(Command::new("ffmpeg").args([
        "-y", "-v", "error", "-f", "lavfi", "-i",
        "testsrc=size=64x36:rate=8:duration=0.5", "-pix_fmt", "yuv420p",
    ]).arg(&clip).status().unwrap().success());
    let bin = env!("CARGO_BIN_EXE_bootleg-play");
    let status = Command::new(bin)
        .args(["export", "--video"]).arg(&clip)
        .arg("-o").arg(&out)
        .args(["--fps", "8", "--grid-cols", "32"])
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&out).unwrap();
    let (meta, _) = bootleg_play::footer::read(&bytes).unwrap();
    assert_eq!((meta.src_width, meta.src_height), (64, 36));
    assert_eq!(meta.ref_cols, 32);
    assert_eq!(meta.ref_rows % 2, 0);
    assert!(meta.frame_count >= 3);
    assert_eq!(meta.fps, 8.0);
}

#[test]
fn export_refuses_to_overwrite_itself() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_bootleg-play"));
    let status = Command::new(&bin)
        .args(["export", "--video", "unused.mp4", "-o"]).arg(&bin)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}
```

`CARGO_BIN_EXE_bootleg-play` requires a `src/main.rs` binary. Create a stub `main` that exits 2 so the test binary exists and fails the export assertion.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg-play --test export -- --test-threads=1`

Expected: FAIL. The stub does not export.

- [ ] **Step 3: Write minimal implementation**

`player/src/decode.rs` provides `decode`, `export`, and `run_args`. `decode` runs ffprobe for width, height, and `r_frame_rate`, then:

```rust
let ref_rows = crate::render::round_half_even(
    (height as f64 / width as f64) * grid_cols as f64 / 2.0,
).max(0) as u32 * 2;
let ref_rows = ref_rows.max(1);
```

ffmpeg args are `-v error -i PATH -vf scale={grid_cols}:{ref_rows}:flags=area -f rawvideo -pix_fmt rgb24 -`. Read `grid_cols * ref_rows * 3` bytes per frame until a short read. Drop stdout, wait up to 3 seconds, then `child.kill()`. `fps_override == 0.0` keeps the probed rate. `export` reads `current_exe()`, returns an error whose `run_args` maps to exit 1 when `out` is that path, copies those bytes, appends the packed frames through `footer::append`, writes `out`, and sets permissions `0o755`.

`export.rs` can live inside `decode.rs` as `export()`. It reads `std::env::current_exe()`, returns `Err` when `out` is that path, copies the exe bytes, decodes, concatenates frames in order, calls `footer::append`, writes the file, and sets mode `0o755`.

`play.rs`:

```rust
pub fn play(meta: &footer::Meta, frames: &[render::Frame]) -> i32 {
    let size = match winsize() {
        Some(s) => s,
        None => {
            eprintln!("bootleg-play: stdout is not a terminal");
            return 1;
        }
    };
    let quit = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, quit.clone());
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, quit.clone());
    let _guard = Restore(raw_mode());
    let (cols, rows) = render::grid(size.0, size.1, meta.ref_cols, meta.ref_rows);
    let cols = cols as usize;
    let rows = rows as usize;
    write_all(b"\x1b[?1049h\x1b[2J\x1b[?25l\x1b[?7l\x1b[0m");
    let frame0 = render::render_frame(&frames[0], cols, rows);
    write_all(b"\x1b[H");
    write_all(&frame0);
    let t0 = std::time::Instant::now();
    let mut last = 0usize;
    let fps = meta.fps;
    let n = frames.len();
    loop {
        if quit.load(std::sync::atomic::Ordering::Relaxed) || stdin_quit() {
            break;
        }
        match clock::tick(t0.elapsed().as_secs_f64(), fps, n, last) {
            clock::Tick::Done => break,
            clock::Tick::Wait => {}
            clock::Tick::Show(idx) => {
                let frame = render::render_frame(&frames[idx], cols, rows);
                write_all(b"\x1b[H");
                write_all(&frame);
                last = idx;
            }
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let next = (elapsed * fps).floor() + 1.0;
        let delay = (next / fps - elapsed).max(0.0);
        let slice = delay.min(0.05);
        std::thread::sleep(std::time::Duration::from_secs_f64(slice));
    }
    0
}
```

Do not add a `scopeguard` crate. Implement the guard locally:

```rust
struct Restore(Option<libc::termios>);
impl Drop for Restore {
    fn drop(&mut self) {
        if let Some(state) = self.0.take() {
            unsafe { libc::tcsetattr(1, libc::TCSANOW, &state); }
        }
        write_all(b"\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l");
    }
}
```

`winsize` uses `libc::ioctl` on fd 1 with `TIOCGWINSZ`. Return `None` when that fails. `raw_mode` reads termios on fd 1, clears `ICANON | ECHO`, and sets `O_NONBLOCK` on fd 0. `stdin_quit` reads available bytes and calls the same quit rules as the server (`q` prefix, ETX, EOT, the three CSI-u sequences). `EAGAIN` means no input.

`main.rs`:

```rust
fn main() {
    let exe = std::env::current_exe().expect("exe");
    let bytes = std::fs::read(&exe).expect("read exe");
    if let Some((meta, range)) = bootleg_play::footer::read(&bytes) {
        let frames = frames_from_payload(&meta, &bytes[range]);
        std::process::exit(bootleg_play::play::play(&meta, &frames));
    }
    match bootleg_play::decode::run_args(std::env::args().skip(1)) {
        Ok(()) => {}
        Err(Error::Usage) => {
            eprintln!("bootleg-play export --video CLIP -o PATH [--fps N] [--grid-cols N]");
            std::process::exit(2);
        }
        Err(Error::Fail(msg)) => {
            eprintln!("bootleg-play: {msg}");
            std::process::exit(1);
        }
    }
}
```

`run_args` accepts only `export --video CLIP -o PATH` with optional `--fps` and `--grid-cols`. Anything else is `Error::Usage`. A decode failure is `Error::Fail`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg-play --test export -- --test-threads=1`

Expected: PASS when ffmpeg is installed. The self-overwrite test passes either way.

- [ ] **Step 5: Commit**

```bash
git add player/src/decode.rs player/src/play.rs player/src/main.rs player/src/lib.rs player/tests/export.rs
git commit -m "Export a one-shot bootleg-play binary and play it back."
```

---

### Task 5: PTY playback test, docs, and the server image

**Files:**
- Create: `player/tests/play_pty.rs`
- Create: `docs/examples/bootleg-play.tty1.service`
- Modify: `README.md`
- Modify: `Dockerfile`

**Interfaces:**
- Consumes: the `bootleg-play` binary and `footer::read`
- Produces: the documented systemd example and a server image build that still sees the workspace member

- [ ] **Step 1: Write the failing test**

```rust
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Read;
use std::process::Command;

#[test]
fn exported_file_plays_blocks_and_exits() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skip: ffmpeg is required");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bootleg-pty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mp4");
    let out = dir.join("intro");
    assert!(Command::new("ffmpeg").args([
        "-y", "-v", "error", "-f", "lavfi", "-i",
        "color=c=red:s=64x36:r=8:d=0.5", "-pix_fmt", "yuv420p",
    ]).arg(&clip).status().unwrap().success());
    assert!(Command::new(env!("CARGO_BIN_EXE_bootleg-play"))
        .args(["export", "--video"]).arg(&clip)
        .arg("-o").arg(&out)
        .args(["--fps", "8", "--grid-cols", "16"])
        .status().unwrap().success());
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(out);
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let status = child.wait().unwrap();
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    assert!(status.success());
    assert!(text.contains("\u{1b}[38;2;"));
    assert!(text.contains('█') || text.contains('▀'));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg-play --test play_pty -- --test-threads=1`

Expected: FAIL until playback under a PTY exits 0 with block bytes. If Task 4 already passes this, keep the test and confirm PASS in Step 4.

- [ ] **Step 3: Write the docs and Dockerfile change**

Create `docs/examples/bootleg-play.tty1.service`:

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

Add a README section:

```markdown
## One-shot player

`bootleg-play` is a separate binary. It does not serve SSH.

```bash
cargo build -p bootleg-play --release
./target/release/bootleg-play export --video clip.mp4 -o intro
./intro
```

`intro` plays the clip once in this terminal and exits. Copy that file onto a machine that has no ffmpeg. `docs/examples/bootleg-play.tty1.service` is an example oneshot that runs it on tty1 before getty. `/opt/bootleg-intro` in that unit is the path you chose for the exported file.
```

In `Dockerfile`, after `COPY server ./server`, add:

```dockerfile
COPY player ./player
```

The build command stays `cargo build --release -p bootleg`. The runtime image still contains only `/app/bootleg`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg-play`

Expected: PASS. The PTY test skips only when ffmpeg is missing.

Run: `cargo test -p bootleg`

Expected: the server package still passes. The workspace now has two members.

- [ ] **Step 5: Commit**

```bash
git add player/tests/play_pty.rs docs/examples/bootleg-play.tty1.service README.md Dockerfile
git commit -m "Document the one-shot bootleg player and cover it with a PTY test."
```
