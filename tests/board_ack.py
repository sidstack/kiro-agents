#!/usr/bin/env python3
"""A delayed/refused attach must leave the board intact until ACK succeeds."""
import fcntl
import json
import os
import re
from pathlib import Path
import select
import signal
import socket
import struct
import sys
import tempfile
import termios
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / 'target/debug/kiro-agents'


def drain(master, seconds=0.15):
    output = b''
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.02)[0]:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            output += chunk
    return output


def expect(master, marker):
    output = b''
    deadline = time.monotonic() + 5
    while marker not in re.sub(rb' +', b' ', re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b' ', output)):
        assert time.monotonic() < deadline, (marker, output[:700], output[-300:])
        output += drain(master)
    return output


with tempfile.TemporaryDirectory(prefix='kiro-board-ack-') as tmp:
    state = Path(tmp)
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(state / 'host.sock'))
    listener.listen()
    listener.settimeout(0.1)
    waiting = threading.Event()
    release = threading.Event()
    stopped = threading.Event()
    row = dict(id='delayed-ack', cwd=tmp, agent=None, generation='v2',
               pid=None, state='live', exit_code=None, notice=None)
    errors = []

    def peer():
        while not stopped.is_set():
            try:
                connection, _ = listener.accept()
            except socket.timeout:
                continue
            with connection:
                try:
                    request = json.loads(connection.makefile('rb').readline())
                    action = request['action']
                    if action == 'Status':
                        response = {'Sessions': [row]}
                    else:
                        assert 'Attach' in action, action
                        waiting.set()
                        assert release.wait(5), 'test never released attach ACK'
                        response = {'Error': 'test attach refusal'}
                    connection.sendall(json.dumps(response).encode() + b'\n')
                except Exception as error:
                    errors.append(error)
                    return

    worker = threading.Thread(target=peer, daemon=True)
    pid, master = os.forkpty()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', 25, 110, 0, 0))
        env = dict(os.environ, KIRO_AGENTS_HOME=tmp, TERM='xterm-256color')
        os.execve(str(BINARY), [str(BINARY), 'board'], env)
    worker.start()
    try:
        expect(master, b'Kiro sessions')
        drain(master)
        os.write(master, b'\r')
        assert waiting.wait(3), 'board never sent attach request'
        pending = drain(master, 0.35)
        assert b'\x1b[?1049l' not in pending, 'board exposed shell while ACK pending'
        assert b'\x1b[2J' not in pending, 'board cleared before attach was accepted'
        assert not termios.tcgetattr(master)[3] & termios.ICANON
        release.set()
        refused = expect(master, b'test attach refusal')
        assert b'\x1b[?1049l' not in refused, 'refused attach must retain board screen'
        assert not termios.tcgetattr(master)[3] & termios.ICANON
        os.write(master, b'q')
        deadline = time.monotonic() + 5
        while True:
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                pid = None
                assert os.waitstatus_to_exitcode(status) == 0, status
                break
            assert time.monotonic() < deadline, 'board failed to quit after refusal'
            drain(master)
        assert termios.tcgetattr(master)[3] & termios.ICANON
        assert not errors, errors
    finally:
        release.set()
        stopped.set()
        if pid is not None:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        os.close(master)
        worker.join(timeout=2)
        listener.close()
print('PASS: delayed/refused attach retains board display and raw mode; quit restores terminal')
