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
# NOT covered: any listing with MORE THAN ONE entry. bash walks its hash table
# to print, so the line order is bucket order, not sorted (#555).
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

harness_summary
