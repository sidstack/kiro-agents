# Dashboard usability follow-up

## Milestone 1: usable selection

- [x] Normalize legacy BS/Ctrl+H and DEL before control shortcuts.
- [x] Rank fuzzy directory and agent queries; Enter selects the best result.
- [x] Discover scoped profiles without relying on unsupported CLI JSON output.

## Milestone 2: identifiable sessions and native scrolling

- [x] Add metadata-only names, F2 editing and CLI rename/clear.
- [x] Match native Kiro titles by owned PID, cwd, identity and lock freshness.
- [x] Offer a bounded first-Prompt title fallback with an auto/kiro/off toggle.
- [x] Release the board alternate screen after ACK and forward wheel/PageUp bytes.

## Milestone 3: verification and branch preview

- [x] Run unit, snapshot, lifecycle and PTY regressions on temporary fake Kiro state.
- [ ] Test optimized archive binaries through every PTY script in branch CI.
- [ ] Publish a beta for user acceptance; keep native browser/TUI scrolling visual
      acceptance distinct from byte forwarding and handoff checks.

Two Pi swarm workers using Strata Qwen 3.8 own fuzzy picker tests and agent discovery.
The coordinator owns naming, native screen handoff, final integration and review.

Local evidence: 65 library tests, four framing tests, four lifecycle tests, eight
fake-Kiro PTY scripts, and 22 Python packaging/provenance tests passed. Strict Clippy,
format checks, Actionlint and RustSec audit passed. Native Guacamole scrolling and
real authenticated profile acceptance remain user visual checks; no paid Kiro calls
or real Kiro state writes were used for validation.
