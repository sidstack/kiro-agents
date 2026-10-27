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

Ctrl+Q opens a centered floating dashboard over the current pane. Esc or Ctrl+Q
closes it without detaching Kiro. Enter on a session already attached in this
private workspace server focuses its pane/window. An inactive live session opens
a placement chooser: v splits right, h splits below, and w opens a new window.
Ctrl+N chooses directory and agent, then placement before starting a session.
The first session fills the empty launcher slot; later sessions never replace an
active Kiro or shell pane. An external attachment is never stolen.
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
| % | Open dashboard with right-split placement |
| " | Open dashboard with below-split placement |
| Arrow | Focus a pane |
| Ctrl+Arrow | Resize a pane |
| z | Zoom/unzoom the selected pane |
| x | Confirm closing a pane |
| c | Open dashboard with new-window placement |
| n / p | Next/previous window |
| [ | Tmux scrollback/copy mode |
| d | Detach this workspace |
| T | New plain shell pane |
| C | New plain shell window |

Press **Ctrl+Q** alone to open the dashboard. Press **Ctrl+B then d** to detach
the entire workspace. Kiro's native Ctrl+G crew monitor and Left Arrow pass
through unchanged. Zoom only changes presentation, never session ownership.

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
or restarting the host. Sessions attached by older frontends have no pane-navigation metadata. Detach
that frontend once and reattach using the updated binary; no host restart is
required. Existing mapped sessions can be selected directly in the popup. Workspaces do not resume stopped historical sessions.

Unit/behavioral tests use private temporary sockets, fake Kiro and temporary
HOME/XDG/Kiro paths. A real tmux client in a PTY exercises split/window/shell bindings,
focus, zoom, close confirmation, Ctrl+Q popup navigation and workspace detach.
The same suite runs against the extracted optimized beta binary in branch CI.
Authenticated Kiro and browser/Guacamole visual acceptance remain manual checks.

MobaXterm SSH wheel compatibility is not resolved: the user reports working direct
scrolling and workspace PageUp/PageDown, but wheel input arrives as CSI arrows
inside tmux and SS3 arrows outside. SGR and legacy mouse-report tests pass; the
reported client instead substitutes ordinary arrow keys. Those are indistinguishable from intentional
prompt-history keys, so the application must not guess and intercept them. See
the MobaXterm investigation in [findings](FINDINGS.md) and the optional input probe.
Run `python3 probes/scroll_input.py` in an ordinary SSH shell to capture original
client input. For comparison, run it with `--inside-tmux` in a plain shell pane to
capture what tmux forwards. Do not run it in a Kiro prompt. The two capture points
answer different questions; forwarded arrows cannot reveal their original source.
For this MobaXterm SSH setup, the user verified **Shift+MouseWheel** scrolling with
mouse mode enabled. PageUp/PageDown also work; q leaves tmux scrollback. Ordinary
wheel scrolling still produces prompt-history arrows, so use the Shift modifier.
This confirms the workaround for the tested setup, not all client versions or a
plain-wheel fix. Native Up/Down prompt-history keys remain unchanged.
