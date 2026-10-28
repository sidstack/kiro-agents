#!/usr/bin/env python3
"""Popup navigation with a real tmux client, fake Kiro and private temporary state."""
import fcntl
import json
import os
from pathlib import Path
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from board_smoke import Board

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / 'target/debug/kiro-agents'


def wait(check, label):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.03)
    raise AssertionError(label)


def popup_pid(server, binary):
    todo = [int(server)]
    seen = set()
    while todo and len(seen) < 128:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        proc = Path(f'/proc/{pid}')
        try:
            args = (proc / 'cmdline').read_bytes().split(b'\0')
            if b'__workspace-board' in args and os.readlink(proc / 'exe') == str(binary):
                return pid
            for task in list((proc / 'task').iterdir())[:64]:
                todo.extend(int(p) for p in (task / 'children').read_text().split())
        except (FileNotFoundError, ProcessLookupError):
            pass
    return None


def main():
    with tempfile.TemporaryDirectory(prefix='ka-popup-') as tmp:
        root = Path(tmp)
        binary = root / "agent's $binary #test"
        shutil.copyfile(BINARY, binary)
        binary.chmod(0o755)
        project = root / "project's $tree #{literal}"
        project.mkdir()
        state = root / "state's #tree"
        socket = state / 'workspaces/tmux.sock'
        env = dict(os.environ, HOME=str(root / 'home'), SHELL='/bin/sh', TERM='xterm-256color',
                   KIRO_AGENTS_HOME=str(state), KIRO_HOME=str(root / 'kiro'),
                   KIRO_SESSIONS_DIR=str(root / 'kiro/sessions/cli'), KIRO_AGENT_CONFIG_DIR=str(root / 'kiro/agents'),
                   XDG_DATA_HOME=str(root / 'data'), XDG_CONFIG_HOME=str(root / 'config'),
                   XDG_CACHE_HOME=str(root / 'cache'), KIRO_AGENTS_WORKSPACES='on', KIRO_AGENTS_AUTO_NAME='off',
                   KIRO_AGENTS_KIRO_BIN=str(ROOT / 'tests/fake-kiro/kiro-cli'))
        for key in ['TMUX', 'TMUX_PANE', 'KIRO_AGENTS_WORKSPACE_SOCKET', 'KIRO_AGENTS_WORKSPACE_CLIENT']:
            env.pop(key, None)
        client = master = None
        standalone = None
        frames = bytearray()

        def cli(*args):
            return subprocess.check_output([str(binary), *args], env=env, cwd=project, timeout=15, stderr=subprocess.PIPE)

        def tmux(*args, check=True):
            result = subprocess.run(['tmux', '-S', str(socket), *args], env=env, capture_output=True, text=True, timeout=5)
            if check:
                assert result.returncode == 0, (args, result.stderr)
            return result.stdout

        def drain():
            while select.select([master], [], [], 0)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                frames.extend(data)

        def press(keys):
            os.write(master, keys)
            time.sleep(0.12)
            drain()

        def panes():
            return tmux('list-panes', '-a', '-F', '#{pane_id}\t#{pane_pid}\t#{window_id}\t#{@kiro-agents-session}').splitlines()

        def session_pane(identifier):
            for line in panes():
                fields = line.split('\t')
                if len(fields) == 4 and fields[3] == identifier:
                    return fields[0]
            return None

        def popup():
            drain()
            return popup_pid(tmux('display-message', '-p', '#{pid}').strip(), binary)

        def open_dashboard():
            press(b'\x11')
            return wait(popup, 'Ctrl+Q did not open a popup')

        def choose(identifier, key):
            press(b'/')
            press(identifier.encode())
            press(b'\r')
            press(key)

        def current():
            return tmux('display-message', '-p', '#{pane_id}').strip()

        try:
            for agent in ['reviewer', 'planner', 'tester']:
                cli('new', str(project), '--agent', agent, '--no-attach')
            rows = json.loads(cli('status', '--json'))
            original = {row['id']: row['pid'] for row in rows}
            cli('workspace', 'open', 'test', '--dir', str(project), '--detach')
            assert len(panes()) == 1
            first = panes()[0].split('\t')[0]
            # Existing private servers can retain the previous wrapper binding.
            tmux('bind-key', '-T', 'root', 'C-g', 'display-message', 'legacy-dashboard')
            cli('workspace', 'open', 'test', '--detach')
            assert 'C-g' not in tmux('list-keys', '-T', 'root'), 'legacy Ctrl+G binding survived'
            assert tmux('show-options', '-gv', 'prefix').strip() == 'C-b'
            client, master = os.forkpty()
            if client == 0:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', 45, 180, 0, 0))
                os.chdir(project)
                os.execve(str(binary), [str(binary), 'workspace', 'open', 'test'], env)
            wait(popup, 'workspace did not open centered dashboard')
            assert len(panes()) == 1, 'popup created a dashboard tile'
            choose(rows[0]['id'], b'v')
            wait(lambda: session_pane(rows[0]['id']) == first, 'first session did not fill empty launcher')
            wait(lambda: popup() is None, 'dashboard did not close after opening')
            assert len(panes()) == 1
            first_frontend = panes()[0].split('\t')[1]
            wait(lambda: 'READY:' in tmux('capture-pane', '-p', '-t', first), 'native attach not ready')
            press(b'\x07')
            wait(lambda: 'BYTES:07' in ''.join(tmux('capture-pane', '-p', '-t', first).split()), 'Ctrl+G did not reach native Kiro')
            assert popup() is None, 'Ctrl+G opened the wrapper dashboard'
            open_dashboard()
            press(b'\x11')
            wait(lambda: popup() is None, 'Ctrl+Q did not toggle the popup closed')
            open_dashboard()
            press(b'\x1b')
            wait(lambda: popup() is None, 'Escape did not dismiss dashboard')
            assert current() == first
            open_dashboard()
            choose(rows[1]['id'], b'v')
            second = wait(lambda: session_pane(rows[1]['id']), 'right split did not attach')
            assert second != first and len(panes()) == 2
            assert int(tmux('display-message', '-p', '-t', second, '#{pane_left}')) > int(tmux('display-message', '-p', '-t', first, '#{pane_left}'))
            open_dashboard()
            choose(rows[0]['id'], b'\r')
            wait(lambda: current() == first and popup() is None, 'Enter did not focus already attached session')
            assert len(panes()) == 2 and panes()[0].split('\t')[1] == first_frontend
            open_dashboard()
            choose(rows[2]['id'], b'w')
            third = wait(lambda: session_pane(rows[2]['id']), 'new window did not attach')
            assert tmux('display-message', '-p', '-t', third, '#{window_id}') != tmux('display-message', '-p', '-t', first, '#{window_id}')
            open_dashboard()
            choose(rows[1]['id'], b'\r')
            wait(lambda: current() == second and popup() is None, 'cross-window focus failed')
            assert len(panes()) == 3
            # Create through all three popup steps; placement must not overwrite
            # the current live session, and the host should spawn exactly once.
            open_dashboard()
            press(b'\x0e')
            press(b'\x15')
            press(str(project).encode())
            press(b'\r')
            press(b'\r')
            assert len(json.loads(cli('status', '--json'))) == 3, 'created before placement choice'
            press(b'h')
            created = wait(lambda: next((row for row in json.loads(cli('status', '--json')) if row['id'] not in original), None), 'popup create did not start a session')
            below = wait(lambda: session_pane(created['id']), 'created session did not attach below')
            wait(lambda: popup() is None, 'creation did not close dashboard')
            assert len(panes()) == 4 and len(json.loads(cli('status', '--json'))) == 4
            assert int(tmux('display-message', '-p', '-t', below, '#{pane_top}')) > int(tmux('display-message', '-p', '-t', second, '#{pane_top}'))
            # Move an attachment out of tmux without stopping its native child.
            standalone = Board(env)
            standalone.search(rows[0]['id'])
            standalone.press(b'm')
            standalone.wait(b'Move session here?')
            standalone.press(b'n')
            assert session_pane(rows[0]['id']) == first, 'cancel changed workspace attachment'
            standalone.frames = b''
            standalone.press(b'm')
            standalone.press(b'y')
            standalone.wait(b'READY:')
            wait(lambda: session_pane(rows[0]['id']) is None, 'old workspace attachment stayed open')
            wait(lambda: not Path(f'/proc/{first_frontend}').exists(), 'source frontend did not exit')
            standalone.frames = b''
            # Bring it back via the popup, choosing its destination before release.
            origin_frontend = tmux('display-message', '-p', '-t', below, '#{pane_pid}').strip()
            open_dashboard()
            choose(rows[0]['id'], b'm')
            press(b'y')
            # Esc at placement must preserve the standalone attachment.
            press(b'\x1b')
            assert session_pane(rows[0]['id']) is None
            choose(rows[0]['id'], b'm')
            press(b'y')
            press(b'v')
            moved = wait(lambda: session_pane(rows[0]['id']), 'standalone session did not move into workspace')
            wait(lambda: popup() is None and current() == moved, 'moved pane was not focused')
            standalone.wait(b'Kiro sessions')
            assert tmux('display-message', '-p', '-t', below, '#{pane_pid}').strip() == origin_frontend, 'move replaced the active origin pane'
            # Moving the current pane to itself is a focus, not self-replacement.
            count = len(panes())
            moved_pid = tmux('display-message', '-p', '-t', moved, '#{pane_pid}').strip()
            open_dashboard()
            choose(rows[0]['id'], b'm')
            press(b'y')
            press(b'v')
            wait(lambda: popup() is None, 'self move did not dismiss popup')
            assert len(panes()) == count
            assert tmux('display-message', '-p', '-t', moved, '#{pane_pid}').strip() == moved_pid
            standalone.finish()
            standalone = None
            after = {row['id']: row['pid'] for row in json.loads(cli('status', '--json'))}
            assert {key: after[key] for key in original} == original, 'navigation respawned a Kiro process'
            for pid in original.values():
                os.kill(pid, 0)
            print('PASS: centered popup without tile, Esc preservation, empty launcher fill, right split/new window, focus existing/cross-window sessions, standalone/workspace round-trip moves and stable Kiro PIDs')
        finally:
            if standalone is not None:
                standalone.close()
            tmux('kill-server', check=False)
            if client is not None:
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    done, _ = os.waitpid(client, os.WNOHANG)
                    if done:
                        client = None
                        break
                    time.sleep(0.02)
                if client is not None:
                    os.kill(client, signal.SIGKILL)
                    os.waitpid(client, 0)
            if master is not None:
                os.close(master)
            subprocess.run([str(binary), 'shutdown'], env=env, capture_output=True, timeout=10)


if __name__ == '__main__':
    main()
