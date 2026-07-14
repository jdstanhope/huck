# Reading payload (consumes stdin) + a reading tri.
RD='{ IFS= read -r __l; printf "IN[%s]\n" "$__l"; }'
rdtri() { local L="$1" R="$2"
  check "inproc-rd $L" "$RD $R$SUF"
  check "extern-rd $L" "/bin/cat $R; printf 'RC=%s V=%s\n' \"\$?\" \"\${v-unset}\""
}

# ===== A. SINGLE ATOMS (output payload) =====
tri "out >f"          '>f'
tri "out >>f"         '>>f'
tri "out 2>f"         '2>f'
tri "out >|f"         '>|f'
tri "out <>f"         '<>f'
tri "out &>f"         '&>f'
tri "out &>>f"        '&>>f'
tri "dup 1>&2"        '1>&2'
tri "dup 2>&1"        '2>&1'
tri "dup >&2"         '>&2'
tri "close >&-"       '>&-'
tri "close 2>&-"      '2>&-'
tri "baddup >&9"      '>&9'
tri "baddup 2>&9"     '2>&9'
tri "move 2>&1-"      '2>&1-'
tri "move >&2-"       '>&2-'
tri "fd3 3>f"         '3>f'
tri "fd9 9>f"         '9>f'
tri "fd3 close 3>&-"  '3>&-'
# named-fd single
tri "nf {v}>f"        '{v}>f'
tri "nf {v}>&1"       '{v}>&1'
tri "nf {v}>&2"       '{v}>&2'
tri "nf {v}>&-"       '{v}>&-'
tri "nf {v}>&9"       '{v}>&9'

# ===== B. INTERACTION PAIRS (both orders) =====
tri "file+dup >f 2>&1"    '>f 2>&1'
tri "file+dup 2>&1 >f"    '2>&1 >f'
tri "baddup+file 2>&9 >f" '2>&9 >f'
tri "file+baddup >f 2>&9" '>f 2>&9'
tri "open+dup 3>f 4>&3"   '3>f 4>&3'
tri "dup+open 4>&3 3>f"   '4>&3 3>f'
tri "open+close 3>f 3>&-" '3>f 3>&-'
tri "close+dup 1>&- 2>&1" '1>&- 2>&1'
tri "twofile >a >b"       '>a >b'
tri "swap 3>&1 1>&2 2>&3" '3>&1 1>&2 2>&3'
tri "trip >f 2>&1 3>&2"   '>f 2>&1 3>&2'
# named-fd interactions (THE REGRESSION CLUSTER)
tri "nf+use {v}>f 2>&\$v"  '{v}>f 2>&$v'
tri "use+nf 2>&\$v {v}>f"  '2>&$v {v}>f'
tri "nf+fail {v}>f 2>&9"   '{v}>f 2>&9'
tri "nf+num 3>a {v}>x"     '3>a {v}>x'
tri "num+nf {v}>x 3>a"     '{v}>x 3>a'
tri "twonf {v}>a {w}>b"    '{v}>a {w}>b'
tri "nf move {v}>&1-"      '{v}>&1-'

# ===== C. INPUT redirects (reading payload) =====
rdtri "in <f1"        '<f1'
rdtri "in 0<f1"       '0<f1'
rdtri "in dup <&0"    '<&0'
rdtri "in herestr"    '<<<hey'
rdtri "in badin <&9"  '<&9'
rdtri "in fd3 3<f1 <&3" '3<f1 <&3'
check "heredoc read"  "$RD <<EOF
line1
EOF"
check "heredoc-var read" "x=hi; $RD <<EOF
\$x
EOF"
check "heredoc-quoted"   "$RD <<'EOF'
\$x lit
EOF"

# ===== D. bash-suite-inspired constructs =====
# canonical: assignment prefix + 2>&1 >file ordering (redir.tests a=N echo)
check "asgn echo 2>&1>f builtin" 'a=2 echo foo 2>&1 >f; printf "S["; cat f; printf "]\n"'
check "asgn echo 2>&1>f extern"  'a=2 /bin/echo foo 2>&1 >f; printf "S["; cat f; printf "]\n"'
check "echo 2>&1 >f then cat"    'echo foo 2>&1 >f; cat f'
# exec dup + use + close lifecycle
check "exec 3>&1 use close"      'exec 3>&1; echo hi >&3; exec 3>&-; echo done'
check "exec dup then bad"        'exec 9>&2; echo x >&9; exec 9>&-; echo ok'
check "exec move 0<&0-"          'exec 3<f1; exec 0<&3-; IFS= read -r l; echo "got=$l"'
# nested subshell dup (redir9/10.sub style)
check "nested subshell dup"      '( ( echo hello 1>&3 ) 3>&1 )'
check "nested 3>f capture"       '( ( echo hi 1>&3 ) 3>f ); cat f'
# builtin writing to closed stdout (#137 class)
check "builtin closed stdout"    'echo hi >&-; echo "rc=$?"'
check "printf closed stdout"     'printf hi >&-; echo "rc=$?"'
# read from herestring / heredoc
check "read herestring"          'read x <<< "hello world"; echo "[$x]"'
check "read -u fd"               'exec 5<f1; read -u 5 y; echo "[$y]"; exec 5<&-'
# both streams to one file via order
check "both to file"             '{ echo a; echo b >&2; } >f 2>&1; sort f'
# multiple redirects same fd last-wins
check "lastwins >a >b >c"        'echo hi >a >b >c; printf "a[%s]b[%s]c[%s]\n" "$(cat a 2>/dev/null)" "$(cat b 2>/dev/null)" "$(cat c 2>/dev/null)"'
# exec {var} lifecycle (persistence)
check "exec nf lifecycle"        'exec {v}>f; echo hi >&$v; echo "v=$v"; exec {v}>&-; cat f'
check "nf then close then use"   'exec {v}>f; exec {v}>&-; echo x >&$v 2>/dev/null; echo "rc=$?"'
