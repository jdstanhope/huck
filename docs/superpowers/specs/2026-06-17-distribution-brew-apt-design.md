# huck distribution via Homebrew + apt (self-hosted, build-from-source) — Design

**Status:** approved 2026-06-17
**Type:** distribution / packaging infrastructure (not a numbered shell-feature iteration; no bash-divergence flip)
**Origin:** user request — "setup brew and apt distribution of the shell, automate with scripts, and test that the scripts work the same in both bash and huck."

## Goal

Let people install `huck` via Homebrew (`brew install jdstanhope/huck/huck`) and on Debian/Ubuntu via a `.deb` (`apt install ./huck_*.deb`, or a `curl | sh` installer), driven by shell scripts that are themselves a huck-compatibility testbed: their pure logic and `--dry-run` output run byte-identically under bash and huck, enforced by a `tests/scripts/*_diff_check.sh` harness.

## Decisions (locked during brainstorming)

- **Distribution model:** self-hosted off GitHub Releases (`github.com/jdstanhope/shuck`). No homebrew-core, no Launchpad PPA, no external gatekeepers.
- **Build approach:** build-from-source. The Homebrew formula compiles on the user's machine (`depends_on "rust" => :build`) — so NO macOS cross-compilation or CI macOS runner is needed. The `.deb` is built locally on Linux.
- **apt depth:** a plain `.deb` attached to each GitHub Release + a `curl | sh` install script. No GPG-signed apt repo, no `apt update` auto-upgrades.
- **License:** MIT (new `LICENSE` file; `license = "MIT"` in `Cargo.toml`).
- **Version source of truth:** `Cargo.toml` `[package] version`; a release is git tag `v$VERSION`. First release: `v0.1.0`.

## Components

### A. Repo prerequisites

- `LICENSE` — MIT, `Copyright (c) 2026 John Stanhope`.
- `Cargo.toml [package]` gains: `license = "MIT"`, `description = "A POSIX-ish shell written in Rust"`, `repository = "https://github.com/jdstanhope/shuck"`, `homepage = "https://github.com/jdstanhope/shuck"`. (Binary name stays `huck`.)

### B. `.deb` build (apt side — fully buildable on Linux)

`packaging/deb/build-deb.sh [--dry-run]` produces `dist/huck_<version>_<arch>.deb`:
1. `cargo build --release` (skipped/echoed under `--dry-run`).
2. Stage a tree: `usr/bin/huck` (the release binary, mode 0755), `usr/share/doc/huck/copyright` (the MIT text in Debian copyright form), `usr/share/doc/huck/changelog.Debian.gz`.
3. Generate `DEBIAN/control` from `pack_lib.sh` helpers (fields: `Package: huck`, `Version: <version>`, `Architecture: <deb arch>`, `Maintainer`, `Section: shells`, `Priority: optional`, `Depends: libc6`, `Description`).
4. `dpkg-deb --build --root-owner-group <staging> dist/huck_<version>_<arch>.deb`.

Arch is `dpkg --print-architecture` (amd64/arm64); a fallback maps `uname -m` (`x86_64`→amd64, `aarch64`→arm64) for non-dpkg hosts. Output dir `dist/` is git-ignored.

### C. `curl | sh` installer (apt end-user path)

