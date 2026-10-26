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

## Branch beta binaries

Every push to a branch other than `main` runs **Branch beta**. It runs `make check`
(Rust tests, snapshots, fake-Kiro PTY integration and packaging tests), a strict
RustSec audit, then packages an optimized Linux x86_64 binary. The extracted binary
must pass all six PTY integration scripts before the `beta-linux` artifact is uploaded.
Fork pull requests get the same build/tests as a read-only artifact, without publication.
No real Kiro authentication or paid model calls are used.

Beta versions derive from the branch's Cargo version: `0.3.0` becomes
`0.3.1-beta.RUN.ATTEMPT`. Run numbers identify builds across branches; the attempt
increments on rerun. This predicts a patch base for testing, not the next stable
version: a later feature release may become `0.4.0`. The exact source SHA is recorded
in `beta.json` and the prerelease tag. Version stamping occurs only in the build
checkout; no version changes, commits or tags are pushed back to the branch.

A separate **Publish beta** workflow runs trusted default-branch code after a
successful same-repository branch build. It verifies the workflow identity, source
SHA, version, archive member allowlist and checksum, then creates a GitHub prerelease
with the archive, SHA256SUMS and beta.json. It never executes the branch binary or
checks out the branch with a write token. Dependency bot branches are supported.
Fork artifacts are never published by this privileged workflow.

Prereleases are marked `--prerelease --latest=false`, so the stable release remains
latest. Download a beta from GitHub Releases, verify its SHA256SUMS and use the normal
binary installation steps below. `kiro-agents --version` must show the beta suffix.
To test without replacing an installation, run the extracted executable directly.

To rebuild a branch manually after these workflows are merged into `main`:

```sh
gh workflow run beta.yml --ref YOUR_BRANCH
```

Rerun the original Branch beta workflow to retry a beta build/publication; a new
attempt produces a new beta identity. Do not use the stable Release binary workflow
for beta tags: their source Cargo version intentionally remains unchanged.

Stable Release Please versions, manifests and publication remain separate. Review
and merge the dependency/feature PR only after testing its beta; merge the generated
stable release PR when ready to publish. Bootstrap requires the workflow infrastructure
on `main` first, then update existing branches from `main` to build their betas.

## GitHub setup

Use `main` as the repository's default branch and enable GitHub Actions. Under
Settings → Actions → General, permit GitHub Actions to create pull requests.
The workflow uses the repository's built-in GITHUB_TOKEN; no crates.io or npm
credentials are needed. This project does not publish to either registry.

Bot-created PR events can require approval, and token-created tag pushes do not
start ordinary tag workflows. The release workflow explicitly dispatches Checks,
Security and Branch beta on its generated PR branch using the built-in GITHUB_TOKEN. GitHub
permits workflow_dispatch from that token, so no personal access token is needed.
The binary job is called directly using Release Please's tag output. See
[GitHub's workflow trigger rules](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow).

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
local check. The public GitHub repository now has hosted automation enabled; v0.3.0 was
published and its downloaded archive passed checksum, version and PTY handoff checks.
