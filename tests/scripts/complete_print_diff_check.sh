#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `complete -p`'s PRINT FORM (#527).
#
# bash's `complete -p` is supposed to re-input: pasting a line back must
# rebuild the same compspec. Its shape, measured on bash 5.2.21:
#
#   complete [-o opt]... [short action flags] [-A action]... \
#            [-G glob] [-W words] [-P pre] [-S suf] [-X filt] [-F func] \
#            [-D|-E] [name]
#
#   * `-o` options print in bash's `compopts[]` table order (alphabetical),
#     NOT the order they were typed.
#   * actions print in `compacts[]` table order, TWICE over the table: the
#     one-letter forms first (`-u -v`), then the long ones (`-A hostname`).
#     They are a bitmask in bash, so `-u -u` collapses.
#   * every option VALUE is single-quoted EXCEPT `-F`, which is quoted only
#     when the name needs it — the same `sh_contains_shell_metas` test the
#     command name gets, but an empty `-F` value stays bare.
#   * `-D`/`-E` sit where the name would be, at the END of the line.
#   * the NAME is single-quoted only when `sh_contains_shell_metas` says so,
#     and there is never a `--` in front of it.
#
# The ORDER of the lines from a bare `complete -p` is a walk of bash's single
# prog-completion hash table: bucket = FNV-1(name) % 512, buckets ascending,
# newest-registered first within a bucket. The `-D`/`-E` compspecs live in
# that same table under the reserved names `_DefaultCmD_` / `_EmptycmD_`, so
# their lines are NOT pinned to the end — they land wherever those hash. The
# 120-name block at the bottom is the collision stress case; it fails on any
# other bucket count.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# DRIVER: `-c` with an EXPLICIT $0 ("huck5"), so the error prologue of the
# `-F` validation rows matches byte for byte without normalisation.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 bash --norc --noprofile -c "$frag" huck5 2>&1)
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1)
    compare "$label" "$b" "$h"
}

# --- the line shape ---
check "bare name"          'complete foo; complete -p'
check "one option"         'complete -o nospace foo; complete -p foo'
check "two options order"  'complete -o nospace -o filenames foo; complete -p foo'
check "all options"        'complete -o default -o dirnames -o filenames -o bashdefault -o nospace -o nosort -o noquote -o plusdirs foo; complete -p foo'
check "short actions"      'complete -a -b -c -d -e -f -g -j -k -s -u -v foo; complete -p foo'
check "long actions"       'complete -A arrayvar -A binding -A disabled -A enabled -A function -A helptopic -A hostname -A running -A setopt -A shopt -A signal -A stopped foo; complete -p foo'
check "action dedupe"      'complete -u -u -v foo; complete -p foo'
check "short before long"  'complete -A hostname -u foo; complete -p foo'
check "generators order"   'complete -o nospace -o default -u -v -A hostname -G "*.c" -W "a b" -F _f -X "!*.o" -P pre -S suf foo; complete -p foo'
check "-F unquoted"        'complete -F _f foo; complete -p foo'
check "-F quoted when meta" 'complete -F "a\$b" foo; complete -p foo'
check "-F empty stays bare" 'complete -F "" foo; complete -p foo'
check "-W quoted"          'complete -W "a b" foo; complete -p foo'
check "-W empty quoted"    'complete -W "" foo; complete -p foo'
check "-W with quote"      "complete -W \"a'b\" foo; complete -p foo"
check "-P -S -X quoted"    'complete -P pre -S suf -X "!*.c" foo; complete -p foo'

# --- the name, and the `--` that must not be there ---
check "name needs no quote" 'complete a~b; complete a#b; complete a=b; complete a:b; complete a,b; complete a%b; complete a@b; complete a+b; complete -p'
check "name needs quote"    'complete "a b"; complete "a^b"; complete "a[b"; complete "a{b"; complete "a!b"; complete "a;b"; complete -p'
check "name with squote"    "complete \"a'b\"; complete -p"
check "empty name"          'complete ""; complete -p'
check "dash name"           'complete -o nospace -- -foo; complete -p'

