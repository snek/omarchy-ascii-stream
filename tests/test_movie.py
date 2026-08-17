from __future__ import annotations

from pathlib import Path

import pytest

from ascii_ssh import Movie, probe

from tests.conftest import requires_ffmpeg


@requires_ffmpeg
def test_probe_reads_generated_clip(test_clip: Path) -> None:
    w, h, fps, _n = probe(str(test_clip))
    assert w == 64
    assert h == 36
    assert fps == pytest.approx(8.0)


@requires_ffmpeg
def test_movie_decodes_generated_clip(test_clip: Path) -> None:
    movie = Movie(str(test_clip), ref_cols=32, fps=0)
    assert movie.width == 64
    assert movie.height == 36
    assert movie.ref_cols == 32
    assert movie.ref_rows % 2 == 0
    assert len(movie.frames) >= 3
    assert movie.frames[0].shape[2] == 3
    assert movie.frames[0].dtype == "uint8"
    out = movie.render_block(0, cols=16, rows=8)
    assert out.count("\u2588") + out.count("\u2580") == 16 * 8


@requires_ffmpeg
def test_missing_video_raises(tmp_path: Path) -> None:
    missing = tmp_path / "nope.mp4"
    with pytest.raises((RuntimeError, Exception)):
        Movie(str(missing), ref_cols=16, fps=12)
