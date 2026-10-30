#!/usr/bin/env python3
"""Lifecycle filters over a synthetic metadata peer; never infer native turn state."""
import os
from pathlib import Path
import tempfile
import time
from board_smoke import Board
from naming_upgrade import LegacyPeer


def main():
    with tempfile.TemporaryDirectory(prefix='kiro-lifecycle-board-') as tmp:
        root = Path(tmp)
        peer = LegacyPeer(root / 'state')
        board = None
        def row(identifier, state='live', activity=None):
            result = dict(id=identifier, name='Label ' + identifier, cwd=str(root), agent=None,
                          generation='v2', pid=None, state=state, exit_code=None, notice=None)
            if activity is not None:
                result['activity'] = activity
            return result
        peer.rows = [row('legacy-unknown'), row('waiting-approval', activity='needs_you'),
                     row('active-turn', activity='working'), row('settled-turn', activity='ready'),
                     row('stopped-stale-ready', state='stopped', activity='ready'),
                     row('future-unknown', activity='future_status')]
        env = dict(os.environ, HOME=str(root / 'home'), KIRO_AGENTS_HOME=str(root / 'state'),
                   KIRO_HOME=str(root / 'kiro'), KIRO_SESSIONS_DIR=str(root / 'kiro/sessions/cli'),
                   KIRO_AGENTS_AUTO_NAME='off', TERM='xterm-256color')
        ids = [r['id'].encode() for r in peer.rows]
        try:
            board = Board(env)
            for label, expected in [(b'All', ids), (b'Needs you', [ids[1]]),
                                    (b'Working', [ids[2]]), (b'Ready', [ids[3]]),
                                    (b'Inactive', [ids[4]])]:
                screen = board.screen()
                assert label in screen
                for identifier in ids:
                    assert (identifier in screen) == (identifier in expected), (label, identifier, screen)
                board.press(b'\t')
            board.search('settled-turn')
            for _ in range(3):
                board.press(b'\t')
            assert ids[3] in board.screen()
            peer.rows[3]['activity'] = 'working'
            deadline = time.monotonic() + 8
            while b'Label settled-turn' in board.screen():
                assert time.monotonic() < deadline, 'stale Ready row survived a refresh'
                time.sleep(0.05)
            board.press(b'\x1b[Z')
            assert b'Label settled-turn' in board.screen(), 'query lost while switching back to Working'
            assert not peer.bad_action, 'filters must use Status only'
            board.finish()
            print('PASS: five lifecycle filters, old/future activity unknown, stale state, reverse cycling and query preservation')
        finally:
            if board:
                board.close()
            peer.close()


if __name__ == '__main__':
    main()
