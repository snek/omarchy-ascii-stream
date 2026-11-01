# bootleg SSH Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Python SSH server with a Rust binary named `bootleg` that streams a separate clip file to each login from frame 0.

**Architecture:** A Cargo workspace member `server` decodes the clip once with ffmpeg, renders truecolor block ANSI into a shared cache, and serves one russh session per connection. Each session has its own monotonic clock. The player crate is not part of this plan.

**Tech Stack:** Rust edition 2021, Tokio, russh (docs.rs/russh latest Handler API: `auth_none`, `ChannelOpenHandle`, `Handle::data`), bytes, proptest, ffmpeg/ffprobe, Docker.

## Global Constraints

- Binary name `bootleg`. Package path `server/`. Workspace member `server` only.
- Banner byte string: `\x1b[?1049h\x1b[2J\x1b[?25l\x1b[?7l\x1b[0m\r\n\x1b[38;2;255;255;255mbootleg  ·  {width}x{height}  ·  {n} frames @ {fps}fps  ·  'q' quits\x1b[0m\r\n\r\n`
- `{fps}` in the banner uses Python 3 `{fps:.0f}` via `round_half_even`.
- Logs: `[bootleg] decoding {path} ...`, `[bootleg] {n} frames @ {fps:.2f}fps ref {cols}x{rows}`, `[bootleg] listening on {host}:{port}`.
- Env defaults: `VIDEO_PATH=/clips/video.mp4`, `HOST=0.0.0.0`, `PORT=2222`, `FPS=0`, `GRID_COLS=160`, `HOST_KEY=./host_key`, `ANSI_CACHE_MIB=512`.
- `ref_rows = max(1, round_half_even((src_height / src_width) * GRID_COLS / 2) * 2)`.
- Grid: `cols = min(max(10, saturating_sub(term_cols, 1)), ref_cols)`, `aspect_rows = max(4, round_half_even((ref_rows / ref_cols) * 2 * cols))`, `rows = min(aspect_rows, max(4, saturating_sub(term_rows, 1)))`.
- Sample the stored frame (`ref_rows` by `ref_cols`), not the source pixel size. `dec_h = max(2, rows * 2)`. `step_y = max(1, round_half_even(ref_rows / dec_h))`. `step_x = max(1, round_half_even(ref_cols / cols))`.
- Full block when top RGB equals bottom RGB: `\x1b[49m\x1b[38;2;R;G;Bm` plus `█` times the run. Otherwise `\x1b[38;2;Rt;Gt;Btm\x1b[48;2;Rb;Gb;Bbm` plus `▀` times the run. End each character row with `\x1b[0m`. Join rows with `\x1b[E`.
- Restore sequence: `\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l`.
- Quit when bytes contain `\x03`, `\x04`, `\x1b[99;5u`, `\x1b[67;5u`, or `\x1b[3;5u`, or the chunk starts with `q`. Also INT, TERM, QUIT, HUP, and channel EOF. Empty input and any other chunk stay connected. russh has no break callback; Ctrl+C arrives as `\x03`.
- Each session clock starts after frame 0 is written. `idx = floor(elapsed_seconds * fps) mod frame_count`. Send only the new index. Sleep until `t0 + (floor(elapsed * fps) + 1) / fps`. A non-positive delay yields once.
- First terminal size is always cached. Later sizes are cached only while retained ANSI bytes are under the budget. Over budget, snap to smallest `|Δcols|`, then smallest `|Δrows|`, then more cells, then the earlier entry.
- Image `bootleg:0.1.0`, service name `bootleg`, `CMD /app/bootleg`, install path in docs `/opt/bootleg`. Git remote URL stays `https://github.com/snek/omarchy-ascii-stream.git`.
- Do not add the `player` crate in this plan.

---

### Task 1: Workspace and block renderer

**Files:**
- Create: `Cargo.toml`
- Create: `server/Cargo.toml`
- Create: `server/src/lib.rs`
- Create: `server/src/render.rs`
- Test: `server/src/render.rs` (`mod tests`)

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub fn round_half_even(value: f64) -> i64`
  - `pub fn grid(term_cols: u32, term_rows: u32, ref_cols: u32, ref_rows: u32) -> (u32, u32)`
  - `pub struct Frame { pub width: usize, pub height: usize, pub rgb: Vec<u8> }`
  - `pub fn render_frame(frame: &Frame, cols: usize, rows: usize) -> Vec<u8>`

- [ ] **Step 1: Write the failing test**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["server"]

[profile.release]
lto = true
codegen-units = 1
```

Create `server/Cargo.toml`:

```toml
[package]
name = "bootleg"
version = "0.1.0"
edition = "2021"
license = "MIT"
publish = false

[dependencies]

[dev-dependencies]
proptest = "1"
```

Create `server/src/lib.rs`:

```rust
pub mod render;
```

