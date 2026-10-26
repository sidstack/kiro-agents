# Session names and native terminal behavior

The board displays a task name first, then agent, generation, wrapper ID and directory.
Names follow this order: explicit wrapper name, verified native Kiro title, a short
first-user-message title, then directory/agent plus the unique wrapper ID. IDs remain visible to distinguish
sessions that share the same directory or agent.

Use F2 to edit a selected session's name. Enter saves; an empty name restores automatic
naming; Ctrl+U clears and Esc cancels. CLI equivalents:

```sh
kiro-agents new /path/to/project --name 'Fix picker search'
kiro-agents rename SESSION_ID 'Investigate agent discovery'
kiro-agents rename SESSION_ID --clear
```

Manual names are shared persistent wrapper metadata in `KIRO_AGENTS_HOME/names.json`,
limited to 80 characters with no control characters. The store has its own format version; unknown versions are never overwritten.
Updates are locked, atomic and private (0600). They survive host restarts and never rewrite the daemon registry or
Kiro's own files. All updated frontends using that same wrapper state see the names.
F2 and CLI rename work with older live hosts without requiring a new RPC or restart.
Clearing a name explicitly restores automatic/fallback labels, including an override
previously supplied by a newer host.

Automatic naming is an optional read-only feature. Set `KIRO_AGENTS_AUTO_NAME=auto`
(default), `kiro` (native titles only), or `off` for the frontend. The frontend can enrich
older hosts' metadata read-only without replacing the host. Host-side configuration is
still inherited at host startup; updating the frontend never stops existing sessions. The feature keeps no child
processes, open files, models or background tasks to clean up on host close.

Native titles are accepted only when one fresh lock matches the owned live child PID,
its matching metadata has the same session ID and cwd, and the lock is newer than that
process's start. Names are never guessed from the newest conversation in a directory.
Missing, ambiguous, stale, unsupported or oversized metadata uses the fallback label.
The scan stops at 10,000 directory entries; metadata files are capped at 64 KiB.

With `auto`, an empty native title may fall back to the first `Prompt` text record in
its associated JSONL file. The reader examines at most eight records, each capped at
16 KiB, and retains at most ten words / 80 characters as the display name. It ignores
assistant/tool content. This is a local extractive fallback; Kiro's native title takes
precedence when available. No additional embedded model, download or paid call is
required. Auto-labels containing URLs or common credential prefixes are rejected. Only the derived name is saved in wrapper metadata; no transcript is copied
or logged. Unsupported SQLite-only session layouts fall back without inspecting the
auth database or guessing a session identity.

New naming fields are optional on the wire and in old registries. The dashboard and
CLI use the shared state-store name map after reading status, including responses from
pre-naming hosts. Older frontend versions do not understand the new name map. Native
SQLite TUI sessions without a verified PID/session association cannot get automatic
Kiro titles yet; they show unique project/agent/ID labels and support manual naming.
Verified SQLite/V2/V3 store adapters remain next-batch work. Do not replace or stop a
host with live sessions to make naming work. The dashboard header displays the frontend
version so a successful binary upgrade is visible.

# Search and scrolling

Directory and agent creation supports fuzzy typing followed by Enter. Arrow keys are
optional. Backspace accepts both DEL and legacy BS/Ctrl+H encodings, including Unicode
text. Tab/Right browses a highlighted directory; Left goes to its parent.

The dashboard uses an alternate screen. After an accepted attachment it releases that
screen, keeping raw mode, and native Kiro chooses its screen as it does when invoked
directly. Ordinary primary-screen output uses the outer terminal's scrollback. Kiro's
own full-screen UI controls its own history; wheel/PageUp input is forwarded unchanged.
Returning to the dashboard resets child private modes and restores its alternate screen.
No wrapper terminal emulator or transcript-based scrollback is introduced.

Profile fallback scans workspace/global scopes (or the explicit
`KIRO_AGENT_CONFIG_DIR` override), with legacy directories used when modern scopes
are absent. It reads genuine JSON/Markdown profiles, including file symlinks and
nested `team/name` identifiers, without traversing directory symlinks. Each scope
has a 500-entry, depth-four and 8 MiB read budget; individual profiles are capped at
64 KiB. Invalid or oversized JSON is skipped. Kiro remains the authority on whether
a profile is valid for the selected engine; no profile is rewritten or upgraded.
