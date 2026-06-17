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
