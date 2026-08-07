#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v225: posix-gated special-builtin
# prefix-assignment persistence + the posix flag plumbing.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {  # stdin-piped fragment, default mode
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
check_flag() {  # --posix CLI flag, fragment via -c
    local label="$1" frag="$2" b h
    b=$(bash --posix -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" --posix -c "$frag" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Generic special builtin: default restores, posix persists.
check "return default"       'var=0; f(){ var=20 return 5; }; f; echo "$? $var"'
check "return posix"         'set -o posix; var=0; f(){ var=20 return 5; }; f; echo "$? $var"'
check "colon default"        'var=0; var=20 :; echo "$var"'
check "colon posix"          'set -o posix; var=0; var=20 :; echo "$var"'
# Enclosing prefix: the func3.sub case.
check "enclosing default"    'var=0; f(){ var=20 return 5; }; var=30 f; echo "$? $var"'
check "enclosing posix"      'set -o posix; var=0; f(){ var=20 return 5; }; var=30 f; echo "$? $var"'
check "multilevel posix"     'set -o posix; a=0; m(){ a=3 return; }; o(){ a=2 m; }; a=1 o; echo "$a"'
# Assignment-builtin absorption: persists in default mode too.
check "export named default"  'FOO=val export FOO; echo "[${FOO-U}]"'
check "export nested default" 'FOO=0; f(){ FOO=20 export FOO; }; FOO=30 f; echo "[$FOO]"'
check "export nested posix"   'set -o posix; FOO=0; f(){ FOO=20 export FOO; }; FOO=30 f; echo "[$FOO]"'
check "readonly named default" 'BAR=ro readonly BAR; echo "[${BAR-U}]"'
# Flag plumbing.
check "set -o posix listing"  'set -o posix; set -o | grep posix'
check_flag "--posix listing"  'set -o | grep posix'
check_flag "--posix return"   'var=0; f(){ var=20 return 5; }; var=30 f; echo "$? $var"'

harness_summary
