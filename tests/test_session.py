from __future__ import annotations

from types import SimpleNamespace

import pytest

from ascii_ssh import AnonServer, StreamSession

from tests.conftest import solid_frame
from ascii_ssh import Movie


class FakeChan:
    def __init__(self) -> None:
        self.chunks: list[str] = []
        self.closed = False

    def write(self, data: str) -> None:
        self.chunks.append(data)

    def is_closing(self) -> bool:
        return self.closed

    def close(self) -> None:
        self.closed = True

    @property
    def text(self) -> str:
        return "".join(self.chunks)


@pytest.fixture
def session() -> StreamSession:
    movie = Movie.from_arrays(
        [solid_frame(20, 40, (1, 2, 3))], fps=24.0, src_width=100, src_height=50,
    )
    sess = StreamSession(movie)
    sess.connection_made(FakeChan())
    return sess


def test_begin_auth_is_off(session: StreamSession) -> None:
    server = AnonServer(session._movie)
    assert server.begin_auth("anyone") is False
    assert isinstance(server.session_requested(), StreamSession)


def test_shell_and_pty_accepted(session: StreamSession) -> None:
    assert session.shell_requested() is True
    assert session.pty_requested("xterm", (64, 20), None) is True
    assert session._tw == 64
    assert session._th == 20


def test_pty_from_object(session: StreamSession) -> None:
    size = SimpleNamespace(width=50, height=18)
    assert session.pty_requested("xterm", size, None) is True
    assert (session._tw, session._th) == (50, 18)


@pytest.mark.parametrize("payload", ["q", "quit", "\x03", "\x04", "\x1b[99;5u"])
def test_quit_keys_close_channel(session: StreamSession, payload: str) -> None:
    chan = session._chan
    assert isinstance(chan, FakeChan)
    session.data_received(payload, 0)
    assert session._quit is True
    assert chan.closed is True


def test_break_and_signal_quit(session: StreamSession) -> None:
    chan = session._chan
    assert isinstance(chan, FakeChan)
    assert session.break_received(0) is True
    assert session._quit is True
    assert chan.closed is True


def test_signal_int_quits(session: StreamSession) -> None:
    session.signal_received("INT")
    assert session._quit is True


def test_eof_quits(session: StreamSession) -> None:
    assert session.eof_received() is False
    assert session._quit is True


def test_empty_and_other_input_ignored(session: StreamSession) -> None:
    session.data_received("", 0)
    session.data_received("x", 0)
    assert session._quit is False
    assert isinstance(session._chan, FakeChan)
    assert session._chan.closed is False


def test_grid_clamps_to_ref_and_terminal(session: StreamSession) -> None:
    session._tw = 8
    session._th = 6
    cols, rows = session._grid()
    assert cols == 10  # floor
    assert rows >= 4
    session._tw = 999
    session._th = 3
    cols, rows = session._grid()
    assert cols == session._movie.ref_cols
    assert rows >= 4


@pytest.mark.asyncio
async def test_pump_writes_banner_then_frame(session: StreamSession) -> None:
    import asyncio

    session._tw, session._th = 16, 10
    task = asyncio.create_task(session._pump())
    await asyncio.sleep(0.05)
    session._quit = True
    await asyncio.wait_for(task, timeout=2)
    chan = session._chan
    assert isinstance(chan, FakeChan)
    assert "quattro" in chan.text
    assert "100x50" in chan.text
    assert "q' quits" in chan.text
    assert "\u2588" in chan.text or "\u2580" in chan.text
