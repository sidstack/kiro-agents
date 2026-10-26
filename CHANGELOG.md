# Changelog

Versions follow Semantic Versioning. Future release entries are generated from
Conventional Commits by release-please.

## Local MVP history (before automated releases)

## [0.4.0](https://github.com/sidstack/kiro-agents/compare/v0.3.0...v0.4.0) (2026-10-26)


### Features

* improve session dashboard and add release installer ([#14](https://github.com/sidstack/kiro-agents/issues/14)) ([4c1250e](https://github.com/sidstack/kiro-agents/commit/4c1250e5ae79b2b04f967afde108bdb43e29ccac))


### Bug Fixes

* **deps:** consolidate dependency maintenance ([#9](https://github.com/sidstack/kiro-agents/issues/9)) ([5c7e7d7](https://github.com/sidstack/kiro-agents/commit/5c7e7d71e5883c6e61607352e1ed8891468064a8))

## 0.3.0 (2026-10-26)


### Features

* ship MVP v3 with automated binary releases ([03d8e4b](https://github.com/sidstack/kiro-agents/commit/03d8e4b651644543c16811e165bac92db6909578))
* ship MVP v3 with automated binary releases ([2c7a855](https://github.com/sidstack/kiro-agents/commit/2c7a85527851bcc0fa0fbde7a74225043670f769))

### 0.3.0 — 2026-10-26 (GitHub release pending)

- Select directories and native Kiro agents with keyboard-driven pickers.
- Preserve terminal ownership through board/native transitions.
- Capture agent discovery asynchronously with bounded output and cancellation.
- Refresh status in the background and avoid unnecessary idle redraws.
- Add PTY regressions for transitions, discovery retry and keyboard-only creation.

### 0.2.0 — 2026-10-16 (local MVP)

- Add the interactive session board, search, filters and confirmed stop.

### 0.1.0 — 2026-10-16 (local MVP)

- Add persistent native Kiro V2 PTY sessions, detach and reconnect.
