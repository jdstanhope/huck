#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #264: `shopt -s extdebug` WRITES the
# functrace (`-T`) and errtrace (`-E`) flags; it does not merely imply them.
#
# huck modelled this as an implication evaluated at each read site
# (`functrace || extdebug()`), which is wrong in four separate ways and was
# also simply forgotten at one of the three sites — the subshell fork, which is
# what #264 reported. The measured model is bash's `shopt_set_debug_mode`:
#
#     function_trace_mode = error_trace_mode = <the extdebug value>
#
# a plain assignment, performed when extdebug is set OR unset. Consequences the
# rows below pin, each of which an implication cannot produce:
#
#   * `$-` and `set -o` must REPORT -T and -E after `shopt -s extdebug`;
#   * `set +T` after `shopt -s extdebug` must turn tracing back off;
#   * `shopt -u extdebug` clears -T and -E UNCONDITIONALLY — even flags the
#     user set explicitly and never enabled extdebug for;
#   * `set -T` does NOT turn extdebug on (the write is one-way).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$( ulimit -v 800000; timeout 10 bash --norc --noprofile -c "$frag" 2>&1 | head -c 2000; echo "EXIT:$?" )
    h=$( ulimit -v 800000; timeout 10 "$HUCK_BIN" -c "$frag" 2>&1 | head -c 2000; echo "EXIT:$?" )
    compare "$label" "$b" "$h"
}

# --- the flags are WRITTEN, and therefore REPORTED --------------------------
check "extdebug sets -T and -E in \$-"  'shopt -s extdebug; echo "$-"'
check "extdebug in set -o"              'shopt -s extdebug; set -o | grep -E "^(functrace|errtrace)"'
check "extdebug off in set -o"          'set -o | grep -E "^(functrace|errtrace)"'
check "plain set -T in \$-"             'set -T; echo "$-"'
check "plain set -E in \$-"             'set -E; echo "$-"'
check "unset extdebug clears both"      'shopt -s extdebug; shopt -u extdebug; echo "$-"'
check "unset clears an explicit -T"     'set -T; shopt -u extdebug; echo "$-"'
check "unset clears an explicit -E"     'set -E; shopt -u extdebug; echo "$-"'
check "unset clears explicit -T -E"     'set -T -E; shopt -u extdebug; echo "$-"'
check "set -T does not set extdebug"    'set -T; shopt extdebug'
check "set -E does not set extdebug"    'set -E; shopt extdebug'
check "extdebug twice"                  'shopt -s extdebug; shopt -s extdebug; echo "$-"'
check "set +T then extdebug"            'set +T; shopt -s extdebug; echo "$-"'

# --- turning a written flag back off has effect -----------------------------
check "extdebug then set +T (\$-)"      'shopt -s extdebug; set +T; echo "$-"'
check "extdebug then set +E (\$-)"      'shopt -s extdebug; set +E; echo "$-"'
check "extdebug then set +T (DEBUG)"    'shopt -s extdebug; set +T; trap "echo D" DEBUG; f(){ ( echo a ); }; f'
check "extdebug then set +E (ERR)"      'shopt -s extdebug; set +E; trap "echo E" ERR; f(){ false; }; f'

# --- #264 proper: the subshell fork preserves DEBUG/RETURN ------------------
check "extdebug subshell in function"   'shopt -s extdebug; trap "echo D" DEBUG; f(){ ( echo a ); }; f'
check "set -T subshell in function"     'set -T; trap "echo D" DEBUG; f(){ ( echo a ); }; f'
check "extdebug subshell top level"     'shopt -s extdebug; trap "echo D" DEBUG; ( echo a; echo b )'
check "set -T subshell top level"       'set -T; trap "echo D" DEBUG; ( echo a; echo b )'
check "no flag subshell"                'trap "echo D" DEBUG; ( echo a; echo b )'
check "extdebug nested subshells"       'shopt -s extdebug; trap "echo D" DEBUG; f(){ ( ( echo a ) ); }; f'
check "extdebug RETURN in subshell"     'shopt -s extdebug; trap "echo R" RETURN; f(){ ( g ); }; g(){ echo a; }; f'
check "set -T RETURN in subshell"       'set -T; trap "echo R" RETURN; f(){ ( g ); }; g(){ echo a; }; f'
check "extdebug and set -T together"    'shopt -s extdebug; set -T; trap "echo D" DEBUG; f(){ ( echo a ); }; f'
check "extdebug pipeline stage"         'shopt -s extdebug; trap "echo D" DEBUG; f(){ echo a | cat; }; f'

# --- trap inheritance into functions ---------------------------------------
check "extdebug DEBUG into function"    'shopt -s extdebug; trap "echo D" DEBUG; f(){ echo a; }; f'
check "extdebug ERR into function"      'shopt -s extdebug; trap "echo E" ERR; f(){ false; }; f'
check "extdebug RETURN into function"   'shopt -s extdebug; trap "echo R" RETURN; f(){ g; }; g(){ echo a; }; f'
check "extdebug unset, DEBUG in fn"     'shopt -s extdebug; shopt -u extdebug; trap "echo D" DEBUG; f(){ echo a; }; f'

# --- regression guards ------------------------------------------------------
check "extdebug skip still works"       'shopt -s extdebug; false; trap "trap - DEBUG; false" DEBUG; true; echo rc=$?'
check "extdebug return 2 still works"   'shopt -s extdebug; trap "trap - DEBUG; exit 2" DEBUG; f(){ echo a; }; f; echo rc=$?'
check "plain -T DEBUG in function"      'set -T; trap "echo D" DEBUG; f(){ echo a; }; f'
check "plain -E ERR in function"        'set -E; trap "echo E" ERR; f(){ false; }; f'
check "no flags, DEBUG in function"     'trap "echo D" DEBUG; f(){ echo a; }; f'
check "shopt -p extdebug"               'shopt -s extdebug; shopt -p extdebug'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
