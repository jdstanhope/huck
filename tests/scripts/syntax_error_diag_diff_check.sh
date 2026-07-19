#!/usr/bin/env bash
# v314 (#211): huck's top-level syntax-error diagnostics match bash's 3 shapes.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck|huck): #SH: #"; }
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1 >/dev/null | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" -c "$frag" 2>&1 >/dev/null | norm); hr=${PIPESTATUS[0]}
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
# Shape 1 — near unexpected token
check s1-rparen   'echo )'
check s1-dsemi    'echo a ;; echo b'
check s1-done     'done'
check s1-esac     'esac'
check s1-fi       'fi'
check s1-then     'then echo x'
check s1-caseesac 'case esac in esac) ;; esac'
check s1-amp      '& echo x'
check s1-pipe     '| echo x'
check s1-lessgt   'echo <>'
check s1-in       'for x in ; do :; done; in'
check s1-do       'do echo x'
# Shape 2 — unexpected end of file
check s2-subshell '( echo hi'
check s2-brace    '{ echo hi'
check s2-if       'if true'
check s2-then     'if true; then echo'
check s2-case     'case x in'
check s2-for      'for i in 1 2'
check s2-while    'while true'
# Shape 3 — EOF looking for matching
check s3-dquote   'echo "hi'
check s3-squote   "echo 'hi"
check s3-cmdsub   'echo $(foo'
check s3-arith    'echo $((1+'
check s3-paramexp 'echo ${x'
check s3-procsub-in  'cat <(foo'
check s3-procsub-out 'cat >(foo'
check s3-backtick 'echo `foo'
if [ $FAIL -ne 0 ]; then echo "syntax_error_diag_diff_check FAILED" >&2; exit 1; fi
echo "syntax_error_diag_diff_check OK"
