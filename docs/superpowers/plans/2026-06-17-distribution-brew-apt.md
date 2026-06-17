# huck distribution via Homebrew + apt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `huck` via a build-from-source Homebrew tap and a `.deb` (+ `curl|sh` installer) attached to GitHub Releases, with all packaging logic written in a bash subset huck runs identically — enforced by a `tests/scripts/*_diff_check.sh` harness.

**Architecture:** Pure, deterministic packaging logic lives in a sourceable `packaging/lib/pack_lib.sh`; thin side-effectful scripts (`build-deb.sh`, `install.sh`, `release.sh`) call it. A new harness `packaging_diff_check.sh` runs the pure functions and the scripts' `--dry-run` output through both bash and huck and asserts byte-identical results.

**Tech Stack:** POSIX-ish shell (run under bash 5.x AND huck), `dpkg-deb`, `cargo`, Homebrew Ruby formula, `gh`, `sha256sum`.

**Spec:** `docs/superpowers/specs/2026-06-17-distribution-brew-apt-design.md`

**Branch:** `dist-brew-apt`

**Conventions:** commit trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Maintainer string is `John Stanhope <jdstanhope@gmail.com>`. Repo is `github.com/jdstanhope/huck`; binary + package name is `huck`.

**Environment facts (verified):** `dpkg-deb`, `cargo`, `ruby`, `gh`, `curl`, `sha256sum`, `gzip` all present; `dpkg --print-architecture` = `amd64`; `uname -m` = `x86_64`. Build huck first so `target/debug/huck` exists: `cargo build`.

**Hard rule for every script:** it MUST run under huck. The harness is the guardrail. If huck genuinely cannot run a construct a script needs, REWRITE the script into huck's supported subset — do NOT weaken the harness. Only if the failure is a real huck bug (a construct bash and the POSIX spec support but huck mishandles) do you stop and report it as a newly-found divergence rather than work around it.

---

## File map