Create `server/src/render.rs` with only the tests below, calling functions that do not exist yet:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::Config as ProptestConfig;

    fn solid(h: usize, w: usize, rgb: [u8; 3]) -> Frame {
        let mut frame = vec![0u8; h * w * 3];
        for px in frame.chunks_exact_mut(3) {
            px.copy_from_slice(&rgb);
        }
        Frame { width: w, height: h, rgb: frame }
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

    fn cell_count(ansi: &[u8]) -> usize {
        let full = "█".as_bytes();
        let half = "▀".as_bytes();
        ansi.windows(full.len()).filter(|w| *w == full).count()
            + ansi.windows(half.len()).filter(|w| *w == half).count()
    }

    #[test]
    fn half_even_ties_go_to_even() {
        assert_eq!(round_half_even(0.5), 0);
        assert_eq!(round_half_even(1.5), 2);
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(4.5), 4);
        assert_eq!(round_half_even(2.6), 3);
    }

    #[test]
    fn exact_solid_row_is_one_sgr_run() {
        let frame = solid(2, 2, [10, 20, 30]);
        let out = render_frame(&frame, 2, 1);
        let expect = "\u{1b}[49m\u{1b}[38;2;10;20;30m██\u{1b}[0m";
        assert_eq!(out, expect.as_bytes());
    }

    #[test]
    fn exact_stacked_row_is_half_blocks() {
        let frame = stacked(2, 2, [255, 0, 0], [0, 0, 255]);
        let out = render_frame(&frame, 2, 1);
        let expect = "\u{1b}[38;2;255;0;0m\u{1b}[48;2;0;0;255m▀▀\u{1b}[0m";
        assert_eq!(out, expect.as_bytes());
    }

    #[test]
    fn solid_16_has_full_blocks_and_three_line_breaks() {
        let frame = solid(16, 16, [255, 0, 0]);
        let out = render_frame(&frame, 8, 4);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(cell_count(text.as_bytes()), 32);
        assert!(text.contains('█'));
        assert!(!text.contains('▀'));
        assert!(text.contains("255;0;0"));
        assert_eq!(text.matches("\u{1b}[E").count(), 3);
    }

    #[test]
    fn stacked_8_uses_half_blocks_only() {
        let frame = stacked(8, 8, [255, 0, 0], [0, 0, 255]);
        let out = render_frame(&frame, 8, 4);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(cell_count(text.as_bytes()), 32);
        assert!(text.contains('▀'));
        assert!(!text.contains('█'));
    }

    #[test]
    fn tiny_and_huge_terminals_clamp() {
        let (cols, rows) = grid(8, 6, 40, 20);
        assert_eq!(cols, 10);
        assert!(rows >= 4);
        let (cols, rows) = grid(999, 3, 40, 20);
        assert_eq!(cols, 40);
        assert!(rows >= 4);
    }

    #[test]
    fn aspect_uses_half_even_on_4_5() {
        // cols = min(max(10, 15), 40) = 15
        // (6/40)*2*15 = 4.5, half-even -> 4, and 4 is already the floor.
        let (cols, rows) = grid(16, 100, 40, 6);
        assert_eq!((cols, rows), (15, 4));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(25))]
        #[test]
        fn cell_count_matches_grid(cols in 8usize..25, rows in 4usize..13, r in 0u8..=255, g in 0u8..=255, b in 0u8..=255) {
            let frame = solid(rows * 2, cols, [r, g, b]);
            let out = render_frame(&frame, cols, rows);
            prop_assert_eq!(cell_count(&out), cols * rows);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg --lib render -- --test-threads=8`

Expected: FAIL to compile because `round_half_even`, `Frame`, `render_frame`, and `grid` are missing.

- [ ] **Step 3: Write minimal implementation**

Replace `server/src/render.rs` with the tests from Step 1 unchanged at the bottom, and this implementation above them:

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
    let cols = (term_cols.saturating_sub(1)).max(10).min(ref_cols);
    let aspect = round_half_even((ref_rows as f64 / ref_cols as f64) * 2.0 * cols as f64).max(0) as u32;
    let aspect_rows = aspect.max(4);
    let rows = aspect_rows.min((term_rows.saturating_sub(1)).max(4));
    (cols, rows)
}

fn at(frame: &Frame, y: usize, x: usize) -> [u8; 3] {
    let i = (y * frame.width + x) * 3;
    [frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2]]
}

pub fn render_frame(frame: &Frame, cols: usize, rows: usize) -> Vec<u8> {
    let dec_h = (rows * 2).max(2);
    let step_y = (round_half_even(frame.height as f64 / dec_h as f64).max(1)) as usize;
    let step_x = (round_half_even(frame.width as f64 / cols as f64).max(1)) as usize;

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
    while ys.len() < dec_h {
        ys.push(last_y);
    }
    while xs.len() < cols {
        xs.push(last_x);
    }

    let mut pixels = vec![[0u8; 3]; dec_h * cols];
    for (iy, sy) in ys.iter().enumerate() {
        for (ix, sx) in xs.iter().enumerate() {
            pixels[iy * cols + ix] = at(frame, *sy, *sx);
        }
    }

    let mut char_rows = dec_h / 2;
    if dec_h % 2 == 1 {
        char_rows += 1;
    }
    let mut lines: Vec<Vec<u8>> = Vec::with_capacity(char_rows);
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
                    line.extend_from_slice(format!(
                        "\u{1b}[49m\u{1b}[38;2;{};{};{}m",
                        top[0], top[1], top[2]
                    ).as_bytes());
                    for _ in 0..run {
                        line.extend_from_slice("█".as_bytes());
                    }
                } else {
                    line.extend_from_slice(format!(
                        "\u{1b}[38;2;{};{};{}m\u{1b}[48;2;{};{};{}m",
                        top[0], top[1], top[2], bot[0], bot[1], bot[2]
                    ).as_bytes());
                    for _ in 0..run {
                        line.extend_from_slice("▀".as_bytes());
                    }
                }
                run_start = x;
            }
        }
        line.extend_from_slice(b"\x1b[0m");
        lines.push(line);
    }
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b"\x1b[E");
        }
        out.extend_from_slice(line);
    }
    out
}
```

`min(ref_cols)` is the spec. When `ref_cols` is below 10 the width is `ref_cols`, and `clamp` would panic, so this function does not use `clamp`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg --lib render -- --test-threads=8`

Expected: PASS, including the proptest.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml server/Cargo.toml server/src/lib.rs server/src/render.rs Cargo.lock
git commit -m "Add bootleg's block renderer and terminal grid."
```

---

### Task 2: Banner, quit matcher, and server clock

**Files:**
- Create: `server/src/protocol.rs`
- Modify: `server/src/lib.rs`

**Interfaces:**
- Consumes: `round_half_even` from `render.rs`
- Produces:
  - `pub fn format_fps_0f(fps: f64) -> String`
  - `pub fn banner(width: u32, height: u32, frames: usize, fps: f64) -> Vec<u8>`
  - `pub const RESTORE: &[u8]`
  - `pub fn is_quit(data: &[u8]) -> bool`
  - `pub fn frame_index(elapsed_secs: f64, fps: f64, frame_count: usize) -> usize`
  - `pub fn indexes_due(last: usize, elapsed_secs: f64, fps: f64, frame_count: usize) -> Vec<usize>`

- [ ] **Step 1: Write the failing test**

Create `server/src/protocol.rs` containing only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_matches_the_spec_bytes() {
        let out = banner(64, 36, 4, 8.0);
        let expect = "\u{1b}[?1049h\u{1b}[2J\u{1b}[?25l\u{1b}[?7l\u{1b}[0m\r\n\u{1b}[38;2;255;255;255mbootleg  ·  64x36  ·  4 frames @ 8fps  ·  'q' quits\u{1b}[0m\r\n\r\n";
        assert_eq!(out, expect.as_bytes());
        assert_eq!(format_fps_0f(23.976), "24");
    }

    #[test]
    fn quit_keys() {
        for payload in [b"q".as_slice(), b"quit", b"\x03", b"\x04", b"\x1b[99;5u", b"\x1b[67;5u", b"\x1b[3;5u", b"x\x03"] {
            assert!(is_quit(payload), "{payload:?}");
        }
        assert!(!is_quit(b""));
        assert!(!is_quit(b"x"));
        assert!(!is_quit(b"Q"));
    }

    #[test]
    fn late_wake_sends_only_the_current_index() {
        let fps = 8.0;
        let elapsed = 5.1 / fps;
        let due = indexes_due(0, elapsed, fps, 12);
        assert_eq!(due, vec![5]);
        assert_eq!(frame_index(elapsed, fps, 12), 5);
    }

    #[test]
    fn index_wraps_with_frame_count() {
        assert_eq!(frame_index(2.0, 8.0, 4), 0);
    }
}
```

Add `pub mod protocol;` to `server/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg --lib protocol -- --test-threads=8`

Expected: FAIL to compile. `banner`, `is_quit`, `indexes_due`, and `frame_index` are missing.

- [ ] **Step 3: Write minimal implementation**

Add above the tests:

```rust
use crate::render::round_half_even;

