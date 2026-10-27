#!/usr/bin/env python3
"""Fuzzy type-to-select in the real picker PTYs.

The board redraws single cells, so assertions run against a small screen grid
fed by every frame: the query line, the candidate rows and the notices.
"""
import codecs
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile
from board_smoke import Board, BINARY, ROOT

ROWS, COLS = 26, 120
TOKEN = re.compile(rb"(?:\x1b\[([0-9;?]*)([A-Za-z@]))|([^\x1b]+)")
CAFE = "caf\u00e9"
CAFE_DIR = CAFE + "-api"


class Grid:
    """Minimal cursor model: enough to read ratatui's incremental output."""

    def __init__(self, rows=ROWS, cols=COLS):
        self.rows, self.cols = rows, cols
        self.cells = [[" "] * cols for _ in range(rows)]
        self.cursor = [0, 0]
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def feed(self, data):
        for match in TOKEN.finditer(data):
            params, code, text = match.group(1), match.group(2), match.group(3)
            if text is not None:
                for ch in self.decoder.decode(text):
                    self.put(ch)
            elif code is not None:
                self.command(params.decode(), code.decode())

    def command(self, params, code):
        def arg(index, default=0):
            parts = params.split(";")
            if index < len(parts) and parts[index].isdigit():
                return int(parts[index])
            return default

        row, col = self.cursor
        if code in ("H", "f"):
            self.cursor = [max(arg(0, 1) - 1, 0), max(arg(1, 1) - 1, 0)]
        elif code == "A":
            self.cursor = [max(row - max(arg(0, 1), 1), 0), col]
        elif code == "B":
            self.cursor = [min(row + max(arg(0, 1), 1), self.rows - 1), col]
        elif code == "C":
            self.cursor = [row, min(col + max(arg(0, 1), 1), self.cols - 1)]
        elif code == "D":
            self.cursor = [row, max(col - max(arg(0, 1), 1), 0)]
        elif code == "J" and arg(0, 0) in (0, 1, 2, 3):
            self.clear()
        elif code == "K":
            mode, start, stop = arg(0, 0), 0, self.cols
            if mode == 0:
                start = col
            elif mode == 1:
                stop = col + 1
            for index in range(start, stop):
                self.cells[row][index] = " "

    def put(self, ch):
        if ch == "\n":
            self.cursor[0] = min(self.cursor[0] + 1, self.rows - 1)
            self.cursor[1] = 0
            return
        if ch in "\r\t":
            self.cursor[1] = 0 if ch == "\r" else ((self.cursor[1] // 8) + 1) * 8
            return
        row, col = self.cursor
        if 0 <= row < self.rows and 0 <= col < self.cols:
            self.cells[row][col] = ch
            self.cursor[0] = row
            self.cursor[1] = min(col + 1, self.cols - 1)

    def clear(self):
        self.cells = [[" "] * self.cols for _ in range(self.rows)]

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.cells)


class Probe(Board):
    """Board plus a live screen model."""

    def __init__(self, env):
        super().__init__(env)
        self.screen = Grid()
        self.absorb()

    def absorb(self):
        self.screen.feed(self.frames)
        self.frames = b""

    def key(self, data, settle=0.15):
        self.press(data)
        self.drain(settle)
        self.absorb()
        return self.screen.text()

    def type(self, word, settle=0.05):
        for ch in word:
            self.press(ch.encode())
            self.drain(settle)
        self.absorb()
        return self.screen.text()

    def wait_for(self, marker, limit=8.0):
        import time
        deadline = time.monotonic() + limit
        while marker not in self.screen.text():
            assert time.monotonic() < deadline, f"missing {marker!r}"
            self.drain(0.2)
            self.absorb()
        return self.screen.text()

    def raw(self, data, settle=0.2):
        self.press(data)
        return self.drain(settle)


def run_cli(env, *args):
    return subprocess.check_output([str(BINARY), *args], env=env, timeout=8)


def expect(text, *markers):
    for marker in markers:
        assert marker in text, f"missing {marker!r} in:\n{text}"
    return text


with tempfile.TemporaryDirectory(prefix="kiro-search-test-") as tmp:
    scratch = Path(tmp)
    work = scratch / "projects"
    for name in ["api-gateway", "code-reviewer", CAFE_DIR, "nested"]:
        (work / name).mkdir(parents=True)
    # A project next to the listed directory, reachable without left arrow.
    (scratch / "design-system").mkdir()
    fixture = scratch / "agents.json"
    # Deliberately not in best-match order: the ranking has to move it up.
    fixture.write_text(json.dumps(
        {"agents": [{"name": name} for name in
                    ["planner", "code-reviewer", "review-bot"]]}))
    env = dict(
        os.environ,
        KIRO_AGENTS_HOME=str(scratch / "state"),
        KIRO_HOME=str(scratch / "kiro"),
        KIRO_SESSIONS_DIR=str(scratch / "kiro/sessions/cli"),
        KIRO_AGENT_CONFIG_DIR=str(scratch / "profiles"),
        KIRO_AGENTS_KIRO_BIN=str(ROOT / "tests/fake-kiro/kiro-cli"),
        KIRO_AGENTS_FAKE_AGENT_FIXTURE=str(fixture),
        TERM="xterm-256color",
    )

    previous = os.getcwd()
    os.chdir(work)
    board = None
    try:
        board = Probe(env)
        expect(board.key(b"\x0e"), "New session: directory", "Search or path:")

        # 1. A bare fragment finds the sibling project: no left arrow, no
        # arrow keys, Enter takes the top ranked row.
        screen = board.type("design-sys")
        expect(screen, "sibling", str(scratch / "design-system"))
        screen = board.key(b"\r", 0.4)
        expect(screen, "New session: agent", str(scratch / "design-system"))

        # 2. Fuzzy typing ranks profiles, so Enter creates with the best one.
        board.wait_for("review-bot")
        board.type("rev")
        raw = board.raw(b"\r", 0.5)
        assert b"READY:" in raw, raw[-900:]
        assert b"/design-system:review-bot" in raw, raw[-900:]
        rows = json.loads(run_cli(env, "status", "--json"))
        assert len(rows) == 1 and rows[0]["agent"] == "review-bot", rows
        board.raw(b"\x07", 0.5)
        board.absorb()
        board.wait_for("Kiro sessions")

        # 3. Esc from the agent form returns with the typed query preserved.
        board.key(b"\x0e", 0.3)
        expect(board.type(CAFE), "dir", CAFE_DIR)
        board.key(b"\r", 0.4)
        board.key(b"\x1b", 0.3)
        expect(board.screen.text(), f"Search or path: {CAFE}")

        # 4. DEL (0x7f) drops one whole character, not one byte: the multi-byte
        # letter survives and the shortened query re-ranks with no arrow.
        expect(board.key(b"z"), "No directory matches")
        expect(board.key(b"\x7f", 0.3), CAFE_DIR)
        assert "No directory matches" not in board.screen.text()

        # 5. BS (0x08, the legacy Ctrl+Backspace/Ctrl+H encoding) edits alike.
        expect(board.key(b"z"), "No directory matches")
        expect(board.key(b"\x08", 0.3), CAFE_DIR)
        assert "No directory matches" not in board.screen.text()

        # 6. A mistyped tail is recoverable and Enter still needs no arrow.
        board.key(b"\x1b", 0.3)
        board.key(b"\x0e", 0.3)
        board.type("code-rev")
        board.type("zz")
        expect(board.screen.text(), "No directory matches")
        board.key(b"\x7f", 0.1)
        board.key(b"\x7f", 0.3)
        expect(board.screen.text(), "code-reviewer")
        expect(board.key(b"\r", 0.4), "New session: agent",
               str(work / "code-reviewer"))
        # Esc steps back agent -> directory -> board without losing state.
        expect(board.key(b"\x1b", 0.3), "New session: directory",
               f"Search or path: code-rev")
        board.key(b"\x1b", 0.3)
        expect(board.screen.text(), "Enter attaches")
        board.frames = b""
        board.finish()
        board = None
    finally:
        os.chdir(previous)
        if board is not None:
            board.close()
        try:
            run_cli(env, "shutdown")
        except subprocess.CalledProcessError:
            pass

print("PASS: fuzzy Enter takes the ranked top match, a sibling fragment needs "
      "no arrow, 0x7f and 0x08 both edit one character and re-rank, Esc keeps "
      "the query")
