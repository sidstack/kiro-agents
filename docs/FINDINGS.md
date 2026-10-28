# Findings — 2026-10-16

Target: Linux x86_64, Rust 1.99.1 / Cargo 1.99.1, Kiro CLI **2.32.0**.
Reproduce with `make check` and `make probe`. Sanitized real-probe summaries are
written to ignored `probes/out/v2-summary.json`; no terminal transcript, auth URL,
credential, or child environment is logged. Findings override the reference docs.

## Observed facts and contradictions

| Probe | Observed | Consequence |
| --- | --- | --- |
| `kiro-cli --version` | Exit 0, `kiro-cli 2.32.0` | Tested build; V2 is an engine choice within the installed CLI, not a separately installed V2 executable. |
| `kiro-cli chat --help` | Exit 0; advertises `--v2`, `--tui`, `--resume-id`, `--agent`, `--list-sessions`; engine help calls V2 the default | MVP explicitly selects `chat --v2 --tui`, independent of saved defaults. |
| `kiro-cli chat --help-all` | Initial inherited-environment isolated run timed out at 20s; minimal-environment reruns exit 1 | Contradicts the plan's use of this command as a capability probe. Use the successful `chat --help`; do not infer features from a version alone. The exact failure cause is not established. |
| `kiro-cli agent list --format json-pretty` in scratch cwd | Exit 1, reports not logged in | Cannot verify profile enumeration JSON or cwd scoping before login. MVP delegates the command to Kiro and surfaces its own output. |
| `kiro-cli chat --v2 --list-sessions --format json` | Inherited-environment isolated run timed out at 8s; minimal-environment reruns exit 1 | Session-list schema and generation reporting remain unverified; no import/resume implementation. |
| `chat --v2 --tui` on a scratch PTY | Emitted terminal sequences; stayed alive until explicitly terminated, with no prompt sent | Confirms executable startup and terminal I/O only, not authenticated chat, screen fidelity, or model behavior. |
| Startup with KIRO_HOME, KIRO_DATA_DIR and XDG_DATA_HOME pointing to separate scratch paths | Kiro created `kiro/settings/cli.json` and `share/kiro-cli/data.sqlite3`; no database under KIRO_DATA_DIR | KIRO_DATA_DIR alone is not an isolation guarantee for this path/build. Probes isolate HOME and XDG roots as well. Kiro itself created these files in temporary state; the wrapper wrote no Kiro state. |
| Real MVP `new --v2 --no-attach`, attach/detach twice, status | The same real Kiro PID remained `live` across both frontend exits | Real native-process persistence works before authentication. No prompts or model calls were made. |
| Fake peer emits >2 MiB while disconnected | Finished output; same child reattached with bounded retained bytes | Host drains detached PTYs rather than letting the child block. |
| Fake peer receives dimensions and arbitrary bytes | Resize reached 121x43 (protocol) and 117x39 (actual frontend); native control/non-UTF-8 bytes preserved | Validates forwarding and leader filter without interpreting Kiro output. |
| Kill attached frontend with SIGKILL | Child PID survived; fresh frontend attached to it | Frontend lifetime is independent of the host-owned PTY. |
| Detach and SIGTERM an attached frontend | Exited and restored canonical terminal mode | Normal/signal cleanup verified in a real controlling PTY. |
| Competing host and SIGKILL host/restart | One host owns the lifetime flock; former live metadata becomes failed with recovery notice | No child restart, resume, or prompt replay. Saved PIDs are never signal targets. |
| Fake missing agent | Exit 4 recorded as failed | Verifies wrapper exit handling only; the real CLI's missing-agent behavior remains login-dependent. |

## P0 questions still unresolved

The original sixteen-question P0 matrix remains open where it depends on an
authenticated real session or V3. No fake-peer result establishes a Kiro storage
contract.

