# Live session navigation workstream

Requested 2026-10-29. Branch: feat/live-session-navigation, created from main after
the v0.6.0 release PR merge. This worktree contains no lifecycle draft changes.

## Requested interaction

- Ctrl+J focuses the next live session; Ctrl+K focuses the previous one.
- Preserve the current layout when not zoomed. If the originating pane is zoomed,
  focus the destination session fullscreen too.
- Let the user read output, respond to native approvals and move on without
  reopening the dashboard. Navigation must not inject input into Kiro.
- Keep Enter and native approval handling unchanged. Scope tmux bindings to the
  private workspace server; standalone attach remains byte passthrough.

## Milestones

- [x] Create isolated worktree from latest main, separate from lifecycle.
- [ ] Define deterministic session order and scope (attached session panes across
  owned workspaces versus unattached live sessions); no silent replacement of a pane.
- [ ] Verify Ctrl+J/Ctrl+K encoding/conflicts in native Kiro and terminal clients.
  Ctrl+J is LF, so preserve native Enter (CR) and document any lost native binding.
- [ ] Add navigation through WorkspaceFeature, preserving frontend ownership
  validation, exclusive attachment and native process identity.
- [ ] Preserve zoom when switching panes/windows; skip shells, empty launchers,
  stale registrations and foreign workspaces. Handle a session closing mid-switch.
- [ ] Exercise real attached-PTY keys, wraparound, one/no live pane, zoom and
  nonzoom paths, cross-window scope, native approval/Enter byte passthrough,
  process stability, feature-off and private config overrides.
- [ ] Run make check, audit and extracted-beta integration before review.

Do not merge lifecycle changes into this workstream or restart live hosts.

## Implementation contract, 2026-10-29

The worker selected `KIRO_AGENTS_WORKSPACE_NAV=on|off` (default on), hidden
`__workspace-nav next|previous`, and `WorkspaceFeature::step_live_session`.
Cycle existing live owned session panes across the private server's workspaces,
sorted by workspace name, window index, pane index and pane id. Unattached live
sessions remain in the dashboard; cycling never creates or replaces a pane.
Private tmux overrides load after default bindings. Native Enter (CR) must remain
unchanged; Ctrl+J (LF) and Ctrl+K are consumed only in managed session panes.

Pi's tool guard blocks private socket test invocations; the worker must not bypass
that guard. Coordinator owns real attached-client PTY verification. Added cases
for next/previous wraparound, cross-window focus, repeated fullscreen preservation,
dashboard Ctrl+K handling, navigation off/re-enable and native Enter passthrough.
These tests are pending the worker's implementation, not yet passing evidence.

## Coordinator integration

Real attached-PTY verification passed for next/previous wraparound across windows,
zoom propagation through a single-pane window, default popup origin selection,
popup Ctrl+K stop/cancel, navigation disable/re-enable, and native Enter passthrough.
Review corrected shell-command quoting, keyboard fallback (not mouse forwarding),
PTY client-name validation, and host-status validation for live candidates. A
private pane marker retains fullscreen intent through a single-pane window;
ordinary split panes use tmux's actual zoom flag. No host or Kiro process restarts.
Full make check and RustSec audit are required before publication.

Final validation: full CARGO_BUILD_JOBS=2 make check and cargo audit --deny warnings
passed locally. The shared workspace_dashboard.py regression also covers shell
Ctrl+J/Ctrl+K passthrough, invalid-toggle rejection without binding mutations and
private-config precedence, and is already part of extracted-beta integration.
