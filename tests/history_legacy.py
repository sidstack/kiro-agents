#!/usr/bin/env python3
"""History resume against synthetic hosts that close the reply channel.

Two distinct failures must produce two distinct messages:

1. A pre-resume host cannot even parse the `Resume` request and closes the
   connection without replying. A capability probe (an unnameable action the
   newer host answers with `Reply::Error`) also closes, so the board can say
   the running host predates the feature and name the one user action that
   helps (`kiro-agents shutdown`).
2. A current host answers the probe but still closes the resume request
   before replying (crash, kill — possibly after starting the child). The
   board must state the outcome is unknown and must not push the user into
   stopping live sessions.

In neither case does the board restart or replace the running host itself.

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


class SyntheticHost:
    """Fake IPC peer; subclass decides how each request is treated."""

    def __init__(self, state):
        state.mkdir(parents=True)
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.bind(str(state / "host.sock"))
        self.socket.listen()
        self.socket.settimeout(0.1)
        self.resume_seen = False
        self.probe_seen = False
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
                    # Both hosts drop the resume connection without a Reply.
                    self.resume_seen = True
                    continue
                if isinstance(request, dict) and "HistorySupportProbe" in request:
                    self.handle_probe(connection)
                    continue
                if request == "Status":
                    connection.sendall(json.dumps({"Sessions": []}).encode() + b"\n")

    def handle_probe(self, connection):
        # A pre-resume host fails deserialization on any unknown action.
        pass

    def close(self):
        self.stopped.set()
        self.socket.close()
        self.thread.join(timeout=2)


class CurrentHost(SyntheticHost):
    """Speaks the current protocol: answers unnameable actions, but the
    resume connection closes before any reply is sent."""

    def handle_probe(self, connection):
        self.probe_seen = True
        connection.sendall(json.dumps({
            "Error": "Action HistorySupportProbe is not supported by the running host"
        }).encode() + b"\n")


def run_scenario(scratch, host, expected_substrings, forbidden_substrings):
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

    probe = None
    try:
        probe = Probe(env)
        probe.key(b"\x0f")  # Ctrl+O opens the read-only quick-open
        probe.wait_for("read-only quick-open")
        probe.type("fuzzy picker")
        probe.wait_for("Enter resumes in")
        probe.key(b"\r")
        screen = probe.wait_for(expected_substrings[0])
        for needle in expected_substrings:
            assert needle in screen, f"{needle!r} missing from: {screen}"
        for needle in forbidden_substrings:
            assert needle not in screen, f"{needle!r} present in: {screen}"
        assert host.resume_seen, "the board must still attempt the resume"
        # The board never restarts or replaces the running host itself.
        listed = json.loads(subprocess.check_output(
            [str(BINARY), "status", "--json"], env=env, timeout=10))
        assert listed == [], listed
    finally:
        if probe is not None:
            probe.close()
        host.close()
        assert native  # the conversation stays untouched in the store
        assert not (sessions / f"{native}.lock").exists()


def main():
    with tempfile.TemporaryDirectory(prefix="kiro-history-legacy-") as tmp:
        scratch = Path(tmp) / "old"
        scratch.mkdir()
        host = SyntheticHost(scratch / "state")
        run_scenario(
            scratch, host,
            ["predates history resume", "kiro-agents shutdown"],
            ["IPC connection closed"],
        )
        host.close()
        print("PASS: older host resume failure is explained, not replayed, "
              "and never restarts the host")

        scratch = Path(tmp) / "current"
        scratch.mkdir()
        host = CurrentHost(scratch / "state")
        run_scenario(
            scratch, host,
            ["outcome unknown", "not have started"],
            ["predates history resume", "kiro-agents shutdown",
             "IPC connection closed"],
        )
        host.close()
        assert host.probe_seen, "the board must probe host capability"
        print("PASS: a current host closing mid-resume leaves the outcome "
              "unknown without blaming the host version")


if __name__ == "__main__":
    main()
