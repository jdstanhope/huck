#!/usr/bin/env bash
# Byte-identical bash<->huck harness: a `name=(…)` compound array-literal
# assignment is accepted ONLY in an assignment-acceptable position — a LEADING
# assignment, or an argument to a declaration builtin (declare/typeset/local/
# export/readonly/alias/eval/let). In plain argument position bash rejects the
# unexpected `(` (rc 2); huck must match. Compares exit code AND stderr-presence
# (an error line was / was not emitted) for each fragment.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Compare parse outcome (rc + whether stderr is non-empty) between bash and huck.
check() {
    local label="$1" frag="$2" brc hrc berr herr
    local btxt htxt
    btxt=$(printf '%s\n' "$frag" | timeout 10 bash --norc --noprofile -n 2>/tmp/al_b.$$); brc=$?
    htxt=$(printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" -n 2>/tmp/al_h.$$); hrc=$?
    [[ -s /tmp/al_b.$$ ]] && berr=1 || berr=0
    [[ -s /tmp/al_h.$$ ]] && herr=1 || herr=0
    rm -f /tmp/al_b.$$ /tmp/al_h.$$
    if [[ "$brc" == "$hrc" && "$berr" == "$herr" ]]; then
        printf 'PASS: %-34s rc=%s err=%s\n' "$label" "$brc" "$berr"; PASS=$((PASS+1))
    else
        printf 'FAIL: %-34s bash(rc=%s err=%s) huck(rc=%s err=%s)\n' \
            "$label" "$brc" "$berr" "$hrc" "$herr"; FAIL=$((FAIL+1))
    fi
}

# --- Rejected: array literal in plain ARGUMENT position ---
check "arg after cmd"          'printf "%s\n" -a a=(a b)'
check "plain echo arg"         'echo a=(x)'
check "append as arg"          'echo a+=(3)'
check "two array args"         'foo a=(1) b=(2)'
check "cmd then two arrays"    'cmd a=(1 2) b=(3 4)'
check "command builtin prefix" 'command declare a=(1 2)'
check "mid-command"            'pre a=(1 2) post'

# --- Accepted: leading assignments ---
check "sole leading"           'a=(1 2)'
check "two leading"            'a=(1) b=(2)'
check "scalar + array"         'x=1 a=(1 2)'
check "leading append"         'a+=(3)'
check "element assign"         'a[0]=x'

# --- Accepted: declaration-builtin arguments ---
check "declare arg"            'declare -a a=(1 2)'
check "declare two arrays"     'declare a=(1 2) b=(3 4)'
check "typeset arg"            'typeset a=(1 2)'
check "local arg"             'f(){ local a=(1 2); }; f'
check "export arg"             'export a=(1 2)'
check "readonly arg"           'readonly a=(1 2)'
check "alias arg"              'alias a=(1 2)'
check "eval arg"               'eval a=(1 2)'
check "let arg"                'let a=(1 2)'
check "decl after leading"     'x=1 declare a=(1 2)'

# --- Unaffected: quoted / non-array shapes ---
check "quoted =("              'echo "a=(x)"'
check "quoted arg"             'echo a="(x)"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
