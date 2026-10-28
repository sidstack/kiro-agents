# MVP v3 plan and journal — 2026-10-26

MVP v3 is the third product iteration. It retains the explicitly selected Kiro
V2 engine; Kiro V3 support and saved-conversation resume remain separate work.

The user requested a smoother, faster dashboard inspired by Codex agents and
`../pi-codex-agents`, with directory and agent selection instead of mandatory
typing. Two pi-swarm workers use `strata/qwen3.8-flash-next-iq3_s-128k`.

## Milestones and todos

### M1 — baseline and reference review

- [x] Read lifecycle, session, generation and existing MVP guidance.
- [x] Inspect Pi's bounded searchable path picker and terminal attachment design.
- [x] Run the existing full check suite before integration (14 Rust tests, three PTY scripts, strict Clippy and formatting).

### M2 — selection-first session creation (worker: mvp3-pickers)

- [x] Select current/recent project directories with arrows and Enter.
- [x] Browse filesystem directories, search, and retain explicit path entry.
- [x] Discover native agents for the chosen directory without printing into the TUI.
- [x] Keep loading, cancellation and discovery errors recoverable in the board.
- [x] Add picker behavior tests and review updated renderer snapshots.

### M3 — terminal transitions and latency (worker: mvp3-transitions)

- [x] Keep the board visible until native attach has acknowledged ownership.
- [x] Avoid the intermediate shell screen and cooked/raw mode round trip.
- [x] Preserve standalone terminal cleanup and native byte passthrough (existing terminal smoke passed after attach refactor).
- [x] Improve input responsiveness and add PTY regression coverage.

### M4 — integration and delivery (supervisor)

- [x] Integrate the two disjoint implementations and review edge cases.
- [x] Run formatting, strict Clippy, Rust tests and all fake-CLI PTY checks.
- [x] Update version, README and final acceptance record.

Workers own disjoint files: picker worker owns `src/board/` and `src/kiro/mod.rs`;
transition worker owns `src/attach/mod.rs` and Python/fake-CLI test peers. The
supervisor owns integration, documentation and release metadata. Existing user
changes remain intact and uncommitted. No real Kiro model calls are required.

## Acceptance checks

Automated checks use deterministic fake Kiro processes in temporary state homes:

1. Open create, select a recent directory and a discovered agent using only
   navigation keys, and verify the child's cwd/profile metadata.
2. Search and browse directories, use an explicit path, cancel each step and
   retry after a discovery error without creating a child accidentally.
3. Switch two sessions repeatedly and verify their PIDs remain stable; detach
   and normal child exit return to the board with its selection intact.
4. Check the emitted handoff control sequences for unnecessary shell restoration
   and verify raw-mode ownership through board-to-child-to-board transitions.
5. Preserve standalone attach cleanup, raw control bytes, resize, crash recovery,
   confirmed stop, and quit/signal restoration regressions.
6. Render the picker with a long list and a small terminal; selection stays visible.

Guacamole SSH visual smoothness and Kiro's own repaint behavior still require
manual acceptance on an authenticated native session. Fake PTY checks establish
wrapper sequencing and input behavior, not native Kiro visual fidelity. Native
agent-list output shapes remain provisional until a safe authenticated probe
verifies them; unknown output must produce a recoverable error or manual fallback.

## Run notes

The initial pi-swarm broker was not running. The supervisor started a task-local
broker with two worker slots. Both initial worker processes failed before edits
because the older SDK could not resolve the discovery-only Strata model catalog.
Retry workers use an isolated agent directory with the existing router's explicit
model definitions. They run the requested Strata Qwen 3.8 model. No repository
configuration or Kiro-owned files were changed to repair worker startup.

The repository has `master` rather than `main`; `mvp-v3` was branched from that
existing base, preserving all prior uncommitted MVP work.

Discovery hardening by the supervisor: capture stdout nonblockingly with a strict
deadline and byte cap; stop only its owned process group while its child is
unreaped; cancel/join the scan on handle drop; reject unknown metadata keys as
profile names; cap filesystem entries examined. Ten isolated discovery tests
passed, including inherited-pipe timeout and prompt cancellation/reaping.

## Integration record

The picker worker delivered the initial discovery and picker models. After
supervisor review, the supervisor took over those files, corrected Unicode and
partial-path matching, explicit manual-agent selection, cancellation, and test
fixtures, then completed the runtime and renderer. The picker worker was stopped
to avoid overlapping edits during integration. The transition worker completed
the attach API and initial PTY test updates. Both used the requested Strata
Qwen 3.8 model.

