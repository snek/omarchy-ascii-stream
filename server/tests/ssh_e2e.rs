use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use bootleg::cache::AnsiCache;
use bootleg::movie::decode_file;
use bootleg::ssh::{load_host_key, serve};
use tokio::net::TcpListener;

#[tokio::test]
async fn two_clients_each_get_a_stream() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ssh").arg("-V").output().is_err()
    {
        eprintln!("skip: ffmpeg and ssh are required");
        return;
    }
    let dir = std::env::temp_dir().join(format!("bootleg-ssh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mp4");
    let key_path = dir.join("host_key");
    assert!(Command::new("ffmpeg").args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=64x36:rate=8:duration=0.5", "-pix_fmt", "yuv420p"]).arg(&clip).status().unwrap().success());
    assert!(Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-f"]).arg(&key_path).args(["-N", ""]).status().unwrap().success());
    let movie = Arc::new(decode_file(&clip, 32, 8.0).await.unwrap());
    let cache = Arc::new(AnsiCache::new(64 * 1024 * 1024));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = load_host_key(&key_path).await.unwrap();
    let server = tokio::spawn(async move {
        let _ = serve(movie, cache, listener, key).await;
    });
    let one = tokio::spawn(read_ssh(port));
    let two = tokio::spawn(read_ssh(port));
    let one = tokio::time::timeout(Duration::from_secs(8), one).await.unwrap().unwrap();
    let two = tokio::time::timeout(Duration::from_secs(8), two).await.unwrap().unwrap();
    for text in [&one, &two] {
        assert!(text.contains("bootleg"), "{text}");
        assert!(text.contains("64x36"), "{text}");
        assert!(text.contains("\u{1b}[38;2;"), "{text}");
        assert!(text.contains('█') || text.contains('▀'), "{text}");
    }
    server.abort();
}

async fn read_ssh(port: u16) -> String {
    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-tt",
            "-p", &port.to_string(),
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            "-o", "PreferredAuthentications=none",
            "watch@127.0.0.1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    use tokio::io::AsyncWriteExt;
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(b"q").await;
    }
    let out = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output()).await.unwrap().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}
