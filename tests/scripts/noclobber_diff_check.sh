#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v123: noclobber (set -C) + >| redirect
# (M-21). File-arg execution. The noclobber *error message* prefix differs
# (huck: vs bash: line N:), so blocking cases suppress stderr and assert
# rc + file content; the error text is checked in the Rust integration test.
# (file-arg execution avoids huck's history-expansion-on-piped-stdin divergence, L-27.)
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# check: capture stdout+stderr (for non-blocking cases where output is identical)
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# check_nostderr: suppress stderr (for blocking cases whose error prefix differs).
# Compares stdout + exit code only.
check_nostderr() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check_nostderr "blocked-overwrite" 'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo new > "$d/f"; echo "rc=$? c=$(cat "$d/f")"'
check         "force-clobber"     'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo new >| "$d/f"; echo "rc=$? c=$(cat "$d/f")"'
check         "append-allowed"    'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo more >> "$d/f"; echo "rc=$?"; cat "$d/f"'
check         "new-file-allowed"  'd=$(mktemp -d); set -C; echo new > "$d/nf"; echo "rc=$? c=$(cat "$d/nf")"'
check         "devnull-exempt"    'set -C; echo x > /dev/null; echo "rc=$?"'
check         "stderr-force"      'd=$(mktemp -d); echo orig > "$d/f"; set -C; cat /nonexistent_huck_xyz 2>| "$d/f"; cat "$d/f"'
check_nostderr "ampredir-blocked"  'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo new &> "$d/f"; echo "c=$(cat "$d/f")"'
check         "toggle-off"        'd=$(mktemp -d); echo orig > "$d/f"; set -C; set +C; echo new > "$d/f"; echo "c=$(cat "$d/f")"'
check         "off-baseline"      'd=$(mktemp -d); echo orig > "$d/f"; echo new > "$d/f"; echo "c=$(cat "$d/f")"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
