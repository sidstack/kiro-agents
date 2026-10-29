#!/usr/bin/env python3
"""Exercise owned tmux panes with fake Kiro; never touch the default server."""
import json
import fcntl
import os
import re
from pathlib import Path
import shutil
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from workspace_dashboard import popup_pid

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / 'target/debug/kiro-agents'
TMUX = shutil.which('tmux')
if TMUX is None:
    raise SystemExit('workspace integration requires tmux 3.2+')


def wait(check, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(message)


def main():
    with tempfile.TemporaryDirectory(prefix='ka-mux-') as tmp:
        root = Path(tmp)
        state = root / "state's #tree"
        socket = state / 'workspaces/tmux.sock'
        foreign = root / 'foreign.sock'
        project = root / "project's $tree #{pane_id} ; untouched"
        project.mkdir()
        binary = root / "kiro-agent's $binary #name"
        shutil.copyfile(BINARY, binary)
        binary.chmod(0o755)
        env = dict(os.environ, HOME=str(root / 'home'), SHELL='/bin/sh', TERM='xterm-256color',
                   KIRO_AGENTS_HOME=str(state), KIRO_HOME=str(root / 'kiro'),
                   KIRO_SESSIONS_DIR=str(root / 'kiro/sessions/cli'),
                   KIRO_AGENT_CONFIG_DIR=str(root / 'kiro/agents'), KIRO_AGENTS_AUTO_NAME='off',
                   XDG_DATA_HOME=str(root / 'data'), XDG_CONFIG_HOME=str(root / 'config'),
                   XDG_CACHE_HOME=str(root / 'cache'), KIRO_AGENTS_WORKSPACES='on',
                   KIRO_AGENTS_KIRO_BIN=str(ROOT / 'tests/fake-kiro/kiro-cli'))
        for key in ['TMUX', 'TMUX_PANE', 'KIRO_AGENTS_WORKSPACE_SOCKET']:
            env.pop(key, None)
        client = None
        master = None
        pane_processes = {}

        def drain():
            while select.select([master], [], [], 0)[0]:
                try:
                    if not os.read(master, 65536):
                        break
                except OSError:
                    break

        def press(keys):
            os.write(master, keys)
            time.sleep(0.05)
            drain()

        def client_done():
            drain()
            pid, status = os.waitpid(client, os.WNOHANG)
            if pid:
                assert os.waitstatus_to_exitcode(status) == 0, status
                return True
            return False

        def cli(*args, context=None, success=True):
            result = subprocess.run([str(binary), *args], cwd=project, env=context or env,
                                    capture_output=True, text=True, timeout=15)
            if success:
                assert result.returncode == 0, (args, result.stdout, result.stderr)
            else:
                assert result.returncode != 0, (args, result.stdout)
            return result.stdout

        def tmux(*args, path=socket, check=True):
            result = subprocess.run([TMUX, '-S', str(path), *args], env=env,
                                    capture_output=True, text=True, timeout=5)
            if check:
                assert result.returncode == 0, (args, result.stderr)
            return result.stdout

        def capture(pane):
            return tmux('capture-pane', '-p', '-t', pane)

        def pane_cwd(pane):
            pane_pid = tmux('display-message', '-p', '-t', pane, '#{pane_pid}').strip()
            return os.readlink(f'/proc/{pane_pid}/cwd')

        def panes():
            result = tmux('list-panes', '-a', '-F', '#{pane_id}').splitlines()
            for pane in result:
                pid = int(tmux('display-message', '-p', '-t', pane, '#{pane_pid}'))
                try:
                    pane_processes[pid] = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
                except FileNotFoundError:
                    pass
            return result

        def frontend_alive(pid, start):
            try:
                fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
                return fields[0] != 'Z' and fields[19] == start and os.readlink(f'/proc/{pid}/exe') == str(binary)
            except FileNotFoundError:
                return False

        def context(pane, path=socket):
            value = tmux('display-message', '-p', '-t', pane, '#{socket_path},#{pid},0', path=path).strip()
            return dict(env, TMUX=value, TMUX_PANE=pane, KIRO_AGENTS_WORKSPACE_SOCKET=str(socket))

        def popup():
            drain()
            return popup_pid(tmux('display-message', '-p', '#{pid}').strip(), binary)

        def pick(identifier, placement=b'v'):
            wait(popup, 'dashboard popup did not open')
            press(b'/')
            press(identifier.encode())
            press(b'\r')
            press(placement)
            wait(lambda: popup() is None, 'dashboard popup did not close')
            return wait(lambda: next((line.split('\t')[0] for line in tmux('list-panes', '-a', '-F', '#{pane_id}\t#{@kiro-agents-session}').splitlines() if line.endswith('\t' + identifier)), None), 'selected session did not attach')

        try:
            disabled = dict(env, KIRO_AGENTS_WORKSPACES='off', KIRO_AGENTS_HOME=str(root / 'disabled'))
            cli('workspace', 'open', 'disabled', '--detach', context=disabled, success=False)
            cli('workspace', 'reload', context=disabled, success=False)
            assert not (root / 'disabled/workspaces/tmux.sock').exists()
            missing = dict(env, KIRO_AGENTS_TMUX_BIN=str(root / 'missing-tmux'))
            cli('workspace', 'open', 'missing', '--detach', context=missing, success=False)
            assert not socket.exists()
            cli('workspace', 'open', 'no-tty', success=False)
            assert not socket.exists()
            for name in ['.hidden', 'bad:target', '-flag']:
                cli('workspace', 'open', '--detach', '--', name, success=False)
            assert not socket.exists()
            cli('new', str(project), '--agent', 'reviewer', '--no-attach')
            cli('new', str(project), '--agent', 'planner', '--no-attach')
            rows = json.loads(cli('status', '--json'))
            assert len(rows) == 2
            for agent in ['extra-right', 'extra-below']:
                cli('new', str(project), '--agent', agent, '--no-attach')
            rows = json.loads(cli('status', '--json'))
            assert len(rows) == 4
            original_pids = {row['id']: row['pid'] for row in rows}
            cli('workspace', 'open', 'test', '--dir', str(project), '--detach')
            assert 'test' in cli('workspace', 'list')
            first = panes()[0]
            tmux('resize-window', '-t', 'test', '-x', '220', '-y', '45')
            actual_cwd = pane_cwd(first)
            assert actual_cwd == str(project), (actual_cwd, str(project))
            own = context(first)
            # A real attached client runs the prefix parser and owns the popup.
            client, master = os.forkpty()
            if client == 0:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', 45, 220, 0, 0))
                os.chdir(project)
                os.execve(str(binary), [str(binary), 'workspace', 'open', 'test'], env)
            wait(lambda: tmux('list-clients', '-F', '#{client_pid}').strip(), 'tmux client did not attach')
            assert pick(rows[0]['id']) == first
            press(b'\x11')
            second = pick(rows[1]['id'])
            assert second != first
            tmux('send-keys', '-t', first, '-l', 'h')
            wait(lambda: 'BYTES:68' in ''.join(capture(first).split()), 'raw input was not forwarded')
            cli('workspace', 'open', 'alternate', '--dir', str(project), '--detach')
            alternate = tmux('list-panes', '-t', 'alternate', '-F', '#{pane_id}').strip()
            cli('workspace', 'open', 'alternate', context=own)
            wait(lambda: tmux('list-clients', '-F', '#{session_name}').strip() == 'alternate', 'owned client did not switch workspace')
            cli('workspace', 'open', 'test', context=context(alternate))
            wait(lambda: tmux('list-clients', '-F', '#{session_name}').strip() == 'test', 'owned client did not switch back')
            cli('workspace', 'close', 'alternate')
            assert len(panes()) == 2, 'reopening workspace created extra panes'
            assert tmux('show-options', '-gv', 'mouse').strip() == 'on'
            # Kiro's primary-screen history belongs to tmux. Echoed page-key
            # bytes are not evidence of scrolling; require a moved viewport.
            tmux('select-pane', '-t', first)
            tmux('send-keys', '-t', second, '-l', 'history')
            wait(lambda: 'SCROLL_END' in capture(second), 'fixture did not produce scrollback')
            assert int(tmux('display-message', '-p', '-t', second, '#{history_size}')) > 0
            x, y = map(int, tmux('display-message', '-p', '-t', second, '#{pane_left} #{pane_top}').split())
            def received(pane):
                return ''.join(re.findall(r'BYTES:([0-9a-f]+)', ''.join(capture(pane).split())))
            def scroll_position(pane):
                return int(tmux('display-message', '-p', '-t', pane, '#{scroll_position}').strip() or '0')
            press(f'\x1b[<64;{x + 3};{y + 3}M'.encode())
            wait(lambda: tmux('display-message', '-p', '-t', second, '#{pane_in_mode}').strip() == '1', 'wheel did not enter hovered pane scrollback')
            wait(lambda: int(tmux('display-message', '-p', '-t', second, '#{scroll_position}')) > 0, 'wheel did not move viewport')
            assert tmux('display-message', '-p', '-t', first, '#{pane_in_mode}').strip() == '0', 'wheel scrolled wrong pane'
            assert '1b5b357e' not in received(second), 'wheel injected PageUp into prompt'
            assert '1b5b41' not in received(second) and '1b5b42' not in received(second), 'wheel became prompt-history arrows'
            before = scroll_position(second)
            press(f'\x1b[<65;{x + 3};{y + 3}M'.encode())
            wait(lambda: scroll_position(second) < before, 'wheel down did not move viewport down')
            tmux('select-pane', '-t', second)
            if tmux('display-message', '-p', '-t', second, '#{pane_in_mode}').strip() == '1':
                press(b'q')
            press(b'\x1b[5~')
            wait(lambda: tmux('display-message', '-p', '-t', second, '#{pane_in_mode}').strip() == '1', 'PageUp did not enter scrollback')
            before = int(tmux('display-message', '-p', '-t', second, '#{scroll_position}'))
            assert before > 0
            press(b'\x1b[6~')
            wait(lambda: scroll_position(second) < before, 'PageDown did not move viewport down')
            if tmux('display-message', '-p', '-t', second, '#{pane_in_mode}').strip() == '1':
                press(b'q')
            # Legacy X10 wheel reporting must work as well as SGR reporting.
            press(b'\x1b[M' + bytes([64 + 32, x + 3 + 32, y + 3 + 32]))
            wait(lambda: scroll_position(second) > 0, 'legacy mouse wheel did not scroll')
            press(b'q')
            # Reproduce the reported MobaXterm substitution: these are ordinary
            # keyboard arrows on the wire, so tmux cannot identify them as wheel
            # events without also stealing intentional prompt-history keys.
            for key in [b'\x1b[A', b'\x1bOA']:
                before_arrows = received(second).count('1b5b41')
                press(key)
                # Tmux normalizes CSI/SS3 input to the child's normal-cursor mode.
                wait(lambda: received(second).count('1b5b41') > before_arrows, 'native arrow-key passthrough changed')
                assert tmux('display-message', '-p', '-t', second, '#{pane_in_mode}').strip() == '0'
            # A full-screen application that requests mouse reporting keeps its
            # native mouse and paging keys; do not force it into copy mode.
            tmux('send-keys', '-t', second, '-l', 'screen')
            wait(lambda: 'NATIVE' in capture(second), 'fixture did not enter native mouse mode')
            for key, expected in [(b'\x1b[5~', '1b5b357e'), (b'\x1b[6~', '1b5b367e')]:
                press(key)
                wait(lambda: expected in received(second), 'native full-screen page key was intercepted')
            press(f'\x1b[<64;{x + 3};{y + 3}M'.encode())
            wait(lambda: '1b5b3c3634' in received(second), 'native application mouse wheel was intercepted')
            assert tmux('display-message', '-p', '-t', second, '#{pane_in_mode}').strip() == '0'
            tmux('send-keys', '-t', second, '-l', 'leave')
            wait(lambda: 'LEFT' in capture(second), 'fixture did not leave native mouse mode')
            tmux('select-pane', '-t', first)
            press(b'\x02z')
            wait(lambda: tmux('display-message', '-p', '-t', first, '#{window_zoomed_flag}').strip() == '1', 'prefix z did not zoom')
            press(b'\x02z')
            wait(lambda: tmux('display-message', '-p', '-t', first, '#{window_zoomed_flag}').strip() == '0', 'prefix z did not unzoom')
            press(b'\x1b[23~')
            wait(lambda: tmux('display-message', '-p', '-t', first, '#{window_zoomed_flag}').strip() == '1', 'F11 did not zoom')
            press(b'\x1b[23~')
            wait(lambda: tmux('display-message', '-p', '-t', first, '#{window_zoomed_flag}').strip() == '0', 'F11 did not unzoom')
            assert tmux('show-options', '-gv', 'pane-border-status').strip() == 'top'
            assert tmux('show-options', '-pv', '-t', first, '@kiro-agents-title').strip()
            config = state / 'workspaces/tmux.conf'
            config.write_text('unbind-key -n F11\nset-option -g @config-test loaded\n')
            cli('workspace', 'reload')
            assert tmux('show-options', '-gv', '@config-test').strip() == 'loaded'
            assert 'F11' not in tmux('list-keys', '-T', 'root')
            config.write_text('invalid-tmux-command\n')
            cli('workspace', 'reload', success=False)
            config.unlink()
            cli('workspace', 'reload')
            assert 'F11' in tmux('list-keys', '-T', 'root')
            tmux('resize-pane', '-t', first, '-x', '90')
            size = tmux('display-message', '-p', '-t', first, '#{pane_width}x#{pane_height}').strip()
            wait(lambda: f'SIZE:{size}' in capture(first), 'exact resize did not reach fake Kiro')
            press(b'\x11')
            wait(popup, 'Ctrl+Q did not open dashboard')
            assert tmux('list-clients', '-F', '#{client_pid}').strip(), 'dashboard detached workspace'
            press(b'\x1b')
            wait(lambda: popup() is None, 'Escape did not dismiss dashboard')
            # Prefix split shortcuts show a popup before creating a pane.
            press(b'\x02%')
            wait(popup, 'prefix % did not open placement dashboard')
            assert len(panes()) == 2
            extra = pick(rows[2]['id'])
            assert len(panes()) == 3 and pane_cwd(extra) == str(project)
            press(b'\x02x')
            press(b'y')
            wait(lambda: len(panes()) == 2, 'prefix x confirmation did not close session pane')
            tmux('select-pane', '-t', first)
            press(b'\x02\x1b[C')
            wait(lambda: tmux('display-message', '-p', '#{pane_id}').strip() != first, 'prefix arrow did not change focus')
            tmux('select-pane', '-t', first)
            width = int(tmux('display-message', '-p', '-t', first, '#{pane_width}'))
            press(b'\x02\x1b[1;5C')
            wait(lambda: int(tmux('display-message', '-p', '-t', first, '#{pane_width}')) > width, 'prefix Ctrl+Arrow did not resize')
            press(b'\x02[')
            wait(lambda: tmux('display-message', '-p', '-t', first, '#{pane_in_mode}').strip() == '1', 'prefix [ did not enter scrollback')
            x, y = map(int, tmux('display-message', '-p', '-t', first, '#{pane_left} #{pane_top}').split())
            press(f'\x1b[<64;{x + 3};{y + 3}M'.encode())
            assert tmux('display-message', '-p', '-t', first, '#{pane_in_mode}').strip() == '1', 'wheel disrupted tmux copy mode'
            assert '1b5b357e' not in received(first), 'copy-mode wheel leaked PageUp into Kiro'
            press(b'q')
            wait(lambda: tmux('display-message', '-p', '-t', first, '#{pane_in_mode}').strip() == '0', 'copy mode did not close')
            press(b'\x02"')
            wait(popup, 'prefix quote did not open placement dashboard')
            assert len(panes()) == 2
            extra = pick(rows[3]['id'], b'h')
            assert len(panes()) == 3 and pane_cwd(extra) == str(project)
            assert int(tmux('display-message', '-p', '-t', extra, '#{pane_top}')) > int(tmux('display-message', '-p', '-t', first, '#{pane_top}'))
            tmux('kill-pane', '-t', extra)
            tmux('select-pane', '-t', first)
            press(b'\x02T')
            wait(lambda: len(panes()) == 3, 'prefix T did not open shell')
            extra = next(pane for pane in panes() if pane not in [first, second])
            tmux('send-keys', '-t', extra, "printf 'KEY_%s_READY\\n' SHELL", 'Enter')
            wait(lambda: 'KEY_SHELL_READY' in capture(extra), 'key shell command did not execute')
            assert pane_cwd(extra) == str(project)
            tmux('kill-pane', '-t', extra)
            cli('workspace', 'split', '--vertical', '--terminal', '--dir', str(project), context=own)
            terminal = next(pane for pane in panes() if pane not in [first, second])
            tmux('send-keys', '-t', terminal, "printf 'SHELL_%s_READY\\n' PANE", 'Enter')
            wait(lambda: 'SHELL_PANE_READY' in capture(terminal), 'shell pane unavailable')
            tmux('select-pane', '-t', first)
            press(b'\x02c')
            wait(popup, 'prefix c did not open placement dashboard')
            extra = pick(rows[2]['id'], b'w')
            assert len(tmux('list-windows', '-t', 'test', '-F', '#{window_id}').splitlines()) == 2
            assert pane_cwd(extra) == str(project)
            press(b'\x02C')
            wait(lambda: len(tmux('list-windows', '-t', 'test', '-F', '#{window_id}').splitlines()) == 3, 'prefix C did not open shell window')
            shell_window = tmux('display-message', '-p', '#{pane_id}').strip()
            press(b'\x02p')
            wait(lambda: tmux('display-message', '-p', '#{pane_id}').strip() == extra, 'prefix p did not select previous window')
            press(b'\x02n')
            wait(lambda: tmux('display-message', '-p', '#{pane_id}').strip() == shell_window, 'prefix n did not select next window')
            tmux('send-keys', '-t', shell_window, "printf 'WINDOW_%s_READY\\n' SHELL", 'Enter')
            wait(lambda: 'WINDOW_SHELL_READY' in capture(shell_window), 'key shell window unavailable')
            cli('workspace', 'window', '--terminal', '--dir', str(project), context=own)
            assert len(tmux('list-windows', '-t', 'test', '-F', '#{window_id}').splitlines()) == 4
            tmux('select-window', '-t', 'test:0')
            panes()  # register every private pane before testing closure
            second_pid = int(tmux('display-message', '-p', '-t', second, '#{pane_pid}'))
            second_start = pane_processes[second_pid]
            tmux('kill-pane', '-t', second)
            wait(lambda: not frontend_alive(second_pid, second_start), 'closed pane left a live frontend')
            after = {row['id']: row for row in json.loads(cli('status', '--json'))}
            for identifier, pid in original_pids.items():
                assert after[identifier]['state'] == 'live' and after[identifier]['pid'] == pid
                os.kill(pid, 0)
            press(b'\x02d')
            wait(client_done, 'prefix d did not detach workspace')
            client = None
            assert 'attached=0' in cli('workspace', 'list')
            tmux('-f', '/dev/null', 'new-session', '-d', '-s', 'sentinel', '/bin/sh', path=foreign)
            foreign_pane = tmux('list-panes', '-t', 'sentinel', '-F', '#{pane_id}', path=foreign).strip()
            before = tmux('list-panes', '-a', '-F', '#{pane_id}', path=foreign)
            cli('workspace', 'split', '--terminal', context=context(foreign_pane, foreign), success=False)
            assert tmux('list-panes', '-a', '-F', '#{pane_id}', path=foreign) == before
            cli('workspace', 'close', 'test')
            for row in json.loads(cli('status', '--json')):
                assert row['state'] == 'live' and row['pid'] == original_pids[row['id']]
            print('PASS: isolated tmux popup, live sessions, splits/windows, shell panes, zoom/resize, close survival, foreign-server refusal and feature-off')
        finally:
            if client is not None:
                os.kill(client, signal.SIGKILL)
                os.waitpid(client, 0)
            if master is not None:
                os.close(master)
            if socket.exists():
                tmux('kill-server', check=False)
            if foreign.exists():
                tmux('kill-server', path=foreign, check=False)
            subprocess.run([str(binary), 'shutdown'], env=env, capture_output=True, timeout=10)
            deadline = time.monotonic() + 1.5
            leaked = []
            while time.monotonic() < deadline:
                leaked = [pid for pid, start in pane_processes.items() if frontend_alive(pid, start)]
                if not leaked:
                    break
                time.sleep(0.02)
            # Only a still-identical frontend launched on this private server is
            # eligible for emergency cleanup; never signal managed Kiro children.
            for pid in leaked:
                if frontend_alive(pid, pane_processes[pid]):
                    os.kill(pid, signal.SIGKILL)
            assert not leaked, f'private workspace leaked frontends (cleaned up): {leaked}'


if __name__ == '__main__':
    main()
