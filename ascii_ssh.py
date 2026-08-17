"""
ascii_ssh.py — anonymous SSH server that streams a video as SOLID-BLOCK art.

Anyone can connect with plain `ssh` (no key, no password) and immediately
watch the clip rendered as colored block cells (U+2580 / U+2588), adapting
to their terminal size.

  ssh -o StrictHostKeyChecking=no watch.<host>
  ssh -p 2222 -o StrictHostKeyChecking=no watch.<host>

Press 'q', or Ctrl+C / Ctrl+D to leave. The server never exposes a shell.

Impl notes:
  AnonServer.begin_auth() -> False   disables authentication entirely
  pty_requested() reads term size    session_started() starts the pump
  data_received() handles q / ^C / ^D

Frames are decoded once at boot. ANSI is rendered lazily (frame 0 first),
cached on the Movie for every viewer, and run-length encoded per row.
"""
from __future__ import annotations

import asyncio
import os
import pathlib
import subprocess
import time

import asyncssh
import numpy as np

VIDEO = os.environ.get("VIDEO_PATH", "/clips/video.mp4")
HOST = os.environ.get("HOST", "0.0.0.0")
PORT = int(os.environ.get("PORT", "2222"))
MVFPS = float(os.environ.get("FPS", "0"))  # 0 = source fps
REF_COLS = int(os.environ.get("GRID_COLS", "160"))
HOSTKEY = pathlib.Path(os.environ.get("HOST_KEY", "./host_key"))

FULL = "\u2588"
HALF = "\u2580"


def probe(path: str) -> tuple[int, int, float, int]:
    out = subprocess.check_output([
        "ffprobe", "-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=width,height,r_frame_rate,nb_frames",
        "-of", "csv=p=0", path]).decode().strip()
    w, h, rate, nbf = out.split(",")
    num, den = map(int, rate.split("/"))
    fps = num / den if den else 30.0
    return int(w), int(h), fps, int(nbf or 0)


def _pack_rgb(rgb: np.ndarray) -> np.ndarray:
    """Pack an (H, W, 3) uint8 array into (H, W) uint32 0x00RRGGBB."""
    r = rgb[:, :, 0].astype(np.uint32)
    g = rgb[:, :, 1].astype(np.uint32)
    b = rgb[:, :, 2].astype(np.uint32)
    return (r << 16) | (g << 8) | b


def _sgr_full(rgb: int) -> str:
    return f"\x1b[49m\x1b[38;2;{(rgb >> 16) & 255};{(rgb >> 8) & 255};{rgb & 255}m"


def _sgr_half(top: int, bot: int) -> str:
    return (
        f"\x1b[38;2;{(top >> 16) & 255};{(top >> 8) & 255};{top & 255}m"
        f"\x1b[48;2;{(bot >> 16) & 255};{(bot >> 8) & 255};{bot & 255}m"
    )


