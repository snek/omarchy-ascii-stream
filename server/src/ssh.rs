use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use russh::server::{Auth, Handler, Msg, Server as _, Session};
use russh::{ChannelId, MethodSet};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::cache::AnsiCache;
use crate::movie::Movie;
use crate::protocol::{self, RESTORE};
use crate::render::grid;

const STOP_LIVE: u8 = 0;
const STOP_QUIT: u8 = 1;
const STOP_CLOSED: u8 = 2;

pub async fn load_host_key(path: &Path) -> anyhow::Result<russh::keys::PrivateKey> {
    russh::keys::load_secret_key(path, None)
        .context(format!("missing host key: {}", path.display()))
}

struct Chan {
    cols: u32,
    rows: u32,
    have_pty: bool,
    stop: watch::Sender<u8>,
    started: bool,
    writer: Option<russh::ChannelWriteHalf<Msg>>,
}

pub struct App {
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    pumps: Arc<AtomicUsize>,
}

impl App {
    pub fn new(movie: Arc<Movie>, cache: Arc<AnsiCache>) -> Self {
        Self {
            movie,
            cache,
            pumps: Arc::new(AtomicUsize::new(0)),
        }
    }
}

pub struct Conn {
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    chans: HashMap<ChannelId, Chan>,
    pumps: Arc<AtomicUsize>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        for ch in self.chans.values() {
            signal_closed(&ch.stop);
        }
    }
}

