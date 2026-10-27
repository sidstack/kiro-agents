# Risks and mitigations

## 1. No stable API contract (closed binary)

Kiro CLI exposes no extension API and no versioned client contract for the interactive path.
Flags, session formats, and TUI behavior can change between releases.

**Mitigation.** Passthrough attach is insensitive to Kiro's rendering choices: we forward
bytes and never parse them. Only argv construction and status inference are version
sensitive, and both are isolated in `src/kiro/`. At host startup, probe capability by
invocation (`kiro-cli chat --help-all`, `agent list --format json`) rather than parsing a
version string. Pin a tested version range in README and report "untested Kiro build"
rather than guessing.

## 2. Detached sessions stall on a full PTY buffer

With no reader, the PTY fills (~64 KiB) and the child blocks on write.

**Mitigation.** The host always drains each PTY master into a bounded ring
(`ringBytes`, default 2 MiB, drop-oldest). P0 measures how much output Kiro produces before
blocking and whether a drain-only session ever blocks on stdin-side writes.

## 3. Reattach shows a stale or torn screen

Byte replay reconstructs what the child wrote while detached; Kiro may have repainted
partially.

**Mitigation.** After replay, force a same-size resize (`TIOCSWINSZ` + `SIGWINCH`) so Kiro
repaints. Config `replayOnAttach`: `ring` (default) | `none` (clean repaint only) |
`full`. P0 verifies which produces a correct screen. This is the weakest point of the
no-emulation design and the first thing to falsify.

## 4. Two processes on one Kiro chat session

Kiro writes `<id>.lock` while alive. A second process on the same id risks corrupting the
session or silently forking history.

**Mitigation.** Hard invariant: one live child per Kiro session id, enforced in
`session/supervisor.rs`. Before spawn, read `<id>.lock` and `kill(pid,0)`; if a foreign
process holds it, report "this session is open in another terminal (pid N)" and offer attach
to our own child only. Never `SIGKILL` a foreign Kiro process.

## 5. Approvals cannot be answered while detached

A tool-approval prompt blocks the agent until someone answers in Kiro's UI.

**Mitigation.** `needs_input` state from Hooks/`AgentStop` absence, visible badge and count
in the status line, optional desktop notification, `keepWarm: true` for approval-heavy
sessions. Never auto-approve: `--trust-all-tools` is only used when the user explicitly
creates an unattended session, and the row is badged `unattended`.

## 6. V3 session store opacity

Kiro's own guidance for programmatic clients is "treat storage as server-owned; use
`session/list`" — which is an ACP method we are not using. V3's on-disk layout is not a
client-readable contract.

**Mitigation.** Our registry is the source of truth for our sessions. History import is
best-effort and generation-split: V2 four-file bundle is parseable **[3p]**; V3 import may
degrade to "sessions we created only". Never write or patch Kiro session files. If P0 shows
V3 files are unreadable, we do not fake it: the board shows what the host created plus a
hint to use Kiro's own `/sessions` picker.

## 7. Cost and resource growth

Each live session is a full agent runtime plus provider usage.

**Mitigation.** `maxLiveSessions` cap with explicit refusal, idle eviction by default,
per-session turn counters from Hook events, and a board column for context usage when
readable from `<id>.json` (`context_usage_percentage`). Kiro's own `/usage` stays the
authoritative billing view.

## 8. Terminal and key conflicts

`Ctrl+B` is tmux's default prefix. Kiro owns `Ctrl+C/D/G/X/O/R/S/T/U/J/Z`, `Tab`,
`Shift+Tab`, `Esc`.

**Current choice.** Ctrl+Q detaches. It is not listed in Kiro's documented default
shortcuts or tmux's default key bindings. Ctrl+G remains Kiro's crew shortcut and
Ctrl+B remains available to tmux. Raw mode disables IXON software flow control;
terminal-client custom bindings still need manual verification. Left Arrow
passes through without guessing editor state. Shortcut configuration is not shipped.

## 9. Secrets and logs

Children inherit auth from `KIRO_HOME`; Kiro logs can contain file paths, code, and command
output.

**Mitigation.** Host never logs child env or child stdout/stderr. Any host diagnostics must also
exclude transcript text and auth URLs; token-pattern redaction alone is insufficient. Registry
stores metadata only, never draft text, never transcript text. Our socket is `0600`. Docs
tell users not to attach `host.log` to bug reports unredacted.

## 10. Interactive subprocesses through the PTY

Kiro's `!` shell escape, `/editor` (`$EDITOR`), and prompts from `sudo`/`ssh` need a real
tty. A PTY child has one, so these should work — unlike pi-codex-agents' documented
limitation for external editors.

**Mitigation.** P0 tests `!npm run build`, `/editor`, and an interactive prompt under our
attach path. If `/editor` conflicts with our raw-mode frontend, document it and offer
explicit detach (Ctrl+Q leaves the session running).

## 11. Host as a long-lived privileged process

The host spawns shells in arbitrary directories and survives terminal close.

**Mitigation.** No shell parsing of user input; argv arrays only. Directory arguments are
canonicalized and must exist. Session names are display strings, never interpolated into
commands. Trust decisions stay with Kiro's own project trust/permissions model. Explicit
`kiro-agents stop` semantics, `SIGTERM` then reap, and a forced exit timer so no unref'd
handle keeps a "stopped" host alive.

## 12. Platform scope

Linux first (abstract sockets, `/proc` start-time identity). macOS second (`KIRO_HOME`
differences, `$TMPDIR` logs, no abstract sockets → path socket + stale-file recovery).
Windows later (`interprocess` named pipes, ConPTY via `portable-pty`).

## Open questions to take upstream

1. Is `kiro-cli chat`'s CLI surface considered stable for wrapper tooling, or only ACP?
2. Is there a supported way to enumerate sessions per generation without reading files?
3. Is one-process-per-session-id enforced by Kiro, or only implied by `.lock`?
4. Are Hooks guaranteed to fire for PTY-launched interactive V3 sessions?
5. Is there a rate/concurrency limit on simultaneous Kiro sessions per account?
