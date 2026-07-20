#!/usr/bin/env bash
# v316 (#213): syntax error in a backtick command-sub body → `command substitution:` marker.
# STDERR-only (the marker); stdout/rc diverge by the pre-existing recover-vs-abort gap (follow-on).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck): #SH: #"; }
snorm() { sed -E "s#^.*/[^:]+: #SH: #"; }
check() { local l=$1 f=$2 b h
  b=$(bash -c "$f" 2>&1 >/dev/null | norm)
  h=$("$HUCK" -c "$f" 2>&1 >/dev/null | norm)
  if [ "$b" != "$h" ]; then echo "FAIL [$l]"; echo "  bash: [$b]"; echo "  huck: [$h]"; FAIL=1; else echo "PASS [$l]"; fi; }
check_script() { local l=$1; shift; local f; f=$(mktemp); printf '%s\n' "$@" > "$f"; local b h
  b=$(bash "$f" 2>&1 >/dev/null | snorm)
  h=$("$HUCK" "$f" 2>&1 >/dev/null | snorm); rm -f "$f"
  if [ "$b" != "$h" ]; then echo "FAIL [$l]"; echo "  bash: [$b]"; echo "  huck: [$h]"; FAIL=1; else echo "PASS [$l]"; fi; }
# backtick bodies → command substitution: marker
check 'bt-unterm-case'  'echo `case x in`'
check 'bt-near-token'   'echo `case esac in esac)`'
check 'bt-unterm-quote' 'echo `echo "hi`'
check 'bt-bad-paren'    'echo `echo )`'
# line base: backtick on script line 3
check_script 'bt-script-line3' 'echo a' 'echo b' 'echo `case x in`'
check_script 'bt-script-near'  'echo a' 'echo b' 'echo `esac`'
# control: $() stays -c: (no marker)
check 'ds-control-case'  'echo $(case x in)'
check 'ds-control-esac'  'echo $(esac)'
if [ $FAIL -ne 0 ]; then echo "comsub_marker_diag_diff_check FAILED" >&2; exit 1; fi
echo "comsub_marker_diag_diff_check OK"