class Movie:
    """Decodes source once; holds raw frames at REF_COLS width."""

    def __init__(self, path: str, ref_cols: int, fps: float):
        sw, sh, src_fps, _nbf = probe(path)
        self.fps = fps or src_fps
        self.ref_cols = ref_cols
        self.ref_rows = max(1, round((sh / sw) * ref_cols / 2) * 2)
        self.width, self.height = sw, sh
        self.frames: list[np.ndarray] = []
        # (cols, rows) -> list[str | None] aligned with self.frames
        self._ansi: dict[tuple[int, int], list[str | None]] = {}
        proc = subprocess.Popen([
            "ffmpeg", "-v", "error", "-i", path,
            "-vf", f"scale={self.ref_cols}:{self.ref_rows}:flags=area",
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        assert proc.stdout is not None
        size = self.ref_cols * self.ref_rows * 3
        try:
            while True:
                raw = proc.stdout.read(size)
                if len(raw) < size:
                    break
                self.frames.append(
                    np.frombuffer(raw, dtype=np.uint8).reshape(
                        self.ref_rows, self.ref_cols, 3))
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
        if not self.frames:
            raise RuntimeError(f"no frames decoded from {path}")

    def frame_ansi(self, frame_idx: int, cols: int, rows: int) -> str:
        """Return cached ANSI for one frame, rendering it on first use."""
        key = (cols, rows)
        bucket = self._ansi.get(key)
        if bucket is None:
            bucket = [None] * len(self.frames)
            self._ansi[key] = bucket
        i = frame_idx % len(self.frames)
        cell = bucket[i]
        if cell is None:
            cell = self.render_block(i, cols, rows)
            bucket[i] = cell
        return cell

    def render_block(self, frame_idx: int, cols: int, rows: int) -> str:
        """Downscale ref frame to cols x (rows*2), emit RLE'd ANSI block lines."""
        src = self.frames[frame_idx % len(self.frames)]
        dec_h = max(2, rows * 2)
        step_y = max(1, round(src.shape[0] / dec_h))
        step_x = max(1, round(src.shape[1] / cols))
        small = src[::step_y, ::step_x][:dec_h, :cols]
        if small.shape[0] < dec_h or small.shape[1] < cols:
            small = self._fit(small, dec_h, cols)

        top = small[0::2]
        bot = small[1::2]
        if len(top) > len(bot):
            bot = np.concatenate([bot, top[-1:]], axis=0)

        top_p = _pack_rgb(top)
        bot_p = _pack_rgb(bot)
        same = top_p == bot_p
        # bit 48 = half-block, low 24 = top RGB, next 24 = bottom RGB
        keys = top_p.astype(np.uint64)
        keys |= np.where(same, np.uint64(0), bot_p.astype(np.uint64)) << 24
        keys |= (~same).astype(np.uint64) << 48

        lines: list[str] = []
        for r in range(keys.shape[0]):
            row = keys[r]
            n = row.size
            starts = [0]
            if n > 1:
                starts.extend((np.flatnonzero(row[1:] != row[:-1]) + 1).tolist())
            starts.append(n)
            parts: list[str] = []
            for a, b in zip(starts, starts[1:]):
                k = int(row[a])
                run = b - a
                top_rgb = k & 0xFFFFFF
                if k & (1 << 48):
                    parts.append(_sgr_half(top_rgb, (k >> 24) & 0xFFFFFF) + HALF * run)
                else:
                    parts.append(_sgr_full(top_rgb) + FULL * run)
            parts.append("\x1b[0m")
            lines.append("".join(parts))
        return "\n".join(lines)

    @staticmethod
    def _fit(a: np.ndarray, dec_h: int, cols: int) -> np.ndarray:
        """Pad (bottom/right) a downsampled frame to exactly dec_h x cols."""
        ph = max(0, dec_h - a.shape[0])
        pw = max(0, cols - a.shape[1])
        if ph or pw:
            a = np.pad(a, ((0, ph), (0, pw), (0, 0)), mode="edge")
        return a[:dec_h, :cols]


HIDE_CURSOR = "\x1b[?25l"
SHOW_CURSOR = "\x1b[?25h"
ALT_SCREEN = "\x1b[?1049h"
ALT_SCREEN_END = "\x1b[?1049l"
HOME = "\x1b[H"
CLEAR = "\x1b[2J"
RESET = "\x1b[0m"


class StreamSession(asyncssh.SSHServerSession):
    def __init__(self, movie: Movie):
        self._movie = movie
        self._chan = None
        self._tw = 80
        self._th = 24
        self._quit = False
        self._prefetch_task: asyncio.Task[None] | None = None

    def connection_made(self, chan: asyncssh.SSHServerChannel) -> None:
        self._chan = chan

    def pty_requested(self, term_type, term_size, term_modes) -> bool:
        if term_size:
            w = getattr(term_size, "width", None) or (
                term_size[0] if isinstance(term_size, tuple) else None)
            h = getattr(term_size, "height", None) or (
                term_size[1] if isinstance(term_size, tuple) else None)
            if w and h:
                self._tw, self._th = int(w), int(h)
        return True

    def shell_requested(self) -> bool:
        return True

    def session_started(self) -> None:
        asyncio.create_task(self._pump())

    def data_received(self, data: str, datatype: int) -> None:
        if not data:
            return
        first = data[0]
        if data.startswith("q") or ord(first) in (3, 4):
            self._quit = True
            if self._chan and not self._chan.is_closing():
                self._chan.close()

    def _grid(self) -> tuple[int, int]:
        m = self._movie
        cols = min(max(10, self._tw), m.ref_cols)
        aspect_rows = max(4, round((m.ref_rows / m.ref_cols * 2) * cols))
        rows = min(aspect_rows, max(4, self._th - 1))
        return cols, rows

    async def _prefetch(self, cols: int, rows: int) -> None:
        """Fill the shared ANSI cache without blocking the first frames."""
        n = len(self._movie.frames)
        for i in range(n):
            if self._quit:
                return
            self._movie.frame_ansi(i, cols, rows)
            if i % 4 == 3:
                await asyncio.sleep(0)

    async def _pump(self) -> None:
        m = self._movie
        try:
            cols, rows = self._grid()
            n = len(m.frames)
            setup = (
                ALT_SCREEN + CLEAR + HIDE_CURSOR + RESET
                + "\r\n\x1b[38;2;255;255;255m"
                + f"quattro  ·  {m.width}x{m.height}  ·  {n} frames @ {m.fps:.0f}fps"
                + "  ·  'q' quits\x1b[0m\r\n\r\n"
            )
            self._chan.write(setup)

            # Frame 0 goes out immediately; the rest warm the shared cache.
            first = m.frame_ansi(0, cols, rows)
            self._chan.write(HOME + first)
            self._prefetch_task = asyncio.create_task(self._prefetch(cols, rows))

            t0 = time.perf_counter()
            last = 0
            while not self._quit:
                idx = int((time.perf_counter() - t0) * m.fps) % n
                if idx != last:
                    self._chan.write(HOME + m.frame_ansi(idx, cols, rows))
                    last = idx
                next_t = t0 + (int((time.perf_counter() - t0) * m.fps) + 1) / m.fps
                delay = next_t - time.perf_counter()
                await asyncio.sleep(delay if delay > 0 else 0)
        except asyncio.CancelledError:
            pass
        except Exception as e:
            try:
                if self._chan and not self._chan.is_closing():
                    self._chan.write(f"\r\n\x1b[0mstream error: {e}\x1b[0m\r\n")
            except Exception:
                pass
        finally:
            if self._prefetch_task and not self._prefetch_task.done():
                self._prefetch_task.cancel()
            if self._chan and not self._chan.is_closing():
                self._chan.write(f"{RESET}{SHOW_CURSOR}{ALT_SCREEN_END}")
                self._chan.close()


class AnonServer(asyncssh.SSHServer):
    def __init__(self, movie: Movie):
        self._movie = movie

    def begin_auth(self, username: str) -> bool:
        return False

    def session_requested(self) -> StreamSession:
        return StreamSession(self._movie)


async def main() -> None:
    if not pathlib.Path(VIDEO).exists():
        raise RuntimeError(f"video not found: {VIDEO}")
    print(f"[ascii_ssh] decoding {VIDEO} ...", flush=True)
    movie = Movie(VIDEO, REF_COLS, MVFPS)
    print(f"[ascii_ssh] {len(movie.frames)} frames @ {movie.fps:.2f}fps "
          f"ref {movie.ref_cols}x{movie.ref_rows}", flush=True)
    if not HOSTKEY.exists():
        raise RuntimeError(
            f"missing host key: {HOSTKEY} "
            f"(run: ssh-keygen -t ed25519 -f {HOSTKEY} -N '')")
    server = await asyncssh.create_server(
        lambda: AnonServer(movie), HOST, PORT,
        server_host_keys=[str(HOSTKEY)], encoding="utf-8")
    print(f"[ascii_ssh] listening on {HOST}:{PORT} — connect with any ssh client",
          flush=True)
    try:
        await asyncio.Event().wait()
    finally:
        server.close()
        await server.wait_closed()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