pub const RESTORE: &[u8] = b"\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l";

pub fn format_fps_0f(fps: f64) -> String {
    round_half_even(fps).to_string()
}

pub fn banner(width: u32, height: u32, frames: usize, fps: f64) -> Vec<u8> {
    format!(
        "\u{1b}[?1049h\u{1b}[2J\u{1b}[?25l\u{1b}[?7l\u{1b}[0m\r\n\u{1b}[38;2;255;255;255mbootleg  ·  {width}x{height}  ·  {frames} frames @ {fps_txt}fps  ·  'q' quits\u{1b}[0m\r\n\r\n",
        fps_txt = format_fps_0f(fps),
    ).into_bytes()
}

pub fn is_quit(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    if data.contains(&0x03) || data.contains(&0x04) {
        return true;
    }
    if contains(data, b"\x1b[99;5u") || contains(data, b"\x1b[67;5u") || contains(data, b"\x1b[3;5u") {
        return true;
    }
    data[0] == b'q'
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

pub fn frame_index(elapsed_secs: f64, fps: f64, frame_count: usize) -> usize {
    let raw = (elapsed_secs * fps).floor() as usize;
    raw % frame_count.max(1)
}

pub fn indexes_due(last: usize, elapsed_secs: f64, fps: f64, frame_count: usize) -> Vec<usize> {
    let idx = frame_index(elapsed_secs, fps, frame_count);
    if idx == last { Vec::new() } else { vec![idx] }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg --lib protocol -- --test-threads=8`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add server/src/protocol.rs server/src/lib.rs
git commit -m "Match bootleg's banner, quit keys, and per-session clock."
```

---

### Task 3: Shared ANSI cache

**Files:**
- Create: `server/src/movie.rs`
- Create: `server/src/cache.rs`
- Modify: `server/Cargo.toml`
- Modify: `server/src/lib.rs`

**Interfaces:**
- Consumes: `Frame`, `render_frame`
- Produces:
  - `pub struct Movie { pub fps: f64, pub width: u32, pub height: u32, pub ref_cols: u32, pub ref_rows: u32, pub frames: Vec<Frame> }`
  - `Movie::from_frames(frames: Vec<Frame>, fps: f64, src_width: u32, src_height: u32) -> Result<Self, String>`
  - `Movie::frame_count(&self) -> usize`
  - `pub fn nearest(existing: &[(u32, u32)], cols: u32, rows: u32) -> (u32, u32)`
  - `pub struct AnsiCache`
  - `AnsiCache::new(budget_bytes: usize) -> Self`
  - `AnsiCache::resolve(&self, cols: u32, rows: u32, frame_count: usize) -> impl Future<Output = (u32, u32)>`
  - `AnsiCache::frame(&self, movie: &Movie, cols: u32, rows: u32, index: usize) -> impl Future<Output = bytes::Bytes>`
  - `AnsiCache::spawn_fill(self: &Arc<Self>, movie: Arc<Movie>, cols: u32, rows: u32) -> impl Future<Output = ()>`

- [ ] **Step 1: Write the failing test**

Replace `server/Cargo.toml` dependencies with:

```toml
[dependencies]
bytes = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
```

Keep the existing `[package]` and `proptest` dev-dependency.

Create `server/src/movie.rs`:

```rust
use crate::render::Frame;

#[derive(Clone)]
pub struct Movie {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub ref_cols: u32,
    pub ref_rows: u32,
    pub frames: Vec<Frame>,
}

impl Movie {
    pub fn from_frames(
        frames: Vec<Frame>,
        fps: f64,
        src_width: u32,
        src_height: u32,
    ) -> Result<Self, String> {
        let first = frames.first().ok_or_else(|| "no frames".to_string())?;
        Ok(Self {
            fps,
            width: src_width,
            height: src_height,
            ref_cols: first.width as u32,
            ref_rows: first.height as u32,
            frames,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
}
```

Create `server/src/cache.rs` with this test module and no other items yet:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::movie::Movie;
    use crate::render::Frame;
    fn movie() -> Movie {
        let rgb = vec![1u8, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3];
        Movie::from_frames(
            vec![
                Frame { width: 2, height: 2, rgb: rgb.clone() },
                Frame { width: 2, height: 2, rgb },
            ],
            8.0,
            20,
            10,
        ).unwrap()
    }

    #[test]
    fn nearest_breaks_ties_by_cols_then_rows_then_cells_then_age() {
        assert_eq!(nearest(&[(10, 4), (40, 10)], 38, 8), (40, 10));
        assert_eq!(nearest(&[(10, 4), (10, 16)], 10, 10), (10, 16));
        assert_eq!(nearest(&[(8, 4), (8, 4)], 8, 4), (8, 4));
    }

    #[tokio::test]
    async fn second_call_reuses_the_stored_buffer() {
        let movie = movie();
        let cache = AnsiCache::new(10_000);
        let (cols, rows) = cache.resolve(2, 1, movie.frame_count()).await;
        let a = cache.frame(&movie, cols, rows, 0).await;
        let b = cache.frame(&movie, cols, rows, 0).await;
        assert_eq!(a, b);
        assert_eq!(a.as_ptr(), b.as_ptr());
    }

    #[tokio::test]
    async fn concurrent_waiters_share_one_buffer() {
        let movie = movie();
        let cache = std::sync::Arc::new(AnsiCache::new(10_000));
        let (cols, rows) = cache.resolve(2, 1, movie.frame_count()).await;
        let c1 = cache.clone();
        let c2 = cache.clone();
        let m1 = movie.clone();
        let m2 = movie.clone();
        let (a, b) = tokio::join!(
            tokio::spawn(async move { c1.frame(&m1, cols, rows, 0).await }),
            tokio::spawn(async move { c2.frame(&m2, cols, rows, 0).await }),
        );
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(a.as_ptr(), b.as_ptr());
    }

    #[tokio::test]
    async fn second_size_snaps_once_budget_is_spent() {
        let movie = movie();
        let cache = AnsiCache::new(1);
        let first = cache.resolve(8, 4, movie.frame_count()).await;
        assert_eq!(first, (8, 4));
        let _ = cache.frame(&movie, first.0, first.1, 0).await;
        let second = cache.resolve(20, 6, movie.frame_count()).await;
        assert_eq!(second, first);
    }
}
```

`Movie` already derives `Clone`. Add `#[derive(Clone)]` to `Frame` in `server/src/render.rs` before this task's tests compile. The two `frame` calls must return clones of one stored `Bytes`, so `as_ptr` matches.

Add `pub mod movie;` and `pub mod cache;` to `server/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg --lib cache -- --test-threads=8`

Expected: FAIL to compile. `nearest` and `AnsiCache` are missing.

- [ ] **Step 3: Write minimal implementation**

Add this above the tests in `server/src/cache.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{Mutex, OnceCell};
use tokio::task::JoinHandle;

use crate::movie::Movie;
use crate::render::render_frame;

pub fn nearest(existing: &[(u32, u32)], cols: u32, rows: u32) -> (u32, u32) {
    existing.iter().copied().min_by(|a, b| {
        let rank = |s: (u32, u32)| {
            let dcols = (s.0 as i32 - cols as i32).abs();
            let drows = (s.1 as i32 - rows as i32).abs();
            let cells = (s.0 as i64) * (s.1 as i64);
            (dcols, drows, -cells)
        };
        rank(*a).cmp(&rank(*b))
    }).unwrap_or((cols, rows))
}

struct Slot {
    gate: Mutex<()>,
    value: OnceCell<Bytes>,
}

impl Slot {
    fn new() -> Self {
        Self { gate: Mutex::new(()), value: OnceCell::new() }
    }

    async fn get(&self, render: impl FnOnce() -> Vec<u8> + Send + 'static) -> (Bytes, bool) {
        if let Some(v) = self.value.get() {
            return (v.clone(), false);
        }
        let _guard = self.gate.lock().await;
        if let Some(v) = self.value.get() {
            return (v.clone(), false);
        }
        let rendered = tokio::task::spawn_blocking(render).await.expect("render");
        let bytes = Bytes::from(rendered);
        let _ = self.value.set(bytes.clone());
        (bytes, true)
    }
}

struct SizeState {
    slots: Vec<Slot>,
    fill: Mutex<Option<JoinHandle<()>>>,
}

struct Inner {
    order: Vec<(u32, u32)>,
    sizes: HashMap<(u32, u32), Arc<SizeState>>,
    retained: usize,
}

pub struct AnsiCache {
    budget: usize,
    inner: Mutex<Inner>,
}

impl AnsiCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            budget: budget_bytes,
            inner: Mutex::new(Inner {
                order: Vec::new(),
                sizes: HashMap::new(),
                retained: 0,
            }),
        }
    }

    pub async fn resolve(&self, cols: u32, rows: u32, frame_count: usize) -> (u32, u32) {
        let mut inner = self.inner.lock().await;
        if inner.sizes.contains_key(&(cols, rows)) {
            return (cols, rows);
        }
        let admit = inner.order.is_empty() || inner.retained < self.budget;
        if !admit {
            return nearest(&inner.order, cols, rows);
        }
        let slots = (0..frame_count).map(|_| Slot::new()).collect();
        inner.order.push((cols, rows));
        inner.sizes.insert((cols, rows), Arc::new(SizeState {
            slots,
            fill: Mutex::new(None),
        }));
        (cols, rows)
    }

    pub async fn frame(&self, movie: &Movie, cols: u32, rows: u32, index: usize) -> Bytes {
        let index = index % movie.frame_count();
        let size = {
            let inner = self.inner.lock().await;
            inner.sizes.get(&(cols, rows)).cloned().expect("resolve before frame")
        };
        let frame = movie.frames[index].clone();
        let cols_us = cols as usize;
        let rows_us = rows as usize;
        let (bytes, stored) = size.slots[index].get(move || render_frame(&frame, cols_us, rows_us)).await;
        if stored {
            let mut inner = self.inner.lock().await;
            inner.retained += bytes.len();
        }
        bytes
    }

    pub async fn spawn_fill(self: &Arc<Self>, movie: Arc<Movie>, cols: u32, rows: u32) {
        let size = {
            let inner = self.inner.lock().await;
            inner.sizes.get(&(cols, rows)).cloned()
        };
        let Some(size) = size else { return };
        let mut fill = size.fill.lock().await;
        if fill.is_some() {
            return;
        }
        let cache = Arc::clone(self);
        *fill = Some(tokio::spawn(async move {
            for index in 0..movie.frame_count() {
                let _ = cache.frame(&movie, cols, rows, index).await;
            }
        }));
    }
}
```

`Iterator::min_by` keeps the first element when the ranks compare equal, so an earlier identical size wins. Callers `.await` `spawn_fill`. The fill mutex is held only while the task is spawned, so a second caller sees the handle and does not start another walk.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg --lib cache movie -- --test-threads=8`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add server/Cargo.toml server/src/lib.rs server/src/movie.rs server/src/cache.rs server/src/render.rs Cargo.lock
git commit -m "Share rendered frames across bootleg sessions."
```

---

### Task 4: ffmpeg decode

**Files:**
- Modify: `server/src/movie.rs`
- Modify: `server/Cargo.toml` (add `process` and `io-util` to the tokio features)
- Create: `server/tests/decode.rs`

**Interfaces:**
- Consumes: `Movie::from_frames`, `Frame`, `round_half_even`
- Produces:
  - `pub fn ref_rows_for(src_width: u32, src_height: u32, grid_cols: u32) -> u32`
  - `pub async fn decode_file(path: &std::path::Path, grid_cols: u32, fps_override: f64) -> Result<Movie, String>`

- [ ] **Step 1: Write the failing test**

Create `server/tests/decode.rs`:

```rust
use std::path::PathBuf;
use std::process::Command;

use bootleg::movie::{decode_file, ref_rows_for};

fn ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok()
        && Command::new("ffprobe").arg("-version").output().is_ok()
}

#[tokio::test]
async fn decodes_a_generated_clip() {
    if !ffmpeg() {
        eprintln!("skip: ffmpeg and ffprobe are required");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bootleg-decode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("clip.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=64x36:rate=8:duration=0.5", "-pix_fmt", "yuv420p"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    let movie = decode_file(&path, 32, 0.0).await.unwrap();
    assert_eq!((movie.width, movie.height), (64, 36));
    assert_eq!(movie.ref_cols, 32);
    assert_eq!(movie.ref_rows % 2, 0);
    assert!(movie.frame_count() >= 3);
    assert_eq!(movie.frames[0].rgb.len(), movie.ref_cols as usize * movie.ref_rows as usize * 3);
    let out = bootleg::render::render_frame(&movie.frames[0], 16, 8);
    let cells = out.windows(3).filter(|w| *w == "█".as_bytes() || *w == "▀".as_bytes()).count();
    assert_eq!(cells, 128);
    assert_eq!(ref_rows_for(64, 36, 32) % 2, 0);
}
```

Tokio in integration tests needs the macros feature, already requested. Add `process` and `io-util` to tokio features in `server/Cargo.toml`:

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "process", "io-util"] }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg --test decode -- --test-threads=8`

Expected: FAIL to compile. `decode_file` is missing.

- [ ] **Step 3: Write minimal implementation**

Append to `server/src/movie.rs`:

```rust
use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::render::round_half_even;

