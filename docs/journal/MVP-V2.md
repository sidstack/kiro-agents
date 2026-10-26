# MVP v2 plan and journal — 2026-10-16

Here, v2 is the second product iteration. Kiro launches continue to use its V2 engine.

## Problem and scope

MVP v1 proved persistent native PTY attach/detach, including the Guacamole cursor
cleanup fix. Its plain list mixes live and historical rows, asks for copied IDs,
and sends users back to the shell after every detach. An exited row cannot be
attached, but the list did not explain that before selection.

This iteration adds a core interactive board using the existing host operations:

- Live rows by default; Tab switches to All, including exited/stopped/failed rows.
- Arrow keys or j/k select; / searches directory, profile, ID and state; Esc clears.
- Enter attaches only a live row. Historical rows explain why they cannot attach.
- Ctrl+N opens directory and optional existing-profile fields, then creates a fresh
  session in that directory. Kiro remains responsible for profile resolution.
- Ctrl+K requests confirmation before stopping the selected live session.
- r and periodic refresh update child liveness while preserving selection by ID.
- Detaching or exiting Kiro from the board returns to the board. Explicit
  `attach` and `new` retain their existing detach-to-shell behavior.
- q or Ctrl+Q leaves the board and keeps children alive. Piped board output
  stays plain text. Terminal modes are restored on error and handled signals.

## Boundaries

Host ownership, protocol, registry, raw child byte forwarding, resize and ring
buffer behavior remain the v1 implementation. No turn-state inference, output
parsing, Kiro-owned state writes, automatic tool approval, or prompt replay.
No saved-conversation resume, Kiro UUID discovery, V3, hooks, idle eviction,
drafts, takeover, or agent-profile synthesis. These require separate evidence
and are not needed to address the v1 navigation feedback.

## Stages and acceptance

Pi supervisor workflow: read-only planning and baseline review; bounded frontend
implementation and independent fake-CLI PTY test development; integration;
independent final review and check execution. The private task ledger lives at
`/tmp/kiro-agents-pi/supervision/tasks.md` for this run.

Baseline `make check` passed: formatting, Clippy with warnings denied, six Rust
framing/lifecycle tests and the existing fake-CLI terminal smoke.

Acceptance uses fake Kiro and real PTYs without model calls:

1. Create two sessions and switch through the board; each retains its PID and
   receives native input. Detach returns to the board without duplicate children.
2. Search/filter and refresh preserve valid selection; historical rows never
   spawn or attach and show clear guidance.
3. Create from the board with cwd/profile; invalid input stays in a usable board.
4. Cancel stop leaves a child alive; confirm stop changes only the selected row.
5. Quit, signals and errors restore the terminal; quitting preserves sessions.
6. Non-TTY output remains useful and never waits for keyboard input.
7. Board model tests and renderer snapshots pass; existing PTY cleanup, framing,
   process survival and crash-recovery regressions continue to pass.

## Implementation and validation

The Pi planning worker recommended a board-only iteration and keeping the host
protocol unchanged. A Pi implementation worker delivered the filtering/selection
model and three regressions. The Pi baseline reviewer reproduced the historical
registry frame limit and delivered the first PTY harness. The supervisor
integrated runtime/rendering, corrected compile/test and harness issues, and
added the error-path PTY checks. A fresh Pi worker reviews the final terminal
handoff and controls.

The runtime explicitly restores the board terminal before native attach and
re-enters the board afterward. A shared termination flag covers signals during
the transition. Explicit CLI attach/new retain their existing shell-return path.
All child output continues through the unchanged native byte pipe.

The first PTY startup exposed Ratatui 0.30's cursor-position query in
`Terminal::clear()`: a bare PTY does not answer it, so startup timed out. Fullscreen
redraw now uses `resize` at the current dimensions, which invalidates the board
buffers without requiring a cursor reply. No child output is interpreted.

Validation: all 14 Rust tests passed, including three model tests and four
control/validation tests; five TestBackend snapshots were generated and
reviewed. Clippy with warnings denied passed. The board PTY smoke passed
filter/search, historical refusal, stable-PID switching, stop confirmation,
create/profile forwarding, natural exit, quit and SIGTERM restoration. The
supplemental PTY check passed invalid-directory handling and recoverable status
frame overflow, retry/quit, and canonical-mode restoration. Final `make check`
passed, including all three PTY scripts. The locked release build also passed.
Independent Pi review found no blocking issue and identified two small defects:
`new` tried to attach with TTY stdin but piped stdout, and Ctrl+N discarded an
active form. Both were fixed, with a piped-output PTY regression and a unit test
for preserving form/confirmation state. A separate Pi test worker verified the
suite and release build, then reran checks after these fixes.

The existing host cannot fit every valid 2 MiB registry in a 128 KiB status
response; the reproduced limit and deferred pagination are in FINDINGS.md.
Real Kiro visual redraw through Guacamole and active real-task progress while
detached still need user validation. No Kiro model calls or authentication-state
copies were made in this iteration.

## Delivery

Installed `kiro-agents 0.2.0` at `~/.cargo/bin/kiro-agents`; `--version` and
`--help` passed. The previous frontend is backed up in the private task
directory for this run. No running real host or Kiro process was stopped or
restarted. Reopen `kiro-agents` to use the board; already-open frontends keep
their existing code. All repository changes remain uncommitted.
