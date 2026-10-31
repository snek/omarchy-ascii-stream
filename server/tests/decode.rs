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
