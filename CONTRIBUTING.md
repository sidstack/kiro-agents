# Contributing

Use Linux, Rust stable, Python 3.10+ and a PTY-capable environment. Read
[AGENTS.md](AGENTS.md) and the architecture before changing lifecycle or Kiro
launch behavior. `rust-toolchain.toml` installs the formatting/lint components.

```sh
make check
cargo install cargo-audit --version 0.22.2 --locked
make audit
python3 tests/release_package.py
```

Tests use temporary state and a deterministic fake Kiro; CI never calls a paid
model, uses real credentials or launches the real CLI. Real probes are opt-in,
require a checkout, and follow the isolation rules in AGENTS.md. Findings beat
design documents: record contradictory observations in `docs/FINDINGS.md`.

Branch from `main`; open PRs into `main`. Use Conventional Commits for commit and
squash-merge titles: `feat: add directory selection`, `fix: restore terminal modes`,
`docs: clarify installation`. Use `!` or a `BREAKING CHANGE:` footer for breaking
changes. Explain the behavior, reason and checks in the PR. Release automation
owns version bumps; see [releasing](docs/RELEASING.md).

Keep changes focused and tests behavioral. Never commit generated build output,
real conversations, credentials, host logs, or child environments. Never rewrite
Kiro profiles or introduce implicit tool approval. Discuss larger architecture
changes in an issue first.

This is an independent project, not affiliated with or endorsed by Amazon,
Kiro, OpenAI or Pi. Respect their trademarks and do not copy their branding.

Branch pushes also run the Branch beta workflow: the full integration suite and
RustSec audit, followed by all six PTY scripts against the extracted optimized beta
binary. Same-repository builds publish GitHub prereleases for manual testing before
merge. Fork PR builds provide read-only artifacts. See `docs/RELEASING.md` for beta
versioning, installation and the separate stable release process.
