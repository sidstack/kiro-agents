# Build plan

Current status (2026-10-16): stable Rust 1.99.1 and Kiro CLI 2.32.0 are installed.
A minimal V2-only P1 slice is implemented; see [MVP scope/review](MVP.md) and
[FINDINGS](FINDINGS.md). Login-dependent P0 probes remain blocked. The phases and
stack below describe the broader roadmap, not the current executable. Dependency
versions actually built are recorded in Cargo.lock. User-directed V2 validation
now precedes V3, history import, and board polish.

## Stack

| Purpose | Crate | Verified version |
| --- | --- | --- |
| PTY spawn, resize, child I/O | `portable-pty` | 0.9.0 |
| Board UI | `ratatui` | 0.30.2 |
| Async runtime, child reaping | `tokio` | 1.53.3 |
| Unix socket IPC (incl. abstract sockets) | `interprocess` | 2.4.6 |
| tty raw mode, `TIOCSWINSZ`, `setsid`, signals | `nix` 0.31.3 / `libc` 0.2.190 | |
| JSONL + config | `serde` + `serde_json` 1.0.154 | |
| CLI parsing | `clap` 4.6.7 | |
| Filesystem watch (`.jsonl`, `.lock`) | `notify` 8.2.0 | |
| ids, hashing, errors, logging | `uuid` 1.28.0, `sha2` 0.11.1, `anyhow` 1.0.104, `thiserror` 2.0.21, `tracing` 0.1.44 + `tracing-subscriber` | |
| Snapshot tests | `insta` 1.49.0 | |
| Release binaries | `cargo-dist` 0.32.0 | |

Deliberately **not** used: `agent-client-protocol` (no ACP client), `vt100` (no terminal
emulation), tmux (no external multiplexer dependency).

## Proposed layout

```text
src/
  main.rs               frontend entry + subcommands
  host/main.rs          daemon entry (kiro-agents-host)
  host/server.rs        IPC server, connection limits, attach/takeover, stop/restart
  host/registry.rs      validated registry, atomic writes, metadata budget
  host/locks.rs         abstract-socket ownership, PID+start-time lifetime lock
  host/drafts.rs        per-session draft files
  session/model.rs      Session record + state machine
  session/supervisor.rs create/resume/stop/evict, one-child-per-session invariant
  session/pty.rs        portable-pty child, process group, drain ring, resize, signals
  session/status.rs     liveness from child + hooks + lock/jsonl + idle timer
  attach/pipe.rs        raw byte pipe, leader-key filter, restore-on-exit
  board/                ratatui board, filters, pickers, status line
  kiro/argv.rs          V2/V3 launch plans
  kiro/profiles.rs      agent profile discovery + validation
  kiro/history.rs       read-only session-file reader (V2 bundle), scan caps
  kiro/generation.rs    generation resolver
  hooks/                optional telemetry hook install/uninstall + event receiver
  config.rs             validated config, feature toggles
  protocol.rs           JSONL message types, PROTOCOL_VERSION, caps
tests/
  framing/ registry/ locks/ generation/ argv/ profiles/ status/ board/
  fake-kiro/            fake kiro-cli binary for integration tests
probes/                 P0 scripts + transcripts (not shipped)
docs/                   this plan
```

## Phases

### P0 — Probe the real CLI (no product code)

Goal: replace every `[probe]` in `docs/GENERATION-SUPPORT.md` and `KIRO-INTEGRATION.md` with
observed facts. Deliverable: `docs/FINDINGS.md` + raw transcripts in `probes/out/`.

Checks:
1. `kiro-cli version`, `kiro-cli chat --help-all`, `kiro-cli agent list --format json-pretty`
   in a directory with `.kiro/agents/` profiles (workspace + global + legacy + built-ins).
2. Launch `kiro-cli chat --v3` and `--v2` under a PTY; confirm the TUI renders, resize works,
   `Ctrl+C`/`Ctrl+D` exit codes, `Ctrl+Z` behavior, `/fullscreen`, `!` shell escape,
   `/editor` behavior, and whether `KIRO_NO_SYNCHRONIZED` is needed.
3. Session files created for both engines: paths, names, `.lock` shape and removal,
   `.jsonl` growth, `title`, `parent_session_id`, whether V3 writes the same bundle.
4. `--resume-id` legality across generations, and behavior when the id is unknown or the
   session is already locked by a live process.
5. Two concurrent `kiro-cli chat` processes in the same directory: allowed? same session id?
   separate ids? lock conflicts?
6. Detached-PTY behavior: run Kiro in a PTY with no reader, measure how much output it can
   produce before blocking, and confirm a forced same-size resize triggers a full repaint.
