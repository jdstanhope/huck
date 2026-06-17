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
