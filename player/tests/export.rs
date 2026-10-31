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
