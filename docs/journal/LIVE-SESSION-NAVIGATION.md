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
