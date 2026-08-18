#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the COMMAND HASH TABLE's lifecycle
# (#655) — who fills it, who reads it, and what empties it.
#
# huck's table used to be display-only: the `hash` builtin could add and list
# entries, but nothing populated it from execution and nothing consulted it. So
# PATH was re-walked on every single invocation, `hash` after running a command
# said "hash table empty", `type` never said "hashed", and `hash -p /bin/echo zz;
# zz` could not run at all.
#
# The rules below were all measured against bash 5.2.21 first. The subtle ones:
#
#   * only a PATH SEARCH hashes — not an absolute path, a builtin, a function,
#     or a name that was not found;
#   * the hit count counts INVOCATIONS: `hash NAME` and `hash -p P NAME` start
#     an entry at 0 and it reaches 1 the first time the command runs;
#   * a hash write only survives if the command ran in THIS shell. A pipeline
#     stage, a background job, a subshell and a command substitution all resolve
#     inside a forked child, so bash's entry dies with it — while `{ }`, `if`,
#     `for` and a function body DO persist, because they do not fork;
#   * `type -a` ignores the table completely, though plain `type`, `type -t`,
#     `type -p` and `command -v`/`-V` all consult it;
#   * a hashed entry SHADOWS the real PATH match (`hash -p /bin/echo ls`);
#   * every assignment form flushes it — `PATH=x`, `PATH+=:x`, `declare PATH=x`,
#     `local PATH=x`, the command-prefix `PATH=$PATH cmd` (and its restore) and
#     `unset PATH` — but a bare `export PATH` assigns nothing and does not;
#   * `set +h` makes EVERY form of `hash` fail with `hashing disabled`, rc 1,
#     and that check runs before option parsing (`set +h; hash -Z` is not an
#     invalid-option error).
#
# ⚠️ ONE ROW IS A DELIBERATE DIVERGENCE and is asserted against huck's own
# expected output rather than bash's — see the STALE section at the end (#664).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# ⚠️ Status captured BEFORE the normalizing pipe: `cmd | sed; echo $?` reports
# sed's status, which is always 0.
check() {
    local label="$1" frag="$2" b h out rc
    out=$(timeout 10 bash --norc --noprofile -c "$frag" 2>&1); rc=$?
    b=$(printf '%s\n' "$out" | sed 's|^bash: ||'; echo "EXIT:$rc")
    out=$(timeout 10 "$HUCK_BIN" -c "$frag" 2>&1); rc=$?
    h=$(printf '%s\n' "$out" | sed "s|^$HUCK_BIN: ||"; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── who fills it ──────────────────────────────────────────────────────────────
check "path search hashes"   'expr 1 + 1 >/dev/null; hash'
check "hit count counts runs" 'expr 1 + 1 >/dev/null; expr 1 + 1 >/dev/null; expr 1 + 1 >/dev/null; hash'
check "two commands"         'ls >/dev/null; cat /dev/null; hash'
check "absolute path: no"    '/usr/bin/expr 1 + 1 >/dev/null; hash'
check "builtin: no"          'echo hi >/dev/null; hash'
check "function: no"         'f(){ :; }; f; hash'
check "not found: no"        'nosuchcmd_xyz 2>/dev/null; hash'
check "explicit starts at 0" 'hash expr; hash'
check "explicit then run"    'hash expr; expr 1 + 1 >/dev/null; hash'
check "hash -p then run"     'hash -p /usr/bin/expr expr; expr 1 + 1 >/dev/null; hash'

# ── whether the write survives: does it fork? ─────────────────────────────────
check "pipeline stage: lost" 'expr 1 + 1 >/dev/null | cat >/dev/null; hash'
check "background: lost"     'expr 1 + 1 >/dev/null & wait; hash'
check "subshell: lost"       '(expr 1 + 1 >/dev/null); hash'
check "substitution: lost"   'x=$(expr 1 + 1); hash'
check "brace group: kept"    '{ expr 1 + 1 >/dev/null; }; hash'
check "if body: kept"        'if expr 1 + 1 >/dev/null; then :; fi; hash'
check "for body: kept"       'for i in 1; do expr 1 + 1 >/dev/null; done; hash'
check "function body: kept"  'f(){ expr 1 + 1 >/dev/null; }; f; hash'

# ── who reads it ──────────────────────────────────────────────────────────────
# The table can name a command PATH alone would never find.
check "lookup runs it"       'hash -p /bin/echo zzmyname; zzmyname HELLO'
check "lookup bumps"         'hash -p /bin/echo zzmyname; zzmyname HELLO; hash'
check "type says hashed"     'expr 1 + 1 >/dev/null; type expr'
check "command -V says so"   'expr 1 + 1 >/dev/null; command -V expr'
check "command -v bare path" 'expr 1 + 1 >/dev/null; command -v expr'
check "type -t still file"   'expr 1 + 1 >/dev/null; type -t expr'
check "type finds hash-only" 'hash -p /bin/echo zz; type zz'
check "type -p hash-only"    'hash -p /bin/echo zz; type -p zz'
check "type -a IGNORES it"   'hash -p /bin/echo zz; type -a zz'
check "hash shadows PATH"    'hash -p /bin/echo ls; type ls'
check "type -a unshadowed"   'hash -p /bin/echo ls; type -a ls'
check "unhashed is plain"    'type expr'

# ── what empties it ───────────────────────────────────────────────────────────
check "PATH= same value"     'expr 1 + 1 >/dev/null; PATH=$PATH; hash'
check "PATH+= append"        'expr 1 + 1 >/dev/null; PATH+=:/tmp; hash'
check "declare PATH="        'expr 1 + 1 >/dev/null; declare PATH=/usr/bin:/bin; hash'
check "unset PATH"           'expr 1 + 1 >/dev/null; unset PATH; hash; PATH=/usr/bin:/bin'
check "command prefix"       'expr 1 + 1 >/dev/null; PATH=$PATH expr 1 + 1 >/dev/null; hash'
check "local PATH"           'expr 1 + 1 >/dev/null; f(){ local PATH=/usr/bin; expr 1 + 1 >/dev/null; }; f; hash'
check "bare export: NOT"     'expr 1 + 1 >/dev/null; export PATH; hash'
check "other var: NOT"       'expr 1 + 1 >/dev/null; readonly zzro=1; hash'
check "hash -r"              'expr 1 + 1 >/dev/null; hash -r; hash'
check "hash -d one"          'expr 1 + 1 >/dev/null; ls >/dev/null; hash -d expr; hash'

# ── set +h ────────────────────────────────────────────────────────────────────
check "+h plain"             'set +h; hash'
check "+h -r"                'set +h; hash -r'
check "+h -p"                'set +h; hash -p /bin/echo z'
check "+h -d"                'set +h; hash -d ls'
check "+h before optparse"   'set +h; hash -Z'
check "+h suppresses fill"   'set +h; expr 1 + 1 >/dev/null; set -h; hash'
check "-h resumes"           'set +h; expr 1 + 1 >/dev/null; set -h; expr 1 + 1 >/dev/null; hash'
check "optparse when on"     'hash -Z'

# ── STALE ENTRIES: the one deliberate divergence (#664, by-design) ────────────
# bash execs a cached path even after the file is gone and fails with
# `No such file or directory` — the classic "I installed it and the shell still
# can't find it, run `hash -r`" trap. huck DISCARDS the stale entry and
# re-searches PATH, so it self-heals.
#
# These rows cannot be bash-diffed (that is the point), so each asserts huck's
# own expected output. `expect_huck` keeps them in the same PASS/FAIL tally.
expect_huck() {
    local label="$1" frag="$2" want="$3" out rc
    out=$(cd "$T" && timeout 10 "$HUCK_BIN" -c "$frag" 2>&1); rc=$?
    compare "$label" "$want" "$(printf '%s\n' "$out" | sed "s|^$HUCK_BIN: ||"; echo "EXIT:$rc")"
}

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/one" "$T/two"
printf '#!/bin/sh\necho FROM_ONE\n' >"$T/one/zzp"
printf '#!/bin/sh\necho FROM_TWO\n' >"$T/two/zzp"
chmod +x "$T/one/zzp" "$T/two/zzp"

# A replacement exists later in PATH: huck finds it; bash would fail here.
expect_huck "stale self-heals" \
    "PATH=$T/one:$T/two:\$PATH; zzp; rm -f $T/one/zzp; zzp; echo rc=\$?" \
    "$(printf 'FROM_ONE\nFROM_TWO\nrc=0\nEXIT:0')"

# Nothing to fall back to: same STATUS as bash (127), and an honest message —
# bash names the vanished path, huck says the name is not findable.
mkdir -p "$T/solo"
printf '#!/bin/sh\necho SOLO\n' >"$T/solo/zzq"
chmod +x "$T/solo/zzq"
expect_huck "stale, no replacement" \
    "PATH=$T/solo:\$PATH; zzq; rm -f $T/solo/zzq; zzq; echo rc=\$?" \
    "$(printf 'SOLO\nline 1: zzq: command not found\nrc=127\nEXIT:0')"

harness_summary
