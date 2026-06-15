#!/usr/bin/env bash
# Runtime sweep batch 4 — feature-combination fragments, bash vs huck.
# Compares STDOUT + EXIT CODE (stderr wording legitimately differs).
# Read-only / tmp-only fragments; no $RANDOM/$SECONDS/PID/timestamps.
set -u
H="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$H" ]] || { echo "build huck first: $H" >&2; exit 1; }
PASS=0; FAIL=0; FAILED=()

chk() { # label ; fragment
  local l="$1" f="$2" b h be he
  b=$(bash --norc --noprofile -c "$f" 2>/dev/null); be=$?
  h=$("$H" -c "$f" 2>/dev/null); he=$?
  if [[ "$b" == "$h" && "$be" == "$he" ]]; then
    PASS=$((PASS+1))
  else
    FAIL=$((FAIL+1)); FAILED+=("$l")
    printf '\n### FAIL: %s\n    frag: %s\n' "$l" "$f"
    printf '    bash(rc=%s): %q\n    huck(rc=%s): %q\n' "$be" "$b" "$he" "$h"
  fi
}

# --- parameter-expansion transforms (@Q @A @E @P @U @L @u) ---
chk 'at-Q quote'          'v="a b\"c"; printf "%s\n" "${v@Q}"'
chk 'at-U upper'          'v=hello; echo "${v@U}"'
chk 'at-L lower'          'v=HeLLo; echo "${v@L}"'
chk 'at-u firstupper'     'v=hello; echo "${v@u}"'
chk 'at-E escapes'        'v="a\tb"; echo "${v@E}"'
chk 'at-A assign-form'    'declare -i n=5; echo "${n@A}"'

# --- case modification on whole array / first char ---
chk 'arr upper all'       'a=(foo bar); echo "${a[@]^^}"'
chk 'arr lower first'     'a=(FOO BAR); echo "${a[@],}"'
chk 'scalar upper patt'   'v=banana; echo "${v^^a}"'

# --- substring / replace edge cases ---
chk 'neg offset'          'v=abcdef; echo "${v: -2}"'
chk 'offset len neg'      'v=abcdef; echo "${v:1:-1}"'
chk 'replace anchored #'  'v=aXbXc; echo "${v/#a/Z}"'
chk 'replace anchored %'  'v=aXbXc; echo "${v/%c/Z}"'
chk 'replace all'         'v=a.b.c; echo "${v//./-}"'
chk 'replace empty-pat'   'v=abc; echo "${v/b}"'

# --- arrays: negative index, slices, keys, append ---
chk 'neg index'           'a=(1 2 3 4); echo "${a[-1]} ${a[-2]}"'
chk 'arr slice @'         'a=(a b c d e); echo "${a[@]:1:2}"'
chk 'arr keys'            'a=(x y z); echo "${!a[@]}"'
chk 'sparse keys'         'a=(); a[3]=x; a[7]=y; echo "${!a[@]}"'
chk 'append array'        'a=(1 2); a+=(3 4); echo "${a[*]}"'
chk 'assoc iterate'       'declare -A m=([b]=2 [a]=1); for k in "${!m[@]}"; do echo "$k=${m[$k]}"; done | sort'
chk 'count star'          'a=(1 2 3); echo "${#a[@]} ${#a[*]}"'

# --- indirect / prefix ---
chk 'indirect ref'        'x=val; y=x; echo "${!y}"'
chk 'prefix names'        'aa=1; ab=2; ac=3; echo "${!a*}"'

# --- read / mapfile ---
chk 'read -a'             'read -a arr <<< "p q r"; echo "${arr[1]}"'
chk 'read -r backslash'   'printf "a\\\\b\n" | { read -r x; echo "$x"; }'
chk 'read two vars'       'echo "one two three" | { read a b; echo "[$a][$b]"; }'
chk 'read IFS'            'IFS=: read a b c <<< "x:y:z"; echo "$a-$b-$c"'
chk 'mapfile'             'mapfile -t lines <<< $'"'"'a\nb\nc'"'"'; echo "${#lines[@]} ${lines[2]}"'
chk 'readarray n'         'readarray -t -n 2 v <<< $'"'"'1\n2\n3'"'"'; echo "${v[*]}"'

