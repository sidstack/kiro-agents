# Parity with pi-codex-agents: what carries, what cannot

pi-codex-agents works because **Pi is an embeddable SDK**: the host creates real Pi SDK
runtimes plus `InteractiveMode` instances and lends them a `RemoteTerminal` whose *method
calls* are serialized to the frontend, where Pi's own TUI renders them. No terminal
emulation is needed because both sides speak the same terminal abstraction.

Kiro CLI is a closed binary. It has no in-process mode, no terminal abstraction we can
implement, and no extension API. So the same **product behavior** is achievable, but the
mechanism must change from *in-process embedding* to *process supervision + passthrough*.

| pi-codex-agents | kiro-agents | Notes |
| --- | --- | --- |
| Persistent native host process | `kiro-agents-host` | same design |
| Abstract Unix socket ownership lock | same (`interprocess` abstract socket) | kernel-released after `SIGKILL` |
| PID + Linux start-time lifetime lock | same | `/proc/<pid>/stat` field 22 |
| Atomic registry (`lock` + `fsync` + rename, 2 MiB budget) | same | metadata only |
| Drafts as private per-session files, never in status | same | `drafts/<sha256(id)>` |
| JSONL IPC, LF-only split, UTF-8 across chunks, record/socket/request/connection caps | same | 1 MiB records, 1/4 MiB per frontend, 2 MiB backlog, 64 pending, 32 connections |
| `PROTOCOL_VERSION` handshake, `safeUpdate` + `settledStop` capabilities, `onlyIfSettled` stop | same | rebuild never interrupts a session |
| One attached terminal, `takeover`, detach on socket close | same | |
| `launchCwd` → open that directory's session on attach | same | |
| Home session as stable navigation target | same | |
| Board with Active/Inactive/All filters, search, status line | same (ratatui instead of Pi's TUI) | |
| Idle eviction of hidden settled sessions + restore on open | same, but restore = relaunch `--resume-id` | pi re-creates an in-process view; we re-spawn a process |
| Crash recovery: history restored, interrupted runs marked failed, never replayed | same | |
| Explicit stop vs quit (`quitFrontend`, `/agents stop`, Ctrl+K) | same | |
| Per-session `running` flag + pending-prompt count, independent of display label | same | restart safety computed per session |
| Optional features behind one assembly point + config toggles | same | |
| History import as read-only foreign rows, scan caps, partial-scan notices | same for V2 files; **best-effort only** for V3 | Kiro says storage is server-owned |
| `autoName` via a bounded model request through Pi's model registry | **different**: prefer Kiro's own `title` from `<id>.json`; optional local heuristic | we have no model API without ACP; do not shell out to a paid model call silently |
| `RemoteTerminal` + Pi `InteractiveMode` per session | **replaced** by `portable-pty` child + raw byte pipe | the core difference |
| Instance-local private UI/shutdown adapters on Pi's owned mode | **not applicable** | no Pi internals; instead `setsid` + signal handling on our child |
| Blocking Pi's native runtime-replacement commands inside the host | **analogous**: never run `kiro-cli` commands that rewrite sessions (`--delete-session`, `/chat new`, `/clear`) without an explicit user action | |
| Feature modules `SessionFeature` contract | same shape in Rust (`trait SessionFeature`) | |
| Smoke tests with real Pi views + deterministic local provider | **different**: `tests/fake-kiro` fake CLI + PTY smoke | no paid calls, no real account in CI |

## What we gain over pi-codex-agents

- Real tty for the child: `/editor`, `!` shell escape, and interactive subprocess prompts can
  work, which pi-codex-agents explicitly does not support.
- Kiro's own features stay intact: crew monitor (`Ctrl+G`), `/spawn`, subagents, plan mode,
  `/fullscreen`, `/rewind`, hooks, MCP, skills, steering.
- No dependence on Pi's private internals, so no "guarded instance-local adapter" fragility.

## What we lose

- No in-process control: we cannot read a session's model/thinking state, inject a prompt
  mid-turn, or abort a specific tool call without Kiro's own UI or ACP.
- No structured transcript: no searchable cross-session history beyond what Kiro's files
  expose (V2 well; V3 uncertain).
- No per-session theme/provider isolation concerns solved by us — Kiro's settings are global
  per `KIRO_HOME`, so per-session model choice is limited to launch flags.
- Status is inferred (child + hooks + files + timers), not reported by the runtime.

## Decision recorded

The user's requirement: *"just native kiro-cli TUI … don't want any relay or ACP … like how
I'm getting the full Pi experience in pi agents."* Interpretation adopted here: **the native
Kiro TUI must be what you see, unmodified**, and the wrapper must not re-render it or speak
ACP. The only mechanism that satisfies both "native" and "keeps running after the terminal
closes" is a host-owned PTY with a raw byte pipe while attached. If that pipe is
unacceptable, the only alternatives are (a) tmux owning the PTYs, or (b) letting Kiro die
with the terminal — both rejected in the plan.