impl russh::server::Server for App {
    type Handler = Conn;
    fn new_client(&mut self, _: Option<SocketAddr>) -> Conn {
        Conn {
            movie: Arc::clone(&self.movie),
            cache: Arc::clone(&self.cache),
            chans: HashMap::new(),
            pumps: Arc::clone(&self.pumps),
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
        let (stop, _) = watch::channel(STOP_LIVE);
        self.chans.insert(
            id,
            Chan {
                cols: 80,
                rows: 24,
                have_pty: false,
                stop,
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
        let stop = ch.stop.subscribe();
        let pumps = Arc::clone(&self.pumps);
        self.pumps.fetch_add(1, Ordering::AcqRel);
        tokio::spawn(async move {
            let _inflight = PumpInflight(pumps);
            pump(writer, movie, cache, cols, rows, stop).await;
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
                signal_quit(&ch.stop);
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
            signal_quit(&ch.stop);
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(ch) = self.chans.get(&channel) {
            signal_closed(&ch.stop);
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
                signal_quit(&ch.stop);
            }
        }
        Ok(())
    }
}

fn signal_quit(stop: &watch::Sender<u8>) {
    let _ = stop.send_if_modified(|state| {
        if *state == STOP_LIVE {
            *state = STOP_QUIT;
            true
        } else {
            false
        }
    });
}

fn signal_closed(stop: &watch::Sender<u8>) {
    let _ = stop.send_if_modified(|state| {
        if *state == STOP_CLOSED {
            false
        } else {
            *state = STOP_CLOSED;
            true
        }
    });
}

struct PumpInflight(Arc<AtomicUsize>);

impl Drop for PumpInflight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// russh's channel writer waits on a notify that 0.54 fires only for
/// WINDOW_ADJUST. Close and a dropped peer do not wake it, so every
/// windowed write races `stop`. A closed channel skips the restore write.
async fn write_bytes(channel: &russh::ChannelWriteHalf<Msg>, bytes: &[u8]) -> Result<(), String> {
    let mut writer = channel.make_writer();
    writer
        .write_all(bytes)
        .await
        .map_err(|err| err.to_string())?;
    writer.flush().await.map_err(|err| err.to_string())?;
    Ok(())
}

async fn wait_until(stop: &mut watch::Receiver<u8>, done: impl Fn(u8) -> bool) {
    loop {
        if done(*stop.borrow()) {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

async fn write_while_live(
    channel: &russh::ChannelWriteHalf<Msg>,
    bytes: &[u8],
    stop: &mut watch::Receiver<u8>,
) -> Result<bool, String> {
    tokio::select! {
        biased;
        _ = wait_until(stop, |state| state != STOP_LIVE) => Ok(false),
        result = write_bytes(channel, bytes) => result.map(|_| true),
    }
}

async fn write_if_open(
    channel: &russh::ChannelWriteHalf<Msg>,
    bytes: &[u8],
    stop: &mut watch::Receiver<u8>,
) -> bool {
    if *stop.borrow() == STOP_CLOSED {
        return false;
    }
    tokio::select! {
        biased;
        _ = wait_until(stop, |state| state == STOP_CLOSED) => false,
        result = write_bytes(channel, bytes) => result.is_ok(),
    }
}

async fn pump(
    channel: russh::ChannelWriteHalf<Msg>,
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    cols: u32,
    rows: u32,
    mut stop: watch::Receiver<u8>,
) {
    let failure = play(&channel, &movie, &cache, cols, rows, &mut stop).await;
    if *stop.borrow() != STOP_CLOSED {
        if let Err(message) = failure {
            let err = format!("\r\n\x1b[0mstream error: {message}\x1b[0m\r\n");
            let _ = write_if_open(&channel, err.as_bytes(), &mut stop).await;
        }
    }
    if *stop.borrow() != STOP_CLOSED {
        let _ = write_if_open(&channel, RESTORE, &mut stop).await;
    }
    if *stop.borrow() != STOP_CLOSED {
        tokio::select! {
            biased;
            _ = wait_until(&mut stop, |state| state == STOP_CLOSED) => {}
            result = channel.close() => {
                let _ = result;
            }
        }
    }
}

async fn play(
    channel: &russh::ChannelWriteHalf<Msg>,
    movie: &Movie,
    cache: &AnsiCache,
    cols: u32,
    rows: u32,
    stop: &mut watch::Receiver<u8>,
) -> Result<(), String> {
    let banner = protocol::banner(movie.width, movie.height, movie.frame_count(), movie.fps);
    if !write_while_live(channel, &banner, stop).await? {
        return Ok(());
    }

    let frame0 = cache.frame(movie, cols, rows, 0).await?;
    if *stop.borrow() != STOP_LIVE {
        return Ok(());
    }
    let mut first = b"\x1b[H".to_vec();
    first.extend_from_slice(&frame0);
    if !write_while_live(channel, &first, stop).await? {
        return Ok(());
    }

    let t0 = Instant::now();
    let mut last = 0usize;
    let n = movie.frame_count();
    let fps = movie.fps;
    loop {
        if *stop.borrow() != STOP_LIVE {
            return Ok(());
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let idx = protocol::frame_index(elapsed, fps, n);
        if idx != last {
            let frame = cache.frame(movie, cols, rows, idx).await?;
            if *stop.borrow() != STOP_LIVE {
                return Ok(());
            }
            let mut msg = b"\x1b[H".to_vec();
            msg.extend_from_slice(&frame);
            if !write_while_live(channel, &msg, stop).await? {
                return Ok(());
            }
            last = idx;
        }
        let next = (elapsed * fps).floor() + 1.0;
        let deadline = t0 + Duration::from_secs_f64(next / fps);
        let now = Instant::now();
        if deadline <= now {
            tokio::task::yield_now().await;
        } else {
            tokio::select! {
                biased;
                _ = wait_until(stop, |state| state != STOP_LIVE) => return Ok(()),
                _ = tokio::time::sleep(deadline - now) => {}
            }
        }
    }
}

fn server_config(key: russh::keys::PrivateKey) -> russh::server::Config {
    russh::server::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        auth_rejection_time: Duration::from_secs(0),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![key],
        methods: MethodSet::empty(),
        ..Default::default()
    }
}

pub async fn serve(
    movie: Arc<Movie>,
    cache: Arc<AnsiCache>,
    listener: TcpListener,
    key: russh::keys::PrivateKey,
) -> anyhow::Result<()> {
    let mut app = App::new(movie, cache);
    app.run_on_socket(Arc::new(server_config(key)), &listener)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use russh::server::Server;
    use tokio::net::TcpListener;

    use super::{load_host_key, server_config, App};
    use crate::cache::AnsiCache;
    use crate::movie::Movie;
    use crate::render::Frame;

    struct AcceptKey;

    impl russh::client::Handler for AcceptKey {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            _server_public_key: &russh::keys::PublicKey,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stalled_window_drop_finishes_the_pump() {
        let dir = std::env::temp_dir().join(format!("bootleg-stall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("host_key");
        assert!(
            std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-f"])
                .arg(&key_path)
                .args(["-N", ""])
                .status()
                .unwrap()
                .success(),
            "ssh-keygen"
        );
        let frame = Frame {
            width: 2,
            height: 2,
            rgb: vec![1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3],
        };
        let movie = Arc::new(Movie::from_frames(vec![frame], 8.0, 2, 2).unwrap());
        let cache = Arc::new(AnsiCache::new(1024 * 1024));
        let mut app = App::new(movie, cache);
        let pumps = Arc::clone(&app.pumps);
        let key = load_host_key(&key_path).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let config = Arc::new(server_config(key));
        let server = tokio::spawn(async move {
            let _ = app.run_on_socket(config, &listener).await;
        });

        // Initial window 0 stays shut: russh only sends WINDOW_ADJUST when the
        // window is below half of this target, and half of 0 is 0.
        let client_config = Arc::new(russh::client::Config {
            window_size: 0,
            nodelay: true,
            ..Default::default()
        });
        let mut session = None;
        for _ in 0..40 {
            match russh::client::connect(Arc::clone(&client_config), addr, AcceptKey).await {
                Ok(connected) => {
                    session = Some(connected);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        let mut session = session.expect("russh client connect");
        let auth = tokio::time::timeout(Duration::from_secs(5), session.authenticate_none("watch"))
            .await
            .expect("auth timed out")
            .expect("auth");
        assert!(auth.success(), "none auth");
        let mut channel =
            tokio::time::timeout(Duration::from_secs(5), session.channel_open_session())
                .await
                .expect("open timed out")
                .expect("open session");
        channel.request_shell(false).await.expect("shell");

        let started = tokio::time::timeout(Duration::from_secs(2), async {
            while pumps.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(started.is_ok(), "pump did not start");

        let admitted = tokio::time::timeout(Duration::from_millis(400), async {
            loop {
                match channel.wait().await {
                    Some(russh::ChannelMsg::Data { .. })
                    | Some(russh::ChannelMsg::ExtendedData { .. }) => {
                        panic!("client window admitted stream data");
                    }
                    Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => {
                        panic!("server closed the channel while the window was shut");
                    }
                    Some(_) => {}
                }
            }
        })
        .await;
        assert!(
            admitted.is_err(),
            "expected the client window to stay closed"
        );
        assert_eq!(
            pumps.load(Ordering::Acquire),
            1,
            "pump exited while the window was closed"
        );

        drop(channel);
        drop(session);

        let finished = tokio::time::timeout(Duration::from_secs(2), async {
            while pumps.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            finished.is_ok(),
            "pump still running after the client dropped: {}",
            pumps.load(Ordering::Acquire)
        );
        server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
