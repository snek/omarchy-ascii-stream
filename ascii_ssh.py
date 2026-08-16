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
BG = os.environ.get("BG", "0;0;0")


# ── video decode (once) ─────────────────────────────────────────────────────
def probe(path: str) -> tuple[int, int, float, int]:
    out = subprocess.check_output([
        "ffprobe", "-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=width,height,r_frame_rate,nb_frames",
        "-of", "csv=p=0", path]).decode().strip()
    w, h, rate, nbf = out.split(",")
    num, den = map(int, rate.split("/"))
    fps = num / den if den else 30.0
    return int(w), int(h), fps, int(nbf or 0)


class Movie:
    """Decodes source once; holds raw frames at REF_COLS width."""

    def __init__(self, path: str, ref_cols: int, fps: float):
        sw, sh, src_fps, nbf = probe(path)
        self.fps = fps or src_fps
        self.ref_cols = ref_cols
        self.ref_rows = max(1, round((sh / sw) * ref_cols / 2) * 2)  # even -> halves clean
        self.width, self.height = sw, sh
        self.frames: list[np.ndarray] = []
        proc = subprocess.Popen([
            "ffmpeg", "-v", "error", "-i", path,
            "-vf", f"scale={self.ref_cols}:{self.ref_rows}:flags=area",
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
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
                proc.kill(); proc.wait()

    def render_block(self, frame_idx: int, cols: int, rows: int) -> str:
        """Downscale ref frame to `cols`x(rows*2), emit ANSI solid-block lines."""
        src = self.frames[frame_idx % len(self.frames)]
        # nearest local downsample (area flags on the fly is fine for a still)
        dec_h = max(2, rows * 2)
        step_y = max(1, round(src.shape[0] / dec_h))
        step_x = max(1, round(src.shape[1] / cols))
        small = src[::step_y, ::step_x][:dec_h, :cols]
        # pad if rounding gave us less than requested
        if small.shape[0] < dec_h or small.shape[1] < cols:
            small = self._fit(small, dec_h, cols)

        T = small[0::2]
        B = small[1::2]
        if len(T) > len(B):
            B = np.concatenate([B, T[-1:][None, ...]], axis=0)
        bg_r, bg_g, bg_b = map(int, BG.split(";"))
        out = []
        for r in range(len(T)):
            tr, br = T[r], B[r]
            same = np.all(tr == br, axis=1)
            cell = []
            for c in range(cols):
                if same[c]:
                    t = tr[c]
                    cell.append(f"\x1b[38;2;{t[0]};{t[1]};{t[2]}m\u2588")
                else:
                    t, b = tr[c], br[c]
                    cell.append(
                        f"\x1b[38;2;{t[0]};{t[1]};{t[2]}m"
                        f"\x1b[48;2;{b[0]};{b[1]};{b[2]}m\u2580")
            out.append("".join(cell) + "\x1b[0m")
        return "\n".join(out)

    @staticmethod
    def _fit(a: np.ndarray, dec_h: int, cols: int) -> np.ndarray:
        """Pad (bottom/right) a downsampled frame to exactly dec_h x cols."""
        ph = max(0, dec_h - a.shape[0])
        pw = max(0, cols - a.shape[1])
        if ph or pw:
            a = np.pad(a, ((0, ph), (0, pw), (0, 0)), mode="edge")
        return a[:dec_h, :cols]


# ─── ANSI helpers ──────────────────────────────────────────────────────────
HIDE_CURSOR = "\x1b[?25l"
SHOW_CURSOR = "\x1b[?25h"
ALT_SCREEN = "\x1b[?1049h"
ALT_SCREEN_END = "\x1b[?1049l"
HOME = "\x1b[H"; CLEAR = "\x1b[2J"
RESET = "\x1b[0m"


# ─── session ────────────────────────────────────────────────────────────────
class StreamSession(asyncssh.SSHServerSession):
    def __init__(self, movie: Movie):
        self._movie = movie
        self._chan = None
        self._tw = 80; self._th = 24
        self._cache: dict[tuple[int, int], list[str]] = {}
        self._quit = False

    def connection_made(self, chan):
        self._chan = chan

    def pty_requested(self, term_type, term_size, term_modes):
        if term_size:
            w = getattr(term_size, "width", None) or (term_size[0] if isinstance(term_size, tuple) else None)
            h = getattr(term_size, "height", None) or (term_size[1] if isinstance(term_size, tuple) else None)
            if w and h:
                self._tw, self._th = int(w), int(h)
        return True

    def shell_requested(self):
        return True

    def session_started(self):
        asyncio.create_task(self._pump())

    def data_received(self, data: str, datatype: int):
        if not data:
            return
        first = data[0]
        if data.startswith("q") or ord(first) in (3, 4):  # q / ^C / ^D
            self._quit = True
            if self._chan and not self._chan.is_closing():
                self._chan.close()

    def _frames(self, cols: int, rows: int):
        """Return (or build) the cached block strings for a given terminal grid."""
        key = (cols, rows)
        if key not in self._cache:
            self._cache[key] = [
                self._movie.render_block(i, cols, rows)
                for i in range(len(self._movie.frames))
            ]
        return self._cache[key]

    async def _pump(self):
        m = self._movie
        try:
            # cols use the full width; rows = keep aspect inside terminal height
            cols = min(max(10, self._tw), m.ref_cols)
            aspect_rows = max(4, round((m.ref_rows / m.ref_cols * 2) * cols))
            rows = min(aspect_rows, max(4, self._th - 1))

            frames = self._frames(cols, rows)
            t0 = time.time()
            setup = (
                ALT_SCREEN + CLEAR + HIDE_CURSOR + RESET +
                f"\r\n\x1b[38;2;255;255;255m"
                f"quattro  ·  {m.width}x{m.height}  ·  {len(m.frames)} frames @ {m.fps:.0f}fps"
                f"  ·  'q' quits\x1b[0m\r\n\r\n"
            )
            self._chan.write(setup)
            n = 0
            while not self._quit:
                idx = int(((time.time() - t0) * m.fps)) % len(frames)
                self._chan.write(HOME)
                self._chan.write(frames[idx])
                n += 1
                await asyncio.sleep(0.05)
                if self._quit:
                    break
        except asyncio.CancelledError:
            pass
        except Exception as e:
            try:
                if self._chan and not self._chan.is_closing():
                    self._chan.write(f"\r\n\x1b[0mstream error: {e}\x1b[0m\r\n")
            except Exception:
                pass
        finally:
            for ch in [self._chan]:
                if ch and not ch.is_closing():
                    ch.write(f"{RESET}{SHOW_CURSOR}{ALT_SCREEN_END}")
                    ch.close()


class AnonServer(asyncssh.SSHServer):
    def __init__(self, movie: Movie):
        self._movie = movie

    def begin_auth(self, username: str) -> bool:
        return False  # no authentication

    def session_requested(self) -> StreamSession:
        return StreamSession(self._movie)


async def main():
    if not pathlib.Path(VIDEO).exists():
        raise RuntimeError(f"video not found: {VIDEO}")
    print(f"[ascii_ssh] decoding {VIDEO} ...", flush=True)
    movie = Movie(VIDEO, REF_COLS, MVFPS)
    print(f"[ascii_ssh] {len(movie.frames)} frames @ {movie.fps:.2f}fps "
          f"ref {movie.ref_cols}x{movie.ref_rows}", flush=True)
    if not HOSTKEY.exists():
        raise RuntimeError(f"missing host key: {HOSTKEY} (run: ssh-keygen -t ed25519 -f {HOSTKEY} -N '')")
    server = await asyncssh.create_server(
        lambda: AnonServer(movie), HOST, PORT,
        server_host_keys=[str(HOSTKEY)], encoding="utf-8")
    print(f"[ascii_ssh] listening on {HOST}:{PORT} — connect with any ssh client", flush=True)
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