7. Hook firing for a PTY-launched V3 session: exact stdin JSON for `SessionStart`,
   `PromptSubmit`, `AgentStop`, `SessionEnd`; whether global `~/.kiro/hooks/` loads.
8. Exit codes for: missing agent (`4`), MCP failure with `--require-mcp-startup` (`3`),
   auth failure (`1`).
9. `kiro-cli agent validate --path` output shape for a broken profile.

Exit criteria: every `[probe]` resolved or explicitly blocked; a decision recorded where
docs and reality disagree.

### P1 — Host + one session + attach (vertical slice)

`kiro-agents new <dir>` spawns a V3 session in a PTY; `kiro-agents` shows a one-row board;
`Enter` attaches as a raw pipe; detaching keeps the child running; closing the terminal
detaches; reopening reconnects to the same live process; `kiro-agents status` reports it;
`kiro-agents stop` stops host and sessions.

Acceptance: kill the frontend with `SIGKILL` while attached → child survives, board reopens
clean, no duplicate child. Kill the host → sessions marked `failed` with the recovery notice,
history intact, no prompt replay.

### P2 — Multi-session board + registry + idle/eviction

Multiple directories, Home session, filters (Active/Idle/All), search, `Ctrl+N` directory
picker, drafts per session, idle eviction + resume-on-open (`keepWarm: false`), status line,
`takeover`, `--launchCwd` behavior (launching `kiro-agents` in a directory opens that
directory's session), atomic registry, locks, crash recovery, protocol handshake with
`safeUpdate`/`settledStop` capabilities.

### P3 — Status fidelity + control

Hook telemetry (opt-in install/uninstall), `.lock`/`.jsonl` fallback with stale-lock PID
checks, `needs_input` detection, `Ctrl+X` abort (`SIGINT` to the process group), `Ctrl+K`
stop, `session/prompt` queue for settled sessions (text typed in the board is delivered on
attach, never injected silently), autoIdle/autoName, per-session `unread`.

### P4 — V2 support + history import

Generation resolver, V2 launch plans, legacy profile detection with `/upgrade-agent`
guidance, read-only import of `~/.kiro/sessions/cli/` V2 bundles as **Idle** rows, resume
legality enforcement, cross-generation refusal message, scan caps and partial-scan reporting.

Acceptance: a V2 session resumes on `--v2`; opening it with `--v3` is refused with the
documented reason; importing never modifies a Kiro file.

### P5 — Agent/profile UX + polish

Profile picker per directory (built-ins, workspace, global, legacy, `.md` profiles),
`kiro-agents agents [dir]` listing, model/effort picker, `unattended` badge when
`--trust-all-tools` was used, read-only grouping of Kiro subagent sessions under their parent
(`parent_session_id`), packaging (`cargo-dist`), CI, smoke scripts, SECURITY.md, docs.

Deferred, explicitly: ACP automation mode, headless `stream-json` runner mode, Windows
(named pipes instead of abstract sockets), remote/cloud sessions, Kiro Crew integration.

## Test strategy

- **Unit**: JSONL framing (LF-only split, UTF-8 across chunks, record caps), registry
  validation and atomic write, lock behavior under simulated `SIGKILL`, generation resolver,
  argv builders per generation, profile scanner against fixture trees (workspace/global/
  legacy/`KIRO_AGENT_CONFIG_DIR`), status reducer (event → state), board layout snapshots
  with `insta` + ratatui `TestBackend`.
- **Integration**: `tests/fake-kiro` — a small Rust binary that mimics `kiro-cli chat`
  (renders a fake TUI, writes a `.lock`, appends `.jsonl`, exits with configurable codes,
  honors `--resume-id`/`--agent` validation). Runs the real host + PTY path without an
  account or paid calls.
- **Probe-gated real tests**: `#[ignore]`d tests that require a real `kiro-cli`, run only via
  `make probe`, using a temporary `KIRO_HOME`/`KIRO_SESSIONS_DIR` and a throwaway directory.
  Never against the user's real `~/.kiro`.
- **Smoke**: PTY scripts that spawn the host, attach, detach, `SIGKILL` the frontend, reopen,
  assert one child per session and surviving state.

## Definition of "works like pi-codex-agents"

1. Sessions outlive the terminal and the frontend process.
2. One command opens the board from any terminal, in any directory.
3. Switching sessions preserves each session's live state; nothing restarts.
4. Idle sessions are restored from their own history without replaying prompts.
5. Stopping is explicit; quitting is not stopping.
6. A crash restores history and marks interrupted work failed, never re-runs a request.
7. Optional features are toggleable; with all toggles off, create/switch/attach/stop still work.
