#!/usr/bin/env python3
"""Exercise the actual frontend in a controlling terminal, using only fake Kiro."""
import fcntl
import json
import os
from pathlib import Path
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "target/debug/kiro-agents"


def expect(master, marker):
    output = b""
    deadline = time.monotonic() + 5
    while marker not in output:
        assert time.monotonic() < deadline, f"Timed out waiting for {marker!r}"
        if select.select([master], [], [], 0.1)[0]:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                raise AssertionError(f"Terminal closed waiting for {marker!r}") from error
            assert data, "Unexpected terminal EOF"
            output += data
    return output


def reap(pid):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        result, status = os.waitpid(pid, os.WNOHANG)
        if result:
            return status
        time.sleep(0.02)
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    raise AssertionError("Frontend did not exit")


def remaining_output(master):
    output = b""
    while select.select([master], [], [], 0.1)[0]:
        try:
            chunk = os.read(master, 65536)
        except OSError:
            break
        if not chunk:
            break
        output += chunk
    return output


with tempfile.TemporaryDirectory(prefix="kiro-terminal-test-") as scratch:
    env = dict(os.environ, KIRO_AGENTS_HOME=scratch + "/state",
               KIRO_AGENTS_KIRO_BIN=str(ROOT / "tests/fake-kiro/kiro-cli"), TERM="xterm-256color")
    env["KIRO_HOME"] = env["KIRO_AGENTS_HOME"] + "/kiro"
    env["KIRO_SESSIONS_DIR"] = env["KIRO_HOME"] + "/sessions/cli"
    env["KIRO_AGENT_CONFIG_DIR"] = env["KIRO_HOME"] + "/agents"

    def call(*args):
        return subprocess.check_output([str(BINARY), *args], env=env, timeout=8)

    def attach(identifier):
        pid, master = os.forkpty()
        if pid == 0:
            os.execve(str(BINARY), [str(BINARY), "attach", identifier], env)
        expect(master, b"READY:")
        return pid, master

    frontends = set()
    try:
        call("new", scratch, "--v2", "--no-attach")
        rows = json.loads(call("status", "--json"))
        identifier, child_pid = rows[0]["id"], rows[0]["pid"]
        pid, master = attach(identifier)
        frontends.add(pid)
        assert not termios.tcgetattr(master)[3] & termios.ICANON
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 39, 117, 0, 0))
        expect(master, b"SIZE:117x39")
        os.write(master, b"hi")
        expect(master, b"BYTES:6869")
        os.write(master, b"\x02d")
        assert os.waitstatus_to_exitcode(reap(pid)) == 0
        frontends.remove(pid)
        assert termios.tcgetattr(master)[3] & termios.ICANON
        cleanup = remaining_output(master)
        assert b"\x1b[?1049l" in cleanup
        assert b"\x1b[39;1H" in cleanup, "Detach must place the shell below retained content"
        os.close(master)
        pid, master = attach(identifier)
        frontends.add(pid)
        os.kill(pid, signal.SIGKILL)
        reap(pid)
        frontends.remove(pid)
        os.close(master)
        time.sleep(0.1)
        assert json.loads(call("status", "--json"))[0]["pid"] == child_pid
        os.kill(child_pid, 0)
        pid, master = attach(identifier)
        frontends.add(pid)
        os.kill(pid, signal.SIGTERM)
        assert os.waitstatus_to_exitcode(reap(pid)) == 0
        frontends.remove(pid)
        assert termios.tcgetattr(master)[3] & termios.ICANON
        os.close(master)
        assert json.loads(call("status", "--json"))[0]["pid"] == child_pid
        pid, master = attach(identifier)
        frontends.add(pid)
        # Inline output has no alternate-screen transition. Native exit must
        # retain its cursor instead of restoring a saved position near the top.
        os.write(master, b"exit")
        assert os.waitstatus_to_exitcode(reap(pid)) == 0
        frontends.remove(pid)
        tail = remaining_output(master)
        assert b"GOODBYE" in tail, "Child's final output must reach the terminal"
        assert b"\x1b[?1049l" not in tail, "Native exit must not restore an old cursor"
        assert b"\x1b[2J" not in tail and b"\x1b[3J" not in tail, "Preserve the transcript"
        assert termios.tcgetattr(master)[3] & termios.ICANON
        os.close(master)
        print("PASS: detach, resize, raw input, SIGKILL survival, reconnect, signal cleanup, native-exit cursor preservation")
    finally:
        for pid in frontends:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        call("shutdown")
