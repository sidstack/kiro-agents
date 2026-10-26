#!/usr/bin/env python3
"""Board <-> native handoff transitions in one live PTY.

Regression for the old cooked/primary round trip: after the attach ACK the
terminal stays raw and on the alternate screen the whole time. Attach-return
cleanup may reset child private modes (the board may reset scroll margins)
but must never emit DECRST 1049, a bottom-row shell cursor sequence or the
standalone trailing newline. A native exit may leave the alternate screen;
the board return must re-enter it (ENTER 1049h) without touching the shell.
"""
import fcntl
import json
import os
import re
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

LEAVE_ALT = b"\x1b[?1049l"
ENTER_ALT = b"\x1b[?1049h"
# The standalone cleanup emits this exact composite (reset margins, reset
# origin mode, park on the bottom row of this 30-row PTY). A board resume
# may legitimately reset margins and home the cursor, but never in this
# consecutive form, and plain CUP sequences are normal ratatui rendering.
SHELL_CURSOR = b"\x1b[r\x1b[?6l\x1b[30;1H"
STANDALONE_TAIL = b"\x1b[?25h\r\n"


def readable(data):
    return re.sub(rb" +", b" ", re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b" ", data))


def forbid(frames, phase):
    assert LEAVE_ALT not in frames, f"{phase}: handoff must never leave the alternate screen"
    assert SHELL_CURSOR not in frames, f"{phase}: no bottom-row shell cursor reposition"
    assert STANDALONE_TAIL not in frames, f"{phase}: no standalone shell newline"


def cooked(master):
    return bool(termios.tcgetattr(master)[3] & termios.ICANON)


class Board:
    def __init__(self, env):
        self.pid, self.master = os.forkpty()
        if self.pid == 0:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
            os.execve(str(BINARY), [str(BINARY)], env)
        self.frames = b""
        self.wait(b"Kiro sessions")

    def drain(self, duration=0.3):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            if select.select([self.master], [], [], 0.03)[0]:
                try:
                    data = os.read(self.master, 65536)
                except OSError:
                    break
                if not data:
                    break
                self.frames += data
        return self.frames

    def press(self, data):
        os.write(self.master, data)
        return self.drain()

    def wait(self, marker):
        deadline = time.monotonic() + 8
        while marker not in readable(self.frames):
            assert time.monotonic() < deadline, f"Missing {marker!r}: {self.frames[-700:]!r}"
            self.drain()
        return self.frames

    def search(self, identifier):
        self.press(b"/")
        self.press(identifier.encode())
        self.press(b"\r")
        return self.press(b"r")

    def finish(self, sig=None):
        if sig:
            os.kill(self.pid, sig)
        else:
            self.press(b"q")
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            done, status = os.waitpid(self.pid, os.WNOHANG)
            if done:
                self.pid = None
                assert os.waitstatus_to_exitcode(status) == 0, status
                return
            self.drain()
        raise AssertionError("Board did not quit")

    def close(self):
        if getattr(self, "pid", None) is not None:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.master)


with tempfile.TemporaryDirectory(prefix="kiro-handoff-test-") as scratch:
    env = dict(os.environ, KIRO_AGENTS_HOME=scratch + "/state",
               KIRO_AGENTS_KIRO_BIN=str(ROOT / "tests/fake-kiro/kiro-cli"),
               TERM="xterm-256color")

    def call(*args):
        return subprocess.check_output([str(BINARY), *args], env=env, timeout=8)

    def create():
        before = {row["id"] for row in json.loads(call("status", "--json"))}
        call("new", scratch, "--v2", "--no-attach")
        after = json.loads(call("status", "--json"))
        return next(row for row in after if row["id"] not in before)

    first, second = create(), create()
    board = Board(env)
    try:
        # 1. Attach transition: board -> host ACK -> native, no shell between.
        board.search(first["id"])
        board.frames = b""
        board.press(b"\r")
        board.wait(b"READY:")
        forbid(board.frames, "attach")
        assert not cooked(board.master), "Raw mode must stay enabled across the handoff"

        # 2. Raw byte forwarding during the handoff.
        board.frames = b""
        board.press(b"hi")
        board.wait(b"BYTES:6869")

        # 3. The child takes private modes, then Ctrl+B d returns to the
        #    board on the same screen: only child-mode resets are allowed.
        board.frames = b""
        board.press(b"screen")
        board.wait(b"NATIVE")
        board.frames = b""
        board.press(b"\x02d")
        board.wait(b"Kiro sessions")
        forbid(board.frames, "detach")
        assert b"\x1b[?2004l" in board.frames and b"\x1b[?1006l" in board.frames, \
            "Child bracketed-paste and mouse modes must be reset on board return"
        assert not cooked(board.master), "Board must resume in raw mode without re-enabling"

        # 4. Native exit during board attach: the child first leaves the
        #    alternate screen like a real TUI does on exit, its final output
        #    reaches the terminal, and the board return re-enters alt.
        board.search(second["id"])
        board.frames = b""
        board.press(b"\r")
        board.wait(b"READY:")
        board.press(b"leave")
        board.wait(b"LEFT")
        # Native bytes deliberately leave alt; inspect only wrapper cleanup
        # after that native action, not the child's own earlier sequence.
        board.frames = b""
        board.press(b"exit")
        board.wait(b"Kiro sessions")
        assert b"GOODBYE" in board.frames, "Child's final output must reach the terminal"
        assert ENTER_ALT in board.frames, "Left alt screen must be restored on board return"
        forbid(board.frames[:board.frames.index(ENTER_ALT)], "native exit")

        # 5. Quitting the board restores the shell exactly once.
        board.frames = b""
        board.finish()
        assert LEAVE_ALT in board.frames, "Board quit is the only alt-screen leave"
        assert cooked(board.master), "Board quit must restore cooked mode"

        rows = {row["id"]: row for row in json.loads(call("status", "--json"))}
        assert rows[first["id"]]["state"] == "live" and rows[second["id"]]["state"] == "exited", rows
        assert rows[first["id"]]["pid"] == first["pid"], "Handoffs must not restart the child"
        os.kill(first["pid"], 0)
        print("PASS: attach/detach stay on one alternate screen in raw mode, "
              "child modes reset, native exit restores alt, board quit restores shell")
    finally:
        board.close()
        call("shutdown")
