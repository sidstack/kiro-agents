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

Shortcut follow-up: Ctrl+Q alone opens the dashboard. Preserve Kiro's Ctrl+G crew
monitor and tmux's Ctrl+B prefix. Left Arrow passes through without inferring an
empty editor from byte traffic. The earlier Ctrl+G choice was withdrawn.

Incident/naming follow-up: corrected closed-PTY input spinning and verified that
closed tmux panes leave no live frontends. A bounded lifecycle/CPU regression also
checks attached terminal loss, intentional detached-host idleness and host EOF
without replay. Naming now verifies the full native lock owner's parent chain,
covering runtime-thread launchers; the prior SQLite-only inference was incorrect
for the observed installation. Local `make check` passed (81 Rust tests, eleven
PTY scenarios and packaging tests), as did strict `make audit`. See the incident
report and findings; real authenticated/browser acceptance remains manual.

Wheel follow-up: enable mouse reporting on the private server and translate wheel
up/down to PageUp/PageDown for the hovered pane, preserving tmux copy mode. The PTY
regression sends real SGR mouse input and rejects history-arrow bytes and delivery
to the wrong pane. Direct sessions retain native wheel passthrough.

Scrolling correction: user testing rejected the key-forwarding approach. Primary
screen wheel/PageUp now scroll tmux's retained buffer; PageDown returns toward live
output and q exits copy mode. Native mouse/full-screen page handling remains native.
Regression acceptance now requires actual viewport movement with numbered history,
rather than echoed escape bytes. The existing private server received only the new
scroll bindings; pane processes were preserved.

MobaXterm SSH acceptance: user confirmed PageUp/PageDown and then Shift+MouseWheel
scrolling work. Ordinary wheel events arrive as prompt-history arrows; keep native
Up/Down unchanged and document the verified Shift modifier. This is a client-specific
workaround, not a claim that ordinary wheel scrolling is fixed for every terminal.

## Floating dashboard and single-chord navigation

- Ctrl+Q opens a centered popup without detaching Kiro; Escape/Ctrl+Q dismiss it.
- Enter focuses a mapped session's existing pane/window; v/h/w place an inactive
  live session right, below or in a new window. Ctrl+N selects placement before
  creating a child. Only the initial empty launcher slot may be reused.
- Frontends register pane ownership only after host attach ACK. PID start time,
  executable inode and foreground process ancestry reject stale mappings. Closing
  a frontend clears only its own registration; externally attached sessions are
  never stolen. Older frontends need one explicit frontend reattach, not a host restart.
- Private server setup removes the superseded Ctrl+G wrapper binding and preserves
  native Kiro Ctrl+G and tmux Ctrl+B. No user tmux server/configuration is modified.
- Real-client PTY coverage verifies popup placement, no extra dashboard tile,
  cross-window focus with stable Kiro PIDs, and legacy shortcut cleanup. The broad
  workspace suite retains viewport scrolling, native mouse/page/arrow passthrough,
  shells, prefix keys, zoom/resize, frontend exit and foreign-server isolation.
