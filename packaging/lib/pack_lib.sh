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