- Create `LICENSE` — MIT text.
- Modify `Cargo.toml` — add `[package]` metadata; Modify `.gitignore` — add `/dist`.
- Create `packaging/lib/pack_lib.sh` — pure helpers (sourceable).
- Create `packaging/deb/build-deb.sh` — builds the `.deb`.
- Create `scripts/install.sh` — POSIX `curl|sh` installer.
- Create `packaging/homebrew/huck.rb` — formula template (initial render).
- Create `scripts/create-tap.sh` — one-time tap-repo creator (user-run).
- Create `scripts/release.sh` — release orchestrator.
- Create `tests/scripts/packaging_diff_check.sh` — bash↔huck harness (#95).
- Create `docs/RELEASING.md`; Modify `README.md` — add `## Installation`.

---

### Task 1: Repo prerequisites (LICENSE + Cargo metadata + gitignore)

**Files:** Create `LICENSE`; Modify `Cargo.toml`, `.gitignore`.

- [ ] **Step 1: Add the MIT LICENSE**

Create `LICENSE` with exactly:
```
MIT License

Copyright (c) 2026 John Stanhope

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 2: Add `[package]` metadata to `Cargo.toml`**

In `Cargo.toml`, replace the `[package]` block:
```toml
[package]
name = "huck"
version = "0.1.0"
edition = "2024"
```
with:
```toml
[package]
name = "huck"
version = "0.1.0"
edition = "2024"
description = "A POSIX-ish shell written in Rust"
license = "MIT"
repository = "https://github.com/jdstanhope/huck"
homepage = "https://github.com/jdstanhope/huck"
readme = "README.md"
```

- [ ] **Step 3: Ignore the build output dir**

In `.gitignore`, add a line `/dist` after `/target`.

- [ ] **Step 4: Verify build + metadata**

Run: `cargo build 2>&1 | tail -1` → `Finished`.
Run: `cargo metadata --no-deps --format-version 1 2>/dev/null | grep -o '"license":"MIT"'` → prints `"license":"MIT"`.

- [ ] **Step 5: Commit**

```bash
git add LICENSE Cargo.toml Cargo.lock .gitignore
git commit -m "dist: add MIT LICENSE + Cargo package metadata, ignore /dist

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(If `Cargo.lock` is untracked/unchanged, drop it from the `git add`.)

---

### Task 2: Pure packaging library + the bash↔huck harness (the dogfood core)

This is the heart: deterministic helpers that both scripts and the harness use. Fully testable here.

**Files:** Create `packaging/lib/pack_lib.sh`, `tests/scripts/packaging_diff_check.sh`.

- [ ] **Step 1: Write `packaging/lib/pack_lib.sh`**

Create `packaging/lib/pack_lib.sh` with exactly:
```sh
# Pure, side-effect-free packaging helpers for huck.
# Written in the POSIX-ish subset that both bash 5.x and huck run identically.
# Sourced by the packaging scripts AND exercised by packaging_diff_check.sh.

# Read the [package] version from a Cargo.toml path (first `version = "X"`).
pack_version() {
    sed -n 's/^version = "\(.*\)"/\1/p' "$1" | head -n 1
}

# Map `uname -m` output to a Debian architecture. rc 1 + "unknown" if unmapped.
pack_deb_arch() {
    case "$1" in
        x86_64|amd64)  printf 'amd64\n' ;;
        aarch64|arm64) printf 'arm64\n' ;;
        armv7l)        printf 'armhf\n' ;;
        i686|i386)     printf 'i386\n' ;;
        *)             printf 'unknown\n'; return 1 ;;
    esac
}

# Git tag for a version: 1.2.3 -> v1.2.3
pack_tag() {
    printf 'v%s\n' "$1"
}

# Emit a Debian DEBIAN/control file. $1=version $2=arch $3=maintainer
pack_render_control() {
    cat <<EOF
Package: huck
Version: $1
Architecture: $2
Maintainer: $3
Section: shells
Priority: optional
Depends: libc6
Homepage: https://github.com/jdstanhope/huck
Description: A POSIX-ish shell written in Rust
 huck is a small POSIX-ish shell implemented in Rust, aiming for
 byte-level bash compatibility on a large command subset.
EOF
}

# Emit the Homebrew formula. $1=version $2=source-tarball sha256
pack_render_formula() {
    cat <<EOF
class Huck < Formula
  desc "POSIX-ish shell written in Rust"
  homepage "https://github.com/jdstanhope/huck"
  url "https://github.com/jdstanhope/huck/archive/refs/tags/v$1.tar.gz"
  sha256 "$2"
  license "MIT"
  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_equal "hi\n", shell_output("#{bin}/huck -c 'echo hi'")
  end
end
EOF
}

# Extract the .deb asset URL for $1=arch from GitHub release JSON read on stdin.
pack_latest_deb_url() {
    grep -o '"browser_download_url": *"[^"]*_'"$1"'\.deb"' \
        | sed 's/.*"\(https[^"]*\)".*/\1/' \
        | head -n 1
}
```

Notes for the implementer: the heredocs interpolate `$1`/`$2`/`$3` but leave `\n`, `#{bin}`, and `*std_cargo_args` literal (only `$` triggers expansion) — that is intentional and identical in bash and huck. Do not add quoting around the heredoc delimiter (that would stop interpolation).

- [ ] **Step 2: Write the harness `tests/scripts/packaging_diff_check.sh`**

Create `tests/scripts/packaging_diff_check.sh` with exactly:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the packaging library (and, as later
# tasks add them, the packaging scripts' --dry-run output). Proves huck runs the
# real packaging logic identically to bash.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Run a shell fragment through bash and huck; assert identical stdout+stderr+exit.
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Run a SCRIPT FILE with args through bash and huck; assert identical output.
check_script() {
    local label="$1"; shift
    local b h
    b=$(bash "$@" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$@" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

L='packaging/lib/pack_lib.sh'
check "deb arch x86_64"  ". $L; pack_deb_arch x86_64"
check "deb arch aarch64" ". $L; pack_deb_arch aarch64"
check "deb arch armv7l"  ". $L; pack_deb_arch armv7l"
check "deb arch unknown" ". $L; pack_deb_arch sparc; echo rc=\$?"
check "tag"              ". $L; pack_tag 1.2.3"
check "version read"     ". $L; pack_version Cargo.toml"
check "control render"   ". $L; pack_render_control 0.1.0 amd64 'John Stanhope <jdstanhope@gmail.com>'"
check "formula render"   ". $L; pack_render_formula 0.1.0 0123abc"
check "latest deb url"   ". $L; printf '%s\n' '  \"browser_download_url\": \"https://github.com/jdstanhope/huck/releases/download/v0.1.0/huck_0.1.0_amd64.deb\"' | pack_latest_deb_url amd64"
check "latest deb miss"  ". $L; printf '%s\n' 'nothing here' | pack_latest_deb_url amd64; echo rc=\$?"

# --- script dry-run parity cases are appended by later tasks ---

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 3: Run the harness — bash↔huck parity**

Run: `cargo build --quiet && chmod +x tests/scripts/packaging_diff_check.sh && bash tests/scripts/packaging_diff_check.sh`
Expected: `Total: 10, Pass: 10, Fail: 0`.
If a `check` FAILs, the script uses a construct huck handles differently — fix `pack_lib.sh` to a more portable form (e.g. avoid the offending bashism); do not edit the harness to hide it. If it's a real huck bug, STOP and report it.

- [ ] **Step 4: Commit**

```bash
git add packaging/lib/pack_lib.sh tests/scripts/packaging_diff_check.sh
git commit -m "dist: pure packaging lib + bash<->huck parity harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `.deb` builder (`build-deb.sh`)

**Files:** Create `packaging/deb/build-deb.sh`; Modify `tests/scripts/packaging_diff_check.sh`.

- [ ] **Step 1: Write `packaging/deb/build-deb.sh`**

Create `packaging/deb/build-deb.sh` with exactly:
```sh
#!/usr/bin/env sh
# Build dist/huck_<version>_<arch>.deb. Pure logic comes from pack_lib.sh.
# `--dry-run` prints every external/side-effecting command instead of running it
# (this is what the bash<->huck parity harness diffs).
set -eu

DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && DRY_RUN=1

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
. "$ROOT/packaging/lib/pack_lib.sh"

VERSION=$(pack_version "$ROOT/Cargo.toml")
ARCH=$(pack_deb_arch "$(uname -m)")
MAINT="John Stanhope <jdstanhope@gmail.com>"
STAGE="$ROOT/dist/huck_${VERSION}_${ARCH}"
DEB="$ROOT/dist/huck_${VERSION}_${ARCH}.deb"

run() {
    if [ "$DRY_RUN" = 1 ]; then printf '+ %s\n' "$*"; else eval "$@"; fi
}

printf 'huck %s (%s) -> %s\n' "$VERSION" "$ARCH" "dist/huck_${VERSION}_${ARCH}.deb"
run "cargo build --release --manifest-path '$ROOT/Cargo.toml'"
run "rm -rf '$STAGE'"
run "mkdir -p '$STAGE/usr/bin' '$STAGE/usr/share/doc/huck' '$STAGE/DEBIAN'"
run "install -m 0755 '$ROOT/target/release/huck' '$STAGE/usr/bin/huck'"
run "cp '$ROOT/LICENSE' '$STAGE/usr/share/doc/huck/copyright'"

if [ "$DRY_RUN" = 1 ]; then
    printf '+ render DEBIAN/control (Version: %s, Architecture: %s)\n' "$VERSION" "$ARCH"
else
    pack_render_control "$VERSION" "$ARCH" "$MAINT" > "$STAGE/DEBIAN/control"
fi

run "dpkg-deb --build --root-owner-group '$STAGE' '$DEB'"
printf 'done\n'
```

Implementer note: `--dry-run` output must be DETERMINISTIC across bash and huck on the same host (it is: `VERSION` from Cargo.toml, `ARCH` from `uname -m` are host-stable and shell-independent). Keep the printed strings byte-stable.

- [ ] **Step 2: Add the dry-run parity case to the harness**

In `tests/scripts/packaging_diff_check.sh`, replace the line
`# --- script dry-run parity cases are appended by later tasks ---`
with:
```bash
check_script "build-deb --dry-run" packaging/deb/build-deb.sh --dry-run
```

- [ ] **Step 3: Verify parity + real build + install smoke**

Run: `chmod +x packaging/deb/build-deb.sh && bash tests/scripts/packaging_diff_check.sh | tail -1`
Expected: `Total: 11, Pass: 11, Fail: 0`.
Run the real build + inspect + extract-and-run smoke:
```bash
cargo build --release --quiet
sh packaging/deb/build-deb.sh
dpkg-deb --info dist/huck_*.deb | grep -E 'Package:|Version:|Architecture:|Depends:'
dpkg-deb --contents dist/huck_*.deb | grep -E 'usr/bin/huck|copyright'
rm -rf /tmp/huckdeb && dpkg-deb -x dist/huck_*.deb /tmp/huckdeb
/tmp/huckdeb/usr/bin/huck -c 'echo hi'
```
Expected: control shows `Package: huck`, the right `Version`/`Architecture: amd64`/`Depends: libc6`; contents list `./usr/bin/huck` and `./usr/share/doc/huck/copyright`; the final line prints `hi`.

- [ ] **Step 4: Commit**

```bash
git add packaging/deb/build-deb.sh tests/scripts/packaging_diff_check.sh
git commit -m "dist: build-deb.sh (.deb builder) + dry-run parity case

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `curl | sh` installer (`scripts/install.sh`)

End users run this BEFORE they have huck, so it targets `/bin/sh` (POSIX). Its URL-parsing logic is already covered by `pack_latest_deb_url` in the harness; here we add the script and a `sh -n` syntax gate.

**Files:** Create `scripts/install.sh`.

- [ ] **Step 1: Write `scripts/install.sh`**

Create `scripts/install.sh` with exactly:
```sh
#!/usr/bin/env sh
# Install the latest huck .deb from GitHub Releases on a Debian/Ubuntu system.
# Usage:  curl -fsSL https://raw.githubusercontent.com/jdstanhope/huck/main/scripts/install.sh | sh
set -eu

REPO="jdstanhope/huck"
API="https://api.github.com/repos/$REPO/releases/latest"

case "$(uname -m)" in
    x86_64|amd64)  ARCH=amd64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    *) echo "huck: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

URL=$(curl -fsSL "$API" \
    | grep -o '"browser_download_url": *"[^"]*_'"$ARCH"'\.deb"' \
    | sed 's/.*"\(https[^"]*\)".*/\1/' \
    | head -n 1)

if [ -z "$URL" ]; then
    echo "huck: no .deb asset for $ARCH in the latest release" >&2
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
echo "huck: downloading $URL"
curl -fsSL "$URL" -o "$TMP/huck.deb"

if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get install -y "$TMP/huck.deb"
else
    sudo dpkg -i "$TMP/huck.deb"
fi
echo "huck: installed. Try:  huck -c 'echo hello from huck'"
```

The `grep`/`sed` URL extraction is intentionally identical to `pack_latest_deb_url` (already parity-tested); this script duplicates it because end users source neither `pack_lib.sh` nor anything else — it must be a single self-contained file.

- [ ] **Step 2: Syntax-check under sh, bash, and huck**

Run:
```bash
sh -n scripts/install.sh && echo "sh ok"
bash -n scripts/install.sh && echo "bash ok"
cargo build --quiet && ./target/debug/huck -n scripts/install.sh && echo "huck ok"
```
Expected: `sh ok`, `bash ok`, `huck ok`. (`-n` = parse-only; we cannot run the real install — no published release — but parse-parity across all three shells confirms the syntax is portable.)
If `huck -n` errors on a construct, rewrite that construct portably (the script must parse under huck too).

- [ ] **Step 3: Commit**

```bash
git add scripts/install.sh
git commit -m "dist: curl|sh apt installer (downloads latest .deb)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Homebrew formula + tap-creation script

**Files:** Create `packaging/homebrew/huck.rb`, `scripts/create-tap.sh`.

- [ ] **Step 1: Render the initial formula**

Generate it from the tested helper so it stays consistent (the `sha256` is a placeholder until the first real release fills it via `release.sh`):
```bash
cargo build --quiet
mkdir -p packaging/homebrew
printf '%s\n' '. packaging/lib/pack_lib.sh; pack_render_formula 0.1.0 0000000000000000000000000000000000000000000000000000000000000000' \
    | ./target/debug/huck > packaging/homebrew/huck.rb
cat packaging/homebrew/huck.rb
```
Expected: a valid `class Huck < Formula … end` with `url …/v0.1.0.tar.gz`, the placeholder `sha256`, `license "MIT"`, `depends_on "rust" => :build`, and the `test do` block.

- [ ] **Step 2: Syntax-check the formula**

Run: `ruby -c packaging/homebrew/huck.rb`
Expected: `Syntax OK`.

- [ ] **Step 3: Write `scripts/create-tap.sh`**

Create `scripts/create-tap.sh` with exactly:
```sh
#!/usr/bin/env sh
# One-time: create the Homebrew tap repo (jdstanhope/homebrew-huck) and seed it
# with the formula. Requires an authenticated `gh`. RUN THIS YOURSELF — it makes
# a public GitHub repo. `release.sh` keeps the formula updated thereafter.
set -eu
TAP="jdstanhope/homebrew-huck"
ROOT=$(cd "$(dirname "$0")/.." && pwd)

if gh repo view "$TAP" >/dev/null 2>&1; then
    echo "tap $TAP already exists"
else
    gh repo create "$TAP" --public \
        --description "Homebrew tap for huck (a POSIX-ish shell in Rust)"
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
gh repo clone "$TAP" "$TMP/tap"
mkdir -p "$TMP/tap/Formula"
cp "$ROOT/packaging/homebrew/huck.rb" "$TMP/tap/Formula/huck.rb"
git -C "$TMP/tap" add Formula/huck.rb
git -C "$TMP/tap" commit -m "huck formula" || echo "no changes"
git -C "$TMP/tap" push
echo "tap ready: brew install $TAP/huck"
```

- [ ] **Step 4: Syntax-check the tap script under sh + huck**

Run: `sh -n scripts/create-tap.sh && ./target/debug/huck -n scripts/create-tap.sh && echo ok`
Expected: `ok`. (We do NOT run it — it creates a real GitHub repo; that is the user's to run.)

- [ ] **Step 5: Commit**

```bash
git add packaging/homebrew/huck.rb scripts/create-tap.sh
git commit -m "dist: Homebrew formula (build-from-source) + tap-creation script

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Release orchestrator (`scripts/release.sh`)

**Files:** Create `scripts/release.sh`; Modify `tests/scripts/packaging_diff_check.sh`.

- [ ] **Step 1: Write `scripts/release.sh`**

Create `scripts/release.sh` with exactly:
```sh
#!/usr/bin/env sh
# Cut a huck release: build the .deb, tag, create the GitHub release, and update
# the Homebrew tap formula's url+sha256. `--dry-run` prints every mutating step
# instead of running it (parity-tested under bash and huck).
set -eu

DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && DRY_RUN=1

ROOT=$(cd "$(dirname "$0")/.." && pwd)
. "$ROOT/packaging/lib/pack_lib.sh"

VERSION=$(pack_version "$ROOT/Cargo.toml")
TAG=$(pack_tag "$VERSION")
TARBALL_URL="https://github.com/jdstanhope/huck/archive/refs/tags/$TAG.tar.gz"

run() {
    if [ "$DRY_RUN" = 1 ]; then printf '+ %s\n' "$*"; else eval "$@"; fi
}

printf 'releasing huck %s (%s)\n' "$VERSION" "$TAG"

# Preflight (skipped under --dry-run so the printed plan is host-independent).
if [ "$DRY_RUN" = 0 ]; then
    [ -z "$(git -C "$ROOT" status --porcelain)" ] || { echo "release: working tree not clean" >&2; exit 1; }
    [ "$(git -C "$ROOT" rev-parse --abbrev-ref HEAD)" = "main" ] || { echo "release: not on main" >&2; exit 1; }
    if git -C "$ROOT" rev-parse "$TAG" >/dev/null 2>&1; then echo "release: tag $TAG already exists" >&2; exit 1; fi
    command -v gh >/dev/null 2>&1 || { echo "release: gh not found" >&2; exit 1; }
fi

run "sh '$ROOT/packaging/deb/build-deb.sh'"
run "git -C '$ROOT' tag '$TAG'"
run "git -C '$ROOT' push origin '$TAG'"
run "gh release create '$TAG' '$ROOT'/dist/huck_*.deb --title '$TAG' --notes 'huck $VERSION'"

if [ "$DRY_RUN" = 1 ]; then
    printf '+ compute sha256 of %s\n' "$TARBALL_URL"
    printf '+ render formula (url=%s) and push to jdstanhope/homebrew-huck\n' "$TARBALL_URL"
else
    SHA=$(curl -fsSL "$TARBALL_URL" | sha256sum | cut -d' ' -f1)
    pack_render_formula "$VERSION" "$SHA" > "$ROOT/packaging/homebrew/huck.rb"
    echo "release: formula rendered with sha256 $SHA — run scripts/create-tap.sh (first time) or push packaging/homebrew/huck.rb to the tap"
fi
printf 'done\n'
```

Implementer note: under `--dry-run` the preflight and the network sha256 are skipped, so the printed plan depends only on `VERSION` (Cargo.toml) — identical under bash and huck on the same checkout.

- [ ] **Step 2: Add the release dry-run parity case**

In `tests/scripts/packaging_diff_check.sh`, after the `build-deb --dry-run` line, add:
```bash
check_script "release --dry-run" scripts/release.sh --dry-run
```

- [ ] **Step 3: Verify parity + dry-run readability**

Run: `chmod +x scripts/release.sh && bash tests/scripts/packaging_diff_check.sh | tail -1`
Expected: `Total: 12, Pass: 12, Fail: 0`.
Run: `sh scripts/release.sh --dry-run`
Expected: a clean `+ …` command list (build-deb, git tag `v0.1.0`, git push, gh release create, sha256, formula render) and a final `done` — no real git/gh/network actions.

- [ ] **Step 4: Commit**

```bash
git add scripts/release.sh tests/scripts/packaging_diff_check.sh
git commit -m "dist: release.sh orchestrator (.deb + tag + gh release + formula) with --dry-run

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Docs (README Installation + RELEASING runbook)

**Files:** Modify `README.md`; Create `docs/RELEASING.md`.

- [ ] **Step 1: Add an Installation section to `README.md`**

Find the first top-level section in `README.md` (the title line at the top). Insert this block immediately after the title's intro paragraph (before the iteration table / module map). If unsure where, place it directly under the H1:
```markdown
## Installation

**Homebrew (macOS/Linux):**
```sh
brew install jdstanhope/huck/huck
```

**Debian/Ubuntu (.deb):**
```sh
curl -fsSL https://raw.githubusercontent.com/jdstanhope/huck/main/scripts/install.sh | sh
# or, manually:
sudo apt install ./huck_<version>_<arch>.deb
```

**From source:**
```sh
cargo install --git https://github.com/jdstanhope/huck huck
```
```
(Use a 4-backtick fence around the whole block when editing so the inner triple-backticks render — or add the three subsections as separate fenced blocks.)

- [ ] **Step 2: Write `docs/RELEASING.md`**

Create `docs/RELEASING.md` with exactly:
```markdown
# Releasing huck

Releases are self-hosted off GitHub Releases. The single source of version truth
is `Cargo.toml` `[package] version`; a release is the git tag `v<version>`.

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
```

- [ ] **Step 3: Verify README renders / no broken fences**

Run: `grep -n '## Installation' README.md && echo ok`
Expected: prints the line + `ok`.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/RELEASING.md
git commit -m "docs: installation instructions + release runbook

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Full regression + final verification

**Files:** none (verification only).

- [ ] **Step 1: All bash-diff harnesses (incl. the new #95) green**

Run: `cargo build --quiet; p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `95 passed, 0 failed`.

- [ ] **Step 2: huck's own test suite unaffected**

Run: `cargo test >/tmp/dist.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/dist.log`
Expected: `exit: 0`, `0`. (No huck source changed, but confirm nothing — e.g. a stray file — broke the build.)

- [ ] **Step 3: clippy clean (no source changes, sanity only)**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 4: End-to-end packaging smoke (Linux)**

Run:
```bash
sh packaging/deb/build-deb.sh
rm -rf /tmp/hk && dpkg-deb -x dist/huck_*.deb /tmp/hk && /tmp/hk/usr/bin/huck -c 'echo packaged-ok'
ruby -c packaging/homebrew/huck.rb
sh scripts/release.sh --dry-run >/dev/null && echo "release dry-run ok"
```
Expected: `packaged-ok`, `Syntax OK`, `release dry-run ok`.

- [ ] **Step 5: No commit (verification task).** Report results.

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `LICENSE`, `Cargo.toml`, `.gitignore`, `packaging/lib/pack_lib.sh`, `packaging/deb/build-deb.sh`, `scripts/{install,create-tap,release}.sh`, `packaging/homebrew/huck.rb`, `tests/scripts/packaging_diff_check.sh`, `README.md`, `docs/RELEASING.md`. Confirm no `src/*.rs` changes (this is infra only).
- Re-run `packaging_diff_check.sh` and confirm 12/12; re-run the whole harness suite (95/95).
- Confirm every script parses under huck (`huck -n <script>`), and the `--dry-run` outputs are byte-identical bash vs huck.
- Merge `dist-brew-apt` to main `--no-ff` after user confirmation (AskUserQuestion); push.
- The user-run, side-effecting steps (NOT done by the agent): `gh auth login`, `sh scripts/create-tap.sh`, first real `sh scripts/release.sh`, and macOS `brew install` verification.
- Record the work in `project_huck_iterations.md` + the MEMORY.md index (this is infra, not a vNN shell iteration and adds NO bash-divergence entry — note that explicitly).

---

## Self-review (plan vs spec)

- **Spec coverage:** A repo prereqs (Task 1) ✓; B `.deb` build + arch + control (Task 3, helpers Task 2) ✓; C `install.sh` (Task 4) ✓; D formula + tap (Task 5) ✓; E release orchestration + `--dry-run` (Task 6) ✓; F dogfood pure-lib + harness + dry-run parity (Tasks 2/3/6) ✓; docs README+RELEASING (Task 7) ✓; full regression (Task 8) ✓. Deferred-to-user items (brew install, gh repo create, real release) are called out, not automated — matches the spec's "what can't be verified here."
- **Placeholder scan:** the formula `sha256` `0000…`/`PLACEHOLDER` is an intentional render-time value (release.sh fills it), documented as such — not a plan gap. No TBD/TODO/"handle errors" placeholders; every script is given in full.
- **Type/name consistency:** `pack_version`, `pack_deb_arch`, `pack_tag`, `pack_render_control`, `pack_render_formula`, `pack_latest_deb_url` are defined once (Task 2) and called with the same signatures everywhere (Tasks 3/5/6 + harness). `--dry-run`, the `run()` helper, `DRY_RUN`, and the `check`/`check_script` harness helpers are used identically across tasks. Harness case counts step up 10 → 11 → 12 across Tasks 2/3/6, ending at 12 (Task 8 expects 95 harness FILES, distinct from the 12 cases inside #95).
