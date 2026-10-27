#!/usr/bin/env python3
"""Fuzzy type-to-select in the real picker PTYs.

The board redraws single cells, so assertions run against a small screen grid
fed by every frame: the query line, the candidate rows and the notices.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
from board_smoke import BINARY, ROOT

from board_probe import Probe

CAFE = "caf\u00e9"
CAFE_DIR = CAFE + "-api"

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
    deep = scratch / "elsewhere/team/services/needle-project"
    deep.mkdir(parents=True)
    for index in range(240):
        (scratch / f"wide/project-{index:03}").mkdir(parents=True)
    (scratch / "node_modules/excluded-needle").mkdir(parents=True)
    fixture = scratch / "agents.json"
    # Deliberately not in best-match order: the ranking has to move it up.
    fixture.write_text(json.dumps(
        {"agents": [{"name": name} for name in
                    ["planner", "code-reviewer", "review-bot"]]}))
    env = dict(
        os.environ,
        HOME=str(scratch),
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

        # Discovery traverses HOME, not just cwd's first-level listing.
        board.type("needleproj")
        board.wait_for(str(deep))
        expect(board.key(b"\r", 0.4), "New session: agent", str(deep))
        board.key(b"\x1b", 0.3)
        board.key(b"\x15", 0.3)
        # Every cached row remains navigable; no top-40 truncation.
        board.type("project-")
        board.wait_for("project-000")
        board.key(b"\x1b[B" * 245, 0.4)
        expect(board.screen.text(), "project-239")
        board.key(b"\x15", 0.3)
        # Warm cache is reused; explicit refresh reveals newly created dirs.
        cache_files = list((scratch / "state/directory-cache").glob("*.json"))
        assert len(cache_files) == 1
        cached_before = cache_files[0].read_bytes()
        new = scratch / "after-cache/newly-created-project"
        new.mkdir(parents=True)
        board.key(b"\x1b", 0.2)
        board.key(b"\x0e", 0.3)
        board.type("newlycreated")
        assert str(new) not in board.screen.text()
        assert cache_files[0].read_bytes() == cached_before
        board.key(b"\x12", 0.3)
        board.wait_for(str(new))
        board.key(b"\x15", 0.3)

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
        board.raw(b"\x11", 0.5)
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
