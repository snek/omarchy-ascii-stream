use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use crate::footer::Meta;
use crate::render::{round_half_even, Frame};

pub struct Decoded {
    pub meta: Meta,
    pub frames: Vec<Frame>,
}

pub enum Error {
    Usage,
    Fail(String),
}

pub fn decode(path: &Path, grid_cols: u32, fps_override: f64) -> Result<Decoded, String> {
    if grid_cols == 0 {
        return Err("grid-cols must be greater than 0".to_string());
    }
    let (width, height, src_fps) = probe(path)?;
    let fps = if fps_override == 0.0 { src_fps } else { fps_override };
    if !fps.is_finite() || fps <= 0.0 {
        return Err("FPS must be finite and greater than 0".to_string());
    }
    let ref_rows = ref_rows_for(width, height, grid_cols);
    let frames = read_frames(path, grid_cols, ref_rows)?;
    let frame_count = u32::try_from(frames.len()).map_err(|_| "too many frames".to_string())?;
    Ok(Decoded {
        meta: Meta {
            src_width: width,
            src_height: height,
            ref_cols: grid_cols,
            ref_rows,
            fps,
            frame_count,
        },
        frames,
    })
}

pub fn export(video: &Path, out: &Path, fps: f64, grid_cols: u32) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if same_file(out, &exe) {
        return Err("refusing to overwrite the executable".to_string());
    }
    let stub = std::fs::read(&exe).map_err(|e| e.to_string())?;
    let decoded = decode(video, grid_cols, fps)?;
    let size = decoded.meta.ref_cols as usize * decoded.meta.ref_rows as usize * 3;
    let mut packed = Vec::with_capacity(size.saturating_mul(decoded.frames.len()));
    for frame in &decoded.frames {
        if frame.rgb.len() != size {
            return Err("decoded frame size does not match the grid".to_string());
        }
        packed.extend_from_slice(&frame.rgb);
    }
    let bytes = crate::footer::append(&stub, &packed, &decoded.meta);
    std::fs::write(out, bytes).map_err(|e| e.to_string())?;
    std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn run_args(args: impl IntoIterator<Item = String>) -> Result<(), Error> {
    let mut args = args.into_iter();
    let command = args.next();
    if command.as_deref() != Some("export") {
        return Err(Error::Usage);
    }
    let mut video = None;
    let mut out = None;
    let mut fps = None;
    let mut grid_cols = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(Error::Usage)?;
        match flag.as_str() {
            "--video" if video.is_none() => video = Some(PathBuf::from(value)),
            "-o" if out.is_none() => out = Some(PathBuf::from(value)),
            "--fps" if fps.is_none() => {
                fps = Some(value.parse::<f64>().map_err(|_| Error::Usage)?);
            }
            "--grid-cols" if grid_cols.is_none() => {
                grid_cols = Some(value.parse::<u32>().map_err(|_| Error::Usage)?);
            }
            _ => return Err(Error::Usage),
        }
    }
    let video = video.ok_or(Error::Usage)?;
    let out = out.ok_or(Error::Usage)?;
    export(&video, &out, fps.unwrap_or(0.0), grid_cols.unwrap_or(160)).map_err(Error::Fail)
}

fn ref_rows_for(width: u32, height: u32, grid_cols: u32) -> u32 {
    let ref_rows = round_half_even((height as f64 / width as f64) * grid_cols as f64 / 2.0).max(0) as u32 * 2;
    ref_rows.max(1)
}

fn same_file(out: &Path, exe: &Path) -> bool {
    if out == exe {
        return true;
    }
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(out), std::fs::canonicalize(exe)) {
        if a == b {
            return true;
        }
    }
    if let (Ok(a), Ok(b)) = (std::fs::metadata(out), std::fs::metadata(exe)) {
        use std::os::unix::fs::MetadataExt;
        return a.dev() == b.dev() && a.ino() == b.ino();
    }
    false
}

fn probe(path: &Path) -> Result<(u32, u32, f64), String> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("ffprobe failed for {}", path.display())
        } else {
            err
        });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().unwrap_or("").trim();
    let mut parts = line.split(',');
    let width: u32 = parts
        .next()
        .ok_or("probe")?
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    let height: u32 = parts
        .next()
        .ok_or("probe")?
        .parse()
        .map_err(|e: std::num::ParseIntError| e.to_string())?;
    let rate = parts.next().ok_or("probe")?;
    if width == 0 || height == 0 {
        return Err(format!("invalid dimensions {width}x{height}"));
    }
    let fps = parse_rate(rate)?;
    Ok((width, height, fps))
}

fn parse_rate(rate: &str) -> Result<f64, String> {
    let fps = if let Some((num, den)) = rate.split_once('/') {
        let num: f64 = num.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
        let den: f64 = den.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
        if den == 0.0 {
            return Err("invalid frame rate".to_string());
        }
        num / den
    } else {
        rate.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?
    };
    if fps.is_finite() && fps > 0.0 {
        Ok(fps)
    } else {
        Err("FPS must be finite and greater than 0".to_string())
    }
}

fn read_frames(path: &Path, ref_cols: u32, ref_rows: u32) -> Result<Vec<Frame>, String> {
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-vf",
            &format!("scale={ref_cols}:{ref_rows}:flags=area"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take();
    let read = match stdout.as_mut() {
        Some(stdout) => collect_frames(stdout, ref_cols, ref_rows),
        None => Err("ffmpeg stdout".to_string()),
    };
    finish_child(stdout, &mut child);
    let frames = read?;
    if frames.is_empty() {
        return Err(format!("no frames decoded from {}", path.display()));
    }
    Ok(frames)
}

fn collect_frames(stdout: &mut impl Read, ref_cols: u32, ref_rows: u32) -> Result<Vec<Frame>, String> {
    let size = ref_cols as usize * ref_rows as usize * 3;
    if size == 0 {
        return Err("empty frame".to_string());
    }
    let mut frames = Vec::new();
    loop {
        let mut buf = vec![0u8; size];
        let mut filled = 0;
        while filled < size {
            match stdout.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        if filled < size {
            break;
        }
        frames.push(Frame {
            width: ref_cols as usize,
            height: ref_rows as usize,
            rgb: buf,
        });
    }
    Ok(frames)
}

fn finish_child(stdout: Option<ChildStdout>, child: &mut Child) {
    drop(stdout);
    let pid = child.id();
    // SAFETY: pid belongs to this unreaped child, so it cannot have been reused.
    let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}
