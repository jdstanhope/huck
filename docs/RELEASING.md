# Releasing huck

Releases are self-hosted off GitHub Releases. The single source of version truth
is the **root** `Cargo.toml` `[package] version` (the `huck` package); a release
is the git tag `v<version>`. (Since v202 the repo is a Cargo workspace; the
`crates/huck-syntax/Cargo.toml` version is INTERNAL and independent — it is a
path dependency, never published, and is NOT the release version. `release.sh`
reads the root `Cargo.toml` only.)

## One-time setup
1. Authenticate gh: `gh auth login`.
2. Create the Homebrew tap repo + seed the formula: `sh scripts/create-tap.sh`
   (creates the public repo `jdstanhope/homebrew-huck`).

## Each release
1. Bump `version` in `Cargo.toml`; commit on `main`.
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
