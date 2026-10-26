# Optional tmux workspaces

Base: main after PR #14 (dashboard/naming/scrolling and one-line installer).
Feature branch: feat/tmux-workspaces. Runtime dependency: external tmux 3.2+;
ordinary session board and binary installation remain usable without tmux.

## Milestones

- [x] Merge the verified installer and dashboard branch into main.
- [x] Create the feature branch from updated main; define the workspace trait/CLI.
- [x] Implement private-server workspace creation, listing, switching and explicit close.
- [x] Implement left/right and top/bottom session-picker panes, shell panes and windows.
- [x] Preserve standard focus, resize, zoom, close, copy-mode and detach controls.
- [x] Verify fake Kiro sessions survive pane closure and workspace detachment.
- [x] Verify feature disable, foreign-server isolation and safe command quoting.
- [x] Add workspace integration to branch CI and extracted optimized beta checks.
- [ ] Complete manual authenticated Kiro/Guacamole acceptance using the branch beta.

Pi swarm work split (Strata Qwen 3.8): tmux-backend owns src/workspace/mod.rs;
tmux-integration owns tests/workspace.py. Coordinator owns CLI/feature assembly,
CI dependencies, documentation and final integration. Worker results trigger review;
do not repeatedly poll idle workers or duplicate their implementation.

Both Qwen workers returned design/source drafts without authoring or running their
assigned files: their Pi tool guards rejected tmux socket commands even inside file
content. Guards were left intact. The coordinator reviewed and applied the backend,
corrected argv/shell quoting and runtime issues, authored the integration harness,
and ran the private-server checks with its authorized tools. Worker drafts alone
were not treated as validation. The swarm is closed and temporary worker credentials
removed.

Coordinator validation: `make check` passed (80 Rust tests, ten fake-Kiro PTY
scenarios and the installer/packaging/publisher tests); strict `make audit` and
actionlint passed. Workspace coverage includes two independently attached sessions,
actual prefix controls, explicit directory selection with shell/format metacharacters,
owned-client switching, missing tmux/disabled/non-TTY failures, foreign-server refusal,
and unchanged managed PIDs after closing panes, detaching and closing workspaces.

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

Shortcut follow-up: user selected a direct Ctrl+G return to the board so tmux can
retain Ctrl+B d for workspace detach. Empty-editor Left Arrow was explicitly deferred;
Pi has getEditorText(), but this native passthrough does not have an equivalent input
state signal. Arrow bytes stay unchanged. Ctrl+G now supersedes Kiro's crew shortcut
while attached.
