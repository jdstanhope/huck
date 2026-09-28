#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #788: `typeset` names ITSELF in its
# diagnostics. bash's `builtin_error` prefixes `this_command_name`, so every
# message the builtin raises says `typeset:` when invoked that way — huck
# hardcoded the literal `declare:` at 13 sites in `builtin_declare_decl`.
#
# Each row is run BOTH ways, `declare` and `typeset`, from one fragment, so the
# harness pins the prefix rather than only the message body.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
norm() { sed -E 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$tmp" && bash --norc --noprofile -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [-c]" "$b" "$h"
    printf '%s\n' "$frag" >"$tmp/f.sh"
    b=$(cd "$tmp" && bash --norc --noprofile f.sh 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" f.sh 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [file]" "$b" "$h"
}

# `both <label> <template>` — runs the template once per builtin name, with
# every `@` replaced by that name. A `printf`-style `%s` template would be a
# trap here: printf REUSES its format for surplus arguments, so one spare `$cmd`
# silently concatenates the fragment to itself.
both() {
    local label="$1" tmpl="$2" cmd
    for cmd in declare typeset; do
        check "$label ($cmd)" "${tmpl//@/$cmd}"
    done
}

both "not found"            '@ -p nope'
both "invalid identifier"   '@ "bad name"=1'
both "readonly assign"      'readonly y=1; @ y=2'
both "readonly attr+assign" 'readonly RO=1; @ -i RO=2'
both "readonly local shadow" 'readonly RO=1; f(){ @ RO; }; f'
both "convert assoc->idx"   'declare -A x; @ -a x'
both "convert idx->assoc"   'declare -a y; @ -A y'
both "destroy array attr"   'declare -a y; @ +a y'
both "destroy assoc attr"   'declare -A x; @ +A x'
both "nameref self ref"     '@ -n r=r'
both "nameref bad name"     '@ -n r="a b"'
both "nameref bad in chain" 'declare -n a=b; @ -n b="bad name"; echo hi'
both "nameref on array"     'declare -a r=(1); @ -n r'

harness_summary