pub fn ref_rows_for(src_width: u32, src_height: u32, grid_cols: u32) -> u32 {
    let rounded = round_half_even((src_height as f64 / src_width as f64) * grid_cols as f64 / 2.0).max(0) as u32;
    (rounded * 2).max(1)
}

pub async fn decode_file(path: &Path, grid_cols: u32, fps_override: f64) -> Result<Movie, String> {
    let (width, height, src_fps) = probe(path).await?;
    let fps = if fps_override == 0.0 { src_fps } else { fps_override };
    let ref_rows = ref_rows_for(width, height, grid_cols);
    let frames = read_frames(path, grid_cols, ref_rows).await?;
    if frames.is_empty() {
        return Err(format!("no frames decoded from {}", path.display()));
    }
    let mut movie = Movie::from_frames(frames, fps, width, height)?;
    movie.ref_cols = grid_cols;
    movie.ref_rows = ref_rows;
    Ok(movie)
}

async fn probe(path: &Path) -> Result<(u32, u32, f64), String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.trim().split(',');
    let w: u32 = parts.next().ok_or("probe")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let h: u32 = parts.next().ok_or("probe")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let rate = parts.next().ok_or("probe")?;
    let (num, den) = rate.split_once('/').unwrap_or(("30", "1"));
    let num: f64 = num.parse().unwrap_or(30.0);
    let den: f64 = den.parse().unwrap_or(1.0);
    let fps = if den == 0.0 { 30.0 } else { num / den };
    Ok((w, h, fps))
}

