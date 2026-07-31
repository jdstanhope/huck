#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `case` roots fixed in v351 (#350):
#   Root 1: a backslash from an UNQUOTED expansion is a pattern escape
#           (`\x`→x, `\*`→literal *, `\\`→literal \), spanning case / [[ == ]] /
#           ${x#pat}; a quoted "$x" stays literal; the regex path (`[[ =~ ]]`)
#           keeps GNU-ERE `\b`/`\w`/`\s` active.
#   Root 2: a `$((xx++))` readonly error in a case pattern reports ONCE (bare)
#           and discards the case with $?=1.
# Backslashes are fragile through `-c`, so most fragments run from a HERE-DOC
# script fed to `bash -s` / `huck -s`; stdout+stderr+exit must match byte-for-byte.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Normalize the leading program-name token on error lines (`bash: line N:` vs
# `/path/to/huck: line N:`) — a non-behavioral argv[0] artifact of piping the
# script on stdin, not a huck<->bash difference.
norm() { sed 's|^[^:]*: line |PROG: line |'; }
# Run an identical multi-line script through bash and huck (script on stdin);
# compare all output + exit.
check_script() {
    local label="$1" script="$2" b h
    b=$(printf '%s' "$script" | bash --norc --noprofile 2>&1 | norm; echo "EXIT:${PIPESTATUS[1]}")
    h=$(printf '%s' "$script" | "$HUCK_BIN" 2>&1 | norm; echo "EXIT:${PIPESTATUS[1]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- Root 1: backslash from an unquoted expansion is a pattern escape ---
check_script "glob \\x matches x"    $'m() { case "$2" in $1) echo M;; *) echo n;; esac; }\np=\'\\\\x\'; m "$p" x\n'
check_script "glob \\* literal star" $'m() { case "$2" in $1) echo M;; *) echo n;; esac; }\np=\'\\\\*\'; m "$p" \'*\'; m "$p" ab\n'
check_script "glob \\\\ literal bslash" $'m() { case "$2" in $1) echo M;; *) echo n;; esac; }\np=\'\\\\\\\\\'; m "$p" \'\\\\\'\n'
check_script "glob a\\*b"            $'m() { case "$2" in $1) echo M;; *) echo n;; esac; }\np=\'a\\\\*b\'; m "$p" \'a*b\'; m "$p" axb\n'
check_script "bare * stays active"  $'m() { case "$2" in $1) echo M;; *) echo n;; esac; }\np=\'*\'; m "$p" ab\n'
check_script "trailing \\ is literal" $'m() { case "$2" in $1) echo M;; *) echo n;; esac; }\np=\'ab\\\\\'; m "$p" \'ab\\\\\'; m "$p" ab\n'
check_script "KEEP quoted \"\$x\" literal" $'x=\'\\\\x\'\ncase x in "$x") echo M;; *) echo n;; esac\ncase \'\\\\x\' in "$x") echo M2;; *) echo n2;; esac\n'
check_script "[[ == ]] \\x matches x" $'x=\'\\\\x\'\n[[ x == $x ]] && echo M || echo n\n'
check_script "\${x#pat} strips \\x"  $'v=xy; p=\'\\\\x\'\necho "${v#$p}"\n'
# regex path keeps GNU-ERE extensions active (Root-1 fix must NOT reach regex)
check_script "[[ =~ ]] keeps \\w"    $'p=\'\\\\w\'; [[ a =~ $p ]] && echo M || echo n\np=\'\\\\bx\'; [[ x =~ $p ]] && echo Mb || echo nb\n'

# --- Root 2: $((xx++)) readonly in a case pattern (single error + $?=1) ---
check_script "case arith readonly"   $'readonly xx=1\ncase 1 in $((xx++)) ) echo hi1 ;; *) echo hi2; esac\necho ${xx}.$?\n'
# KEEP: non-error arithmetic patterns with ;& fall-through
check_script "KEEP arith pattern"    $'x=0 y=1\ncase 1 in $((y=0)) ) ;; $((x=1)) ) ;& $((x=2)) ) echo $x.$y ;; esac\n'
# KEEP: other arith errors stay wrapped (div0 / syntax)
check_script "KEEP div0 wrapped"     $'echo $((1/0))\n'
check_script "KEEP syntax wrapped"   $'echo $((4 + ))\n'
# KEEP: ordinary case exit status
check_script "KEEP ordinary case"    $'case x in x) true;; esac; echo $?\ncase y in x) true;; esac; echo $?\n'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
