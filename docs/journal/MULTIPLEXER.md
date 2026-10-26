# Optional tmux workspaces

Base: main after PR #14 (dashboard/naming/scrolling and one-line installer).
Feature branch: feat/tmux-workspaces. Runtime dependency: external tmux 3.2+;
ordinary session board and binary installation remain usable without tmux.

## Milestones

- [x] Merge the verified installer and dashboard branch into main.
- [x] Create the feature branch from updated main; define the workspace trait/CLI.
- [ ] Implement private-server workspace creation, listing, switching and explicit close.
- [ ] Implement left/right and top/bottom session-picker panes, shell panes and windows.
- [ ] Preserve standard focus, resize, zoom, close, copy-mode and detach controls.
- [ ] Verify fake Kiro sessions survive pane closure and workspace detachment.
- [ ] Verify feature disable, foreign-server isolation and safe command quoting.
- [ ] Test packaged binaries in branch CI and publish a beta for manual acceptance.

Pi swarm work split (Strata Qwen 3.8): tmux-backend owns src/workspace/mod.rs;
tmux-integration owns tests/workspace.py. Coordinator owns CLI/feature assembly,
CI dependencies, documentation and final integration. Worker results trigger review;
do not repeatedly poll idle workers or duplicate their implementation.

## Boundaries

Tmux owns pane terminal rendering, scrollback and shell processes. The application
continues to forward native Kiro bytes without terminal emulation or output scraping.
The existing host owns Kiro processes independently of pane/frontend lifetime. Closing
a Kiro pane detaches its frontend, not the Kiro session. Closing a terminal pane has
normal tmux shell termination semantics; closing a workspace is an explicit command.

Workspaces use their own private socket and configuration under wrapper state. Never
modify the user's default tmux server, ~/.tmux.conf, Kiro configuration, or foreign
processes. The workspace adapter is assembled through a feature trait; the session host
imports no concrete tmux backend and allocates no workspace resources to clean up.
Tmux layout persistence is intentional until the workspace is explicitly closed;
startup temporary files must be removed and the final pane exit lets tmux end its server.

Next batch (planning only): see [backlog](../BACKLOG.md) for verified V2/V3 native
store resume and fuzzy/full-text conversation quick-open. Keep that work out of the
initial multiplexer implementation.
