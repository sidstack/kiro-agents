#!/usr/bin/env python3
"""Real board/frontend PTYs against fake Kiro; temporary metadata only."""
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


def readable(data):
    # A full board redraw may move across blank cells instead of writing spaces.
    return re.sub(rb" +", b" ", re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b" ", data))


class Board:
    def __init__(self, env):
        self.pid, self.master = os.forkpty()
        if self.pid == 0:
            fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 26, 120, 0, 0))
            os.execve(str(BINARY), [str(BINARY)], env)
        self.frames = b""
        self.wait(b"Kiro sessions")
        self.all = False

    def drain(self, duration=0.2):
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
        return readable(self.frames)

    def screen(self):
        # r explicitly clears/redraws the board. Assert on that complete frame,
        # rather than treating ratatui's incremental output as a screen model.
        self.frames = b""
        self.press(b"r")
        return self.wait(b"Kiro sessions")

    def filter(self, all_rows):
        if self.all != all_rows:
            self.press(b"\t")
            self.all = all_rows
        return self.screen()

    def search(self, identifier):
        self.press(b"/")
        self.press(identifier.encode())
        self.press(b"\r")
        return self.screen()

    def attach(self):
        self.frames = b""
        self.press(b"\r")
        return self.wait(b"READY:")

    def detach(self):
        self.frames = b""
        self.press(b"\x02d")
        self.wait(b"Kiro sessions")

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
                assert termios.tcgetattr(self.master)[3] & termios.ICANON
                return
            self.drain()
        raise AssertionError("Board did not quit")

    def close(self):
        if self.pid is not None:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.master)


def main():
    with tempfile.TemporaryDirectory(prefix="kiro-board-test-") as tmp:
        scratch = Path(tmp)
        projects = [scratch / name for name in ["api-gateway", "code-reviewer", "legacy-app"]]
        for project in projects:
            project.mkdir()
        env = dict(os.environ, KIRO_AGENTS_HOME=str(scratch / "state"),
                   KIRO_AGENTS_KIRO_BIN=str(ROOT / "tests/fake-kiro/kiro-cli"), TERM="xterm-256color")
        # Agent discovery fixture: the picker must offer reviewer without typing.
        agents = scratch / "agents.json"
        agents.write_text(json.dumps({"agents": [{"name": "reviewer"}, {"name": "planner"}]}))
        env["KIRO_AGENTS_FAKE_AGENT_FIXTURE"] = str(agents)

        def call(*args):
            return subprocess.check_output([str(BINARY), *args], env=env, timeout=10)

        def rows():
            return {row["id"]: row for row in json.loads(call("status", "--json"))}

        def create(project):
            before = rows()
            call("new", str(project), "--no-attach")
            after = rows()
            return next(row for identifier, row in after.items() if identifier not in before)

        boards = []
        try:
            plain = call("board")
            assert b"sessions" in plain and b"Session ID to attach" not in plain
            # Interactive stdin with piped stdout must create without trying
            # to enter native attach, then print the reconnection hint.
            master, slave = os.openpty()
            before_pipe = rows()
            try:
                piped = subprocess.run([str(BINARY), "new", str(projects[0])],
                                       env=env, stdin=slave, capture_output=True, timeout=10)
                assert piped.returncode == 0, piped.stderr
                assert b"Attach: kiro-agents attach" in piped.stdout
            finally:
                os.close(master)
                os.close(slave)
            piped_id = next(identifier for identifier in rows() if identifier not in before_pipe)
            call("stop", piped_id)
            first, second, history = [create(project) for project in projects]
            call("stop", history["id"])
            board = Board(env)
            boards.append(board)
            screen = board.screen()
            assert first["id"].encode() in screen and second["id"].encode() in screen
            assert history["id"].encode() not in screen
            assert history["id"].encode() in board.filter(True)
            board.press(b"j")
            board.press(b"k")
            screen = board.search(first["id"])
            assert first["id"].encode() in screen and second["id"].encode() not in screen
            board.press(b"\x1b")
            assert second["id"].encode() in board.screen()
            board.search(history["id"])
            before = rows()
            board.press(b"\r")
            assert b"Cannot attach" in board.screen()
            assert rows() == before, "History selection must not spawn or attach"
            board.press(b"\x1b")
            board.filter(False)
            for row, text in [(first, b"hi"), (second, b"yo")]:
                board.search(row["id"])
                startup = board.attach()
                assert str(projects[0 if row == first else 1]).encode() in startup
                board.frames = b""
                board.press(text)
                board.wait(b"BYTES:" + text.hex().encode())
                board.detach()
                assert rows()[row["id"]]["pid"] == row["pid"]
            board.search(second["id"])
            for cancel in [b"n", b"\x1b"]:
                board.press(b"\x0b")
                board.press(cancel)
                assert rows()[second["id"]]["state"] == "live"
            board.press(b"\x0b")
            board.press(b"y")
            assert rows()[second["id"]]["state"] == "stopped"
            assert rows()[first["id"]]["state"] == "live"
            board.press(b"\x1b")
            board.frames = b""
            board.press(b"\x0e")
            board.wait(b"New session: directory")
            # Selection-first: recent directories are already highlighted;
            # arrows and Enter only, no typing.
            board.press(b"\x1b[B")
            board.frames = b""
            board.press(b"\r")
            board.wait(b"agent (optional)")
            board.wait(b"reviewer")
            # Row 0 is the Kiro default (no --agent); arrows to reviewer.
            board.press(b"\x1b[B")
            board.frames = b""
            board.press(b"\r")
            board.wait(b"READY:")
            board.wait(b":reviewer")
            fresh = [row for identifier, row in rows().items() if identifier not in before]
            assert len(fresh) == 1 and fresh[0]["agent"] == "reviewer"
            assert fresh[0]["cwd"] in [str(project) for project in projects], \
                "Arrow selection must create in a recent project directory"
            board.frames = b""
            board.press(b"exit")
            board.wait(b"Kiro sessions")
            assert rows()[fresh[0]["id"]]["state"] == "exited"
            survivors = {identifier: row["pid"] for identifier, row in rows().items() if row["state"] == "live"}
            board.finish()
            for identifier, pid in survivors.items():
                assert rows()[identifier]["pid"] == pid and rows()[identifier]["state"] == "live"
                os.kill(pid, 0)
            board = Board(env)
            boards.append(board)
            board.search(first["id"])
            board.attach()
            board.finish(signal.SIGTERM)
            assert rows()[first["id"]]["state"] == "live"
        finally:
            for board in boards:
                board.close()
            call("shutdown")
    print("PASS: board filter/search/navigation, historical refusal, switch with stable PIDs, stop confirmation, selection-first create (arrow directory + arrow agent), native exit, quit/signal restoration, non-TTY output")


if __name__ == "__main__":
    main()
