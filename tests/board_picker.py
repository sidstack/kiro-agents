#!/usr/bin/env python3
"""Picker cancellation, discovery retry and idle rendering in real fake-CLI PTYs."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from board_smoke import Board, BINARY, ROOT

with tempfile.TemporaryDirectory(prefix='kiro-picker-test-') as tmp:
    scratch = Path(tmp)
    fixture = scratch / 'agents.json'
    fixture.write_text(json.dumps({'agents': [{'name': 'reviewer'}]}))
    env = dict(os.environ, KIRO_AGENTS_HOME=str(scratch / 'state'),
               KIRO_AGENTS_KIRO_BIN=str(ROOT / 'tests/fake-kiro/kiro-cli'),
               KIRO_AGENTS_FAKE_AGENT_FIXTURE=str(fixture),
               KIRO_AGENTS_FAKE_AGENT_LIST_DELAY='1.2', TERM='xterm-256color')

    def call(*args):
        return subprocess.check_output([str(BINARY), *args], env=env, timeout=8)

    board = Board(env)
    try:
        # An idle board writes nothing on periodic status refresh.
        board.drain(0.3)
        board.frames = b''
        board.drain(1.3)
        assert len(board.frames) < 128, f'Idle board kept redrawing: {len(board.frames)} bytes'
        board.press(b'\x0e')
        board.wait(b'New session: directory')
        board.frames = b''
        board.press(b'\r')
        board.wait(b'Discovering agents')
        started = time.monotonic()
        board.frames = b''
        board.press(b'\x1b')
        board.wait(b'Browse:')
        assert time.monotonic() - started < 1, 'Esc waited for slow agent discovery'
        assert json.loads(call('status', '--json')) == [], 'Cancellation created a session'
        board.press(b'\x1b')
        board.frames = b''
        fixture.unlink()
        board.press(b'\x0e')
        board.press(b'\r')
        board.wait(b'listing failed')
        fixture.write_text(json.dumps({'agents': [{'name': 'reviewer'}]}))
        board.frames = b''
        board.press(b'\x12')  # Ctrl+R retries in-place.
        board.wait(b'reviewer')
        board.press(b'\x1b[B')
        board.frames = b''
        board.press(b'\r')
        board.wait(b':reviewer')
        rows = json.loads(call('status', '--json'))
        assert len(rows) == 1 and rows[0]['agent'] == 'reviewer'
        board.frames = b''
        board.press(b'\x02d')
        board.wait(b'Kiro sessions')
        board.finish()
        assert json.loads(call('status', '--json'))[0]['pid'] == rows[0]['pid']
    finally:
        board.close()
        if (scratch / 'state' / 'host.sock').exists():
            call('shutdown')
print('PASS: idle board avoids redraws, slow discovery cancels promptly, failed listing retries, arrow-only agent selection')
