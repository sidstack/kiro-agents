# Architecture

Target architecture. The implemented V2 proof of concept is scoped in [MVP.md](MVP.md).

```text
frontend Pi analogue: kiro-agents (short-lived process, owns your terminal)
  original board view / attach view
  leader key or `kiro-agents` -> temporary terminal handoff
                 | private JSONL: hello, status, control, Terminal frames
                 v
         persistent kiro-agents-host (one per user, survives terminal close)
           |- Home session (stable navigation target)
           |- project A: kiro-cli chat (v3, agent kiro_default)   [PTY]
           |- project B: kiro-cli chat (v3, agent code-reviewer)  [PTY]
           `- project C: kiro-cli chat (v2, legacy session)        [PTY]
```

## Components

| Layer | Responsibility |
| --- | --- |
| `src/main.rs` | Frontend entry: connect to host, board or attach, restore own terminal on exit. |
| `src/host/` | Daemon: session supervisor, registry, locks, drafts, crash recovery, IPC server. |
| `src/session/` | Session record, state machine, PTY child supervision, ring buffer, idle eviction. |
| `src/attach/` | Terminal handoff: raw byte pipe, resize forwarding, leader-key interception, detach. |
| `src/board/` | ratatui session board: filters, search, create, status line. |
| `src/kiro/` | Kiro facts: profile discovery (`kiro-cli agent list`), session-file reader (read-only), generation resolver, launch argv builder. |
| `src/hooks/` | Optional global Hook install + event receiver (NDJSON) for status signals. |
| `src/config.rs` | Validated config snapshot, feature toggles. |

`src/session/` never imports `src/board/`. `src/kiro/` is the only place that knows Kiro
argv and file formats.

## Sessions and processes

One session = one live `kiro-cli chat` child on its own PTY, launched with:

- `cwd` = the project directory (so Kiro resolves that project's `.kiro/` natively)
- argv from the generation adapter: `kiro-cli chat --v3 [--agent <profile>] [--resume-id <id>]`
  or `kiro-cli chat --v2 ...`
- environment inherited from the host, plus per-session overrides
  (`KIRO_HOME`, `KIRO_SESSIONS_DIR`, `KIRO_DATA_DIR`, `KIRO_AGENT_CONFIG_DIR`,
  `KIRO_LOG_LEVEL`, `KIRO_NO_SYNCHRONIZED`) — passed through, never invented
- its own process group and session (`setsid`), so terminal close cannot kill it

Exactly **one live child per Kiro chat session id**. Kiro writes `<id>.lock` while a session
process is alive; two processes on one id is a conflict we must never create. Switching a
session between "attached" and "background" never forks a second child.

## Attach: passthrough, not rendering

While attached, the frontend does this and nothing else:

```
your tty  --read bytes-->  [leader-key filter]  --write-->  PTY master  -->  kiro-cli
your tty  <--write bytes--  (no filter)         <-- read --- PTY master
resize    --> TIOCSWINSZ on PTY master + SIGWINCH to the child's process group
```

- No VT parsing, no screen model, no re-rendering, no scraping. Kiro's TUI draws directly
  into your terminal through the pipe. Scrollback, mouse, synchronized output, hyperlinks,
  and Kiro's own key handling stay Kiro's.
- The pipe exists only because the child must survive your terminal closing. A child that
  shares your real tty dies with it; a child on a PTY owned by the host does not.
- The dashboard shortcut filter is the single exception to "raw": `Ctrl+G` detaches
  to the board (or shell for standalone attach). This explicit user choice overrides
  Kiro's native crew shortcut and avoids tmux's Ctrl+B prefix. All other keys,
  including Left Arrow and Ctrl+B, pass through byte-for-byte. Editor emptiness
  is not inferred from keystrokes or terminal output.

### Detached sessions keep running

The host always drains the PTY master into a bounded ring buffer (default 2 MiB per
session, drop-oldest). Without a reader, a PTY fills (~64 KiB) and the child blocks on
write — draining is what makes "keep running in the background" real. On reattach the host
replays the retained bytes, then sends a resize (same size, forced `SIGWINCH`) so Kiro
repaints the current screen. Ring replay is byte replay, not history parsing.

Consequence we accept: while detached, a Kiro tool-approval prompt cannot be answered. The
session shows **Needs input** and its agent waits. That is the same behavior as an
unanswered native prompt in pi-codex-agents.

## Session state machine

```
spawning -> ready -> running -> ready
              |         |
              |         +-> needs_input   (approval or interactive prompt pending)
              +-> failed                 (child exit != 0, or launch error)
