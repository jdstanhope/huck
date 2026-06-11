#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v138: an untrapped SIGINT aborts the
# running command list; a user INT trap runs and continues; `trap '' INT`
# ignores. Deterministic via `kill -INT $$` (no PTY/timing). Each fragment is run
# as a FILE-ARG (an isolated child); stdout AND exit code are compared.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
TMP=$(mktemp -d)
check() {
    local label="$1" frag="$2" f="$TMP/frag.sh"
    printf '%s\n' "$frag" >"$f"
    local bo bc ho hc
    bo=$(bash "$f" 2>/dev/null); bc=$?
    ho=$("$HUCK_BIN" "$f" 2>/dev/null); hc=$?
    if [[ "$bo" == "$ho" && "$bc" == "$hc" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (bash rc=%s huck rc=%s)\n' "$label" "$bc" "$hc"
        diff <(printf '%s' "$bo") <(printf '%s' "$ho") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}
check "sequence abort"   'echo a; kill -INT $$; echo b'
check "loop abort"       'for i in 1 2 3; do echo $i; kill -INT $$; done; echo after'
check "function abort"   'f(){ echo a; kill -INT $$; echo b; }; f; echo after'
check "nested if abort"  'if true; then echo a; kill -INT $$; echo b; fi; echo c'
check "trap handler"     'trap "echo c" INT; echo a; kill -INT $$; echo b'
check "trap ignore"      'trap "" INT; echo a; kill -INT $$; echo b'
check "legit 130 no abort" 'f(){ return 130; }; f; echo still-here'
rm -rf "$TMP"
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
