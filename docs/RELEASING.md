# Releases and installation

The deliverable is a native Linux x86_64 binary, not a Pi or npm package. Kiro CLI
is a separate dependency. End users need neither Rust nor Python for the binary.

## Automated versions

[Release Please](https://github.com/googleapis/release-please-action) reads
Conventional Commits on `main`, proposes a release PR, and updates Cargo.toml,
Cargo.lock, the version manifest and CHANGELOG.md together. Merge that generated
PR to create the `vVERSION` GitHub release and automatically attach the binary.
Do not bump versions, push release tags or upload workstation builds by hand.

`feat:` bumps the minor version; `fix:` bumps patch. Breaking changes use `!` or
a `BREAKING CHANGE:` footer; before 1.0 they bump minor, from 1.0 they bump major.
Documentation/chore-only changes do not normally produce an application release.
The initial implementation commit carries the one-time `Release-As: 0.3.0`
bootstrap footer. The manifest begins at `0.0.0` to record that no automated
release exists yet; the first release PR updates it to 0.3.0. Subsequent
versions are computed without override footers.
The local 0.1.0/0.2.0 MVP iterations were never published GitHub releases.

## GitHub setup

Use `main` as the repository's default branch and enable GitHub Actions. Under
Settings → Actions → General, permit GitHub Actions to create pull requests.
The workflow uses the repository's built-in GITHUB_TOKEN; no crates.io or npm
credentials are needed. This project does not publish to either registry.

GitHub does not trigger other workflows for PRs/tags created with GITHUB_TOKEN.
The binary build is therefore called directly in the release workflow, using
Release Please's tag output rather than waiting for a tag-push workflow.
For release-PR checks, run Checks and Security manually on the generated PR
branch (both support workflow_dispatch), or configure an optional GitHub App/PAT
as RELEASE_PLEASE_TOKEN so PR checks run automatically. Main and exact-tag
release gates run regardless; no untested binary is attached.

## Release gates and artifacts

`release.yml` runs the test suite, packaging tests and a strict RustSec audit
before version/release work. The reusable `binary.yml` checks out the exact tag,
runs those gates again, checks tag/package/binary version agreement, and builds:

- `kiro-agents-vVERSION-x86_64-unknown-linux-gnu.tar.gz`
- `SHA256SUMS` (SHA-256 of the archive)

The archive contains the executable, MIT license, README and changelog. Tar/gzip
headers are deterministic for identical inputs. This is not a claim of hermetic
or reproducible compiler output across different Rust toolchains.

GitHub-hosted Ubuntu 22.04 builds establish a glibc 2.35 runtime baseline. Newer
compatible glibc-based Linux x86_64 systems can use the binary. Alpine/musl,
ARM, macOS and Windows binaries are not currently shipped. Source builds are
available for supported Linux environments.

A failed artifact job can be retried using the Release binary workflow's manual
Run workflow action with the existing `vVERSION` tag. It rechecks everything and
replaces that release's artifacts, without changing the version. Never pass an
unreleased branch name. The packager refuses a tag/version/HEAD mismatch.

## Install a binary

Download the archive and SHA256SUMS from the repository's GitHub Releases page.
Use a tagged release, not a workflow artifact. For example, for v0.3.0:

```sh
sha256sum --check SHA256SUMS
tar -xzf kiro-agents-v0.3.0-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
install -m 0755 kiro-agents-v0.3.0-x86_64-unknown-linux-gnu/kiro-agents "$HOME/.local/bin/kiro-agents"
export PATH="$HOME/.local/bin:$PATH"
kiro-agents --version
kiro-agents doctor
kiro-agents
```

Install/authenticate Kiro CLI separately. Add ~/.local/bin to your shell PATH
persistently if needed. To upgrade, install a newer verified release and reopen
the frontend; never restart a live host merely to update a binary. A protocol
incompatibility requires an explicit user-controlled host restart.

## Build from a checkout

```sh
cargo install --path . --locked
kiro-agents --version
```

Uninstall the executable you installed: cargo users run `cargo uninstall
kiro-agents`; binary users remove ~/.local/bin/kiro-agents. Removing a frontend
binary leaves background sessions and wrapper metadata intact. Stop owned
sessions explicitly with `kiro-agents shutdown` first when that is your intent.

## Local validation of the release setup (2026-10-26)

The complete check suite passed, including 44 Rust tests, six PTY scripts and
two packaging tests. Actionlint 1.7.12 and the official Release Please JSON
schema accepted the workflows/configuration. Cargo's source-package allowlist
excluded probes, build output, caches and logs.

In an isolated clone, a temporary test tag exercised the complete packager:
optimized binary build, Cargo/tag/binary identity checks, archive construction,
checksum verification, extraction, `--version`, and the extracted binary's
PTY handoff regression. All passed. No tag or release was published by this
local check. Hosted automation still requires a GitHub remote and the setup
permissions above.
