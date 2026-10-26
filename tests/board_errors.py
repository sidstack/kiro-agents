#!/usr/bin/env python3
"""Check recoverable board errors in real PTYs; no real Kiro or model calls."""
import fcntl
import json
import os
import re
from pathlib import Path
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/kiro-agents"


def expect(master, marker):
    output = b""
    deadline = time.monotonic() + 8
    while marker not in re.sub(rb" +", b" ", re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b" ", output)):
        assert time.monotonic() < deadline, f"Missing {marker!r}: {output[-500:]!r}"
        if select.select([master], [], [], 0.1)[0]:
            output += os.read(master, 65536)
    return output


def reap(pid):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            assert os.waitstatus_to_exitcode(status) == 0, status
            return
        time.sleep(0.02)
    raise AssertionError("Board failed to exit")


def scenario(oversized):
    with tempfile.TemporaryDirectory(prefix="kiro-board-errors-") as tmp:
        state = Path(tmp) / "state"
        state.mkdir()
        if oversized:
            rows = [dict(id=f"{n:012d}", cwd="/tmp/status-limit-project", agent=None,
                         generation="v2", pid=None, state="exited", exit_code=0,
                         notice=None) for n in range(1200)]
            (state / "registry.json").write_text(json.dumps(rows))
        env = dict(os.environ, KIRO_AGENTS_HOME=str(state), TERM="xterm-256color",
                   KIRO_AGENTS_KIRO_BIN=str(ROOT / "tests/fake-kiro/kiro-cli"))
        host = subprocess.Popen([str(BINARY), "__host"], env=env,
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        pid = None
        master = None
        try:
            deadline = time.monotonic() + 5
            while not (state / "host.sock").exists():
                assert time.monotonic() < deadline
                time.sleep(0.02)
            pid, master = os.forkpty()
            if pid == 0:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
                os.execve(str(BINARY), [str(BINARY), "board"], env)
            if oversized:
                expect(master, b"Refresh failed:")
                os.write(master, b"r")
                expect(master, b"Refresh failed:")
                os.write(master, b"q")
            else:
                expect(master, b"Kiro sessions")
                os.write(master, b"\x0e")
                os.write(master, b"/nonexistent/kiro-board-test\r")
                # Let the invalid-entry update settle, then resize to request a
                # complete board frame. Incremental diffs can reuse letters from
                # the prior notice and cannot be treated as contiguous text.
                time.sleep(0.3)
                fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 101, 0, 0))
                os.kill(pid, signal.SIGWINCH)
                expect(master, b"Cannot create:")
                assert json.loads((state / "registry.json").read_text()) == []
                os.kill(pid, signal.SIGTERM)
            reap(pid)
            pid = None
            assert termios.tcgetattr(master)[3] & termios.ICANON
        finally:
            if pid is not None:
                os.kill(pid, signal.SIGKILL)
                os.waitpid(pid, 0)
            if master is not None:
                os.close(master)
            host.terminate()
            host.wait(timeout=5)


scenario(False)
scenario(True)
print("PASS: invalid directory, status overflow retry/quit, signal and error terminal restoration")