The final runtime keeps one Screen owner across board/native transitions,
refreshes status in a background task, and renders only on input, resize or
changed state. Create uses directory and agent selection screens with contextual
help and scrolling. Esc preserves the directory selection when returning from
the agent screen; discovery failures retry in place. Creation failure retains
the chosen form. Interactive CLI new also omits the intermediate Created line;
background/piped output retains its session ID and reconnect command.

## Final validation and delivery

Final `make check` passed: formatting; Clippy for all targets/features with
warnings denied; 44 Rust tests (38 unit, six framing/lifecycle integration);
six reviewed board snapshots; and all six fake-CLI PTY scripts. The PTY suite
covers keyboard-only create, stable-PID switching, recovery/errors, native exit,
standalone cleanup, delayed/refused ACK, child terminal modes, discovery
cancellation/retry, and suppression of idle redraws. No real CLI probes or
model calls were rerun.

The initial complete-suite run exposed a PTY harness assumption: incremental
Ratatui output can reuse letters from the prior notice, so it is not contiguous
text. Error assertions now request a complete frame through a terminal resize.
A separate temporary executable-fixture race under parallel Rust tests was
removed by serializing fixture-writing/forking tests. The final complete suite
passed after those corrections.

`cargo install --path . --locked --offline` built the optimized release and
replaced `~/.cargo/bin/kiro-agents` 0.2.0 with 0.3.0. The prior binary is backed
up at `/tmp/kiro-agents-mvp3-swarm/kiro-agents-before-v3`. Reopen the frontend to
use the new dashboard. Existing real Kiro sessions/hosts were neither stopped
nor restarted; the protocol remains compatible. Changes remain uncommitted on
`mvp-v3`. Authenticated agent enumeration and Guacamole visual acceptance remain
the documented manual follow-up, rather than claims made by fake-CLI tests.

Installed release `--version`/`--help` and its real-PTY handoff regression passed.
The task-local pi-swarm was closed, its otherwise-idle owned broker stopped,
and its temporary credential copies removed after workers disconnected.

## Core CPU/naming follow-up — 2026-10-27

- Fixed a reproducible closed-PTY dashboard CPU spin using Crossterm's
  error-propagating input backend plus explicit terminal hangup checks.
- Added bounded frontend/host lifecycle CPU checks to normal and extracted-beta CI.
- Native naming verifies a bounded, start-time-checked process ancestry chain,
  covering runtime-thread delegation through Kiro/bun launchers. No cwd-recency guess,
  native-state write or host restart is used.
- Local make check passed: 77 Rust tests, ten PTY scenarios and packaging/installer
  tests. Strict make audit and actionlint passed. See FINDINGS and the incident report.
- This core backport does not introduce the optional workspace feature or change
  stable shortcut bindings; workspace/Ctrl+G acceptance remains in the feature PR.

## Standalone navigation backport

- Move native-attach navigation from Ctrl+B d to Ctrl+Q on the stable branch.
- Preserve Ctrl+B sequences, arrows, scroll and resize as native input.
- Keep session ownership in the existing host; re-entering with the updated
  frontend adopts the new shortcut without a host restart.
- Validate the actual PTY detach path and regression coverage for forwarded keys.

- Preserve Kiro’s Ctrl+G crew shortcut and verify raw-mode IXON is disabled.

### Recursive directory picker and daily cache

- [x] Match nested directories under home, with a cwd root outside home.
- [x] Discover off the input path and retain all results for scrolling/ranking.
- [x] Reuse a private directory cache for 24 hours; refresh stale lists in the
  background and support Ctrl+R to force refresh.
- [x] Keep selection-only creation and explicit-path fallback, bounded scans,
  cancellation, and no fd/fzf runtime dependency.
- [x] Add model, cache, and real-PTY regression coverage.

### Explicit live-session handoff

- [x] Confirm standalone/workspace registry identity; no native store import.
- [x] Add frontend-owned, token-checked release with bounded wait and cleanup.
- [x] Add Move here confirmation and explicit `attach ID --move` CLI action.
- [x] Preserve existing host RPC, native process identity, and old-peer refusal.
- [x] Exercise consent/cancel, round-trip attachment, cleanup, and older-host PTYs.