# --- case / extglob ---
chk 'case fallthrough'    'shopt -s extglob; for x in 1 2; do case $x in 1) echo a;;& 2) echo b;; esac; done'
chk 'extglob plus'        'shopt -s extglob; v=aaa; [[ $v == +(a) ]] && echo match'
chk 'extglob at'          'shopt -s extglob; v=cat; [[ $v == @(cat|dog) ]] && echo yes'
chk 'extglob neg'         'shopt -s extglob; v=foo.txt; [[ $v == !(*.bak) ]] && echo keep'

# --- regex =~ / BASH_REMATCH ---
chk 'regex match'         '[[ abc123 =~ ([a-z]+)([0-9]+) ]] && echo "${BASH_REMATCH[1]}/${BASH_REMATCH[2]}"'
chk 'regex anchored'      '[[ foobar =~ ^foo ]] && echo head'
chk 'regex nomatch rc'    '[[ xyz =~ ^[0-9]+$ ]]; echo $?'

# --- brace expansion ---
chk 'brace range'         'echo {1..5}'
chk 'brace step'          'echo {0..10..2}'
chk 'brace alpha'         'echo {a..e}'
chk 'brace nested'        'echo {a,b}{1,2}'
chk 'brace zeropad'       'echo {01..05}'
chk 'brace rev'           'echo {5..1}'

# --- arithmetic odds ---
chk 'base N'              'echo $((16#ff)) $((2#1010)) $((8#17))'
chk 'comma in arith'      'echo $((a=3, b=4, a*b))'
chk 'bit ops'             'echo $((5 & 3)) $((5 | 2)) $((5 ^ 1)) $((~0))'
chk 'pre/post mix'        'i=5; echo $((i++ + ++i))'
chk 'arith for'           's=0; for ((i=1;i<=4;i++)); do s=$((s+i)); done; echo $s'
chk 'ternary'             'x=7; echo $(( x>5 ? 100 : 200 ))'

# --- getopts ---
chk 'getopts basic'       'set -- -a -b val x; while getopts "ab:" o; do case $o in a) echo flagA;; b) echo "B=$OPTARG";; esac; done; shift $((OPTIND-1)); echo "rest=$1"'

# --- printf -v / reuse ---
chk 'printf -v'           'printf -v out "%05d" 42; echo "$out"'
chk 'printf reuse'        'printf "%s\n" a b c'
chk 'printf %b'           'printf "%b" "x\ty\n"'

# --- ANSI-C quoting ---
chk 'ansi-c tab'          'printf "%s" $'"'"'a\tb\n'"'"' | od -An -c | tr -s " "'
chk 'ansi-c hex'          'echo $'"'"'\x41\x42'"'"''
chk 'ansi-c unicode'      'echo $'"'"'é'"'"''

# --- string ops anchored ---
chk 'strip shortest #'    'v=aXbXc; echo "${v#*X}"'
chk 'strip longest ##'    'v=aXbXc; echo "${v##*X}"'
chk 'strip shortest %'    'v=aXbXc; echo "${v%X*}"'
chk 'strip longest %%'    'v=aXbXc; echo "${v%%X*}"'

# --- declare -p formatting ---
chk 'declare -p scalar'   'x=hi; declare -p x'
chk 'declare -p array'    'a=(1 2 3); declare -p a'
chk 'declare -p assoc'    'declare -A m=([k]=v); declare -p m'

# --- misc ---
chk 'cmdsub trim nl'      'x=$(printf "line\n\n\n"); echo "[$x]"'
chk 'nested cmdsub'       'echo "$(echo "$(echo deep)")"'
chk 'word split IFS'      'IFS=,; v=a,b,c; set -- $v; echo "$# $2"'
chk 'here-doc tabs'       'cat <<-EOF
	indented
	EOF'
chk 'heredoc expand'      'x=world; cat <<EOF
hello $x
EOF'
chk 'array in func local' 'f(){ local -a la=(1 2 3); echo "${la[1]}"; }; f; echo "${la[1]:-unset}"'

printf '\n========================================\n%d passed, %d failed\n' "$PASS" "$FAIL"
if ((FAIL)); then printf 'Failures: %s\n' "${FAILED[*]}"; fi