use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::{clock, footer, render};

pub fn play(meta: &footer::Meta, frames: &[render::Frame]) -> i32 {
    let size = match winsize() {
        Some(s) => s,
        None => {
            eprintln!("bootleg-play: stdout is not a terminal");
            return 1;
        }
    };
    if frames.is_empty() {
        eprintln!("bootleg-play: no frames");
        return 1;
    }
    let quit = Arc::new(AtomicBool::new(false));
    let _sigint = signal_hook::flag::register(signal_hook::consts::SIGINT, quit.clone());
    let _sigterm = signal_hook::flag::register(signal_hook::consts::SIGTERM, quit.clone());
    let _guard = raw_mode();
    let (cols, rows) = render::grid(size.0, size.1, meta.ref_cols, meta.ref_rows);
    let cols = cols as usize;
    let rows = rows as usize;
    write_all(b"\x1b[?1049h\x1b[2J\x1b[?25l\x1b[?7l\x1b[0m");
    let frame0 = render::render_frame(&frames[0], cols, rows);
    write_all(b"\x1b[H");
    write_all(&frame0);
    let t0 = Instant::now();
    let mut last = 0usize;
    let mut pending = Vec::new();
    let fps = meta.fps;
    let n = frames.len();
    loop {
        if quit.load(Ordering::Relaxed) || stdin_quit(&mut pending) {
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
        let slice = if delay.is_finite() { delay.min(0.05) } else { 0.05 };
        std::thread::sleep(std::time::Duration::from_secs_f64(slice));
    }
    0
}

struct Restore {
    termios: Option<libc::termios>,
    stdin_flags: Option<libc::c_int>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        if let Some(state) = self.termios.take() {
            unsafe { libc::tcsetattr(1, libc::TCSANOW, &state) };
        }
        if let Some(flags) = self.stdin_flags.take() {
            unsafe { libc::fcntl(0, libc::F_SETFL, flags) };
        }
        write_all(b"\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l");
    }
}

fn raw_mode() -> Restore {
    let mut state = unsafe { std::mem::zeroed() };
    let termios = if unsafe { libc::tcgetattr(1, &mut state) } == 0 {
        let mut raw = state;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        if unsafe { libc::tcsetattr(1, libc::TCSANOW, &raw) } == 0 {
            Some(state)
        } else {
            None
        }
    } else {
        None
    };
    let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
    let stdin_flags = if flags >= 0 {
        let _ = unsafe { libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        Some(flags)
    } else {
        None
    };
    Restore { termios, stdin_flags }
}

fn winsize() -> Option<(u32, u32)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) };
    if rc == -1 {
        None
    } else {
        Some((u32::from(size.ws_col), u32::from(size.ws_row)))
    }
}

fn stdin_quit(pending: &mut Vec<u8>) -> bool {
    let mut buf = [0u8; 128];
    loop {
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            let chunk = &buf[..n as usize];
            if is_quit(chunk) {
                return true;
            }
            pending.extend_from_slice(chunk);
            if sequence_quit(pending) {
                return true;
            }
            if pending.len() > 32 {
                pending.drain(..pending.len() - 32);
            }
            continue;
        }
        if n == 0 {
            return false;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return false;
    }
}

fn is_quit(data: &[u8]) -> bool {
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

fn sequence_quit(data: &[u8]) -> bool {
    data.contains(&0x03)
        || data.contains(&0x04)
        || contains(data, b"\x1b[99;5u")
        || contains(data, b"\x1b[67;5u")
        || contains(data, b"\x1b[3;5u")
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn write_all(bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let n = unsafe { libc::write(1, rest.as_ptr().cast(), rest.len()) };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted || err.kind() == std::io::ErrorKind::WouldBlock {
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                continue;
            }
            return;
        }
        if n == 0 {
            return;
        }
        rest = &rest[n as usize..];
    }
}
