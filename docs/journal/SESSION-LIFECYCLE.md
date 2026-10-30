# Session lifecycle and historical resume

Requested 2026-10-28 as a new batch, separate from tmux workspaces. This is an
implementation contract and verification journal, not a shipped feature claim.

## Dashboard contract

Use five selectable filters, in this order: All, Needs you, Working, Ready,
Inactive. Preserve named-session selection and fuzzy search across refreshes.

| Filter | Meaning |
| --- | --- |
| All | Every known managed or supported historical session |
| Needs you | Verified user-input/approval wait, with the reason displayed |
| Working | A verified active turn or tool operation |
| Ready | A verified completed turn, still running and available |
| Inactive | No live process: explicitly stopped, safely unloaded, or saved history |

Process liveness and turn activity are separate fields. A legacy host or missing
telemetry produces an explicit unknown-activity badge in All; it must never be
silently presented as Ready. Failures retain an error badge and recovery reason.
A PreToolUse event alone does not prove an approval is waiting. Silence, terminal
bytes, and transcript keywords cannot establish a completed turn or editor state.

Enter attaches/focuses a live session, offers the existing explicit Move here
choice when appropriate, and resumes a supported inactive conversation. The row
shows an exact reason when resume is unavailable. Native generation and store
format are separate facts; unknown schemas/generations remain unsupported.

## Inactivity policy

Proposed configuration (not yet implemented):

```json
{
  "autoInactive": true,
  "inactiveAfter": "1h"
}
```

Accept configurable durations and an off setting. One hour follows the Pi
reference default. The policy runs in the lifecycle owner, not on the UI input
path, and applies only when every condition is verified:

- The process is owned, detached from every frontend, and Ready.
- No running turn/tool, approval, queued operation, handoff, or unknown activity.
- The native conversation ID, original cwd, agent and generation are recorded.
- A saved, supported native history exists and can be resumed on the same engine.
- Unsent drafts are preserved through a verified native mechanism; unknown draft
  safety prevents automatic unloading. Pi can read its editor through its SDK;
  the raw Kiro passthrough cannot assume that capability.
- The configured idle deadline has passed. Repainting and output silence do not
  define activity, and a failed status probe must not advance eligibility.

Explicit stop and automatic inactivity remain distinguishable metadata. Failed
unload preserves a visible error. Never evict a working, waiting, attached,
unresumable or uncertain session simply to meet an inactivity timeout.

## Resume policy

Capture exact native identity before stopping, not after its lock is gone. Use
verified read-only store adapters and native same-generation resume arguments;
never guess from newest cwd, display name, file extension or schema version.
Recheck native/owned locks at launch, reject foreign owners and concurrent resume,
and preserve native errors. Explicit user resume does not authorize prompt/tool
replay or automatic crash recovery. Old wrapper-only history without native IDs
is visibly unlinked; it needs explicit native-history selection rather than a
fabricated association. Native V2/V3 histories require independently verified
compatibility. No silent engine switch or profile rewriting.

## Milestones

- [x] Inspect the Pi inactivity guard: hidden, Ready, no pending operations,
  runtime idle, draft capture before disposal; default timeout one hour.
- [x] Confirm current wrapper tracks process liveness, not reliable turn activity,
  and does not persist native conversation IDs for stopped-session resume.
- [x] Probe installed CLI help in isolated HOME/Kiro/XDG roots: Kiro 2.32.0 lists
  --resume-id, --resume-picker, --list-sessions, --all-cwds, --format, --v2, --v3.
- [x] Probe empty isolated V2/V3 JSON inventories with no model calls: both fail to
  return parseable JSON within 20 seconds. Owned process groups are terminated
  and reaped. This does not establish an inventory schema or an auth diagnosis.
- [x] Inspect only metadata field names: session_state.version is not by itself
  proof of engine generation or live activity. No transcript text is logged.
- [ ] Obtain sanitized, authenticated scratch-session fixtures and verify native
  identity, generation, resume legality, status signals and draft safety.
- [ ] Implement bounded read-only historical inventory with separate adapters,
  metadata-only persistence, unknown-format handling and cancellation.
- [ ] Add trustworthy activity events behind the optional feature boundary; any
  native telemetry hook installation is explicitly opt-in.
- [x] Add the five filters and frontend-only compatibility for existing hosts,
  with explicit unknown activity and unavailable-resume reasons.
- [x] Encode the unload and resume safety guards as pure, unit-tested decisions with
  no process, file, argv or telemetry side effects (`src/lifecycle/policy.rs`).
- [ ] Connect verified native activity/reason events and last-activity metadata.
- [ ] Add explicit generation-safe Resume and configurable safe inactivity.
- [ ] Exercise stop/resume, expired idle deadlines, working/approval/draft guards,
  duplicate/foreign locks, crashes, unsupported stores and old-peer compatibility
  through fake-Kiro Rust/PTY tests; run checks/audit and publish a branch beta.
- [ ] Complete isolated authenticated manual acceptance before claiming native
  V2/V3 resume or turn-status parity.

