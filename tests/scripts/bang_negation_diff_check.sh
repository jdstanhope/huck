#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v85 `!` pipeline negation.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Second driver: a script FILE rather than piped stdin. Needed for any fragment
# containing a `!`-prefixed WORD, because on piped stdin huck still runs history
# expansion (#21) and turns `!foo` into `foo` — a real but unrelated divergence
# that would make such a row red for the wrong reason. A script file is the
# driver bash and huck agree on today.
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
check_file() {
    local label="$1" frag="$2" b h
    printf '%s\n' "$frag" >"$T/s.sh"
    b=$(cd "$T" && bash s.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$T" && "$HUCK_BIN" s.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
check "bang false"        '! false; echo $?'
check "bang true"         '! true; echo $?'
check "bang if"           'if ! false; then echo yes; fi'
check "bang while"        'while ! true; do echo x; done; echo done'
check "bang and"          '! false && echo ran'
check "bang pipeline ps"  '! false | true; echo "$? ${PIPESTATUS[@]}"'
check "bang errexit"      'set -e; ! true; echo survived'
check "bang pipefail"     'set -o pipefail; ! false | true; echo $?'
check "bang brace"        '! { false; }; echo $?'
check "bang subshell"     '! (exit 3); echo $?'
check "double bang"       '! ! false; echo $?'
check "test arg bang"     '[ ! -e /nonexistent ]; echo $?'

# #652 — the `!` on its OWN LINE inside a compound body. Every case above puts
# the bang on the same line as its opener, which is why they all passed while
# `! cmd` after a newline ran `!` as a command (rc 127). `parse_pipeline`'s
# bang loop ran before the newline skip in `parse_command_impl`, so a newline
# in front of the `!` hid it. Found by the runtime sweep, in nvm's test suite.
check "newline bang then"     'if true; then
! false
fi
echo rc=$?'
check "newline bang else"     'if false; then :; else
! false
fi
echo rc=$?'
check "newline bang elif"     'if false; then :; elif true; then
! false
fi
echo rc=$?'
check "newline bang while"    'while true; do
! false
break
done
echo rc=$?'
check "newline bang until"    'until false; do
! false
break
done
echo rc=$?'
check "newline bang for"      'for x in a; do
! false
done
echo rc=$?'
check "newline bang brace"    '{
! false
}
echo rc=$?'
check "newline bang subshell" '(
! false
)
echo rc=$?'
check "newline bang func"     'f() {
! false
}
f
echo rc=$?'
check "newline bang case"     'case x in
x)
! false
;;
esac
echo rc=$?'
check "newline double bang"   'if true; then
! ! false
fi
echo rc=$?'
check "newline bang pipeline" 'if true; then
! echo hi | grep -q hi
fi
echo rc=$?'
# Controls: what must NOT change now that a newline no longer hides the bang.
check_file "newline glued bang" 'if true; then
!foo
fi'
check "blank lines then bang" 'if true; then


! false
fi
echo rc=$?'
check "newline body heredoc"  'if true; then
cat <<EOF
body
EOF
fi'
harness_summary
