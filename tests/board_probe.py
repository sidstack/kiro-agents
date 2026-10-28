"""Screen-aware fake-Kiro board PTY helpers (no tests on import)."""
import codecs
import re
from board_smoke import Board

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