Native full-text quick-open remains a separate backlog item.

## References

- [Kiro session management](https://kiro.dev/docs/cli/chat/session-management/)
- [Kiro lifecycle hook types](https://kiro.dev/docs/hooks/types/)
- Local Pi reference: src/core/sessions.ts deactivateIdle and src/config.ts.

The advertised flags and documentation are discovery inputs, not substitutes for
verified native generation/lock behavior or authenticated lifecycle acceptance.

## 2026-10-29 — First implementation milestone

Branched from main after PR #27. The dashboard now defaults to All and cycles
All / Needs you / Working / Ready / Inactive with Tab and Shift+Tab. Process
liveness is independent of an additive, optional activity field; missing or
unrecognized values become Unknown. Current hosts provide no turn telemetry and
therefore show “live · activity unknown” in All, never Ready. Inactive includes
stopped, exited and failed managed rows, preserving their error/exit metadata.
Search and named selection survive refresh and filter changes where still visible.

This milestone does not enable native history import/resume, hooks, automatic
inactivity, last-activity clocks, or V3 launches. It does not authorize automatic
stopping. Native identity, generation provenance, real status events and draft
safety remain the next verification gate. Unknown activity is omitted when
serializing, preserving the existing old-host wire shape; no protocol bump or
host restart is required for the frontend filters. Synthetic activity fixtures
exercise UI classification, not claims about real Kiro telemetry.
## 2026-10-29 — Verification gates and pure eligibility gates

This milestone records what the current code actually proves, what still blocks
native identity/activity/resume claims, and the bounded plan to close each gap.
It also adds `src/lifecycle/policy.rs`: pure eligibility decisions with unit tests.
Nothing in this milestone launches, stops, signals or resumes anything.

### Verified code facts (read from this branch, no probes needed)

| Fact | Location | Consequence |
| --- | --- | --- |
| A wrapper row has no native conversation ID. `SessionInfo` carries the wrapper id, cwd, agent, generation string, pid, state, exit code, notice, title fields. | `src/protocol.rs` | Exact stopped-session resume is impossible today: after the child exits nothing links the row to a native conversation. |
| The wrapper id is a random UUID prefix, never a Kiro id. | `src/host/mod.rs` `new_session` | Wrapper ids must not be used to select native history. |
| Launch argv is fixed to `kiro-cli chat --v2 --tui [--agent p]`; no `--resume-id` path exists. | `src/kiro/mod.rs` `launch` | Any resume capability needs a new, separately verified argv builder in `src/kiro/`. |
| `generation` is written as the literal `"v2"` at creation and is never derived. | `src/host/mod.rs` | Generation provenance is honest but only covers wrapper-launched sessions, never imported history. |
| `activity` is only ever the default `Unknown`; no code path sets it. | `src/host/mod.rs` (one `Default::default()`), no other `Activity::` producer | Working/Ready/Needs-you labels have no real source. Synthetic fixtures classify the UI only. |
| There is no last-activity timestamp in the registry or protocol. | `src/protocol.rs` | Idle deadlines cannot be evaluated; a clock would be new persisted metadata. |
| Naming already performs a verified native identity lookup and then throws the identity away. `NativeNames::lookup` accepts a `.lock` only when its pid is a descendant of our owned root on a bounded, start-time-checked parent chain, the sibling `.json` `session_id` equals the filename stem, its `cwd` equals the row cwd, and exactly one candidate exists. The stem is the native conversation id; only the title is returned. | `src/kiro/titles.rs` | The hard part of identity capture (ownership proof, not cwd recency) already exists and is the right seed for exact identity persistence. |
| That lookup is live-only and file-based: it returns early unless `state == "live"`, needs the live pid's start time, and scans only `<sessions-dir>/*.lock`. | `src/kiro/titles.rs` | It cannot describe an inactive or SQLite-only conversation; those stay unsupported rather than guessed. |
| Optional features are one read-only trait method (`title`), assembled in `lib.rs` behind `KIRO_AGENTS_AUTO_NAME`; the host holds only the trait object. | `src/features/mod.rs`, `src/lib.rs` | Activity metadata needs a trait addition plus assembly, i.e. edits outside this task's write paths. |
| Stopping reaps the child and writes `state: "stopped"`; nothing is captured first. | `src/host/mod.rs` `stop` | Identity must be captured before stop, while the native lock still exists. |
| Registry persistence is additive: new optional fields with `#[serde(default)]` keep old rows readable, and unknown activity is omitted on the wire. | `src/protocol.rs`, `tests/lifecycle.rs` | Identity/clock metadata needs no protocol bump or host restart. |
| Inactive rows only explain non-attach; there is no resume affordance. | `src/board/view.rs`, `src/board/mod.rs` | Resume is a new affordance, not a hidden one. |

### What is blocked, and what unblocks it

| Blocker | Status | Actionable next step |
| --- | --- | --- |
| Native identity for stopped rows | Not implemented; no field, no capture | Add optional `native_id`/`native_generation`/`captured_at` metadata to `SessionInfo` (defaulted, wire-compatible), then have the existing ownership proof in `titles.rs` report the id it already verified. Needs coordinator approval: touches `src/protocol.rs`, `src/features/mod.rs`, `src/host/mod.rs`. |
| Which store a live V2 session really uses | Contradictory evidence: SQLite descriptors with no file lock in one layout; verified `.lock` owners further down launcher chains in another | Read-only inventory probe per layout in isolated roots. Until then a row is either "verified file lock" or "unsupported", never inferred. |
| V3 store layout and generation provenance | Unverified | No V3 adapter, no `--v3` launch, until a sanitized authenticated fixture exists. |
| `--resume-id` behavior on both engines (clean failure versus silent replacement session) | Unverified | Isolated scratch probe with an authenticated user-controlled session; record exit code and message. Never in CI. |
| Live native lock contract for resume (who writes it, when it is stale) | Unverified for V3, partially verified for V2 file locks | Keep the existing start-time-checked chain as the only ownership test; treat any unrecognized lock state as blocking. |
| Trustworthy turn activity (which native event fires, its exact payload) | Unverified; no hook installed | Behind a new opt-in feature with a config toggle, cleanup on host close, enabled/disabled tests and docs. A `PreToolUse` event alone must not become "needs you". |
| Draft safety before unload | Unverified: raw passthrough cannot read the native editor | Automatic unload stays off until a verified native mechanism reports or preserves drafts. |
| Full-text/session listing schema (`--list-sessions`, `--resume-picker`) | Unverified: empty isolated calls did not return JSON inside 20 seconds | Separate bounded probe with cancellation and process-group reaping. Out of this task. |

### Bounded plan, in delivery order

1. Identity capture while live (read-only, additive metadata), reusing the ownership
   proof that already exists. No new store reads, no argv change.
2. Freeze that identity at stop into the row, so an inactive row can name its exact
   native conversation and generation, or say explicitly that it cannot.
3. Bounded read-only inventory behind a feature toggle, one adapter per verified
   layout, unknown schemas reported as unsupported. No transcripts in registry or logs.
4. Activity events only from a verified native event source, again behind a toggle,
   with explicit `Unknown` retained as the default.
5. Last-activity metadata and the configurable idle deadline, evaluated in the
   lifecycle owner, never on the input path.
6. Explicit generation-safe resume affordance: identity + matching generation +
   verified supported history + no live native lock, then Kiro's own argv and errors.
7. Fake-Kiro Rust/PTY regressions for each step, then a branch beta.

Each step is separately reviewable and each is gated on its own verification.

### Eligibility gates added in this milestone

`src/lifecycle/policy.rs` is a pure decision layer. It reads no files, signals no
process, launches nothing and is not consulted by the host: the only wiring is the
`pub mod policy;` declaration. It encodes the journal's guards as data, with unknown
and inferred states failing closed:

- `unload_decision` returns the first blocking reason in safety order. Automatic
  unload requires: the toggle enabled, an owned live process, no attached frontend, no
  handoff, no queued operation, no running turn/tool, no pending approval, a verified
  native `Ready` event (not silence, terminal bytes or keywords, and never `Unknown`),
  no failed status probe, a trustworthy idle clock past the deadline, verified native
  identity, verified supported saved history, and verified draft safety. A default
  (half-observed) row is never eligible.
- `resume_gate` returns the one legal engine, or the first blocking reason. It requires
  an explicit user request (host recovery never resumes), no already-live managed child,
  verified identity with a non-empty native id, original cwd and recorded generation, a
  matching verified supported history generation, and a verified free native lock.
  Foreign lock owners, unverified locks, cross-generation combinations and any missing
  identity are refused. It returns a legality answer only: argv construction stays in
  `src/kiro/`.
- `NativeIdentity` distinguishes `Verified`, `WrapperOnly` and `Unknown` provenance.
  Wrapper metadata alone never links to native history.
- `parse_idle_duration` accepts strict single-unit durations (`45s`, `30m`, `1h`), with
  `off`, zero, malformed and over-30-day values rejected rather than defaulted.
- `Deactivation` keeps explicit stop and idle unload as different labels.

Tests (14 in-module cases, run by `cargo test`) cover: the single fully verified
eligible case; disabled policy; unknown, inferred, Working and Needs-you activity never
qualifying as Ready; attached/handoff/queued/running/approval states; failed probe and
missing clock not advancing eligibility; deadline not yet passed; unverified identity,
history and draft safety; host-recovery resume refusal; already-live child resume
refusal; guessed identity/history/lock refusals; cross-generation refusal in both
directions; strict duration parsing; distinct stop and idle labels.

These tests prove the decision logic only. They use no native fixtures and therefore
establish no fact about real Kiro identity, activity, history or draft behavior.

Coordinator review added an unload guard requiring saved-history generation to
match the verified native identity, with a regression for both mismatch directions.
Focused policy tests (14) and strict all-target/all-feature Clippy passed.
