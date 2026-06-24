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
NOTES_FILE="$ROOT/docs/releases/$VERSION.md"

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
    [ -f "$NOTES_FILE" ] || { echo "release: release notes file not found at $NOTES_FILE" >&2; exit 1; }
fi

run "sh '$ROOT/packaging/deb/build-deb.sh'"
run "git -C '$ROOT' tag '$TAG'"
run "git -C '$ROOT' push origin '$TAG'"
run "gh release create '$TAG' '$ROOT'/dist/huck_*.deb --title '$TAG' --notes-file '$NOTES_FILE'"

if [ "$DRY_RUN" = 1 ]; then
    printf '+ compute sha256 of %s\n' "$TARBALL_URL"
    printf '+ render formula (url=%s) and push to jdstanhope/homebrew-huck\n' "$TARBALL_URL"
else
    SHA=$(curl -fsSL "$TARBALL_URL" | sha256sum | cut -d' ' -f1)
    pack_render_formula "$VERSION" "$SHA" > "$ROOT/packaging/homebrew/huck.rb"
    echo "release: formula rendered with sha256 $SHA — run scripts/create-tap.sh (first time) or push packaging/homebrew/huck.rb to the tap"
fi
printf 'done\n'
