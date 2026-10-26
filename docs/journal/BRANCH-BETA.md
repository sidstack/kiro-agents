# Branch beta delivery

## Milestone 1: isolated versioned packaging

- [x] Stamp a beta version only in the ephemeral build checkout.
- [x] Verify source SHA, archive and executable identities.
- [x] Retain stable-tag release checks.

## Milestone 2: integration gates

- [x] Run Rust tests, board snapshots, PTY scripts and packaging tests on branches/PRs.
- [x] Audit the complete dependency lockfile before packaging.
- [x] Check the archive/checksum and run all six PTY scripts on the extracted beta.

## Milestone 3: publication and rollout

- [x] Publish verified artifacts from trusted default-branch code only.
- [x] Keep fork runs read-only and stable releases latest.
- [ ] Enable hosted workflows and beta integration as a merge requirement.
- [ ] Produce and verify a beta for the dependency PR before merging it.

Work split: two Pi swarm workers using Strata Qwen 3.8 implement beta packaging
and installed-binary integration tests; the coordinator owns workflow integration,
publication validation, documentation and rollout.

Local validation: 44 Rust tests and all six debug PTY scripts passed. Ten packaging,
seven installed-archive and five publication boundary tests passed. An isolated clean
clone built `0.3.1-beta.999999.1`; its checksum/version checks and all six PTY scenarios
against the extracted optimized binary passed. Hosted rollout is tracked in PR #10;
dependency preview validation is tracked in PR #9.
