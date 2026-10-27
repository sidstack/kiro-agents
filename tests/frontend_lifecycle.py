#!/usr/bin/env python3
"""Bounded CPU/cleanup regressions using owned PTYs, fake Kiro and temporary state."""
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
from board_smoke import Board, BINARY, ROOT


def ticks(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    return int(fields[11]) + int(fields[12])


def cleanup(board):
    if board.pid is not None:
        os.kill(board.pid, signal.SIGKILL)
        os.waitpid(board.pid, 0)
        board.pid = None
    if board.master is not None:
        os.close(board.master)
        board.master = None


def hangup(board):
    board.drain(0.1)  # enter the input wait before closing the terminal
    os.close(board.master)
    board.master = None
    deadline = time.monotonic() + 1.5
    while time.monotonic() < deadline:
        done, _, usage = os.wait4(board.pid, os.WNOHANG)
        if done:
            board.pid = None
            assert usage.ru_utime + usage.ru_stime < 0.25, 'frontend burned CPU after PTY close'
            return
        time.sleep(0.02)
    raise AssertionError('frontend leaked after PTY close (bounded by harness cleanup)')


def main():
    with tempfile.TemporaryDirectory(prefix='kiro-frontend-life-') as tmp:
        root = Path(tmp)
        env = dict(os.environ, HOME=tmp, KIRO_AGENTS_HOME=str(root / 'state'),
                   KIRO_HOME=str(root / 'kiro'), KIRO_SESSIONS_DIR=str(root / 'kiro/sessions/cli'),
                   KIRO_AGENT_CONFIG_DIR=str(root / 'kiro/agents'), KIRO_AGENTS_AUTO_NAME='off',
                   XDG_DATA_HOME=str(root / 'data'), XDG_CONFIG_HOME=str(root / 'config'),
                   XDG_CACHE_HOME=str(root / 'cache'), TERM='xterm-256color',
                   KIRO_AGENTS_KIRO_BIN=str(ROOT / 'tests/fake-kiro/kiro-cli'))

        def call(*args):
            return subprocess.check_output([str(BINARY), *args], env=env, timeout=10)

        def rows():
            return json.loads(call('status', '--json'))

        boards = []
        try:
            call('new', tmp, '--no-attach')
            row = rows()[0]
            child = row['pid']
            host_pid = int(Path(f'/proc/{child}/stat').read_text().rsplit(')', 1)[1].split()[1])
            # Only the daemon created by this test may be signalled below.
            assert b'__host' in Path(f'/proc/{host_pid}/cmdline').read_bytes().split(b'\0')
            assert f'KIRO_AGENTS_HOME={root / "state"}'.encode() in Path(f'/proc/{host_pid}/environ').read_bytes().split(b'\0')
            for attached in [False, True]:
                board = Board(env)
                boards.append(board)
                if attached:
                    board.search(row['id'])
                    board.attach()
                hangup(board)
                assert rows()[0]['pid'] == child and rows()[0]['state'] == 'live'

            # Detached hosts are intentional, but must remain idle with no client.
            before = ticks(host_pid)
            time.sleep(0.5)
            assert (ticks(host_pid) - before) / os.sysconf('SC_CLK_TCK') < 0.15

            board = Board(env)
            boards.append(board)
            board.search(row['id'])
            board.attach()
            os.kill(host_pid, signal.SIGKILL)
            board.wait(b'Kiro sessions')
            board.drain(0.2)
            before = ticks(board.pid)
            board.drain(0.5)
            assert (ticks(board.pid) - before) / os.sysconf('SC_CLK_TCK') < 0.15, 'host EOF caused frontend spin'
            current = rows()[0]
            assert current['state'] == 'failed' and current['pid'] == child, 'host loss replayed a session'
            board.finish()
            print('PASS: board/attached PTY close exits promptly with bounded CPU, detached host stays idle and sessions survive; host EOF recovers metadata without spinning/replay')
        finally:
            for board in boards:
                cleanup(board)
            subprocess.run([str(BINARY), 'shutdown'], env=env, capture_output=True, timeout=10)


if __name__ == '__main__':
    main()
