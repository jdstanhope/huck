#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `$_` (the last argument of the
# previous command). Each fragment runs through `bash -c` and `huck -c`;
# stdout+exit must match.
#
# SCOPE: the common, deterministic case only — after a simple command runs,
# `$_` is its last argument (post-expansion), or the program name when it had
# no arguments. Covers builtins, externals, functions, and the `command`/
# `builtin` prefixes.
#
# DROPPED (non-deterministic / out of scope, not asserted here):
#   - the STARTUP value of `$_` (bash: the invocation path; huck: argv0) —
#     read only if `$_` is referenced before any command runs;
#   - `$_` in a command's exported environment, after assignment-only
#     commands, and MAILPATH interaction.
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

# --- must-haves ---
check "echo last arg"          'echo a b c; echo "$_"'
check "colon builtin arg"      ': foo; echo "$_"'

# --- last argument of the previous simple command ---
check "echo multiword"         'echo hello world; echo "$_"'
check "true two args"          'true one two; echo "$_"'
check "single arg"             'echo solo; echo "$_"'

# --- no args: program name ---
check "echo no args"           'echo; echo "[$_]"'
check "colon no args"          ': ; echo "[$_]"'

# --- last arg is post-expansion ---
check "post expansion var"     'v=zzz; echo "$v"; echo "$_"'

# --- printf: last arg is the literal format/operand, not the output ---
check "printf format only"     "printf 'x\n'; echo \"[\$_]\""
check "printf with operand"    "printf '%s\n' hello; echo \"[\$_]\""

# --- redirections don't change the last arg ---
check "redirect out"           'ls /tmp >/dev/null; echo "$_"'

# --- inline assignment prefix: last arg of the command, not the assignment ---
check "inline assign prefix"   'x=5 echo hi; echo "$_"'

# --- command / builtin prefixes resolve to the real last arg ---
check "command prefix"         'command echo a b c; echo "[$_]"'
check "builtin prefix"         'builtin echo x y; echo "[$_]"'

# --- function call: last arg of the call ---
check "function call args"     'f(){ :; }; f arg1 arg2; echo "[$_]"'

# --- is-set semantics: `_` is always set after a command ---
check "is-set dash default"    ': hi; echo "[${_-UNSET}]"'
check "is-set -v test"         ': hi; [[ -v _ ]] && echo set || echo unset'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
