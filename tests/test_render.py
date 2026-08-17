from __future__ import annotations

import numpy as np
import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from ascii_ssh import FULL, HALF, Movie, _pack_rgb, _sgr_full, _sgr_half

from tests.conftest import solid_frame, stacked_frame


def cell_count(ansi: str) -> int:
    return ansi.count(FULL) + ansi.count(HALF)


def test_pack_rgb_known_values() -> None:
    arr = np.array([[[255, 128, 1]]], dtype=np.uint8)
    assert int(_pack_rgb(arr)[0, 0]) == (255 << 16) | (128 << 8) | 1


def test_sgr_full_and_half() -> None:
    assert _sgr_full(0xFF0000) == "\x1b[49m\x1b[38;2;255;0;0m"
    assert _sgr_half(0x00FF00, 0x0000FF) == (
        "\x1b[38;2;0;255;0m\x1b[48;2;0;0;255m"
    )


def test_fit_pads_to_requested_size() -> None:
    small = np.zeros((3, 4, 3), dtype=np.uint8)
    fitted = Movie._fit(small, 8, 10)
    assert fitted.shape == (8, 10, 3)


def test_from_arrays_rejects_empty() -> None:
    with pytest.raises(RuntimeError, match="no frames"):
        Movie.from_arrays([])


def test_solid_frame_is_full_blocks(red_movie: Movie) -> None:
    out = red_movie.render_block(0, cols=8, rows=4)
    assert cell_count(out) == 8 * 4
    assert FULL in out
    assert HALF not in out
    assert "255;0;0" in out
    assert out.count("\n") == 3


def test_stacked_colors_use_half_blocks() -> None:
    movie = Movie.from_arrays([stacked_frame(8, 8, (255, 0, 0), (0, 0, 255))])
    out = movie.render_block(0, cols=8, rows=4)
    assert cell_count(out) == 8 * 4
    assert HALF in out
    assert FULL not in out
    assert "255;0;0" in out
    assert "0;0;255" in out


def test_rle_collapses_a_solid_row() -> None:
    movie = Movie.from_arrays([solid_frame(4, 20, (10, 20, 30))])
    out = movie.render_block(0, cols=20, rows=2)
    # one SGR + 20 full blocks per row, not 20 separate SGR sequences
    row0 = out.split("\n")[0]
    assert row0.count(FULL) == 20
    assert row0.count("38;2;10;20;30") == 1


def test_frame_ansi_caches_and_wraps(mixed_movie: Movie) -> None:
    a = mixed_movie.frame_ansi(0, 8, 4)
    b = mixed_movie.frame_ansi(0, 8, 4)
    assert a is b
    wrapped = mixed_movie.frame_ansi(2, 8, 4)  # 2 % 2 == 0
    assert wrapped is a
    other = mixed_movie.frame_ansi(1, 8, 4)
    assert other is not a
    assert mixed_movie.frame_ansi(1, 8, 4) is other


def test_different_grids_are_separate_caches(red_movie: Movie) -> None:
    a = red_movie.frame_ansi(0, 8, 4)
    b = red_movie.frame_ansi(0, 10, 4)
    assert a is not b
    assert cell_count(a) == 8 * 4
    assert cell_count(b) == 10 * 4


@given(
    st.integers(min_value=8, max_value=24),
    st.integers(min_value=4, max_value=12),
    st.integers(min_value=0, max_value=255),
    st.integers(min_value=0, max_value=255),
    st.integers(min_value=0, max_value=255),
)
@settings(max_examples=25, deadline=None)
def test_cell_count_matches_grid(
    cols: int, rows: int, r: int, g: int, b: int,
) -> None:
    h = max(rows * 2, 4)
    w = max(cols, 4)
    movie = Movie.from_arrays([solid_frame(h, w, (r, g, b))])
    out = movie.render_block(0, cols, rows)
    assert cell_count(out) == cols * rows


@given(
    st.integers(min_value=0, max_value=255),
    st.integers(min_value=0, max_value=255),
    st.integers(min_value=0, max_value=255),
)
def test_pack_rgb_roundtrip(r: int, g: int, b: int) -> None:
    packed = int(_pack_rgb(np.array([[[r, g, b]]], dtype=np.uint8))[0, 0])
    assert ((packed >> 16) & 255, (packed >> 8) & 255, packed & 255) == (r, g, b)
