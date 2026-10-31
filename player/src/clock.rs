pub enum Tick {
    Wait,
    Show(usize),
    Done,
}

pub fn tick(elapsed_secs: f64, fps: f64, frame_count: usize, last: usize) -> Tick {
    let idx = (elapsed_secs * fps).floor() as usize;
    if idx >= frame_count {
        Tick::Done
    } else if idx != last {
        Tick::Show(idx)
    } else {
        Tick::Wait
    }
}

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