| Original questions | Status |
| --- | --- |
| 1–4: V3 store, lock format, cross-generation resume, duplicate session UUID behavior | Deferred; login required. MVP neither reads Kiro history nor resumes by UUID. |
| 5: concurrent real conversations in one cwd | Deferred; login required. Fake tests cover multiple managed children only. |
| 6: real Kiro detached-output stall threshold | Deferred; no model output generated. Fake peer verifies host drain beyond 2 MiB. |
| 7: forced same-size resize repaints native Kiro | Manual authenticated visual check required. Signal delivery is tested, repaint fidelity is not. |
| 8–9: profile JSON, workspace/global/legacy scope, KIRO_AGENT_CONFIG_DIR | Blocked by login; no profile scanner or synthesis. |
| 10–11: V3 hooks and global hook loading | Deferred with V3/telemetry; no hook installed. |
| 12: real exit codes 1/3/4 | Exit 1 observed for logged-out agent listing; real missing-agent/MCP startup codes unverified. |
| 13–15: editor, shell escapes, subprocess prompts, Ctrl+Z, synchronized rendering | Manual native-session acceptance required. No override of KIRO_NO_SYNCHRONIZED. |
| 16: account concurrency limits | Unverified; no paid calls made. |

## Scope decisions

Per the user's request, proceed with the minimal V2 process-persistence MVP while
keeping all features that rely on unresolved probes absent. Wrapper IDs are
process identifiers, never minted Kiro UUIDs. Reattach uses the already-live
child. No automatic resume, history import, idle eviction, telemetry, turn-state
inference, V3, or unattended tool approval.

The architecture's proposed child-stderr host log conflicts with repository
privacy rules. Corrected the target document; the MVP persists metadata only and
keeps the bounded output ring in memory. See [MVP review](MVP.md) for scope and
manual acceptance steps.

## Native exit cursor regression — user feedback

The user reported that native Kiro leaves retained text with the shell cursor
below it, while exiting through the wrapper restores a cursor near the top and
shell input overwrites retained text (Guacamole SSH). The registry's exited
rows record child exit code 0; they are not evidence of a host crash.

Inspection found that frontend cleanup always emitted DECRST 1049, including
after normal child exit. That control sequence switches to the normal screen
and restores the saved cursor ([XTerm control sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs-contents.html)).
It can therefore undo the cursor position left by Kiro's own cleanup. Existing
canonical-mode tests did not catch cursor-moving output.

Cleanup now distinguishes child exit from detach/interruption. On child exit it
preserves the native screen/cursor, restores input modes, and adds a newline;
it sends no alternate-screen reset. On detach it resets the screen mode and
scroll region and positions the cursor on the bottom row before the newline.
No child output is parsed. The terminal smoke now checks that native exit
forwards the final text without any added alternate-screen reset or screen
clear, and that detach positions the shell below retained content. This is
sequence-level PTY coverage. The user subsequently confirmed that the cursor
fix works through Guacamole SSH, and that native TUI detach/reattach works,
including returning to a pending permission prompt. Active real-task progress
while detached has not been manually validated. See the
[MVP v1 journal](journal/MVP-V1.md) for the acceptance record and remaining gaps.

## MVP v2 baseline review: status response budget

Pi review found a metadata/protocol budget mismatch in the v1 host. Independently
reproduced with an owned test host and a temporary registry of 1,200 exited
metadata rows (cwd `/tmp/status-limit-project`): registry 236,402 bytes, compact
`Sessions` reply 171,614 bytes. `status --json` exited 1 with
`IPC connection closed` and no rows. No real Kiro was launched. A 900-row fixture
with shorter paths remained below the wire cap and succeeded; the threshold
depends on metadata length, not a fixed row count.

The registry permits 2 MiB while each IPC frame permits only 128 KiB. The send
error closes the connection; this contradicts any assumption that every valid
registry fits a status response. MVP v2 must keep refresh errors recoverable in
the board. Pagination or a revised metadata response budget remains follow-up
work; the board does not delete historical records to conceal this limit.


## MVP v3 transition and picker regressions — 2026-10-26

