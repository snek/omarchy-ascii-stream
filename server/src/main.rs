use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use bootleg::cache::AnsiCache;
use bootleg::movie::{decode_file, resolve_playback_fps};
use bootleg::ssh::{load_host_key, serve};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("[bootleg] {err}");
        std::process::exit(1);
    }
}

fn env_raw(name: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => anyhow::bail!("{name}: {err}"),
    }
}

fn parse_or_default<T>(name: &str, value: Option<String>, default: T) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: Display,
{
    match value {
        None => Ok(default),
        Some(raw) => raw
            .parse()
            .map_err(|err: T::Err| anyhow::anyhow!("{name}: {err}")),
    }
}

fn env_var<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: FromStr,
    T::Err: Display,
{
    parse_or_default(name, env_raw(name)?, default)
}

async fn run() -> anyhow::Result<()> {
    let video = std::env::var("VIDEO_PATH").unwrap_or_else(|_| "/clips/video.mp4".into());
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port: u16 = env_var("PORT", 2222)?;
    let fps: f64 = env_var("FPS", 0.0)?;
    let grid_cols: u32 = env_var("GRID_COLS", 160)?;
    let host_key = PathBuf::from(std::env::var("HOST_KEY").unwrap_or_else(|_| "./host_key".into()));
    let cache_mib: usize = env_var("ANSI_CACHE_MIB", 512)?;
    // `0` means "use the source rate" and is checked after the probe.
    if fps != 0.0 {
        resolve_playback_fps(fps, fps).map_err(anyhow::Error::msg)?;
    }
    if !PathBuf::from(&video).exists() {
        anyhow::bail!("video not found: {video}");
    }
    println!("[bootleg] decoding {video} ...");
    let movie = decode_file(PathBuf::from(&video).as_path(), grid_cols, fps)
        .await
        .map_err(anyhow::Error::msg)?;
    println!(
        "[bootleg] {} frames @ {:.2}fps ref {}x{}",
        movie.frame_count(),
        movie.fps,
        movie.ref_cols,
        movie.ref_rows
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

#[cfg(test)]
mod tests {
    use super::parse_or_default;

    #[test]
    fn unparseable_env_names_the_variable() {
        for (name, raw) in [
            ("PORT", "nope"),
            ("FPS", "nope"),
            ("GRID_COLS", "wide"),
            ("ANSI_CACHE_MIB", "big"),
        ] {
            let err = match name {
                "PORT" => parse_or_default::<u16>(name, Some(raw.into()), 2222)
                    .unwrap_err()
                    .to_string(),
                "FPS" => parse_or_default::<f64>(name, Some(raw.into()), 0.0)
                    .unwrap_err()
                    .to_string(),
                "GRID_COLS" => parse_or_default::<u32>(name, Some(raw.into()), 160)
                    .unwrap_err()
                    .to_string(),
                _ => parse_or_default::<usize>(name, Some(raw.into()), 512)
                    .unwrap_err()
                    .to_string(),
            };
            assert!(err.contains(name), "{err}");
        }
    }
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
