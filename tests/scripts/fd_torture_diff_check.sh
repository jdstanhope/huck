#!/usr/bin/env bash
# fd-plumbing remediation regression net (review: docs/superpowers/reviews/
# 2026-07-13-engine-fd-plumbing-review.md). Concentrated fd/redirect/pipeline/
# background matrix, restricted to behavior huck ALREADY matches bash on, so it is
# green today and its job is to catch REGRESSIONS as Phases 1-5 rework the fd
# machinery. #128 (no-hangup) and #129 (bg stdin) cases are added by their tasks.
# Deliberately excluded until their fixing phase: stage redirect source-order (#50).
# (#135 — in-process whole-command redirect on a freed std fd — was fixed in v291
# Phase 2; its cases are asserted below.)
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'FA\n' > "$WORK/inA"

norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }

# Byte-identical bash vs huck, both timeout-wrapped so a regression that
# reintroduces a hang FAILs instead of wedging. cwd is $WORK for relative paths.
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && timeout 5 bash        -c "$frag" </dev/null 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && timeout 5 "$HUCK_BIN" -c "$frag" </dev/null 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    b=${b//$WORK/@W@}; h=${h//$WORK/@W@}
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- freed std fds x pipelines (post-v288 correct) ---
check "freed fd0: cat|cat"        'exec <&-; cat | cat; echo end'
check "freed fd0: cat|cat|cat"    'exec <&-; cat | cat | cat; echo end'
check "freed fd2: err|cat"        'exec 2>&-; ls /no_such_xyz | cat; echo "end=$?"'
# --- #132: redirect fd landing on a freed std slot, pipeline-stage path (Phase 1 fix) ---
check "132 external stage on freed fd0" 'exec <&-; cat < inA | cat; echo end'
check "132 stdout to file on freed fd1" 'exec >&-; /bin/echo hi > f | cat; cat f'
check "132 bg pipeline file redirect"   'exec <&-; cat < inA | cat & wait; echo end'
# --- per-stage redirects on a non-last stage ---
check "stage stdout redirect"     'echo hi > f | cat; cat f'
check "stage stderr redirect"     'ls /no_such_xyz 2>e | cat; cat e'
check "fd>2 dup"                  'exec 3>f; echo x >&3; exec 3>&-; cat f'
# --- heredoc / here-string into a stage ---
check "heredoc into stage"        $'cat <<EOF | cat\nh1\nh2\nEOF'
check "herestring into stage"     'cat <<< "hs" | cat'
# --- subshell / group redirects ---
check "subshell redirect"         '(echo hi) 2>&1 | cat'
check "group redirect"            '{ echo hi; } > f; cat f'
# --- dup / close / merge ---
check "1>&2 to stderr"            'echo hi 1>&2 2>/dev/null; echo done'
check "2>&1 merge into pipe"      'ls /no_such_xyz 2>&1 | cat'
check "&> merge to file"          'echo hi &> f; cat f'

# --- Phase 3a parity net: semantics the unified lowering must preserve ---
# Lazy dup-source validation: 4>&3 is only valid because 3>file applied first.
check "3a lazy dup after file"     'exec 3>&-; { echo x; } 3>f 4>&3; cat f'
check "3a dup chain swap"          '{ echo o; echo e 1>&2; } 3>&1 1>&2 2>&3 2>/dev/null'
# {var} persists in-process (exec / compound) but is per-command in a child.
check "3a namedfd exec persists"   'exec {v}>f; echo hi >&"$v"; exec {v}>&-; cat f'
check "3a namedfd compound"        '{ echo hi; } {v}>f; echo sep; cat f'
check "3a namedfd external child"  'echo hi {v}>f; echo sep; cat f'
check "3a namedfd move"            'exec 5>f; { echo hi; } {v}>&5-; echo sep; cat f'
# In-process ordering: 2>&1 >file (compound) vs a single external command.
check "3a order compound 2>&1>f"   '{ echo o; echo e 1>&2; } 2>&1 >f; cat f'
check "3a order external 2>&1>f"   '/bin/echo hi 2>&1 >f; cat f'
check "3a fd3 heredoc external"    $'/bin/cat 3<<EOF <&3\nbody\nEOF'

# --- invalid-dup-before-file must NOT truncate (bash left-to-right) ---
check "3a nodup-trunc inproc"      'echo X > t; { echo hi; } 2>&77 >t; echo rc=$?; cat t'
check "3a nodup-trunc external"    'echo X > e; /bin/echo hi 2>&77 >e; echo rc=$?; cat e'
check "3a nodup-trunc exec"        'echo X > t3; ( exec 2>&77 >t3; echo hi ); echo rc=$?; cat t3'
check "3a file-before-baddup trunc" 'echo X > t2; { echo hi; } >t2 2>&77; echo rc=$?; cat t2'
check "3a close-then-dup inproc"   '{ echo hi; } 1>&- 2>&1; echo done'
check "3a same-plan dup external"  'exec 4>&-; /bin/echo hi 3>g 4>&3; cat g'

# --- v292b: in-process {var} interleaving (fixed) ---
check "b nf use later 2>&\$v"  '{ echo err 1>&2; } {v}>f 2>&$v; cat f'
check "b nf persist on fail"   '{ true; } {v}>g 2>&9; echo "v=${v-unset}"'
check "b nf num mixed list"    '{ true; } 3>a {v}>x; echo "v=$v"'

# --- #128: a non-interactive shell must NOT SIGHUP background jobs at exit ---
# The child writes a file after a short sleep while the shell exits immediately;
# bash leaves it running (file appears), huck must match. Poll after exit.
nohangup() {
    local label="$1" shbin="$2" out
    rm -f "$WORK/bg.out"
    timeout 5 "$shbin" -c 'sleep 0.3 && echo alive > "'"$WORK"'/bg.out" & exit 0' </dev/null >/dev/null 2>&1
    for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$WORK/bg.out" ] && break; sleep 0.1; done
    [ -s "$WORK/bg.out" ] && echo alive || echo KILLED
}
b=$(nohangup bg bash); h=$(nohangup bg "$HUCK_BIN")
if [[ "$b" == alive && "$h" == "$b" ]]; then printf 'PASS: #128 bg child survives non-interactive exit\n'; PASS=$((PASS+1))
else printf 'FAIL: #128 bg child survives (bash=%s huck=%s)\n' "$b" "$h"; FAIL=$((FAIL+1)); fi

# --- #129: run_background_sequence stage-0 stdin async rule (Path A) ---
# Shell stdin is a real file ($WORK/inA); the async child prints readlink of its
# fd0. Single-stage async -> /dev/null (non-interactive); bare multi-stage pipeline
# -> inherits the shell's stdin. Trailing `&` at EOF => Path A; poll after exit.
poll_fd0() {
    local shbin="$1" frag="$2" out
    rm -f "$WORK/fd0.out"
    timeout 5 "$shbin" -c "$frag" < "$WORK/inA" >/dev/null 2>&1
    for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$WORK/fd0.out" ] && break; sleep 0.1; done
    cat "$WORK/fd0.out" 2>/dev/null | sed "s#$WORK#@W@#g"
}
p129() {
    local label="$1" frag="$2" b h
    b=$(poll_fd0 bash "$frag"); h=$(poll_fd0 "$HUCK_BIN" "$frag")
    if [[ -n "$b" && "$h" == "$b" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s (bash=[%s] huck=[%s])\n' "$label" "$b" "$h"; FAIL=$((FAIL+1)); fi
}
p129 "#129 single-stage & -> /dev/null" 'readlink /proc/self/fd/0 > "'"$WORK"'/fd0.out" &'
p129 "#129 multi-stage a|b & -> inherit" 'readlink /proc/self/fd/0 | cat > "'"$WORK"'/fd0.out" &'

# --- #135: in-process whole-command redirect on a freed std fd (v291 Phase 2 fix) ---
check "135 brace group freed fd0"  'exec <&-; { /bin/cat; } < inA; echo end'
check "135 subshell freed fd0"     'exec <&-; ( /bin/cat ) < inA; echo end'
# fd1 flavor: prove via the FILE (a trailing builtin write to a closed fd1 is #137,
# a separate open bug — keep this case independent of it).
check "135 stdout to file freed fd1" 'exec >&-; { /bin/echo hi; } > f; /bin/cat f >&2'
check "135 heredoc rfd freed fd3"    'exec 3<&-; { /bin/cat <&3; } 3<<EOF
hh
EOF
echo end'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