ready|running|needs_input|failed -> idle    (evicted: child stopped, history kept)
any -> stopped                              (explicit Ctrl+K / `kiro-agents stop <id>`)
```

Status sources, in priority order (no screen scraping):

1. **Child liveness** — waitpid / exit code. Kiro exit codes: `0` success, `1` general or
   auth failure, `3` required MCP startup failure, `4` requested agent not found.
2. **Kiro Hooks** (opt-in) — `SessionStart`, `PromptSubmit`, `AgentStop`, `SessionEnd`,
   `PreToolUse`, `PostToolUse` from a global `~/.kiro/hooks/kiro-agents.json` write NDJSON
   events to `$KIRO_AGENTS_HOME/events.sock`. `AgentStop` = turn settled; `PromptSubmit` =
   running; `SessionEnd` = gone.
3. **Session files (read-only)** — `~/.kiro/sessions/cli/<id>.lock` (`{"pid","started_at"}`,
   present while alive; validate with `kill(pid,0)` to drop stale locks) and `<id>.jsonl`
   mtime/size growth as a fallback liveness signal when Hooks are off.
4. **Idle timer** — no events, no child output, no attachment for `idleTimeoutMs`.

`running` is only cleared by a settled turn (`AgentStop`, or prompt-settled heuristics),
never by silence alone. `needs_input` is sticky until input arrives.

## Persistence

Host state under `~/.kiro-agents/host/` (override `PI`-style: `KIRO_AGENTS_HOME`):

| File | Contents |
| --- | --- |
| `host.sock` | Unix socket, mode `0600` |
| `host.lock` | Lifetime lock: PID + Linux start time (`/proc/<pid>/stat` field 22) |
| `registry.json` | Session metadata only: id, kiro session id, cwd, agent profile, generation, name, state, unread, lastError, draft key |
| `drafts/<sha256(session-id)>` | Draft editor text per session, never inside the registry or a status response |
| `events/` | Optional Hook NDJSON spool |
| `host.log` | Host diagnostics only; never child stdout/stderr, environment, or transcript |

Ownership and safety, copied from pi-codex-agents because it is correct:

- **Abstract Unix socket** `\0kiro-agents-host-<uid>-<sha256(realpath(state))>[0:24]` as the
  kernel-held ownership lock: released even after `SIGKILL`, serializes stale-file recovery
  against concurrent launchers.
- Stale `host.lock` and `registry.json.lock` unlinked only after the kernel lock is held.
- Registry writes: separate lock, `fsync`, atomic rename, 2 MiB metadata budget.
- Never write inside `~/.kiro/`. Read-only access to `~/.kiro/sessions/cli/` for V2 history
  import, with scan caps (10 000 entries, 32 MiB total, 2 MiB per file, 1 MiB per record,
  512 KiB returned metadata) and partial-result reporting.
- Crash recovery: on host start, sessions whose child died are restored as **failed** with
  "Previous host ended during this session; review the transcript before continuing."
  Nothing is auto-replayed: we never resend a prompt or re-run a tool.

## IPC (frontend <-> host)

JSONL over the Unix socket, LF-split, UTF-8 decoded across chunks, 1 MiB per record,
1 MiB outbound / 4 MiB inbound per frontend, 2 MiB socket backlog, 64 pending requests,
32 connections max. A limit-induced socket loss is reported as an error, never as success.

Requests: `hello` (protocol + capabilities), `status`, `board`, `session/new`,
`session/attach`, `session/detach`, `session/prompt` (queued text for a settled session),
`session/cancel`, `session/stop`, `session/rename`, `session/recover`, `agents/list`,
`host/stop`, `host/restart`, `host/takeover`.

Frames: `input` (bytes), `resize` (cols/rows), `state` (pushed session snapshots),
`response`, `detached`, `stopped`.

Handshake enforces `PROTOCOL_VERSION`, not build identity. Automatic host replacement only
when the old host advertises `safeUpdate` + `settledStop` and atomically proves no running
turn, no pending prompt, no attached terminal, completed startup. Otherwise report it and
require `kiro-agents restart` after confirmation.

## Frontend lifecycle

1. `kiro-agents` (or the leader key inside an attached view) connects **before** touching
   the terminal, so a startup failure leaves the shell usable.
2. Frontend puts its own tty in raw mode, sends `session/attach`, then pumps bytes both
   ways. On detach it restores cooked mode and either shows the board or exits.
3. Closing the terminal detaches it; the host keeps every session. A new terminal runs
   `kiro-agents` to reconnect; `kiro-agents takeover` moves the terminal from another
   frontend without stopping sessions.
4. Quitting the frontend never stops sessions. Stopping sessions is explicit
   (`kiro-agents stop`, or board `Ctrl+K` then confirm).

## Board

Separate ratatui screen, not an overlay on Kiro's UI. Filters **Active** (ready, running,
needs_input, failed), **Idle**, **All**; type to search sessions and projects; `Enter`
attaches; `Ctrl+N` creates in another directory; `Ctrl+X` aborts the selected turn
(`SIGINT` to the child's process group); `Ctrl+K` stops a session; `Ctrl+Q`/`Ctrl+Z` leave
the board. Status line: `sessions 6 · 1 running · 2 needs input · 1 ready · Ctrl+G`.

## Feature toggles

`~/.kiro-agents/config.json`, validated, unknown keys are errors:

```json
{
  "autoAttach": true,
  "boardFullscreen": true,
  "showStatus": true,
  "autoIdle": true,
  "idleTimeoutMs": 3600000,
  "keepWarm": false,
  "autoName": true,
  "historyImport": true,
  "hooksTelemetry": false,
  "leaderKey": "ctrl+g",
  "autoUpdateHost": true,
  "defaultGeneration": "v3",
  "maxLiveSessions": 12,
  "ringBytes": 2097152
}
```

`keepWarm: false` (default) stops the child of a hidden settled idle session and relaunches
it with `--resume-id <kiro session id>` on open, restoring the same conversation. Running
sessions and unanswered approvals are never evicted. `keepWarm: true` keeps children alive
at the cost of one Kiro process per session.

Optional behavior lives in independent feature modules behind one `SessionFeature`-style
trait (history import, navigation keys, status line, idleness, naming), assembled at one
configuration point. Core never imports a concrete feature.

## Implemented upgrade boundary (naming compatibility fix)

The long-lived host owns process/PTY lifecycle independently of any dashboard.
Presentation metadata lives in a shared, locked, atomically updated names.json under
wrapper state, outside dashboard process memory and independent of host IPC version.
Updated frontends use that state-store API to name/clear sessions even when connected
to a pre-naming host; they do not rewrite its registry or restart its processes.
Lifecycle actions still go through the host's existing versioned protocol. This is
dashboard/metadata upgrade compatibility, not hot replacement of a PTY-owning daemon.

### Frontend upgrades and navigation

A new standalone invocation runs the executable selected by the shell. Connecting
to an existing host does not relaunch the host executable as the dashboard or
attach frontend. The host retains the live Kiro process and PTY, while Ctrl+G
filtering runs in the invoking frontend. Installing a replacement does not change
an already running frontend; exit that frontend and invoke the installed binary
again. No host restart is required. Stable v0.4.2 predates the standalone Ctrl+G
backport; its Ctrl+B d behavior is a release omission, not evidence of an old
host imposing its shortcuts on a new frontend.
