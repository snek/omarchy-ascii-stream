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

/// `0.0` means "use the source rate". Anything else must be finite and `> 0`.
/// A probed `0/1` arrives here as `source_fps == 0.0`.
pub fn resolve_playback_fps(fps_override: f64, source_fps: f64) -> Result<f64, String> {
    let fps = if fps_override == 0.0 { source_fps } else { fps_override };
    if fps.is_finite() && fps > 0.0 {
        Ok(fps)
    } else {
        Err("FPS must be finite and greater than 0".to_string())
    }
}

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};

use crate::render::round_half_even;

pub fn ref_rows_for(src_width: u32, src_height: u32, grid_cols: u32) -> u32 {
    let rounded = round_half_even((src_height as f64 / src_width as f64) * grid_cols as f64 / 2.0).max(0) as u32;
    (rounded * 2).max(1)
}

pub async fn decode_file(path: &Path, grid_cols: u32, fps_override: f64) -> Result<Movie, String> {
    let (width, height, src_fps) = probe(path).await?;
    let fps = resolve_playback_fps(fps_override, src_fps)?;
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
    let mut stdout = child.stdout.take();
    let read = match stdout.as_mut() {
        Some(stdout) => collect_frames(stdout, ref_cols, ref_rows).await,
        None => Err("ffmpeg stdout".to_string()),
    };
    shutdown_ffmpeg(stdout, &mut child).await;
    read
}

async fn collect_frames(stdout: &mut ChildStdout, ref_cols: u32, ref_rows: u32) -> Result<Vec<Frame>, String> {
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
    Ok(frames)
}

async fn shutdown_ffmpeg(stdout: Option<ChildStdout>, child: &mut Child) {
    drop(stdout);
    if let Some(pid) = child.id() {
        // SAFETY: pid belongs to this unreaped child, so it cannot have been reused.
        let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
    if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_err() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_playback_fps;

    #[test]
    fn playback_fps_rejects_negative_override_and_probed_zero() {
        let negative = resolve_playback_fps(-5.0, 24.0).unwrap_err();
        assert!(negative.contains("FPS"), "{negative}");
        // ffprobe `0/1` becomes source fps 0.
        let probed = resolve_playback_fps(0.0, 0.0).unwrap_err();
        assert!(probed.contains("FPS"), "{probed}");
        assert!(resolve_playback_fps(0.0, f64::NAN).is_err());
        assert!(resolve_playback_fps(f64::INFINITY, 24.0).is_err());
        assert!(resolve_playback_fps(f64::NEG_INFINITY, 24.0).is_err());
        assert_eq!(resolve_playback_fps(0.0, 23.976).unwrap(), 23.976);
        assert_eq!(resolve_playback_fps(12.5, 24.0).unwrap(), 12.5);
    }
}
