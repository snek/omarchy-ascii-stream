from __future__ import annotations

import asyncio
from pathlib import Path

import asyncssh
import pytest

from ascii_ssh import FULL, HALF, Movie, serve

from tests.conftest import requires_ffmpeg

pytestmark = [pytest.mark.e2e, requires_ffmpeg]


async def _listen(movie: Movie, hostkey: Path) -> tuple[asyncssh.SSHAcceptor, int]:
    # port 0 = ephemeral; asyncssh wraps asyncio.start_server
    server = await serve(movie, "127.0.0.1", 0, hostkey)
    sockets = getattr(server, "sockets", None)
    if not sockets:
        # asyncssh SSHAcceptor exposes the underlying asyncio Server
        inner = getattr(server, "_server", None) or getattr(server, "server", None)
        sockets = inner.sockets if inner is not None else []
    if not sockets:
        raise RuntimeError("could not determine bound SSH port")
    port = int(sockets[0].getsockname()[1])
    return server, port


async def _collect(host: str, port: int, *, seconds: float = 1.5) -> str:
    buf: list[str] = []

    class Collector(asyncssh.SSHClientSession):
        def data_received(self, data: str, datatype: int) -> None:
            buf.append(data)

    async with asyncssh.connect(
        host,
        port,
        username="watch",
        known_hosts=None,
        client_keys=None,
        preferred_auth="none",
        login_timeout=5,
    ) as conn:
        chan, _session = await conn.create_session(
            Collector,
            term_type="xterm-256color",
            term_size=(40, 16),
        )
        await asyncio.sleep(seconds)
        try:
            chan.write("q")
        except Exception:
            pass
        try:
            chan.close()
        except Exception:
            pass
    return "".join(buf)


@pytest.mark.asyncio
async def test_anonymous_ssh_streams_blocks(
    test_clip: Path, tmp_path: Path,
) -> None:
    hostkey = tmp_path / "host_key"
    asyncssh.generate_private_key("ssh-ed25519").write_private_key(str(hostkey))
    movie = Movie(str(test_clip), ref_cols=32, fps=8)
    server, port = await _listen(movie, hostkey)
    try:
        text = await asyncio.wait_for(_collect("127.0.0.1", port), timeout=8)
    finally:
        server.close()
        await server.wait_closed()

    assert "quattro" in text
    assert f"{movie.width}x{movie.height}" in text
    assert FULL in text or HALF in text
    assert "\x1b[38;2;" in text


@pytest.mark.asyncio
async def test_second_viewer_also_gets_a_stream(
    test_clip: Path, tmp_path: Path,
) -> None:
    hostkey = tmp_path / "host_key"
    asyncssh.generate_private_key("ssh-ed25519").write_private_key(str(hostkey))
    movie = Movie(str(test_clip), ref_cols=32, fps=8)
    server, port = await _listen(movie, hostkey)
    try:
        first, second = await asyncio.gather(
            _collect("127.0.0.1", port, seconds=0.8),
            _collect("127.0.0.1", port, seconds=0.8),
        )
    finally:
        server.close()
        await server.wait_closed()

    for text in (first, second):
        assert "quattro" in text
        assert FULL in text or HALF in text