A temporary IPC peer delayed its attach response while a real frontend PTY was
observed. The MVP v2 board emitted alternate-screen leave before the ACK,
exposing the shell during the wait. The integrated v3 board keeps its existing
screen and raw mode through delayed/refused ACKs; native output begins only
after a successful ACK and display handoff. Board return avoids standalone
shell cursor/newline cleanup, while standalone cleanup regressions still pass.

Fake-CLI PTY checks establish arrow-only directory/profile creation, stable PIDs
through switching, child-mode reset, native exit returning to the board, slow
discovery cancellation, and retry after listing failure. An unchanged idle
board writes fewer than 128 terminal bytes over a 1.3-second refresh interval.
This checks redraw suppression, not native Kiro startup or inference speed.

The first threaded capture implementation could wait indefinitely for stdout
inherited by a descendant after killing its immediate listing child. Capture
now reads nonblockingly against a deadline and stops its own process group
before reaping. Cancellation joins and reaps the owned listing. Tests cover a
shell/sleep inherited pipe, output overflow, cancellation, and valid empty lists.
Filesystem listing caps all examined entries, including files, at 2,000.

Native agent enumeration schema remains unverified; fixture JSON is not proof
of a real Kiro contract. Unknown output offers the Kiro default or an explicit
manually entered name. No real Kiro session, account state, or model call was
used for these checks. Guacamole visual acceptance of the new transitions
remains a manual follow-up; prior native repaint limits still apply.

## Dashboard usability and native naming (2026-10-26)

- Installed Kiro CLI 2.32.0 `agent list --help`, run under a temporary HOME and Kiro
  roots, advertises verbose/help options and cwd-local discovery, without a JSON
  format option. Listing with `--format json-pretty` remains login-gated. JSON-only
  capture cannot be treated as complete discovery; scoped read-only profile fallback
  is required. Authentication/model calls were not attempted.
- Read-only shape inspection of existing native session metadata confirmed `.json`
  fields `session_id`, `cwd`, `created_at`, `updated_at`, `title`,
  `session_created_reason`, `session_state`; `.lock` fields `pid`, `started_at`.
  Metadata identity matched the filename stem. Native JSONL record discriminators
  include `Prompt`, `AssistantMessage`, `ToolResults`; prompt text parts use
  `data.content[].kind = text` and a string `data`. No prompt, title, auth state or
  transcript content was printed or committed. These observations support bounded
  read-only naming, not history/resume or a universal storage contract.
- The prior single-alternate-screen handoff prevents ordinary primary-screen terminal
  scrollback for native applications that otherwise use the primary screen. After
  ACK, releasing the board's alternate screen while retaining raw mode gives Kiro
  the same screen choice as direct invocation. No shell resumes between views.
  Terminal wheel/PageUp input remains byte passthrough; Kiro's own alternate-screen
  history behavior and browser-specific scrolling still need visual acceptance.
- Backspace may arrive as Ctrl+Backspace/Ctrl+H in legacy terminals. The control-key
  guard previously discarded those events before editing handlers could process them.

