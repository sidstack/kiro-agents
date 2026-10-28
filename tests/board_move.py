#!/usr/bin/env python3
"""Cooperative terminal handoff with fake Kiro and an optionally older host."""
import json
import os
import socket
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from board_smoke import Board, BINARY, ROOT

HOST_BINARY = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else BINARY

with tempfile.TemporaryDirectory(prefix='kiro-move-test-') as tmp:
    root = Path(tmp)
    work = root / 'project'
    work.mkdir()
    env = dict(os.environ, HOME=tmp, KIRO_AGENTS_HOME=str(root / 'state'),
               KIRO_HOME=str(root / 'kiro'), KIRO_SESSIONS_DIR=str(root / 'kiro/sessions/cli'),
               KIRO_AGENT_CONFIG_DIR=str(root / 'profiles'), KIRO_AGENTS_AUTO_NAME='off',
               KIRO_AGENTS_KIRO_BIN=str(ROOT / 'tests/fake-kiro/kiro-cli'), TERM='xterm-256color')

    def call(*args):
        return subprocess.check_output([str(BINARY), *args], env=env, timeout=8)

    for variable in ('TMUX', 'TMUX_PANE', 'KIRO_AGENTS_WORKSPACE_SOCKET', 'KIRO_AGENTS_WORKSPACE_CLIENT'):
        env.pop(variable, None)

    subprocess.run([str(HOST_BINARY), 'new', str(work), '--no-attach'], env=env,
                   stdout=subprocess.PIPE, check=True, timeout=8)
    original = json.loads(call('status', '--json'))[0]
    source = target = None
    try:
        source = Board(env)
        source.attach()
        source.frames = b''
        target = Board(env)
        target.frames = b''
        target.press(b'\r')
        target.wait(b'Move session here?')
        # A CLI move without a usable destination TTY must not release anyone.
        refused = subprocess.run([str(BINARY), 'attach', original['id'], '--move'],
                                 env=env, input=b'', capture_output=True, timeout=5)
        assert refused.returncode != 0
        # Refusing the move leaves the first terminal interactive.
        target.press(b'n')
        source.press(b'cancel-keeps-source')
        source.wait(b'BYTES:' + b'cancel-keeps-source'.hex().encode())
        source.frames = b''
        target.frames = b''
        target.press(b'm')
        target.wait(b'Move session here?')
        target.frames = b''
        target.press(b'y')
        target.wait(b'READY:')
        source.wait(b'Kiro sessions')
        target.frames = b''
        target.press(b'only-new-terminal')
        target.wait(b'BYTES:' + b'only-new-terminal'.hex().encode())
        assert json.loads(call('status', '--json'))[0]['pid'] == original['pid']
        # Move the same process back. A late old cleanup cannot erase the new lease.
        target.frames = b''
        source.frames = b''
        source.press(b'm')
        source.wait(b'Move session here?')
        source.frames = b''
        source.press(b'y')
        source.wait(b'READY:')
        target.wait(b'Kiro sessions')
        assert json.loads(call('status', '--json'))[0]['pid'] == original['pid']
        source.detach()
        source.finish()
        source = None
        target.finish()
        target = None
        # Normal detach removes active leases/sockets; lock files are reusable metadata.
        directory = root / 'state/frontends'
        deadline = time.monotonic() + 3
        while list(directory.glob('*.sock')) or list(directory.glob('*.json')):
            assert time.monotonic() < deadline, 'handoff registration leaked'
            time.sleep(0.02)
        assert json.loads(call('status', '--json'))[0]['pid'] == original['pid']
        # An older/unregistered frontend is never forcibly disconnected.
        legacy = socket.socket(socket.AF_UNIX)
        legacy.settimeout(5)
        legacy.connect(str(root / 'state/host.sock'))
        legacy_reader = legacy.makefile('rb')
        def send_legacy(action):
            legacy.sendall(json.dumps({'version': 1, 'action': action}).encode() + b'\n')
        send_legacy({'Attach': {'id': original['id'], 'cols': 80, 'rows': 24}})
        assert json.loads(legacy_reader.readline()) == 'Ok'
        target = Board(env)
        target.press(b'm')
        target.frames = b''
        target.press(b'y')
        target.wait(b'Attach failed:')
        marker = b'legacy-still-attached'
        send_legacy({'Input': list(marker)})
        output = b''
        while b'BYTES:' + marker.hex().encode() not in output:
            frame = json.loads(legacy_reader.readline())
            assert isinstance(frame, dict) and 'Output' in frame, frame
            output += bytes(frame['Output'])
        send_legacy('Detach')
        legacy_reader.close()
        legacy.close()
        target.finish()
        target = None
    finally:
        for board in (source, target):
            if board is not None:
                board.close()
        call('shutdown')

print('PASS: busy attach offers explicit move, cancel preserves source, move/back keeps '
      'one native PID, old terminal returns to board, private registrations clean up')