# --- -D / -E placement, both alone and mixed with names ---
check "-D alone"           'complete -D -o nospace; complete -p'
check "-E alone"           'complete -E -o nospace; complete -p'
check "-D then name"       'complete -D -o nospace; complete foo; complete -p'
check "-D name -E"         'complete -D -o nospace; complete zz; complete -E -o filenames; complete -p'
check "-D with -W -F"      'complete -D -W "a b" -F _f; complete -p'
check "-p -D"              'complete -D -o nospace; complete -p -D'
check "-p -E"              'complete -E -o nospace; complete -p -E'

# --- #550: the `-F` NAME is validated at registration, which is what lets
#     `complete -p` print it unquoted. The rule is not "identifier" despite the
#     wording: bash accepts `1abc`, `a-b`, `a.b`, `a/b`, `a$b` and even the
#     empty string, and rejects only a name holding a shell BREAK character. ---
check "-F odd but legal"   'complete -F 1abc a; complete -F a-b b; complete -F a.b c; complete -F a/b d; complete -F "a\$b" e; complete -F "" f; complete -p'
check "-F with space"      'complete -F "a b" foo; echo "rc=$?"; complete -p foo; echo "rc=$?"'
check "-F with semicolon"  'complete -F "a;b" foo; echo "rc=$?"'
check "-F with pipe"       'complete -F "a|b" foo; echo "rc=$?"'
check "-F with amp"        'complete -F "a&b" foo; echo "rc=$?"'
check "-F with parens"     'complete -F "a(b" foo; echo "rc=$?"'
check "-F with redirect"   'complete -F "a<b" foo; echo "rc=$?"'
check "-F with tab"        'complete -F "$(printf "a\tb")" foo; echo "rc=$?"'
check "-F bad with -W"     'complete -W x -F "a b" foo; echo "rc=$?"'
check "-F bad two names"   'complete -F "a b" foo bar; echo "rc=$?"; complete -p'
check "compgen -F bad"     'compgen -F "a b" x; echo "rc=$?"'

# --- print-all ORDER: bash's hash walk, not sorted and not insertion order ---
check "order abc"          'complete a; complete b; complete c; complete -p'
check "order cba"          'complete c; complete b; complete a; complete -p'
check "order five"         'for n in zz aa mm qq bb; do complete -o nospace $n; done; complete -p'
check "order named args"   'complete a; complete b; complete c; complete -p c a b'
# Re-registering an existing name must NOT move it (bash updates the hash item
# in place); removing and re-adding MUST move it to the newest slot.
check "reregister keeps"   'complete xx; complete yy; complete -o nospace xx; complete -p'
check "remove readd moves" 'complete xx; complete yy; complete -r xx; complete xx; complete -p'
check "-r all then re-add" 'complete xx; complete yy; complete -r; complete yy; complete xx; complete -p'

# --- 120 names: enough to collide in a 512-bucket table many times over ---
NAMES='yhaf zcfgw h a l32jfl w a4pe7 o tf5cr8 ct3wrp u2 qb5eav e3ykde ske8 gb l qt ncl it ec9y y4v s3c3 t1 x1srh y_ n q pqmw dkrju1 x cnds z hfut2 cqe0m dm25 hn5 wlnt b7j p3vii bkj7e wka vdfjnc nuny it0u9 efqr_ mr6hy wzdw1 hkyt pt03 f3ks swn f923b z2gbx3 cv ltv s9m qre7nv qu2 i y8hpq lb m v gju6w0 yq4 sw u vv9ok pi7e epk55 wui8 s1cxe l_ceuo tw qy4db1 jvj4 vpbwx_ f g8 x5 eqd2xx j_xo j krh7c g9x7l kxr wl2ka7 wpm1 ilw_64 y3351 xxg6zi d4g7 rq hevzs6 l8m3 v2u16g rfem fvlyr wq au t b zq0uvt s9ky jsupj g11_5 kr0zq v4ups f_ h7w7n nb8z y weepy4 mt q66 oetb5l ey n6 eg4 rir'
check "order 120 names"    "for n in $NAMES; do complete \$n; done; complete -p"
check "order 120 + slots"  "complete -D -o nospace; for n in $NAMES; do complete \$n; done; complete -E -o filenames; complete -p"
check "order 120 churn"    "for n in $NAMES; do complete \$n; done; for n in a b m v t f j i o w z; do complete -r \$n; complete -o nospace \$n; done; complete -p"

harness_summary
