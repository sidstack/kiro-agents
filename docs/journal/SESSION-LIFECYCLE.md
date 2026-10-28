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
- [ ] Add the five filters, status reasons, last activity and frontend-only
  compatibility behavior for existing hosts.
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
