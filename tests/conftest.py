from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import numpy as np
import pytest

from ascii_ssh import Movie

FFMPEG = shutil.which("ffmpeg")
FFPROBE = shutil.which("ffprobe")
HAS_FFMPEG = bool(FFMPEG and FFPROBE)

requires_ffmpeg = pytest.mark.skipif(
    not HAS_FFMPEG, reason="ffmpeg and ffprobe are required")


def solid_frame(h: int, w: int, rgb: tuple[int, int, int]) -> np.ndarray:
    frame = np.empty((h, w, 3), dtype=np.uint8)
    frame[...] = rgb
    return frame


def stacked_frame(
    h: int, w: int, top: tuple[int, int, int], bot: tuple[int, int, int],
) -> np.ndarray:
    """Even rows = top color, odd rows = bot color → half-blocks."""
    frame = np.empty((h, w, 3), dtype=np.uint8)
    frame[0::2] = top
    frame[1::2] = bot
    return frame


@pytest.fixture
def red_movie() -> Movie:
    return Movie.from_arrays(
        [solid_frame(16, 16, (255, 0, 0))],
        fps=12.0,
        src_width=320,
        src_height=180,
    )


@pytest.fixture
def mixed_movie() -> Movie:
    return Movie.from_arrays(
        [
            solid_frame(16, 16, (255, 0, 0)),
            stacked_frame(16, 16, (0, 255, 0), (0, 0, 255)),
        ],
        fps=10.0,
    )


def write_test_clip(path: Path, *, frames: int = 4, fps: int = 8) -> Path:
    if not FFMPEG:
        pytest.skip("ffmpeg is required")
    seconds = frames / fps
    subprocess.check_call([
        FFMPEG, "-y", "-v", "error",
        "-f", "lavfi",
        "-i", f"testsrc=size=64x36:rate={fps}:duration={seconds}",
        "-pix_fmt", "yuv420p",
        str(path),
    ])
    return path


@pytest.fixture
def test_clip(tmp_path: Path) -> Path:
    return write_test_clip(tmp_path / "clip.mp4")
