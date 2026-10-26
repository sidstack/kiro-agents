# Branch beta delivery

## Milestone 1: isolated versioned packaging

- [x] Stamp a beta version only in the ephemeral build checkout.
- [ ] Verify source SHA, archive and executable identities.
- [x] Retain stable-tag release checks.

## Milestone 2: integration gates

- [x] Run Rust tests, board snapshots, PTY scripts and packaging tests on branches/PRs.
- [x] Audit the complete dependency lockfile before packaging.
- [ ] Check the archive/checksum and run all six PTY scripts on the extracted beta.

## Milestone 3: publication and rollout

- [x] Publish verified artifacts from trusted default-branch code only.
- [x] Keep fork runs read-only and stable releases latest.
- [ ] Enable hosted workflows and beta integration as a merge requirement.
- [ ] Produce and verify a beta for the dependency PR before merging it.

Work split: two Pi swarm workers using Strata Qwen 3.8 implement beta packaging
and installed-binary integration tests; the coordinator owns workflow integration,
publication validation, documentation and rollout.
