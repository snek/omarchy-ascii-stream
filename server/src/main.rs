use std::path::PathBuf;
use std::sync::Arc;

use bootleg::cache::AnsiCache;
use bootleg::movie::decode_file;
use bootleg::ssh::{load_host_key, serve};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("[bootleg] {err}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let video = std::env::var("VIDEO_PATH").unwrap_or_else(|_| "/clips/video.mp4".into());
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(2222);
    let fps: f64 = std::env::var("FPS").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let grid_cols: u32 = std::env::var("GRID_COLS").ok().and_then(|s| s.parse().ok()).unwrap_or(160);
    let host_key = PathBuf::from(std::env::var("HOST_KEY").unwrap_or_else(|_| "./host_key".into()));
    let cache_mib: usize = std::env::var("ANSI_CACHE_MIB").ok().and_then(|s| s.parse().ok()).unwrap_or(512);
    if !PathBuf::from(&video).exists() {
        anyhow::bail!("video not found: {video}");
    }
    println!("[bootleg] decoding {video} ...");
    let movie = decode_file(PathBuf::from(&video).as_path(), grid_cols, fps)
        .await
        .map_err(anyhow::Error::msg)?;
    println!(
        "[bootleg] {} frames @ {:.2}fps ref {}x{}",
        movie.frame_count(), movie.fps, movie.ref_cols, movie.ref_rows
    );
    let key = load_host_key(&host_key).await?;
    let listener = TcpListener::bind((host.as_str(), port)).await?;
    println!("[bootleg] listening on {host}:{port}");
    let movie = Arc::new(movie);
    let cache = Arc::new(AnsiCache::new(cache_mib * 1024 * 1024));
    tokio::select! {
        result = serve(movie, cache, listener, key) => result?,
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm() => {}
    }
    Ok(())
}

async fn sigterm() {
    #[cfg(unix)]
    let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("sigterm")
        .recv()
        .await;
    #[cfg(not(unix))]
    std::future::pending::<()>().await;
}