- Profile fallback review also checked [Kiro's custom-agent documentation](https://kiro.dev/docs/cli/custom-agents/):
  it documents nested path-based names and workspace precedence. The upgrade reference
  documents the `KIRO_AGENT_CONFIG_DIR` override and legacy paths. Discovery therefore
  bounds nested traversal and supports read-only profile-file symlinks rather than
  treating a flat regular-file scan as complete. Current V2 CLI acceptance of every
  newer profile format still belongs to Kiro's resolver, not this wrapper.

## Release asset publication window (2026-10-26)

A user installing v0.4.0 hit an asset HTTP 404 after Release Please published the
release record but before the binary job attached its archive/checksum. The release
subsequently completed and latest-stable installation was verified as v0.4.0 in a
temporary prefix. The installer now retries missing assets for a bounded three minutes
per asset, while preserving any existing binary on failure. Fixtures cover delayed
assets and exhaustion; this does not weaken checksum or archive/version verification.

## Naming upgrade and actual V2 store gap (2026-10-26)

Read-only diagnostics found an installed 0.4.0 frontend connected to a still-running
0.2.0 host. Newly created sessions were therefore supervised by the old host, which
never supplied naming fields. The prior naming tests covered fresh new hosts, not
this upgrade path. Production processes were not stopped or restarted.

Managed V2 processes were kiro-cli launchers with kiro-cli-chat children. Their native
store descriptors were SQLite and there were no matching file-based PID locks, including
when checking the child's configured native root. No environment values, conversation
text, titles or credentials were printed. A file-based title reader cannot establish
SQLite conversation ownership and must not be presented as automatic naming for this
layout. Verified SQLite store support remains in the explicit next-batch backlog.

Shared persistent custom names now work with old hosts, alongside unique fallback labels
and a visible frontend version. The regression test uses a pre-naming protocol fixture
in CI and was also run against an isolated copy of the actual v0.2.0 executable with
fake Kiro and temporary HOME/Kiro/XDG/state roots. Manual rename/clear did not alter
legacy registry metadata or child PIDs, and no unsupported Rename RPC was sent.

## Private tmux workspace probes (2026-10-26)

Isolated tmux 3.4 probes found that dots in session names are silently replaced
by underscores. Workspace names therefore reject dots rather than losing identity
between create/list/reopen/close. Tmux expands formats in explicit `-c` directories;
literal hashes must be escaped there. Stored bindings use tmux's current-directory
format. Shell commands and tmux argv have different quoting layers; prequoting argv
values breaks command dispatch and directory selection.

The regression uses private temporary sockets, fake Kiro, a real attached tmux client,
and executable/state/project paths containing spaces, apostrophes, dollar signs and
hashes. It checks the actual pane process cwd through `/proc`, since formatted tmux
path metadata may escape characters. `send-keys` bypasses client prefix handling;
the PTY client is necessary to verify Ctrl+B Ctrl+B d versus workspace Ctrl+B d.
No production tmux/Kiro server, native store, authentication or model call is involved.

Shortcut follow-up: the user replaced the doubled-prefix workflow with direct Ctrl+G
detach. The reference Pi extension can check `ctx.ui.getEditorText()` before consuming
Left Arrow; the wrapper's native PTY passthrough has no equivalent editor-state signal.
The user explicitly deferred that shortcut. Left Arrow remains byte passthrough,
and Ctrl+G was initially an intentional override of Kiro's crew key. This was
subsequently withdrawn: Ctrl+Q is the current shortcut; see the executable-identity
follow-up below.

## CPU incident and verified naming ownership follow-up (2026-10-26)

See [the incident report](INCIDENT-2026-10-26-CPU.md). A bounded isolated reproduction
closed one dashboard PTY and observed a surviving frontend using 0.49 CPU seconds
in a 0.5-second interval; the harness immediately killed and reaped its own process.
Crossterm 0.29's selected mio backend retries terminal EOF/EIO inside its input loop,
preventing the application's signal flag from being checked. The implementation now
selects its error-propagating use-dev-tty backend and checks terminal HUP/ERR separately.
This reproduces the incident signature, not the provenance of every historical process.

The earlier SQLite-only inference was incorrect for the user's three live sessions.
Their lock owners were descendants along launcher → kiro-cli-chat → bun → kiro-cli-chat
chains. Immediate/main-thread children inspection missed runtime-thread launches.
Walking each native lock owner's parent chain verified ownership without guessing by
directory or reading terminal output. The launcher's open SQLite general-state database
had no conversation rows; its descriptor alone did not identify the active session store.
No auth rows, transcript text or native titles were printed. The real sessions were
stopped during containment, so the new behavior is verified with isolated delegated
fake-Kiro fixtures rather than claiming a live authenticated acceptance result.

## Workspace wheel paging (2026-10-27)

The user confirmed native scrolling works outside tmux for the same Kiro session,
while workspace wheel input cycles prior prompts. The private server previously
left mouse reporting off, allowing the outer terminal's alternate-screen scrolling
to generate Up/Down. Workspace configuration now enables mouse reporting and maps
wheel up/down to PageUp/PageDown in the hovered pane. Copy-mode wheel events remain
with tmux; direct attached input handling is unchanged. A real tmux client PTY test
sends SGR wheel events and verifies page-key bytes, no history arrows, correct pane
targeting, and copy-mode isolation. Browser/Guacamole acceptance remains manual.

The page-key forwarding fix did not resolve the user's real session. Read-only
workspace metadata showed primary-screen panes (`alternate_on=0`) with no native
mouse request (`mouse_any_flag=0`). The previous fixture only proved delivery of
PageUp/PageDown bytes; it did not prove scrolling. In this mode the scrollback
belongs to tmux, so forwarding keys into Kiro is the wrong layer. Wheel and
primary-screen PageUp now operate on tmux copy mode. Native mouse-requesting/full-screen
applications retain their input paths. The regression produces a numbered retained
history, requires positive/decreasing scroll positions, verifies the correct pane,
and checks that scrollback navigation injects no prompt keys. Browser acceptance
still requires the user's confirmation; fake byte echo alone is not acceptance.

## MobaXterm SSH wheel reproduction gap (2026-10-27)

The user confirmed that the same Kiro session scrolls correctly through direct
kiro-agents, including in MobaXterm over SSH. Workspace PageUp/PageDown work after
the copy-mode correction, while wheel scrolling remains broken. This is not a
successful authenticated/client acceptance result; stop changing live bindings
until the client input is established.

An isolated real-tmux PTY regression now checks both SGR and legacy X10 mouse
reports: both scroll retained output. Replaying ordinary CSI/SS3 Up sequences
instead reproduces the prompt-input path without entering scrollback. Tmux may
normalize the arrow encoding for the child, but cannot know whether it originated
from a wheel or a keyboard. Do not remap ordinary arrows based on guesses.

A [MobaXterm report with byte-level evidence](https://github.com/anthropics/claude-code/issues/71085)
describes this arrow substitution even while mouse reporting is enabled. This is
a candidate explanation, not a capture from the user's exact client/version.
The opt-in `python3 probes/scroll_input.py` probe compares a normal screen with
tmux-like alternate/application-cursor/mouse modes over the user's SSH connection.
It classifies only recognized control events, discards printable input, saves
no files and restores terminal modes on exit; it never accesses Kiro state. The
parser self-test and a bounded PTY cleanup/text-discard test passed. Actual
MobaXterm-client capture remains required before claiming reproduction or resolution.

User capture follow-up: wheel movement produced CSI Up/Down inside tmux and SS3
Up/Down outside, with unchanged PageUp/PageDown encodings. Both wheel results are
ordinary arrow events, matching the isolated arrow-substitution reproduction;
the CSI/SS3 difference is consistent with tmux's cursor-mode normalization. The
client version and modifier behavior remain unspecified. This does not establish
that all MobaXterm versions/settings behave the same way or that the issue is fixed.

Upstream search found a [tmux maintainer explanation of this alternate-screen interaction](https://github.com/tmux/tmux/issues/1795)
and [Mobatek's documented Shift+MouseWheel support for tmux](https://blog.mobatek.net/post/mobaxterm-new-release-8.2/).
Mobatek also documents [ScrollMod=shift](https://blog.mobatek.net/post/mobaxterm-configuration-settings/).
These support trying the client modifier before further server key remapping.
The historical mode-mouse option in the old Mobatek post is obsolete; this private
server already uses modern mouse=on. Shift+wheel acceptance for this client remains
unverified. Do not disable alternate-screen support or steal ordinary prompt arrows
as an automatic compatibility fix.

Manual acceptance follow-up: the user confirmed that Shift+MouseWheel works in the
current MobaXterm SSH workspace. PageUp/PageDown were already confirmed working.
Record Shift+wheel as a verified workaround for that setup, not a plain-wheel fix
or acceptance for every terminal/version. No additional live bindings or native
arrow-key overrides were needed. The MobaXterm version was not supplied.

## 2026-10-27 — standalone shortcut and executable identity

Stable v0.4.2 contains the Ctrl+B d attach filter. The user withdrew the feature
branch's Ctrl+G choice after encountering Kiro's native crew panel; Ctrl+Q is the
replacement single shortcut. Kiro's documented shortcuts do not list Ctrl+Q and
tmux 3.4's default bindings do not assign it. Sources:
https://kiro.dev/docs/cli/terminal-ui/ and
https://github.com/tmux/tmux/blob/3.4/key-bindings.c.

The installed executable reported 0.4.2-beta.36.1 but differed from that release's
checksum-verified executable: the installed copy forwarded Ctrl+G and detached on
Ctrl+B d; the published beta intercepted Ctrl+G. A version string alone does not
establish executable identity. No installed binary or running user host was changed
by this investigation.

Attach filtering belongs to the invoking frontend, independently of host version.
A fake-Kiro PTY regression accepts an optional older host binary to verify this
boundary, forwards native Ctrl+G and Ctrl+B d unchanged, checks IXON is disabled,
and detaches with Ctrl+Q while preserving the Kiro PID:
`python3 tests/terminal_smoke.py target/debug/kiro-agents /path/to/v0.4.2/kiro-agents`.
Already running frontends retain their executable until exit; installer replacement
applies to subsequent launches. SSH-client shortcut acceptance remains manual.

## 2026-10-27 — Recursive directory search and daily cache

Inspection of the previous picker showed that fuzzy ranking was applied only to
up to 200 immediate children and siblings, then reduced to 40 rows. This could not
find deeply nested projects or present the complete directory list. The local
`fdh` reference enumerates directories recursively under `$HOME` before fuzzy
selection. Discovery now follows that scope in a background worker, with an
additional cwd root outside home, and retains all discovered matches. Explicit
paths and current/recent directory choices remain available immediately.

A private, atomically replaced JSON cache records only directory paths and scan
metadata, keyed by roots, with a rolling 24-hour TTL. Fresh lists are reused across
frontend launches; expired lists are shown while a replacement is scanned. Ctrl+R
bypasses the cache. Hidden/dependency/build trees and symlinks are excluded; scans
have explicit directory, entry and elapsed-time bounds. This is a read-only
filesystem enumeration, with no Kiro/session-file access or external fd/fzf
requirement. Model and real-PTY regressions cover deep matching, lists beyond the
old 200/40 limits, fresh-cache reuse, forced refresh, daily expiry, corrupt-cache
recovery, and private permissions.

## 2026-10-28 — Shared registry and cooperative attachment moves

Standalone and workspace frontends already address the same host registry via
`KIRO_AGENTS_HOME`. The observed “already attached” error represents exclusive
terminal ownership, not separate session stores. Native historical import remains
out of scope. An explicit frontend-to-frontend release followed by ordinary Attach
moves the live terminal connection without changing native identity, host RPC,
generation, or Kiro-owned files. PTY regressions cover consent/cancellation,
round-trip movement with the same child PID, registration cleanup, refusal of a
noninteractive destination, and an unregistered older frontend retaining its
connection. Compatibility is also tested with a published v0.4.2 host.
## 2026-10-28 — Lifecycle/resume capability investigation

The installed 2.32.0 chat help advertises --resume-id, --resume-picker,
--list-sessions, --all-cwds, JSON formatting, --v2 and --v3. Empty inventory calls
for both engines, under independent temporary HOME/KIRO_HOME/KIRO_SESSIONS_DIR
and XDG roots with stdin closed, did not return JSON within 20 seconds. Their
owned process groups were killed and reaped; no real authentication or session
state was copied, and no model prompt was sent. These results establish neither
an authenticated listing schema nor a specific cause for the timeout.

Native metadata has session_state, but its schema version alone is not an engine
identifier or a live-turn signal. The existing wrapper's live/stopped metadata
cannot support honest Working/Ready/Needs-you labels or exact stopped-session
resume on its own. The Pi reference only unloads hidden settled sessions and
captures editor drafts through its SDK before disposal. Equivalent draft safety
must be verified for Kiro before automatic idle stopping. The new batch contract
and remaining verification gates are in journal/SESSION-LIFECYCLE.md.
