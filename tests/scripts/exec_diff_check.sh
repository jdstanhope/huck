#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `exec` builtin. Each fragment runs
# through `bash -c` and `huck -c`; stdout+stderr+exit must match.
#
# Two modes:
#   - `exec command [args]` replaces the shell process image (no fork).
#   - `exec [redirections]` (no command) applies them permanently to the shell.
#
# NOT byte-compared (known divergences): failure DIAGNOSTICS differ in wording
# ("huck: exec: NAME: not found" vs "bash: line N: NAME: No such file or
# directory"), so failure cases here suppress stderr and assert only the exit
# status. fd>2 redirects (`exec 3<file`) are unsupported by huck's command AST.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- mode 1: replace the process image ---
check "replaces process"        'exec echo hi; echo NOT_REACHED'
check "passes args"             'exec printf "%s-%s\n" a b'
check "exit code is true"       'exec true'
check "exit code is false"      'exec false; echo NOT_REACHED'
check "inherits exported var"   'export V=xyz; exec sh -c "echo $V"'
check "inline assign exported"  "V=abc exec sh -c 'echo \$V'"
check "-a sets argv0"           'exec -a MYNAME sh -c "echo \$0"'
check "-- ends options"         'exec -- echo dashdash'
check "in a subshell"           '(exec echo sub); echo parent-alive'
check "as a pipeline stage"     'exec cat <<< piped | tr a-z A-Z'
check "command-prefixed exec"   'command exec echo viacommand'

# --- mode 2: permanent redirections (no command) ---
check "bare exec is noop rc0"   'exec; echo "still-here rc=$?"'
check ">file then writes"       'f=$(mktemp); exec > "$f"; echo line1; echo line2; exec 1>&2; cat "$f" 1>&2; rm -f "$f"'
check ">>file appends"          'f=$(mktemp); printf head > "$f"; exec >> "$f"; echo body; exec 1>&2; cat "$f" 1>&2; rm -f "$f"'
check "<file becomes stdin"     'f=$(mktemp); printf "a\nb\n" > "$f"; exec < "$f"; read x; read y; echo "$x$y" 1>&2; rm -f "$f"'

# --- failure: status only (stderr suppressed; wording diverges) ---
check "missing command exits 127" '(exec /no/such/cmd_xyz) 2>/dev/null; echo "rc=$?"'
check "failed redirect continues"  '(exec > /no/such_dir_xyz/f) 2>/dev/null; echo "rc=$?"'
# NOTE: `exec -Z` (bad option) yields rc 2 in both shells, but the diagnostic
# wording diverges and bash's same-command `2>/dev/null` is applied around the
# builtin (suppressing it) whereas huck parses exec flags before its permanent
# redirect — so it is not byte-comparable. parse_exec_flags has a unit test.

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
