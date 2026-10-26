#!/usr/bin/env python3
"""No prompts, model calls, credentials, or persisted terminal output."""
import json
import fcntl
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import subprocess
import struct
import tempfile
import time
import termios


def run():
    binary = shutil.which("kiro-cli") or str(Path.home() / ".local/bin/kiro-cli")
    summary = {}
    with tempfile.TemporaryDirectory(prefix="kiro-v2-probe-") as scratch:
        root = Path(scratch)
        env = {"PATH": os.environ["PATH"], "LANG": "C.UTF-8", "TERM": "xterm-256color"}
        for key, suffix in {
            "HOME": "home", "KIRO_HOME": "kiro", "KIRO_SESSIONS_DIR": "sessions",
            "KIRO_DATA_DIR": "data", "XDG_CONFIG_HOME": "config", "XDG_DATA_HOME": "share",
            "XDG_STATE_HOME": "state", "XDG_CACHE_HOME": "cache", "XDG_RUNTIME_DIR": "runtime",
        }.items():
            path = root / suffix
            path.mkdir(mode=0o700)
            env[key] = str(path)
        for args in [
            ["--version"], ["chat", "--help"], ["chat", "--help-all"],
            ["agent", "list", "--format", "json-pretty"],
            ["chat", "--v2", "--list-sessions", "--format", "json"],
        ]:
            child = subprocess.Popen([binary] + args, env=env, cwd=scratch,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
            try:
                out, err = child.communicate(timeout=5)
                record = {"exit": child.returncode, "requires_login": b"not logged in" in out + err,
                          "unsupported_argument": b"unexpected argument" in out + err}
                if args == ["--version"]:
                    record["version"] = out.decode().strip()
                if args == ["chat", "--help"]:
                    record.update(v2=b"--v2" in out, tui=b"--tui" in out)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.communicate()
                record = {"timeout": True}
            summary[" ".join(args)] = record
        master, slave = pty.openpty()
        child = subprocess.Popen([binary, "chat", "--v2", "--tui"], env=env, cwd=scratch,
                                 stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        data = bytearray()
        deadline = time.monotonic() + 4
        try:
            while time.monotonic() < deadline and child.poll() is None:
                if select.select([master], [], [], 0.1)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    data.extend(chunk)
            summary["PTY startup"] = {
                "bytes_received": len(data), "terminal_sequences": b"\x1b[" in data,
                "alive_at_deadline": child.poll() is None,
                "limitation": "No authenticated rendering, repaint, session-store, or approval validation",
            }
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            os.close(master)
        summary["created_paths"] = sorted(str(p.relative_to(root)) for p in root.rglob("*") if p.is_file())
        mvp = Path(os.environ.get("KIRO_AGENTS_PROBE_BIN", Path(__file__).resolve().parents[1] / "target/debug/kiro-agents"))
        if mvp.exists():
            env.update(KIRO_AGENTS_HOME=str(root / "mvp-state"), KIRO_AGENTS_KIRO_BIN=binary)

            def call(*args):
                return subprocess.check_output([str(mvp), *args], cwd=scratch, env=env, timeout=8)

            try:
                call("new", scratch, "--v2", "--no-attach")
                row = json.loads(call("status", "--json"))[0]
                for _ in range(2):
                    master, slave = pty.openpty()
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                    frontend = subprocess.Popen([str(mvp), "attach", row["id"]], env=env,
                                                stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
                    os.close(slave)
                    time.sleep(0.4)
                    os.write(master, b"\x02d")
                    try:
                        frontend.wait(timeout=4)
                        assert frontend.returncode == 0, "MVP attach/detach failed"
                    finally:
                        if frontend.poll() is None:
                            frontend.kill()
                            frontend.wait()
                        os.close(master)
                after = json.loads(call("status", "--json"))[0]
                summary["real MVP detach/reconnect"] = {"same_pid": row["pid"] == after["pid"],
                                                        "state": after["state"], "authenticated": False}
                assert after["state"] == "live" and row["pid"] == after["pid"]
            finally:
                call("shutdown")
    output = Path(__file__).parent / "out"
    output.mkdir(exist_ok=True)
    (output / "v2-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))
    assert summary["chat --help"]["v2"] and summary["chat --help"]["tui"]


if __name__ == "__main__":
    run()
