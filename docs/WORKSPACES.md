# Tmux workspaces (feature branch)

Workspace mode is optional and requires external tmux 3.2+. The normal session board
is unchanged; Kiro processes remain owned by the existing session host. Workspaces
use a dedicated tmux socket/configuration under KIRO_AGENTS_HOME/workspaces and never
modify your default tmux server or ~/.tmux.conf. Set KIRO_AGENTS_WORKSPACES=off to disable
workspace commands; setting it on (default) makes explicit workspace commands available.

```sh
kiro-agents workspace                         # open/create main workspace
kiro-agents workspace open project --dir /path/to/project
kiro-agents workspace open project --detach   # create without attaching a client
kiro-agents workspace list
kiro-agents workspace close project           # explicit: closes its shell panes too
```

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

Tmux owns the Ctrl+B prefix. To send the application's existing Ctrl+B d detach
sequence through tmux and return one Kiro pane to its board, press Ctrl+B Ctrl+B d.
The first doubled prefix passes a literal Ctrl+B to the application; workspace detach
is the ordinary Ctrl+B d. Zoom only changes presentation, never session ownership.

Closing a Kiro pane closes the attached frontend while its managed Kiro process stays
alive. Closing a shell pane terminates that pane's shell as tmux normally does. Closing
a workspace never calls the Kiro host shutdown command. To stop a managed Kiro session,
use the existing board stop action or kiro-agents stop explicitly.

The feature is still under implementation on its branch. Unit/behavioral tests must
use private temporary sockets, fake Kiro and temporary HOME/XDG/Kiro paths; no paid calls.
