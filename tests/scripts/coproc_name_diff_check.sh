#!/usr/bin/env bash
# Byte-identical bash<->huck harness: `coproc NAME compound-command` accepts ANY
# word as NAME (bash grammar `coproc WORD compound`) and defers the
# valid-identifier check to RUNTIME. A bogus name parses, then at runtime prints
# `` `NAME': not a valid identifier `` and does NOT start the coprocess (exit 1).
# A valid name (control) is unaffected. Compares full output + exit status.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Runtime parity: full stdout+stderr and exit status. huck's error prologue is
# `huck: …` vs bash's `bash: line N: …`, so strip the tool/line prefix before
# comparing the message body (huck matches bash's error TEXT, not its prologue).
runtime() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | timeout 10 bash --norc --noprofile 2>&1 \
        | sed -E 's/^bash: (line [0-9]+: )?//'; echo "EXIT:${PIPESTATUS[0]}")
    h=$(printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" 2>&1 \
        | sed -E 's#^[^:]*/?huck: (line [0-9]+: )?##'; echo "EXIT:${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Parse parity: `-n` exit status only (a bogus name must PARSE like bash).
parse() {
    local label="$1" frag="$2" brc hrc
    printf '%s\n' "$frag" | timeout 10 bash --norc --noprofile -n >/dev/null 2>&1; brc=$?
    printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" -n >/dev/null 2>&1; hrc=$?
    if [[ "$brc" == "$hrc" ]]; then printf 'PASS: %-28s (parse rc=%s)\n' "$label" "$brc"; PASS=$((PASS+1))
    else printf 'FAIL: %-28s bash=%s huck=%s\n' "$label" "$brc" "$hrc"; FAIL=$((FAIL+1)); fi
}

# --- Parse: bogus names still PARSE (rc 0) ---
parse "bogus @"          'coproc @ { :; }'
parse "bogus digit"      'coproc 123 { :; }'
parse "bogus hyphen"     'coproc foo-bar { :; }'
parse "bogus @ subshell" 'coproc @ ( : )'
parse "valid name"       'coproc MYCO { :; }'
parse "anonymous"        'coproc { :; }'

# --- Runtime: bogus name → not-a-valid-identifier, coproc not started ---
runtime "runtime @"        'coproc @ { echo hi; }; echo "rc=$?"'
runtime "runtime digit"    'coproc 1x { echo hi; }; echo "rc=$?"'
runtime "runtime hyphen"   'coproc a-b { echo hi; }; echo "rc=$?"'
# Valid-name control: coproc starts, roundtrip works.
runtime "valid roundtrip"  'coproc MYP { read l; echo "e:$l"; }; echo yo >&"${MYP[1]}"; read r <&"${MYP[0]}"; echo "$r"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
