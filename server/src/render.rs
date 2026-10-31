#[derive(Clone)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

pub fn round_half_even(value: f64) -> i64 {
    value.round_ties_even() as i64
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
    fn edge_pad_fills_the_grid_and_repeats_the_last_pixel() {
        // 2x2 is shorter than dec_h (4) and narrower than cols (3).
        // Padding repeats the sampled edge, pixel (1, 1) = 7,8,9.
        let frame = Frame {
            width: 2,
            height: 2,
            rgb: vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 7, 8, 9],
        };
        let cols = 3;
        let rows = 2;
        let out = render_frame(&frame, cols, rows);
        assert_eq!(cell_count(&out), cols * rows);
        let expect = "\u{1b}[38;2;255;0;0m\u{1b}[48;2;0;0;255m▀\u{1b}[38;2;0;255;0m\u{1b}[48;2;7;8;9m▀▀\u{1b}[0m\u{1b}[E\u{1b}[49m\u{1b}[38;2;0;0;255m█\u{1b}[49m\u{1b}[38;2;7;8;9m██\u{1b}[0m";
        assert_eq!(out, expect.as_bytes());
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
