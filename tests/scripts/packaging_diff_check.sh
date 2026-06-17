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
