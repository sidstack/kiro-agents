#!/usr/bin/env python3
"""History resume against a synthetic older host.

The host predates the `Resume` action, so it cannot even parse the request and
closes the connection without replying — the exact failure an installed host
produces when a newer frontend is dropped in without restarting it. The board
must explain the situation and name the one user action that helps, instead of
showing a dead-end transport error or restarting anything itself.

Fake Kiro and synthetic stores only: no real Kiro state, no auth, no network.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading

from board_history import make_conversation
from board_probe import Probe
from board_smoke import BINARY, ROOT


class LegacyHost:
    """Speaks the pre-resume protocol: unknown actions kill the connection."""

    def __init__(self, state):
        state.mkdir(parents=True)
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.bind(str(state / "host.sock"))
        self.socket.listen()
        self.socket.settimeout(0.1)
        self.resume_seen = False
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        while not self.stopped.is_set():
            try:
                connection, _ = self.socket.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with connection:
                connection.settimeout(2)
                try:
                    request = json.loads(connection.makefile("rb").readline())["action"]
                except (ValueError, OSError):
                    continue
                if isinstance(request, dict) and "Resume" in request:
                    # A real pre-resume host fails at deserialization and drops
                    # the connection: no Reply at all.
                    self.resume_seen = True
                    continue
                if request == "Status":
                    connection.sendall(json.dumps({"Sessions": []}).encode() + b"\n")

    def close(self):
        self.stopped.set()
        self.socket.close()
        self.thread.join(timeout=2)


def main():
    with tempfile.TemporaryDirectory(prefix="kiro-history-legacy-") as tmp:
        scratch = Path(tmp)
        work = scratch / "work"
        work.mkdir()
        sessions = scratch / "kiro/sessions/cli"
        sessions.mkdir(parents=True)
        native = make_conversation(sessions, "pickr", str(work.resolve()),
                                   "fix the fuzzy picker ranking order")

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

        host = LegacyHost(scratch / "state")
        probe = None
        try:
            probe = Probe(env)
            probe.key(b"\x0f")  # Ctrl+O opens the read-only quick-open
            probe.wait_for("read-only quick-open")
            probe.type("fuzzy picker")
            probe.wait_for("Enter resumes in")
            probe.key(b"\r")
            screen = probe.wait_for("predates history resume")
            assert "kiro-agents shutdown" in screen, screen
            assert "IPC connection closed" not in screen, screen
            assert host.resume_seen, "the board must still attempt the resume"
            # The board never restarts or replaces the running host itself.
            listed = json.loads(subprocess.check_output(
                [str(BINARY), "status", "--json"], env=env, timeout=10))
            assert listed == [], listed
            print("PASS: older host resume failure is explained, not replayed, "
                  "and never restarts the host")
        finally:
            if probe is not None:
                probe.close()
            host.close()
            assert native  # the conversation stays untouched in the store
            assert not (sessions / f"{native}.lock").exists()


if __name__ == "__main__":
    main()