async fn read_frames(path: &Path, ref_cols: u32, ref_rows: u32) -> Result<Vec<Frame>, String> {
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vf", &format!("scale={ref_cols}:{ref_rows}:flags=area"), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("ffmpeg stdout")?;
    let size = ref_cols as usize * ref_rows as usize * 3;
    let mut frames = Vec::new();
    loop {
        let mut buf = vec![0u8; size];
        let mut read = 0;
        while read < size {
            let n = stdout.read(&mut buf[read..]).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            read += n;
        }
        if read < size {
            break;
        }
        frames.push(Frame { width: ref_cols as usize, height: ref_rows as usize, rgb: buf });
    }
    drop(stdout);
    match tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await {
        Ok(Ok(status)) if status.success() || !frames.is_empty() => {}
        _ => {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
    Ok(frames)
}
```

`child.start_kill` exists on `tokio::process::Child`. If the compiler says it is `kill`, call `child.kill().await` instead.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg --test decode -- --test-threads=8`

Expected: PASS when ffmpeg is installed. When it is not, the test returns early and still passes.

- [ ] **Step 5: Commit**

```bash
git add server/src/movie.rs server/Cargo.toml server/tests/decode.rs Cargo.lock
git commit -m "Decode the bootleg clip once at startup."
```

---

### Task 5: russh server

**Files:**
- Create: `server/src/ssh.rs`
- Create: `server/tests/ssh_e2e.rs`
- Modify: `server/Cargo.toml`
- Modify: `server/src/lib.rs`

**Interfaces:**
- Consumes: `Movie`, `AnsiCache`, `banner`, `RESTORE`, `is_quit`, `frame_index`, `grid`, `decode_file` is not used here
- Produces:
  - `pub async fn load_host_key(path: &std::path::Path) -> anyhow::Result<russh::keys::PrivateKey>`
  - `pub async fn serve(movie: Arc<Movie>, cache: Arc<AnsiCache>, listener: tokio::net::TcpListener, key: russh::keys::PrivateKey) -> anyhow::Result<()>`

- [ ] **Step 1: Write the failing test**

Add to `server/Cargo.toml`:

```toml
anyhow = "1"
russh = "0.54"
```

`russh = "0.54"` matches the Handler API this task calls: `auth_none` returns `Auth::Accept`, `channel_open_session` takes `ChannelOpenHandle`, and `Handle::data` accepts `impl Into<bytes::Bytes>`.

Add `net` to tokio features.

Create `server/tests/ssh_e2e.rs`:

```rust
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use bootleg::cache::AnsiCache;
use bootleg::movie::decode_file;
use bootleg::ssh::{load_host_key, serve};
use tokio::net::TcpListener;

#[tokio::test]
async fn two_clients_each_get_a_stream() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ssh").arg("-V").output().is_err()
    {
        eprintln!("skip: ffmpeg and ssh are required");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bootleg-ssh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mp4");
    let key_path = dir.join("host_key");
    assert!(Command::new("ffmpeg").args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=64x36:rate=8:duration=0.5", "-pix_fmt", "yuv420p"]).arg(&clip).status().unwrap().success());
    assert!(Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-f"]).arg(&key_path).args(["-N", ""]).status().unwrap().success());
    let movie = Arc::new(decode_file(&clip, 32, 8.0).await.unwrap());
    let cache = Arc::new(AnsiCache::new(64 * 1024 * 1024));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = load_host_key(&key_path).await.unwrap();
    let server = tokio::spawn(async move {
        let _ = serve(movie, cache, listener, key).await;
    });
    let one = tokio::spawn(read_ssh(port));
    let two = tokio::spawn(read_ssh(port));
    let one = tokio::time::timeout(Duration::from_secs(8), one).await.unwrap().unwrap();
    let two = tokio::time::timeout(Duration::from_secs(8), two).await.unwrap().unwrap();
    for text in [&one, &two] {
        assert!(text.contains("bootleg"), "{text}");
        assert!(text.contains("64x36"), "{text}");
        assert!(text.contains("\u{1b}[38;2;"), "{text}");
        assert!(text.contains('█') || text.contains('▀'), "{text}");
    }
    server.abort();
}

async fn read_ssh(port: u16) -> String {
    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-tt",
            "-p", &port.to_string(),
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            "-o", "PreferredAuthentications=none",
            "watch@127.0.0.1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    use tokio::io::AsyncWriteExt;
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(b"q").await;
    }
    let out = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output()).await.unwrap().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}
```

Add `pub mod ssh;` to `server/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p bootleg --test ssh_e2e -- --test-threads=1`

Expected: FAIL to compile. `bootleg::ssh` is missing.

- [ ] **Step 3: Write minimal implementation**

Create `server/src/ssh.rs`:

```rust
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use russh::server::{Auth, Handler, Msg, Server as _, Session};
use russh::{ChannelId, MethodSet};
use tokio::net::TcpListener;

use crate::cache::AnsiCache;
use crate::movie::Movie;
use crate::protocol::{self, RESTORE};
use crate::render::grid;

pub async fn load_host_key(path: &Path) -> anyhow::Result<russh::keys::PrivateKey> {
    russh::keys::load_secret_key(path, None).context(format!("missing host key: {}", path.display()))
}

struct Chan {
    cols: u32,
    rows: u32,
    have_pty: bool,
    quit: Arc<AtomicBool>,
    started: bool,
}

pub struct App {
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
}

impl App {
    pub fn new(movie: Arc<Movie>, cache: Arc<AnsiCache>) -> Self {
        Self { movie, cache }
    }
}

struct Conn {
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    chans: HashMap<ChannelId, Chan>,
}

impl russh::server::Server for App {
    type Handler = Conn;
    fn new_client(&mut self, _: Option<SocketAddr>) -> Conn {
        Conn {
            movie: Arc::clone(&self.movie),
            cache: Arc::clone(&self.cache),
            chans: HashMap::new(),
        }
    }
}

impl Handler for Conn {
    type Error = anyhow::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.chans.insert(channel.id(), Chan {
            cols: 80,
            rows: 24,
            have_pty: false,
            quit: Arc::new(AtomicBool::new(false)),
            started: false,
        });
        reply.accept().await?;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        col_width: u32,
        row_height: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(ch) = self.chans.get_mut(&channel) {
            if !ch.started {
                ch.cols = col_width;
                ch.rows = row_height;
                ch.have_pty = true;
            }
        }
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let Some(ch) = self.chans.get_mut(&channel) else { return Ok(()) };
        if ch.started {
            return Ok(());
        }
        ch.started = true;
        let term_cols = if ch.have_pty { ch.cols } else { 80 };
        let term_rows = if ch.have_pty { ch.rows } else { 24 };
        let (cols, rows) = grid(term_cols, term_rows, self.movie.ref_cols, self.movie.ref_rows);
        let (cols, rows) = self.cache.resolve(cols, rows, self.movie.frame_count()).await;
        self.cache.spawn_fill(Arc::clone(&self.movie), cols, rows).await;
        let handle = session.handle();
        let movie = Arc::clone(&self.movie);
        let cache = Arc::clone(&self.cache);
        let quit = Arc::clone(&ch.quit);
        tokio::spawn(async move {
            pump(handle, channel, movie, cache, cols, rows, quit).await;
        });
        Ok(())
    }

    async fn exec_request(&mut self, channel: ChannelId, _: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn subsystem_request(&mut self, channel: ChannelId, _: &str, session: &mut Session) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: russh::server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::reject())
    }

    async fn data(&mut self, channel: ChannelId, data: &[u8], _: &mut Session) -> Result<(), Self::Error> {
        if protocol::is_quit(data) {
            if let Some(ch) = self.chans.get(&channel) {
                ch.quit.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, _: &mut Session) -> Result<(), Self::Error> {
        if let Some(ch) = self.chans.get(&channel) {
            ch.quit.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn signal(&mut self, channel: ChannelId, signal: russh::Sig, _: &mut Session) -> Result<(), Self::Error> {
        if matches!(signal, russh::Sig::INT | russh::Sig::TERM | russh::Sig::QUIT | russh::Sig::HUP) {
            if let Some(ch) = self.chans.get(&channel) {
                ch.quit.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

async fn pump(
    handle: russh::server::Handle,
    id: ChannelId,
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    cols: u32,
    rows: u32,
    quit: Arc<AtomicBool>,
) {
    let send = |bytes: Vec<u8>| {
        let handle = handle.clone();
        async move { handle.data(id, bytes).await.ok() }
    };
    if send(protocol::banner(movie.width, movie.height, movie.frame_count(), movie.fps)).await.is_none() {
        return;
    }
    let frame0 = match cache.frame(&movie, cols, rows, 0).await {
        bytes => bytes,
    };
    let mut first = b"\x1b[H".to_vec();
    first.extend_from_slice(&frame0);
    if send(first).await.is_none() {
        let _ = handle.data(id, RESTORE.to_vec()).await;
        let _ = handle.close(id).await;
        return;
    }
    let t0 = Instant::now();
    let mut last = 0usize;
    let n = movie.frame_count();
    let fps = movie.fps;
    loop {
        if quit.load(Ordering::Relaxed) {
            break;
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let idx = protocol::frame_index(elapsed, fps, n);
        if idx != last {
            let frame = cache.frame(&movie, cols, rows, idx).await;
            let mut msg = b"\x1b[H".to_vec();
            msg.extend_from_slice(&frame);
            if handle.data(id, msg).await.is_err() {
                break;
            }
            last = idx;
        }
        let next = (elapsed * fps).floor() + 1.0;
        let deadline = t0 + Duration::from_secs_f64(next / fps);
        let now = Instant::now();
        if deadline <= now {
            tokio::task::yield_now().await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(deadline - now) => {}
                _ = wait_quit(&quit) => break,
            }
        }
    }
    let _ = handle.data(id, RESTORE.to_vec()).await;
    let _ = handle.close(id).await;
}

async fn wait_quit(quit: &AtomicBool) {
    while !quit.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn serve(
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    listener: TcpListener,
    key: russh::keys::PrivateKey,
) -> anyhow::Result<()> {
    let config = russh::server::Config {
        inactivity_timeout: None,
        auth_rejection_time: Duration::from_secs(0),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        methods: MethodSet::NONE,
        ..Default::default()
    };
    let mut app = App::new(movie, cache);
    app.run_on_socket(Arc::new(config), &listener).await?;
    Ok(())
}
```

The `send` closure clones `handle` on each call, so the later `handle.data` still owns the original handle. `auth_none` returning `Auth::Accept` is what lets the client in. If `Config` has no `methods` field, delete that line. If `MethodSet::NONE` is not in this russh version, use `MethodSet::empty()`.

`session.handle()` is the public clone of the session outbound handle. Use it.

`session.channel_success` returns `Result`. Keep `?`.

Do not accept `channel_open_direct_tcpip`, `channel_open_forwarded_tcpip`, `channel_open_x11`, or `tcpip_forward`. The default drops the open handle, which rejects the channel. `tcpip_forward` defaults to `false`. Leave those methods unimplemented.

On a render or write failure inside `pump`, before restore, write:

```rust
let err = format!("\r\n\x1b[0mstream error: {message}\x1b[0m\r\n");
```

Use that when `cache.frame` is unreachable. `frame` does not return Result today. If `spawn_blocking` panics, the task dies; the drop path is the restore already at the end of the loop. Wrap the body after the banner in a block and always restore in the existing tail. That matches "on the way out, including after a session error".

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg --test ssh_e2e -- --test-threads=1`

Expected: PASS, or skip when `ffmpeg` or `ssh` is absent. A password prompt means `auth_none` did not accept. Fix that before continuing. Both clients must contain `bootleg` and a truecolor sequence.

- [ ] **Step 5: Commit**

```bash
git add server/src/ssh.rs server/src/lib.rs server/Cargo.toml server/tests/ssh_e2e.rs Cargo.lock
git commit -m "Serve bootleg frames over anonymous SSH."
```

---

### Task 6: Process entrypoint, image, and rename

**Files:**
- Create: `server/src/main.rs`
- Modify: `Dockerfile`
- Modify: `docker-compose.yml`
- Modify: `entrypoint.sh`
- Modify: `.env.example`
- Modify: `.dockerignore`
- Modify: `.gitignore`
- Modify: `README.md`
- Modify: `DEPLOY.md`
- Delete: `ascii_ssh.py`, `pyproject.toml`, `uv.lock`, `tests/`

**Interfaces:**
- Consumes: `decode_file`, `AnsiCache`, `serve`, `load_host_key`
- Produces: the `bootleg` binary and the `bootleg:0.1.0` image

- [ ] **Step 1: Write the failing test**

There is no new unit test. The check is `cargo run -p bootleg` exiting non-zero when `VIDEO_PATH` points at a missing file.

Create `server/src/main.rs`:

```rust
use std::path::PathBuf;
use std::sync::Arc;

use bootleg::cache::AnsiCache;
use bootleg::movie::decode_file;
use bootleg::ssh::{load_host_key, serve};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("[bootleg] {err}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let video = std::env::var("VIDEO_PATH").unwrap_or_else(|_| "/clips/video.mp4".into());
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(2222);
    let fps: f64 = std::env::var("FPS").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let grid_cols: u32 = std::env::var("GRID_COLS").ok().and_then(|s| s.parse().ok()).unwrap_or(160);
    let host_key = PathBuf::from(std::env::var("HOST_KEY").unwrap_or_else(|_| "./host_key".into()));
    let cache_mib: usize = std::env::var("ANSI_CACHE_MIB").ok().and_then(|s| s.parse().ok()).unwrap_or(512);
    if !PathBuf::from(&video).exists() {
        anyhow::bail!("video not found: {video}");
    }
    println!("[bootleg] decoding {video} ...");
    let movie = decode_file(PathBuf::from(&video).as_path(), grid_cols, fps).await?;
    println!(
        "[bootleg] {} frames @ {:.2}fps ref {}x{}",
        movie.frame_count(), movie.fps, movie.ref_cols, movie.ref_rows
    );
    let key = load_host_key(&host_key).await?;
    let listener = TcpListener::bind((host.as_str(), port)).await?;
    println!("[bootleg] listening on {host}:{port}");
    let movie = Arc::new(movie);
    let cache = Arc::new(AnsiCache::new(cache_mib * 1024 * 1024));
    tokio::select! {
        result = serve(movie, cache, listener, key) => result?,
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm() => {}
    }
    Ok(())
}

async fn sigterm() {
    #[cfg(unix)]
    let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("sigterm")
        .recv()
        .await;
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}
```

Add tokio feature `signal`.

- [ ] **Step 2: Run test to verify it fails**

Run: `VIDEO_PATH=/no/such.mp4 cargo run -p bootleg`

Expected: process exit code 1 and stderr containing `video not found`. This fails first if `main.rs` does not compile. Fix compile errors, then re-run until the missing-file path exits 1.

- [ ] **Step 3: Write the container and docs**

Replace `Dockerfile` with:

```dockerfile
# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server ./server
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
```

Replace the `quattro:` service key and `image: quattro:0.1.0` in `docker-compose.yml` with `bootleg:` and `image: bootleg:0.1.0`. Add:

```yaml
      ANSI_CACHE_MIB: ${ANSI_CACHE_MIB:-512}
```

next to the other environment entries.

In `entrypoint.sh`, replace every `[quattro]` with `[bootleg]` and the ssh-keygen comment `quattro` with `bootleg`.

Append to `.env.example`:

```bash
# Retained rendered-ANSI budget, in MiB, inside the 1 GiB container cap.
ANSI_CACHE_MIB=512
```

Add a `target` line to `.dockerignore`.

Replace `.gitignore` python block with:

```
# rust
/target
```

Remove the python ignore lines.

In `README.md`, replace the title and body name `quattro` with `bootleg`, the test section with:

```bash
cargo test -p bootleg
```

and the layout block so `ascii_ssh.py` is gone and `server/` is the SSH server. Keep the SSH usage, env table, and security model. Add `ANSI_CACHE_MIB` to the env table with default `512`.

In `DEPLOY.md`, replace `/opt/quattro` with `/opt/bootleg`, the firewall name `quattro` with `bootleg`, the hostname example `quattro` with `bootleg`, and the two sentences that say "not quattro" / "publish quattro" so they say bootleg. Leave `https://github.com/snek/omarchy-ascii-stream.git` unchanged.

Delete `ascii_ssh.py`, `pyproject.toml`, `uv.lock`, and the `tests/` directory.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p bootleg`

Expected: PASS. Decode and SSH tests skip only when their tools are missing. On this machine they should run.

Run: `VIDEO_PATH=/no/such.mp4 cargo run -p bootleg ; echo exit:$?`

Expected: exit code 1 and `video not found`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Ship bootleg as the SSH server and drop the Python runtime."
```

Check `git status` afterwards. Do not commit `.env`, `data/`, or `clips/` contents. If `git add -A` staged those, unstage them and commit again only if the first commit has not been created. If the commit already included them, remove them in a follow-up commit. Do not amend unless the commit rules allow it.
