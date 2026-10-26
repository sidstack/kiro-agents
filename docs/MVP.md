# Kiro V2 MVP and plan review — 2026-10-16

The native-PTY supervisor is the right vertical slice to validate the requested
experience. The full plan mixes that question with a much larger product: board
filters, history import, hooks, draft persistence, eviction, and dual engines.
The user requested a minimal V2 build and feedback before expanding it.

## Review findings

1. **Probe flags before relying on reference docs.** Installed Kiro CLI 2.32.0
   supports `chat --v2 --tui`. `--help-all` is not a usable capability probe on
   this install. Use `chat --help`. See FINDINGS for bounded probe results.
2. **Authentication gates useful P0 checks.** Agent enumeration fails without
   login; schema, locks, resume, approvals, and authenticated redraw remain
   unverified. These assumptions must not become history/resume code yet.
3. **Liveness is not turn status.** A quiet child may be thinking or awaiting
   approval. The MVP shows only process state. No idle eviction or inference
   from terminal output is safe at this stage.
4. **The logging plan conflicts with the hard rules.** ARCHITECTURE proposed
   child stderr in host.log, but stderr can contain transcript text and auth
   material. The MVP forwards both child streams through the PTY and persists
   neither; only metadata enters the registry.
5. **Byte replay is the main UX hypothesis.** A bounded ring can begin partway
   through an escape sequence. SIGWINCH requests a repaint, but does not prove
   that Kiro repaints correctly. Test real reattach visually before building
   board polish or automatic background behavior.
6. **Isolation needs more than the advertised overrides.** Real startup put
   data.sqlite3 under XDG_DATA_HOME despite KIRO_DATA_DIR. Probes isolate HOME
   and all relevant XDG roots, without copying real auth.

## Implemented slice

One executable has a hidden host entry point. CLI commands are `new`, `status`,
`board` (also default), `attach`, `stop`, `shutdown`, `agents`, and `doctor`.
`src/kiro/` alone builds Kiro argv and invokes Kiro profile discovery.
The host uses standard threads, portable-pty, a lifetime flock, a private Unix
socket, versioned bounded JSONL frames, and atomic fsynced metadata writes.
No async runtime or ratatui is needed for this first interaction check.

A fresh session launches the actual V2 TUI in the requested canonical cwd.
An input worker prevents a blocked child stdin from blocking the host registry;
output is continuously drained. Each session retains at most 2 MiB of bytes
and allows one attachment. Slow attached consumers are disconnected with an
error rather than blocking the child. Process-group TERM with bounded grace
then KILL is available only through an owned, unreaped Child handle.

The registry contains wrapper ID, cwd, requested agent, generation, child PID,
process state, exit code, and a recovery notice. It contains no Kiro session ID:
this MVP never resumes or imports sessions, and never invents a Kiro UUID.
Concurrent startup is serialized with a lifetime flock; stale socket recovery
happens only while that lock is held. The kernel releases it on host death.

The larger architecture's abstract socket, async runtime, feature trait/config,
and full IPC caps are future design targets. Current fixed limits: eight live
sessions, 32 clients, 128 KiB frames, 1 MiB attachment output queue, 64 KiB input
queue. Optional product features are absent, so there are no half-working toggles.

## Acceptance and feedback

Subsequent user validation and the cursor-cleanup fix are recorded in the
[MVP v1 journal](journal/MVP-V1.md). The checklist below is the original
acceptance plan; the journal distinguishes completed checks from open ones.

Automated checks use tests/fake-kiro and real PTYs without paid model calls.
They cover leader-key forwarding, fragmented framing, frame bounds, one
attachment, output drain/replay, resize, natural child exit, explicit stop,
missing-agent exit 4, concurrent host ownership, crash recovery without replay,
frontend SIGKILL survival, and terminal restoration.

Before expanding scope, manually log in and try:

1. Start a V2 session in a project and confirm its native profile/permissions load.
2. Send a small prompt, detach, open another terminal, and reattach by ID.
3. Check the screen and any pending approval; resize the terminal.
4. Create a second project's session and switch between the two.

Feedback needed: does reattach feel native, is Ctrl+B d comfortable, and is a
richer board necessary? Revisit replay if rendering fails. Keep storage import,
resume, V3, telemetry, and eviction deferred until their real probes pass.

## Project MVP v2

The second product iteration adds a navigable session board over the existing
V2 host: Live/All filters, search, create, confirmed stop, and return-to-board
after native attach. Explicit CLI attach/new retain detach-to-shell. Historical
rows explain their unattachable state. The protocol, Kiro launch flags, and
saved-conversation boundary stay as above. See the concrete
[MVP v2 plan and journal](journal/MVP-V2.md) for acceptance and verification.


## Project MVP v3

The third product iteration focuses on selection-first directory/profile creation,
responsive discovery and a continuous board/native terminal handoff inspired by
`pi-codex-agents`. It retains the V2 engine and fresh-launch-only boundary. See the
[MVP v3 milestones and journal](journal/MVP-V3.md) for task ownership and validation.
