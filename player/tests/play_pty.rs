use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Read;
use std::process::Command;

#[test]
fn exported_file_plays_blocks_and_exits() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skip: ffmpeg is required");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bootleg-pty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mp4");
    let out = dir.join("intro");
    assert!(Command::new("ffmpeg").args([
        "-y", "-v", "error", "-f", "lavfi", "-i",
        "color=c=red:s=64x36:r=8:d=0.5", "-pix_fmt", "yuv420p",
    ]).arg(&clip).status().unwrap().success());
    assert!(Command::new(env!("CARGO_BIN_EXE_bootleg-play"))
        .args(["export", "--video"]).arg(&clip)
        .arg("-o").arg(&out)
        .args(["--fps", "8", "--grid-cols", "16"])
        .status().unwrap().success());
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
    let cmd = CommandBuilder::new(out);
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let status = child.wait().unwrap();
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    assert!(status.success());
    assert!(text.contains("\u{1b}[38;2;"));
    assert!(text.contains('█') || text.contains('▀'));
}
