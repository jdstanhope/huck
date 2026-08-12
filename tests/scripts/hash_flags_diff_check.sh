#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `hash`'s FLAG ORDER (#509).
#
# The flags are not a priority ladder over the whole command. bash's
# `hash_builtin` reads them at three separate points, and that is what makes
# combinations behave the way they do:
#
#   1. `-d` / `-t` with no NAMEs is a usage error (bash's `sh_needarg`, status
#      1), checked FIRST — so `hash -rt` errors and the `-r` flush never runs.
#   2. `-r` flushes the table, before anything is listed or added, and with no
#      NAMEs left returns SILENTLY (no `hash table empty` line).
#   3. With no NAMEs, the whole table is listed; `-p` and `-l` do not suppress
#      that, so `hash -p /bin/ls` (no name) prints the table.
#   4. Per NAME: `-t` (report) beats `-p` (set) beats `-d` (delete) beats the
#      default PATH search. huck ran reset > delete > set_path > list, so
#      `hash -dt ls` DELETED where bash reports, and `hash -p X -t ls` SET
#      where bash reports the old entry.
#
# Two more measured rules that fall out of the same source: a name containing
# a slash is silently skipped by every branch except `-t` (bash's
# `absolute_program` check sits after the report branch), and `-d` on a table
# that has never held an entry is a silent success, because bash's
# `phash_remove` returns success when `hashed_filenames` is still null.
#
# Both shells run with an EXPLICIT $0 ("huck5") so the error prologue matches
# byte for byte without normalisation.
#
# The listing ORDER is bash's hash-table walk: bucket = FNV-1(name) % 256,
# buckets ascending, newest-registered first within a bucket (#555). The
# 90-name block at the bottom is the collision stress case; it fails on any
# other bucket count.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

# DRIVER: `-c` with an explicit $0.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- (1) a NAME-less -d/-t is a usage error, and it precedes the -r flush ---
check "-t no names"       'hash -t'
check "-d no names"       'hash -d'
check "-dl no names"      'hash -dl'
check "-lt no names"      'hash -lt'
check "-rt no names"      'hash -rt'
check "-rt leaves table"  'hash -p /bin/ls ls; hash -rt; hash -l'

# --- (2) the flush, and its silence ---
check "-r bare"           'hash -r'
check "-r then list"      'hash -p /bin/ls ls; hash -r; hash'
check "-r with a name"    'hash -p /bin/ls ls; hash -r ls; hash -t ls'
check "-r before -p"      'hash -p /bin/ls ls; hash -rp /bin/cat ls; hash -t ls'
check "-r before -d"      'hash -p /bin/ls ls; hash -rd ls'

# --- (3) no NAMEs lists the table, whatever else was asked for ---
check "-p with no name"   'hash -p /bin/ls'
check "bare empty"        'hash'
check "-l empty"          'hash -l'
check "bare one entry"    'hash -p /bin/ls ls; hash'
check "-l one entry"      'hash -p /bin/ls ls; hash -l'

# --- (4) per-name branch order: -t > -p > -d > PATH search ---
check "-dt reports"       'hash -p /bin/ls ls; hash -dt ls; hash -t ls'
check "-p then -t"        'hash -p /bin/ls ls; hash -p /bin/cat -t ls; hash -t ls'
check "-dp sets"          'hash -p /bin/ls ls; hash -dp /bin/cat ls; hash -t ls'
check "-lt reports"       'hash -p /bin/ls ls; hash -lt ls'
check "-tl reports"       'hash -p /bin/ls ls; hash -tl ls'
check "-l alone rehashes" 'hash -p /bin/ls ls; hash -l ls; hash -t ls'
check "-d then gone"      'hash -p /bin/ls ls; hash -d ls; hash -t ls'
check "-t two names"      'hash -p /bin/ls ls; hash -t ls nosuchcmd12'
check "-p two names"      'hash -p /bin/ls aa bb; hash -t aa; hash -t bb'

# --- -d against a table that has never held anything is silent ---
check "-d fresh shell"    'hash -d nosuchhashed; echo "rc=$?"'
check "-d two fresh"      'hash -d nosuch1 nosuch2; echo "rc=$?"'
check "-d after -r fresh" 'hash -r; hash -d nosuch; echo "rc=$?"'
check "-d once created"   'hash -p /bin/ls ls; hash -d ls; hash -d nosuch'
check "-d after flush"    'hash -p /bin/ls ls; hash -r; hash -d nosuch'
check "-d mixed names"    'hash -p /bin/ls ls; hash -d ls nosuch'

# --- a name with a slash: skipped everywhere except -t ---
check "slash bare"        'hash /bin/ls; hash'
check "slash relative"    'hash bin/ls; hash'
check "slash -p name"     'hash -p /bin/ls a/b; hash'
check "slash -d"          'hash -d /bin/ls'
check "slash -t reports"  'hash -t /bin/ls'

# --- controls: the plain forms must not move ---
check "-t hashed"         'hash -p /bin/ls ls; hash -t ls'
check "-t unhashed"       'hash -t nosuchcmd12'
check "-l unhashed name"  'hash -l nosuchcmd12'
check "rehash missing"    'hash nosuchcmd12'

# --- #555: the listing ORDER is a hash walk, not sorted and not insertion ---
check "order three -l"     'hash -p /bin/ls zz; hash -p /bin/cat aa; hash -p /bin/cp mm; hash -l'
check "order three bare"   'hash -p /bin/ls a; hash -p /bin/cat b; hash -p /bin/cp c; hash'
check "order ten"          'for n in q w e r t y u i o p; do hash -p /bin/ls $n; done; hash -l'
# Re-hashing an existing name keeps its place; a delete-then-re-add moves it.
check "order reregister"   'hash -p /bin/ls a; hash -p /bin/cat b; hash -p /bin/cp a; hash -l'
check "order delete readd" 'hash -p /bin/ls a; hash -p /bin/cat b; hash -d a; hash -p /bin/cp a; hash -l'
check "order after flush"  'hash -p /bin/ls x; hash -r; hash -p /bin/cat y; hash -l'
NAMES='nb tyur f4b3k e3hbi ql k fp9o1d iau fvwzz ktkw84 if3q fks3r c x023 hog1 n_opx w6 m j rg n x x09nq w4n0 lk z5a5 mh4o2x vh7 rjbbld qino kju d2 obsrp wgl viieje sgb xlr_ g8km79 oi3g dq qdty tlqi a4cv28 mi e0o vw p jpnrt bl1f20 xu egpoia k4b_z e7q6d9 o52fcj ot in0 n5a h0h s1bv aqrpx1 o kx4v ef ur9t1b aey dr q9 vf qaf q3g7 bo o8 ozt u q7 gw gva sml olvu d7 af1h aw0w8 z ov l0o tt o4u0 xfk v4_5qd lgix4'
check "order 90 names -l"  "for n in $NAMES; do hash -p /bin/ls \$n; done; hash -l"
check "order 90 names"     "for n in $NAMES; do hash -p /bin/ls \$n; done; hash"
check "order 90 churn"     "for n in $NAMES; do hash -p /bin/ls \$n; done; for n in c k m j x z u p o; do hash -d \$n; hash -p /bin/cat \$n; done; hash -l"

harness_summary
