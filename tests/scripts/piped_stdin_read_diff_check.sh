#!/usr/bin/env bash
# Byte-identical bash<->huck harness: when a script is read from a NON-TTY stdin
# (a pipe or `< file`), the shell must read it one line at a time from fd 0 so
# that a child process or the `read` builtin sharing fd 0 sees the correct
# stream position. huck used to read the whole script ahead (rustyline's
# readline_direct BufReader), so a child `read`/`cat` saw EOF and the parent
# then ran the swallowed lines as commands. Compares stdout+stderr+rc.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Run FRAGMENT through both shells via a PIPE (non-tty stdin); compare all output.
check() { local l="$1" f="$2" b h
  b=$(printf '%b' "$f" | bash 2>&1; echo "EXIT:$?")
  h=$(printf '%b' "$f" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# The core case: a child command reads the REST of the shared script stream.
check "child cat drains rest"   'echo A\ncat\nBBB\necho C\n'
# The parent `read` builtin consumes the next physical script line.
check "parent read next line"   'read x\nhello world\necho "[$x]"\n'
check "parent read multi-var"   'read a b c\n1 2 3 4\necho "$a|$b|$c"\n'
check "while read drains rest"  'while read ln; do echo "L:$ln"; done\nx\ny\nz\n'
# A subshell invocation whose child inherits fd 0 (the shape input-test tests).
check "child shell reads line"  'echo pre\nsh -c "read v; echo child-got:\$v"\nDATA HERE\necho post\n'

# Regressions: normal piped scripts must still parse/run identically.
check "simple multi-line"       'echo a\necho b\necho c\n'
check "multi-line if"           'if true; then\necho yes\nfi\n'
check "for loop"                'for i in 1 2 3; do echo $i; done\n'
check "heredoc body"            'cat <<EOF\nl1\nl2\nEOF\necho after\n'
check "backslash continuation"  'echo one \\\ntwo\n'
check "multi-line dquote"       'echo "start\nmid\nend"\n'
check "function def + call"     'foo() {\n  echo hi\n}\nfoo\n'
check "comment line"            'echo x\n# comment\necho y\n'
check "here-string read"        'read a b <<< "1 2"; echo "$a-$b"\necho next\n'
check "pipe-scoped read"        'echo hi | while read q; do echo "q=$q"; done\necho next\n'
check "no trailing newline"     'echo nonewline'
check "empty stdin"             ''

echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ "$FAIL" -eq 0 ]]
