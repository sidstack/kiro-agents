#!/usr/bin/env python3
"""History quick-open through a real board PTY.

Fake Kiro plus synthetic native stores only: no real Kiro state, no auth, no
model calls. The test asserts what the operator sees, that a resume hands the
exact native id to Kiro, and that the wrapper never writes into a native store.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from board_probe import Probe
from board_smoke import BINARY, ROOT

PICKER = "read-only quick-open"

NAMES = {
    # Resumable: no lock, existing project directory.
    "pickr": ("11111111-1111-4111-8111-111111111111", "Repair picker ranking"),
    # Held by a live foreign Kiro process.
    "held": ("22222222-2222-4222-8222-222222222222", "Locked work café"),
    # The recorded project directory is gone.
    "gone": ("33333333-3333-4333-8333-333333333333", "Deleted project work"),
}


def make_conversation(sessions, key, cwd, prompt):
    native_id, title = NAMES[key]
    (sessions / f"{native_id}.json").write_text(json.dumps({
        "session_id": native_id,
        "cwd": cwd,
        "created_at": "2026-10-16T23:07:53Z",
        "updated_at": "2026-10-27T10:59:37Z",
        "title": title,
        "session_created_reason": "subagent",
        "session_state": {"version": "v1", "agent_name": "kiro_default"},
    }))
    (sessions / f"{native_id}.jsonl").write_text(json.dumps({
        "version": "v1",
        "kind": "Prompt",
        "data": {"content": [{"kind": "text", "data": prompt}]},
    }) + "\n")
    return native_id


def tree_digest(root):
    return {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(Path(root).rglob("*"))
        if path.is_file()
    }


def main():
    with tempfile.TemporaryDirectory(prefix="kiro-history-test-") as tmp:
        scratch = Path(tmp)
        work = scratch / "work"
        work.mkdir()
        gone = scratch / "gone-project"
        gone.mkdir()
        sessions = scratch / "kiro/sessions/cli"
        sessions.mkdir(parents=True)
        native = make_conversation(sessions, "pickr", str(work.resolve()),
                                   "fix the fuzzy picker ranking order")
        held = make_conversation(sessions, "held", str(work.resolve()),
                                 "hold this conversation open")
        make_conversation(sessions, "gone", str(gone.resolve()), "work in a deleted project")
        # A newer per-workspace store row: discoverable, never resumed here.
        session_dir = scratch / "kiro/sessions/874b7102139062fc/sess_44444444-4444-4444-8444-444444444444"
        session_dir.mkdir(parents=True)
        (session_dir / "session.json").write_text(json.dumps({
            "schemaVersion": "1.0.0",
            "dataModelVersion": 1,
            "id": "sess_44444444-4444-4444-8444-444444444444",
            "title": "Newer store session",
            "agentMode": "vibe",
            "rootPaths": [str(work.resolve())],
            "createdAt": "2026-10-27T10:59:23Z",
            "lastModifiedAt": "2026-10-27T10:59:37Z",
        }))
        # A live foreign Kiro holds one conversation.
        foreign = subprocess.Popen(["sleep", "300"])
        (sessions / f"{held}.lock").write_text(json.dumps({"pid": foreign.pid, "started_at": "fixture"}))
        gone.rmdir()  # the store still records it; the directory is gone

        env = dict(
            os.environ,
            HOME=str(scratch),
            KIRO_AGENTS_HOME=str(scratch / "state"),
            KIRO_HOME=str(scratch / "kiro"),
            KIRO_SESSIONS_DIR=str(sessions),
            KIRO_AGENT_CONFIG_DIR=str(scratch / "kiro/agents"),
            KIRO_AGENTS_KIRO_BIN=str(ROOT / "tests/fake-kiro/kiro-cli"),
            TERM="xterm-256color",
        )

        def call(*args):
            return subprocess.check_output([str(BINARY), *args], env=env, timeout=10)

        def rows():
            return json.loads(call("status", "--json"))

        before = tree_digest(scratch / "kiro")
        probe = None
        try:
            probe = Probe(env)
            probe.key(b"\x0f")  # Ctrl+O opens the read-only quick-open
            probe.wait_for(PICKER)
            screen = probe.wait_for("Searched v1+v2 store(s)")
            assert "nothing is written to Kiro" in screen, screen

            # Full-text search finds a conversation by its prompt words.
            probe.type("fuzzy picker")
            probe.wait_for("Repair picker ranking")
            # Unicode titles and prompts are searchable as typed.
            probe.key(b"\x15")  # Ctrl+U clears the query
            probe.type("café")
            probe.wait_for("Locked work café")

            # A conversation a live foreign Kiro already holds is never resumed.
            stable = rows()
            probe.key(b"\r")
            screen = probe.wait_for("Kiro already has this")
            assert "never taken over" in screen, screen
            assert rows() == stable, "a blocked resume must not spawn anything"

            # A recorded project directory that no longer exists is refused.
            probe.key(b"\x15")
            probe.type("deleted project")
            probe.key(b"\r")
            probe.wait_for("no longer available")

            # The newer-generation store is listed but never resumed.
            probe.key(b"\x15")
            probe.type("newer store")
            probe.key(b"\r")
            probe.wait_for("Stored only in Kiro")
            assert rows() == stable, "an unsupported row must not spawn anything"

            # The verified path: Enter resumes through native Kiro.
            probe.key(b"\x15")
            probe.type("fuzzy picker")
            probe.wait_for("Enter resumes in")
            # Hand-off to native Kiro: the PTY bytes belong to Kiro now, so read
            # the raw stream rather than the board's screen model.
            probe.absorb()
            probe.press(b"\r")
            startup = probe.wait(b"READY:")
            assert f"resume:{native}".encode() in startup, startup[-800:]
            assert b"work" in startup, startup[-800:]

            resumed = [row for row in rows() if row.get("native_id") == native]
            assert len(resumed) == 1, resumed
            assert resumed[0]["generation"] == "v2" and resumed[0]["state"] == "live", resumed

            # Leaving the native session returns to the board; the wrapper never
            # wrote anything into Kiro's store.
            probe.raw(b"exit", settle=0.8)
            assert tree_digest(scratch / "kiro") == before, "native store changed"
            assert not (sessions / f"{native}.lock").exists(), "resume must not fake a lock"
            probe.wait_for("Kiro sessions", limit=10)

            # Quitting the frontend leaves the resumed session supervised.
            probe.finish()
            probe = None
            listed = rows()
            assert len([row for row in listed if row.get("native_id") == native]) == 1, listed
            print("PASS: read-only V1+V2 discovery, fuzzy/full-text/Unicode search, "
                  "live-lock and generation blocks, exact-id native resume, untouched stores")
        finally:
            if probe is not None:
                probe.close()
            subprocess.run([str(BINARY), "shutdown"], env=env, timeout=10, check=False)
            foreign.kill()
            foreign.wait()


if __name__ == "__main__":
    main()
