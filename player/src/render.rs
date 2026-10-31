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
