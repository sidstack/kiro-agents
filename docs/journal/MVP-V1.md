# MVP v1 journal — 2026-10-16

## Goal

Validate the smallest useful version of kiro-agents with Kiro's native V2 TUI:
start sessions, detach, and return to the same live processes across terminals.
Get user feedback before implementing the larger board and lifecycle plan.
Here, **v1 refers to this project's first MVP**, not Kiro's V1 agent engine.

## Setup and plan review

The repository started with design documents and no implementation. Rust and
Kiro CLI were absent. Installed stable Rust 1.99.1 / Cargo 1.99.1 and official
Kiro CLI 2.32.0, then installed the MVP as `~/.cargo/bin/kiro-agents`.
Work is on branch `mvp-v2`.

The review kept native PTY passthrough as the core experiment and deferred
history import, saved-conversation resume, V3, hooks, idle eviction, and a full
ratatui board. Standard threads were sufficient for the first slice.

Probes confirmed `chat --v2 --tui`. The planned `--help-all` capability check
was unreliable; `chat --help` worked. Agent enumeration required login, and
the database location showed that KIRO_DATA_DIR alone was insufficient for
probe isolation. Probes therefore use temporary HOME, Kiro, and XDG paths,
without copying credentials or sending model prompts. Detailed observations
and unresolved questions are in [FINDINGS.md](../FINDINGS.md).

## What shipped

- A Rust frontend and background host in one executable.
- Fresh `kiro-cli chat --v2 --tui` launches in the selected project directory,
  optionally using an existing named Kiro profile.
- A plain session list, plus `new`, `attach`, `status`, `stop`, `shutdown`,
  `agents`, and `doctor` commands.
- Host-owned PTYs, raw byte forwarding, resize forwarding, and continuous output
  draining while detached. Each session retains a bounded 2 MiB in-memory ring.
- `Ctrl+B d` to detach; `Ctrl+B Ctrl+B` to forward a literal Ctrl+B.
- One attachment per managed session, a lifetime host lock, private socket,
  bounded protocol frames/queues, and atomic metadata persistence.
- Crash recovery that marks former live rows failed without launching children,
  replaying prompts, or sending signals to saved PIDs.

The wrapper leaves Kiro profiles, configuration, and permission decisions to
Kiro. It neither parses the terminal output nor logs transcripts or child
environments. Wrapper IDs identify managed processes, not Kiro session UUIDs.

## First user feedback: exited rows

The user selected an `exited` row and received an attachment error. Its recorded
exit code was 0. This established a UX gap: the list shows historical rows
beside live rows and still invites selection without explaining attachability.
No board-selection fix shipped in this iteration.

The current distinction is explicit: `live` can be reattached; `exited`,
`stopped`, and `failed` cannot. Exiting Kiro ends its process. Detaching keeps
that process available. Saved-conversation resume remains outside this MVP.

## Second user feedback: cursor overwrote retained text

Through Guacamole SSH, exiting native Kiro left its text with the cursor below
it. Exiting through kiro-agents instead put the shell cursor near the top,
so new shell input overwrote the retained session text.

The wrapper's cleanup unconditionally sent an alternate-screen reset that
restored a saved cursor, even after Kiro had already cleaned up. Changed it to:

- Preserve Kiro's screen and cursor after child exit, restore input modes,
  and add a newline without another alternate-screen reset.
- On detach/interruption, clean up terminal modes and place the shell cursor
  at the bottom before adding a newline.

Added PTY regression checks for these sequences, ran the checks, and reinstalled
the binary. The user confirmed the fix worked through Guacamole. The existing
host and live sessions did not need restarting for this frontend change.

## Validation at the end of v1

| Evidence | What it establishes |
| --- | --- |
| `make check` passed after the cursor fix | Formatting, Clippy with warnings denied, six Rust framing/lifecycle tests, and the terminal smoke passed. |
| Isolated real-CLI probe passed during initial implementation | The V2/TUI flags work; the same real Kiro PID stayed live across two attach/detach cycles before authentication. |
| Fake CLI with real PTYs | Output draining beyond 2 MiB, bounded replay, byte/resize forwarding, duplicate-attachment rejection, exit/stop handling, host ownership, crash recovery, frontend SIGKILL survival, and terminal cleanup. |
| User tested native Kiro through Guacamole SSH | Basic TUI interaction, detach/reattach, and the corrected exit/cursor behavior work in the user's terminal path. |
| User returned to a session awaiting permission | A live session and its pending user interaction survived detach; permission was not automatically granted. |

The user's description of “resuming” sessions means **reattaching to an existing
live Kiro process** in this implementation. It does not validate restarting
Kiro from a saved conversation after process exit or host restart.

## Still unverified or deferred

A real model/tool task actively progressing while detached was not manually
validated. Returning to a waiting permission prompt demonstrates persistence,
not continued computation past that prompt. Automated background-drain and
process-survival checks support the design but do not replace this real check.

Also unverified: exhaustive native redraw behavior, external editors and shell
subprocesses, custom-profile scope, real storage/lock schemas, and account
concurrency limits. V3, history import, automatic resume, turn-status telemetry,
idle eviction, drafts, and richer board interactions remain deferred.

## Result and next useful check

MVP v1 validates the core native-TUI attach/detach experience on the user's
Guacamole SSH path, including survival of a session waiting for permission.
It is sufficient for initial feedback, not full acceptance of the larger plan.

The next manual check is to detach during an explicitly chosen real task,
return later, and verify that it progressed or correctly stopped for approval.
The next small UX improvement is to distinguish attachable live rows from
exited history in the board. Neither step requires expanding into saved-session
resume or V3 yet.
