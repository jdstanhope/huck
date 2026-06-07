#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v104: the linear source reader.
# Each fragment is fed as a whole script to both shells; check compares
# combined stdout+stderr+EXIT to verify the tokenize-once / parse+execute-
# one-command-at-a-time reader matches bash across multi-line constructs.
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

# 1: semicolon list + and-or on one line
check "semicolon and-or" 'echo a; echo b && echo c'

# 2: multi-line if/then/fi then a following command
check "multiline if" 'if true
then echo hi
fi
echo after'

# 3: multi-line while loop
check "multiline while" 'i=0
while [ $i -lt 3 ]; do
  echo $i
  i=$((i+1))
done'

# 4: case across multiple lines
check "multiline case" 'case foo in
  bar) echo no ;;
  foo) echo yes ;;
esac'

# 5: function definition then call with an argument
check "function def+call" 'greet() {
  echo hello $1
}
greet world'

# 6: heredoc body
check "heredoc body" 'cat <<EOF
l1
l2
EOF
echo done'

# 7: blank lines and a comment interspersed
check "blanks and comment" 'echo a

# a comment

echo b'

# 8: mid-file `shopt -s extglob` then an extglob case pattern
check "mid-file extglob" 'shopt -s extglob
case abc in @(abc|xyz)) echo hit;; *) echo miss;; esac'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
