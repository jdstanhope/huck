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
