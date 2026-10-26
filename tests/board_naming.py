#!/usr/bin/env python3
"""Named sessions, matched native titles and manual overrides through a real PTY."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
from board_smoke import Board, BINARY, ROOT


def main():
    with tempfile.TemporaryDirectory(prefix='kiro-naming-test-') as tmp:
        root = Path(tmp)
        native = root / 'kiro/sessions/cli'
        native.mkdir(parents=True)
        env = dict(os.environ, KIRO_AGENTS_HOME=str(root / 'state'),
                   KIRO_HOME=str(root / 'kiro'), KIRO_SESSIONS_DIR=str(native),
                   KIRO_AGENT_CONFIG_DIR=str(root / 'kiro/agents'),
                   KIRO_AGENTS_AUTO_NAME='auto', TERM='xterm-256color',
                   KIRO_AGENTS_KIRO_BIN=str(ROOT / 'tests/fake-kiro/kiro-cli'))

        def call(*args):
            return subprocess.check_output([str(BINARY), *args], env=env, timeout=10)

        def rows():
            return json.loads(call('status', '--json'))

        board = None
        try:
            call('new', str(root), '--name', 'Manual first name', '--no-attach')
            first = rows()[0]
            native_id = '11111111-1111-4111-8111-111111111111'
            meta = dict(session_id=native_id, cwd=str(root.resolve()), title='Native task title')
            (native / (native_id + '.lock')).write_text(json.dumps(dict(pid=first['pid'], started_at='fixture')))
            (native / (native_id + '.json')).write_text(json.dumps(meta))
            assert rows()[0]['name'] == 'Manual first name'
            call('rename', first['id'], '--clear')
            assert rows()[0]['title'] == 'Native task title'
            assert rows()[0]['title_source'] == 'kiro'
            board = Board(env)
            board.wait(b'Native task title')
            board.press(b'\x1bOQ')  # F2
            board.wait(b'Name session')
            board.press('Fix Ünïcödé pickerx'.encode())
            board.press(b'\x08')  # Guacamole/legacy BS encoding
            board.press(b'\r')
            board.wait(b'Session name updated')
            renamed = rows()[0]
            assert renamed['name'] == 'Fix Ünïcödé picker', renamed
            assert renamed['pid'] == first['pid']
            board.search('Fix Ünïcödé')
            board.wait('Fix Ünïcödé picker'.encode())
            board.press(b'\x1bOQ')
            board.wait(b'Name session')
            board.press(b'\x15\r')  # clear override, automatic title returns
            board.wait(b'Session name updated')
            assert rows()[0].get('name') is None
            assert rows()[0]['title'] == 'Native task title'
            meta['title'] = ''
            (native / (native_id + '.json')).write_text(json.dumps(meta))
            (native / (native_id + '.jsonl')).write_text(json.dumps(dict(
                kind='Prompt', data=dict(content=[dict(kind='text', data='Repair fuzzy agent selection and terminal scrolling')])) ) + '\n')
            assert rows()[0]['title'] == 'Repair fuzzy agent selection and terminal scrolling'
            assert rows()[0]['title_source'] == 'first_message'
            # A different session in the same cwd must never borrow this title.
            call('new', str(root), '--no-attach')
            other = next(row for row in rows() if row['id'] != first['id'])
            assert other.get('title') is None, other
            board.finish()
            print('PASS: native PID-matched names, first-prompt fallback, Unicode F2 rename/clear, search and stable child PIDs')
        finally:
            if board is not None:
                board.close()
            call('shutdown')


if __name__ == '__main__':
    main()
