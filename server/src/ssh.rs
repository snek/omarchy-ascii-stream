use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use russh::server::{Auth, Handler, Msg, Server as _, Session};
use russh::{ChannelId, MethodSet};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use crate::cache::AnsiCache;
use crate::movie::Movie;
use crate::protocol::{self, RESTORE};
use crate::render::grid;

pub async fn load_host_key(path: &Path) -> anyhow::Result<russh::keys::PrivateKey> {
    russh::keys::load_secret_key(path, None)
        .context(format!("missing host key: {}", path.display()))
}

struct Chan {
    cols: u32,
    rows: u32,
    have_pty: bool,
    quit: Arc<AtomicBool>,
    started: bool,
    writer: Option<russh::ChannelWriteHalf<Msg>>,
}

pub struct App {
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
}

impl App {
    pub fn new(movie: Arc<Movie>, cache: Arc<AnsiCache>) -> Self {
        Self { movie, cache }
    }
}

pub struct Conn {
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    chans: HashMap<ChannelId, Chan>,
}

impl russh::server::Server for App {
    type Handler = Conn;
    fn new_client(&mut self, _: Option<SocketAddr>) -> Conn {
        Conn {
            movie: Arc::clone(&self.movie),
            cache: Arc::clone(&self.cache),
            chans: HashMap::new(),
        }
    }
}

impl Handler for Conn {
    type Error = anyhow::Error;

    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<Msg>,
        _: &mut Session,
    ) -> Result<bool, Self::Error> {
        // The read half only duplicates Handler callbacks. Leaving it unread
        // fills a bounded channel and stalls this connection on chan.send().
        let (_read, writer) = channel.split();
        let id = writer.id();
        self.chans.insert(
            id,
            Chan {
                cols: 80,
                rows: 24,
                have_pty: false,
                quit: Arc::new(AtomicBool::new(false)),
                started: false,
                writer: Some(writer),
            },
        );
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        col_width: u32,
        row_height: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(ch) = self.chans.get_mut(&channel) {
            if !ch.started {
                ch.cols = col_width;
                ch.rows = row_height;
                ch.have_pty = true;
            }
        }
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        let Some(ch) = self.chans.get_mut(&channel) else {
            return Ok(());
        };
        if ch.started {
            return Ok(());
        }
        ch.started = true;
        let term_cols = if ch.have_pty { ch.cols } else { 80 };
        let term_rows = if ch.have_pty { ch.rows } else { 24 };
        let (cols, rows) = grid(
            term_cols,
            term_rows,
            self.movie.ref_cols,
            self.movie.ref_rows,
        );
        let (cols, rows) = self
            .cache
            .resolve(cols, rows, self.movie.frame_count())
            .await;
        self.cache
            .spawn_fill(Arc::clone(&self.movie), cols, rows)
            .await;
        let Some(writer) = ch.writer.take() else {
            return Ok(());
        };
        let movie = Arc::clone(&self.movie);
        let cache = Arc::clone(&self.cache);
        let quit = Arc::clone(&ch.quit);
        tokio::spawn(async move {
            pump(writer, movie, cache, cols, rows, quit).await;
        });
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn x11_request(
        &mut self,
        channel: ChannelId,
        _: bool,
        _: &str,
        _: &str,
        _: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }

    async fn agent_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        session.channel_failure(channel)?;
        // russh 0.54 sends a global SSH_MSG_REQUEST_FAILURE when this returns false.
        // Ok(true) makes that follow-up a no-op; the channel failure above is the reply.
        Ok(true)
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::reject())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        if protocol::is_quit(data) {
            if let Some(ch) = self.chans.get(&channel) {
                ch.quit.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(ch) = self.chans.get(&channel) {
            ch.quit.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn signal(
        &mut self,
        channel: ChannelId,
        signal: russh::Sig,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        if matches!(
            signal,
            russh::Sig::INT | russh::Sig::TERM | russh::Sig::QUIT | russh::Sig::HUP
        ) {
            if let Some(ch) = self.chans.get(&channel) {
                ch.quit.store(true, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

/// `make_writer` blocks until `bytes` fit in the client window, or the channel sender is closed.
/// `Handle::data` only waits for a session-queue slot and then appends the overflow to `pending_data`.
async fn write_window(
    channel: &russh::ChannelWriteHalf<Msg>,
    bytes: &[u8],
) -> Result<(), russh::Error> {
    let mut writer = channel.make_writer();
    writer.write_all(bytes).await?;
    writer.flush().await?;
    Ok(())
}

async fn pump(
    channel: russh::ChannelWriteHalf<Msg>,
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    cols: u32,
    rows: u32,
    quit: Arc<AtomicBool>,
) {
    let failure = play(&channel, &movie, &cache, cols, rows, &quit).await;
    if let Err(message) = failure {
        let err = format!("\r\n\x1b[0mstream error: {message}\x1b[0m\r\n");
        let _ = write_window(&channel, err.as_bytes()).await;
    }
    let _ = write_window(&channel, RESTORE).await;
    let _ = channel.close().await;
}

async fn play(
    channel: &russh::ChannelWriteHalf<Msg>,
    movie: &Movie,
    cache: &AnsiCache,
    cols: u32,
    rows: u32,
    quit: &AtomicBool,
) -> Result<(), String> {
    write_window(
        channel,
        &protocol::banner(movie.width, movie.height, movie.frame_count(), movie.fps),
    )
    .await
    .map_err(|err| err.to_string())?;

    let frame0 = cache.frame(movie, cols, rows, 0).await?;
    let mut first = b"\x1b[H".to_vec();
    first.extend_from_slice(&frame0);
    write_window(channel, &first)
        .await
        .map_err(|err| err.to_string())?;

    let t0 = Instant::now();
    let mut last = 0usize;
    let n = movie.frame_count();
    let fps = movie.fps;
    loop {
        if quit.load(Ordering::Relaxed) {
            return Ok(());
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let idx = protocol::frame_index(elapsed, fps, n);
        if idx != last {
            let frame = cache.frame(movie, cols, rows, idx).await?;
            let mut msg = b"\x1b[H".to_vec();
            msg.extend_from_slice(&frame);
            write_window(channel, &msg)
                .await
                .map_err(|err| err.to_string())?;
            last = idx;
        }
        let next = (elapsed * fps).floor() + 1.0;
        let deadline = t0 + Duration::from_secs_f64(next / fps);
        let now = Instant::now();
        if deadline <= now {
            tokio::task::yield_now().await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(deadline - now) => {}
                _ = wait_quit(quit) => return Ok(()),
            }
        }
    }
}

async fn wait_quit(quit: &AtomicBool) {
    while !quit.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn serve(
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    listener: TcpListener,
    key: russh::keys::PrivateKey,
) -> anyhow::Result<()> {
    let config = russh::server::Config {
        inactivity_timeout: None,
        auth_rejection_time: Duration::from_secs(0),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        methods: MethodSet::empty(),
        ..Default::default()
    };
    let mut app = App::new(movie, cache);
    app.run_on_socket(Arc::new(config), &listener).await?;
    Ok(())
}
