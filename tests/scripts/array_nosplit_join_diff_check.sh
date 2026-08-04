#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #292: how an array/positional
# expansion joins in a NO-SPLIT context (assignment RHS, `case` subject,
# `[[ ]]` operand). `@` joins with a SPACE whatever IFS says; `*` joins with
# IFS[0]. Quoting does not change either rule.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
A='a=(x y z); '

# --- assignment RHS: @ is space-joined, * is IFS[0]-joined ------------------
check "@ unquoted"        "${A}IFS=,; v=\${a[@]}; echo \"[\$v]\""
check "@ quoted"          "${A}IFS=,; v=\"\${a[@]}\"; echo \"[\$v]\""
check "* unquoted"        "${A}IFS=,; v=\${a[*]}; echo \"[\$v]\""
check "* quoted"          "${A}IFS=,; v=\"\${a[*]}\"; echo \"[\$v]\""
check "@ multi-char IFS"  "${A}IFS=:-; v=\${a[@]}; echo \"[\$v]\""
check "* multi-char IFS"  "${A}IFS=:-; v=\${a[*]}; echo \"[\$v]\""
check "@ empty IFS"       "${A}IFS=; v=\${a[@]}; echo \"[\$v]\""
check "* empty IFS"       "${A}IFS=; v=\${a[*]}; echo \"[\$v]\""
check "@ unset IFS"       "${A}unset IFS; v=\${a[@]}; echo \"[\$v]\""
check "@ default IFS"     "${A}v=\${a[@]}; echo \"[\$v]\""
check "append form"       "${A}IFS=,; v=pre; v+=\${a[@]}; echo \"[\$v]\""

# --- positionals ------------------------------------------------------------
check '$@ quoted'         'set -- a b; IFS=,; v="$@"; echo "[$v]"'
check '$@ unquoted'       'set -- a b; IFS=,; v=$@; echo "[$v]"'
check '$* quoted'         'set -- a b; IFS=,; v="$*"; echo "[$v]"'
check '$* unquoted'       'set -- a b; IFS=,; v=$*; echo "[$v]"'
check '$* empty IFS'      'set -- a b; IFS=; v="$*"; echo "[$v]"'

# --- associative arrays share the rule --------------------------------------
check "assoc @"           'declare -A m=([k]=1 [j]=2); IFS=,; v=${m[@]}; echo "[$v]"'
check "assoc *"           'declare -A m=([k]=1 [j]=2); IFS=,; v=${m[*]}; echo "[$v]"'

# --- the other no-split contexts --------------------------------------------
check "case subject"      "${A}IFS=,; case \"\${a[@]}\" in \"x y z\") echo SP;; \"x,y,z\") echo IFS;; *) echo OTHER;; esac"
check "case subject *"    "${A}IFS=,; case \"\${a[*]}\" in \"x y z\") echo SP;; \"x,y,z\") echo IFS;; *) echo OTHER;; esac"
check "[[ ]] operand"     "${A}IFS=,; [[ \${a[@]} == \"x y z\" ]] && echo SP || echo OTHER"
check "[[ ]] operand *"   "${A}IFS=,; [[ \${a[*]} == \"x,y,z\" ]] && echo IFS || echo OTHER"

# --- a SPLITTING context is unchanged ---------------------------------------
check "split @ IFS=,"     "${A}IFS=,; set -- \${a[@]}; echo \"\$#:\$1\""
check "split * IFS=,"     "${A}IFS=,; set -- \${a[*]}; echo \"\$#:\$1\""
check "quoted @ words"    "${A}IFS=,; set -- \"\${a[@]}\"; echo \"\$#:\$1\""

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
