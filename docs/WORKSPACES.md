# Tmux workspaces (feature branch)

Workspace mode is optional and requires external tmux 3.2+. The normal session board
is unchanged; Kiro processes remain owned by the existing session host. Workspaces
use a dedicated tmux socket under KIRO_AGENTS_HOME/workspaces, start with an empty
configuration, and install their own bindings in that private server. They never
modify your default tmux server or ~/.tmux.conf. Set KIRO_AGENTS_WORKSPACES=off to disable
workspace commands; setting it on (default) makes explicit workspace commands available.

```sh
kiro-agents workspace                         # open/create main workspace
kiro-agents workspace open project --dir /path/to/project
kiro-agents workspace open project --detach   # create without attaching a client
kiro-agents workspace list
kiro-agents workspace close project           # explicit: closes its shell panes too
```

Names start with a letter or digit and contain up to 64 ASCII letters, digits,
hyphens or underscores. Dots are rejected because tmux silently rewrites them.
Install tmux using your system package manager if it is missing; the binary
installer does not install system dependencies. `KIRO_AGENTS_TMUX_BIN` can select
an explicit tmux executable. Workspace sockets need a short absolute state path
(at most 100 bytes including `/workspaces/tmux.sock`); errors explain the limit.

Each new Kiro pane opens a board to choose or create its own session. A managed Kiro
session can have one attached frontend; an existing attachment is never stolen.
To mix in plain shells, use the terminal shortcuts or these commands from a pane in
an owned workspace:

```sh
kiro-agents workspace split --horizontal --dir .
kiro-agents workspace split --vertical --terminal --dir .
kiro-agents workspace window --terminal --dir .
```

Press Ctrl+B, release it, then the following key:

| Key | Action |
| --- | --- |
| % | Split left/right; new pane opens the Kiro board |
| " | Split top/bottom; new pane opens the Kiro board |
| Arrow | Focus a pane |
| Ctrl+Arrow | Resize a pane |
| z | Zoom/unzoom the selected pane |
| x | Confirm closing a pane |
| c | New window with a Kiro board |
| n / p | Next/previous window |
| [ | Tmux scrollback/copy mode |
| d | Detach this workspace |
| T | New plain shell pane |
| C | New plain shell window |

Press **Ctrl+G** to detach Kiro and return that pane to its dashboard. Press
**Ctrl+B then d** to detach the entire workspace. While attached, Ctrl+G is reserved
for dashboard navigation instead of Kiro's native crew monitor. Left Arrow passes
through unchanged, including when the native editor is empty. Zoom only changes
presentation, never session ownership.

Mouse wheel up/down scrolls the hovered pane's retained terminal output. For
primary-screen Kiro, PageUp enters tmux scrollback and PageDown moves toward the
live view; q exits scrollback. These keys are not injected into the prompt.
Applications that request mouse reporting receive their native mouse events;
full-screen alternate-screen applications retain native PageUp/PageDown. Ctrl+B [
also opens tmux scrollback, including for plain shell panes. Direct non-workspace
sessions retain their native wheel/PageUp passthrough.

Closing a Kiro pane closes the attached frontend while its managed Kiro process stays
alive. Closing a shell pane terminates that pane's shell as tmux normally does. Closing
a workspace never calls the Kiro host shutdown command. To stop a managed Kiro session,
use the existing board stop action or kiro-agents stop explicitly.

This feature is available on its branch for beta testing. Existing managed sessions
remain in the same host and registry; select them in any pane without migrating state
or restarting the host. Detach an existing frontend before attaching that session in
another pane. Workspaces do not resume stopped historical sessions.

Unit/behavioral tests use private temporary sockets, fake Kiro and temporary
HOME/XDG/Kiro paths. A real tmux client in a PTY exercises split/window/shell bindings,
focus, zoom, close confirmation, Ctrl+G inner detach and workspace detach.
The same suite runs against the extracted optimized beta binary in branch CI.
Authenticated Kiro and browser/Guacamole visual acceptance remain manual checks.

MobaXterm SSH wheel compatibility remains unconfirmed: the user reports working
direct-session scrolling and working workspace PageUp/PageDown, but broken wheel
input. SGR and legacy mouse-report tests pass; some clients may substitute ordinary
arrow keys instead of mouse events. Those are indistinguishable from intentional
prompt-history keys, so the application must not guess and intercept them. See
the MobaXterm investigation in [findings](FINDINGS.md) and the optional input probe.
