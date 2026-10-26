#!/usr/bin/env python3
"""New frontend names work with a pre-naming daemon; no host restart or new RPC."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
from board_smoke import Board, BINARY, ROOT

class LegacyPeer:
    def __init__(self, state):
        self.state = state
        state.mkdir()
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.bind(str(state / 'host.sock'))
        self.socket.listen()
        self.socket.settimeout(0.1)
        self.rows = []
        self.bad_action = False
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        while not self.stopped.is_set():
            try: connection, _ = self.socket.accept()
            except socket.timeout: continue
            except OSError: break
            with connection:
                connection.settimeout(2)
                request = json.loads(connection.makefile('rb').readline())['action']
                if request == 'Status': reply = {'Sessions': self.rows}
                elif isinstance(request, dict) and 'New' in request:
                    data = request['New']
                    row = dict(id=f'{len(self.rows)+1:012x}', cwd=data['cwd'], agent=data.get('agent'),
                               generation='v2', pid=None, state='live', exit_code=None, notice=None)
                    self.rows.append(row)
                    (self.state / 'registry.json').write_text(json.dumps(self.rows))
                    reply = {'Created': row}
                else:
                    self.bad_action = True
                    reply = {'Error': 'unsupported legacy action'}
                connection.sendall(json.dumps(reply).encode() + b'\n')

    def close(self):
        self.stopped.set()
        self.socket.close()
        self.thread.join(timeout=2)


def main():
    with tempfile.TemporaryDirectory(prefix='kiro-name-upgrade-') as tmp:
        root = Path(tmp)
        state = root / 'state'
        env = dict(os.environ, HOME=str(root / 'home'), XDG_DATA_HOME=str(root / 'data'), XDG_CONFIG_HOME=str(root / 'config'), XDG_CACHE_HOME=str(root / 'cache'), KIRO_AGENTS_HOME=str(state), KIRO_HOME=str(root / 'kiro'),
                   KIRO_SESSIONS_DIR=str(root / 'kiro/sessions/cli'), KIRO_AGENT_CONFIG_DIR=str(root / 'kiro/agents'),
                   KIRO_AGENTS_AUTO_NAME='off', KIRO_AGENTS_KIRO_BIN=str(ROOT / 'tests/fake-kiro/kiro-cli'), TERM='xterm-256color')
        actual_legacy = os.environ.get('KIRO_AGENTS_LEGACY_BIN')
        process = None
        peer = None
        board = None
        if actual_legacy:
            process = subprocess.Popen([actual_legacy, '__host'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            deadline = time.monotonic() + 8
            while not (state / 'host.sock').exists():
                assert time.monotonic() < deadline
                assert process.poll() is None
                time.sleep(0.05)
        else:
            peer = LegacyPeer(state)

        def call(*args):
            return subprocess.check_output([str(BINARY), *args], env=env, timeout=10)

        def rows(): return json.loads(call('status', '--json'))

        try:
            call('new', str(root), '--name', 'Initial task', '--no-attach')
            first = rows()[0]
            assert first['name'] == 'Initial task'
            call('new', str(root), '--no-attach')
            second = next(row for row in rows() if row['id'] != first['id'])
            board = Board(env)
            board.search(first['id'])
            board.wait(b'Initial task')
            board.press(b'\x1bOQ')
            board.wait(b'Name session')
            board.press(b'\x15')
            board.press('Upgrade-safe Ünïcödé name'.encode())
            board.press(b'\r')
            board.wait(b'Session name updated')
            named = next(row for row in rows() if row['id'] == first['id'])
            assert named['name'] == 'Upgrade-safe Ünïcödé name'
            assert named['pid'] == first['pid'] and named['state'] == 'live'
            saved = json.loads((state / 'registry.json').read_text())
            assert all('name' not in row for row in saved), 'Frontend must not rewrite legacy host registry'
            call('rename', first['id'], '--clear')
            assert next(row for row in rows() if row['id'] == first['id']).get('name') is None
            board.screen()
            board.wait(('#' + first['id'][:6]).encode())
            assert second['id'] != first['id']
            listing = call('status').decode()
            assert '#' + first['id'] in listing and '#' + second['id'] in listing
            board.finish()
            if peer: assert not peer.bad_action, 'Naming sent an unsupported request to the old daemon'
            print('PASS: legacy host naming via CLI/F2, Unicode, clear, distinct labels and unchanged registry/processes')
        finally:
            if board: board.close()
            if peer: peer.close()
            if process:
                subprocess.run([str(BINARY), 'shutdown'], env=env, capture_output=True, timeout=10)
                process.terminate() if process.poll() is None else None
                process.wait(timeout=5)

if __name__ == '__main__': main()
