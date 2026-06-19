#!/usr/bin/env bash
# Build the runtime-sweep sandbox image. Run from the repo root:
#   sg docker -c 'bash tools/runsweep/build.sh'
# Prereq: a release huck binary (cargo build --release) and tools/runsweep/paths.txt.
set -eu
cd "$(git rev-parse --show-toplevel)"
DIR=tools/runsweep

[ -x target/release/huck ] || { echo "build huck first: cargo build --release" >&2; exit 1; }
[ -s "$DIR/paths.txt" ]    || { echo "missing $DIR/paths.txt" >&2; exit 1; }

# Stage the build context (the COPY sources must sit next to the Dockerfile).
cp target/release/huck "$DIR/huck"

# Corpus tarball: tar strips the leading `/` from each listed path (storing it
# relative), so extracting with `-C /` in the image restores the absolute path.
# `--no-recursion` because paths.txt lists files, not directories.
echo "packing corpus ($(wc -l < "$DIR/paths.txt") files)…"
tar czf "$DIR/corpus.tgz" --no-recursion -T "$DIR/paths.txt" 2>/dev/null
echo "corpus.tgz: $(du -h "$DIR/corpus.tgz" | cut -f1)"

echo "building image huck-runsweep…"
docker build -t huck-runsweep "$DIR"

# Tidy the staged binary/tarball (kept out of git via tools/runsweep/.gitignore).
rm -f "$DIR/huck" "$DIR/corpus.tgz"
echo "done: image 'huck-runsweep'"
