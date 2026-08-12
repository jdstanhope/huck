#!/usr/bin/env bash
# Byte-identical bash<->huck harness for LOGICAL `cd`'s physical fallback
# (#517).
#
# bash's `change_to_directory` does not give up when the canonicalized path
# cannot be entered: it retries the argument AS WRITTEN, and on success takes
# the new directory from getcwd() rather than from the canonical name. Two
# setups need it, and huck failed both:
#
#   1. An ancestor that lost search permission after the shell moved inside it.
#      `chdir("/a/b/c")` is EACCES while `chdir(".")` still succeeds, so bash's
#      `cd .` is a no-op success where huck reported `Permission denied`.
#
#   2. A logical path through a symlink whose canonical form does not exist.
#      From a symlinked `lnk -> p/q`, bash's `cd ../q` lands in `p/q` and `$PWD`
#      becomes the PHYSICAL path; huck reported `No such file or directory`.
#
# On a SECOND failure the error bash reports is the FIRST one: `cd ./nosuch`
# under setup 1 says `Permission denied` (the canonical attempt) and not the
# `No such file or directory` the literal retry would give.
#
# Both shells run with an EXPLICIT $0 ("huck5") so the error prologue matches.
# Every case runs in a fresh tree, and the revoked-permission directory is
# restored afterwards so the cleanup can remove it.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-cdlog.XXXXXX")
trap 'chmod -R u+rwX "$TMPROOT" 2>/dev/null; rm -rf "$TMPROOT"' EXIT

# The `$PWD`s printed by these fragments contain the temp path, which differs
# between the two runs only if the two runs use different directories — so each
# case gets its OWN directory and the path is edited out of both outputs.
run() {
    local kind="$1" setup="$2" frag="$3" d out
    d=$(mktemp -d "$TMPROOT/case.XXXXXX")
    mkdir -p "$d/t9/sub" "$d/p/q"
    ln -s p/q "$d/lnk"
    if [[ "$kind" == bash ]]; then
        out=$( cd "$d/$setup" && timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1 )
    else
        out=$( cd "$d/$setup" && timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1 )
    fi
    printf '%s\n' "${out//$d/ROOT}"
    chmod -R u+rwX "$d" 2>/dev/null
    rm -rf "$d"
}

check() {
    local label="$1" setup="$2" frag="$3" b h
    b=$(run bash "$setup" "$frag")
    h=$(run huck "$setup" "$frag")
    compare "$label" "$b" "$h"
}

# --- setup 1: an ancestor loses search permission under our feet ------------
# Each fragment revokes it itself, so the two shells see identical state.
REVOKE='chmod 000 ../../t9;'

check "cd . stays put"      t9/sub "$REVOKE"' cd .; echo "rc=$? PWD=$PWD"'
check "cd ./."              t9/sub "$REVOKE"' cd ./.; echo "rc=$?"'
check "cd ././."            t9/sub "$REVOKE"' cd ././.; echo "rc=$?"'
check "cd ./ trailing"      t9/sub "$REVOKE"' cd ./; echo "rc=$?"'
check "cd -e ."             t9/sub "$REVOKE"' cd -e .; echo "rc=$? PWD=$PWD"'
check "cd -P ."             t9/sub "$REVOKE"' cd -P .; echo "rc=$?"'
check "cd -L ."             t9/sub "$REVOKE"' cd -L .; echo "rc=$?"'
# The forms that must STILL fail — an absolute or upward path cannot be walked.
check "cd .. still fails"   t9/sub "$REVOKE"' cd ..; echo "rc=$?"'
check "cd -P .. fails"      t9/sub "$REVOKE"' cd -P ..; echo "rc=$?"'
check "cd \$PWD fails"      t9/sub "$REVOKE"' cd "$PWD"; echo "rc=$?"'
check "cd abs/. fails"      t9/sub "$REVOKE"' cd "$(pwd)/."; echo "rc=$?"'
check "cd sub2 fails"       t9/sub "$REVOKE"' cd sub2; echo "rc=$?"'
# The first error wins: EACCES from the canonical attempt, not ENOENT from the
# literal retry.
check "cd ./nosuch errno"   t9/sub "$REVOKE"' cd ./nosuch; echo "rc=$?"'
check "cd nosuch errno"     t9/sub "$REVOKE"' cd nosuch; echo "rc=$?"'

# --- setup 2: a logical path whose canonical form does not exist ------------
check "symlink cd ../q"     lnk 'cd ../q; echo "rc=$? PWD=$PWD"; pwd -P'
check "symlink cd -e ../q"  lnk 'cd -e ../q; echo "rc=$? PWD=$PWD"'
check "symlink cd -P ../q"  lnk 'cd -P ../q; echo "rc=$? PWD=$PWD"'
check "symlink cd ../nope"  lnk 'cd ../nope; echo "rc=$?"'
check "symlink cd .."       lnk 'cd ..; echo "rc=$? PWD=$PWD"'
check "symlink cd . "       lnk 'cd .; echo "rc=$? PWD=$PWD"'

# --- controls: ordinary logical cd is untouched ------------------------------
check "plain cd sub"        t9 'cd sub; echo "rc=$? PWD=$PWD"'
check "plain cd sub then .." t9 'cd sub; cd ..; echo "rc=$? PWD=$PWD"'
check "plain cd nosuch"     t9 'cd nosuch; echo "rc=$?"'
check "plain cd file"       t9 'touch f; cd f; echo "rc=$?"'

harness_summary
