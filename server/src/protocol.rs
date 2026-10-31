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