`scripts/install.sh` — POSIX `sh` (run via `/bin/sh`, not huck, since end users won't have huck yet):
1. Detect arch (`uname -m` → amd64/arm64).
2. Resolve the latest release's `.deb` asset URL from the GitHub API (`https://api.github.com/repos/jdstanhope/shuck/releases/latest`), parsed without `jq` (grep/sed) so it has no deps.
3. Download to a temp file; install via `sudo apt-get install -y ./<file>` with a `sudo dpkg -i` fallback.
4. `set -eu`, clear error if no asset for the arch, cleanup trap on exit.

### D. Homebrew formula (build-from-source tap)

- `packaging/homebrew/huck.rb` (the canonical template kept in THIS repo; the release script renders + pushes it to the tap):
  ```ruby
  class Huck < Formula
    desc "POSIX-ish shell written in Rust"
    homepage "https://github.com/jdstanhope/shuck"
    url "https://github.com/jdstanhope/shuck/archive/refs/tags/v0.1.0.tar.gz"
    sha256 "<sha256 of that tarball>"
    license "MIT"
    depends_on "rust" => :build
    def install
      system "cargo", "install", *std_cargo_args
    end
    test do
      assert_equal "hi\n", shell_output("#{bin}/huck -c 'echo hi'")
    end
  end
  ```
- The tap is a SEPARATE GitHub repo `jdstanhope/homebrew-huck` (Homebrew convention). One-time creation is scripted (`gh repo create`) but RUN BY THE USER. End users: `brew install jdstanhope/huck/huck`.

### E. Release orchestration

`scripts/release.sh [--dry-run]`:
1. Preflight: working tree clean, on `main`, `v$VERSION` tag does not already exist, `gh` present + authenticated.
2. Build the `.deb` (calls `build-deb.sh`).
3. Create + push the git tag `v$VERSION`.
4. `gh release create v$VERSION dist/*.deb --title … --notes …`.
5. Compute the source-tarball `sha256` (`gh release download`/`curl` the auto-tarball, `sha256sum`), render `packaging/homebrew/huck.rb` with the new `url`+`sha256`, and push it to the tap repo (clone/commit/push, or `gh` API).

Under `--dry-run` every step that mutates git/GitHub/the tap is PRINTED, not executed (this is what the dogfood harness diffs).

### F. Dogfood test layer (the novel requirement)

- `packaging/lib/pack_lib.sh` — a sourceable library of PURE, deterministic, side-effect-free functions, written in the bash subset huck supports:
  - `pack_version` (read version from a passed Cargo.toml path),
  - `pack_deb_arch <uname-m>` (→ amd64/arm64),
  - `pack_render_control <version> <arch> <maintainer>` (emit the `DEBIAN/control` text),
  - `pack_render_formula <version> <sha256>` (emit the `.rb` from the template string),
  - `pack_latest_deb_url <arch> <api-json>` (extract the asset URL),
  - `pack_tag <version>` (→ `v<version>`).
  The side-effectful scripts (B/C/E) call ONLY these for their logic, keeping the impure parts (cargo/dpkg/gh/curl) thin and isolated.
- `tests/scripts/packaging_diff_check.sh` — harness #95, same `check()` structure as the existing 94: each case sources `pack_lib.sh` and calls a pure function with fixed inputs, asserting byte-identical stdout+exit under `bash --norc --noprofile` and huck. Covers every `pack_*` function plus the `--dry-run` command-list output of `build-deb.sh` and `release.sh`.

## Testing strategy

- **Automated, in this repo's suite:** `packaging_diff_check.sh` joins the `for s in tests/scripts/*_diff_check.sh` loop (94 → 95). It is the primary "scripts work the same in bash and huck" gate.
- **Local smoke (Linux, verifiable here):** run `build-deb.sh`, then `dpkg-deb --info`/`--contents dist/huck_*.deb`, extract with `dpkg-deb -x` to a temp dir, and run `<tmp>/usr/bin/huck -c 'echo hi'` → `hi`. Run `release.sh --dry-run` and `build-deb.sh --dry-run` under BOTH shells and confirm identical command lists. `ruby -c packaging/homebrew/huck.rb` syntax-checks the formula (if `ruby` present; else skipped).
- **Deferred to the user / a Mac (cannot run in this Linux, no-`gh`-auth env):** actual `brew install jdstanhope/huck/huck` and `brew test`/`brew audit` (macOS — matches the standing macOS-portability constraint); `gh repo create jdstanhope/homebrew-huck`; the first real `gh release create`. The design makes all of these `--dry-run`-clean and documents the exact `! gh …` / `! brew …` commands.

## Documentation

A new `## Installation` section in `README.md`: `brew install jdstanhope/huck/huck`, the `curl … install.sh | sh` one-liner, and `apt install ./huck_*.deb`. A `docs/RELEASING.md` capturing the release runbook (tag → `release.sh` → create tap on first run → verify on Mac).

## Scope boundary

In scope: the MIT `LICENSE` + Cargo metadata; `build-deb.sh` + the `.deb`; `install.sh`; the Homebrew formula template + tap creation script; `release.sh`; `pack_lib.sh` + `packaging_diff_check.sh`; README/RELEASING docs. **Not** in scope: a GPG-signed apt repo with `apt update` (deferred — the .deb-on-Releases model was chosen); homebrew-core or a Launchpad PPA submission; GitHub Actions CI / prebuilt bottles / cross-compiled binaries (build-from-source was chosen); Windows/other-distro packaging; any change to huck's own source behavior. No new bash-divergence entries (this is infra, not a shell feature), though any huck incompatibility surfaced by writing the scripts is either worked around in the script or logged as a divergence if it's a real huck bug.

## Risks / notes

- The packaging scripts must stay within huck's supported subset; the diff harness is the guardrail. If a script genuinely needs a construct huck lacks, that's either a script rewrite or a newly-logged huck divergence (decided case-by-case, not silently).
- `gh` must be authenticated for a real release; `release.sh` preflights and fails clearly otherwise.
- The tap repo is a one-time manual/`gh` step; until it exists, `brew install` instructions don't work — `release.sh` will note this on first run.
- huck is `0.1.0` (early); these scripts are version-driven, so future bumps need only a `Cargo.toml` version change + `release.sh`.
