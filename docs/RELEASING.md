# Releasing huck

Releases are self-hosted off GitHub Releases. The single source of version truth
is the **root** `Cargo.toml` `[package] version` (the `huck` package); a release
is the git tag `v<version>`. (Since v202 the repo is a Cargo workspace. As of
0.3.0 the workspace crates — `huck-cli`, `huck-engine`, `huck-syntax` — are kept
version-aligned with the root `huck` package: bump all four together. They are
still path dependencies, never published to crates.io; `release.sh` reads the
root `Cargo.toml` only, but keeping the crate versions in lockstep with the
release avoids the confusion of a `0.3.0` binary built from `0.1.0` crates.)

## One-time setup
1. Authenticate gh: `gh auth login`.
2. Create the Homebrew tap repo + seed the formula: `sh scripts/create-tap.sh`
   (creates the public repo `jdstanhope/homebrew-huck`).

## Each release
1. Bump `version` in the root `Cargo.toml` **and** in each workspace crate
   (`crates/huck-cli`, `crates/huck-engine`, `crates/huck-syntax`) to the same
   value, run `cargo update -p huck -p huck-cli -p huck-engine -p huck-syntax`
   to refresh `Cargo.lock`, and write the release notes to
   `docs/releases/<version>.md`; commit on `main`.
2. Dry run and eyeball the plan: `sh scripts/release.sh --dry-run`.
3. Cut it: `sh scripts/release.sh`. This builds the `.deb`, tags `v<version>`,
   creates the GitHub release with the `.deb` attached, computes the source
   tarball sha256, and renders `packaging/homebrew/huck.rb`.
4. Push the updated formula to the tap (re-run `scripts/create-tap.sh`, which
   copies the refreshed `packaging/homebrew/huck.rb`).
5. Verify on macOS (the only place a build-from-source formula can be fully
   tested): `brew install jdstanhope/huck/huck && huck -c 'echo hi'`, and
   `brew audit --new jdstanhope/huck/huck`.

## Local checks (Linux, no release)
- `bash tests/scripts/packaging_diff_check.sh` — packaging logic + script
  dry-runs run byte-identically under bash and huck.
- `sh packaging/deb/build-deb.sh` then `dpkg-deb --info dist/huck_*.deb`.
