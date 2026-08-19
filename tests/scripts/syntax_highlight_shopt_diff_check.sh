#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the LISTINGS around huck's own `shopt`
# option, `syntax_highlight` (v363, #666).
#
# The option itself is a deliberate divergence — bash has no name for it — but
# the divergence has to stay confined to that one name. Bare `shopt`, `shopt -p`
# and `compgen -A shopt` all print the option table, and those three outputs are
# compared with bash byte for byte here and in several other harnesses. Putting
# the option in the shared table would have reddened every one of them, so it
# lives in a separate table that the listings never iterate.
#
# What this harness asserts is precisely that containment: the listings agree,
# INCLUDING after the option has been toggled, and every other unknown name is
# still rejected exactly as bash rejects it.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# ⚠️ Status captured BEFORE any pipe — `cmd | sed; echo $?` reports sed's status,
# which would make every rc assertion here vacuous.
#
# ⚠️ The program-name prefix of an error line is normalised away. bash under
# `-c` says `bash:` while huck says its own argv[0], which is the absolute path
# the harness invoked — a difference about how the binary was found, not about
# the message, and one that would fail every error row for the wrong reason.
norm() { sed -E 's#^[^:]*: line #SH: line #'; }
check() {
    local label="$1" frag="$2" b h out rc
    out=$(bash --norc --noprofile -c "$frag" 2>&1); rc=$?
    b=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    out=$("$HUCK_BIN" --norc -c "$frag" 2>&1); rc=$?
    h=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── the listings must be identical ────────────────────────────────────────────
check 'bare shopt'          'shopt'
check 'shopt -p'            'shopt -p'
check 'shopt -s (set only)' 'shopt -s'
check 'shopt -u (unset)'    'shopt -u'
check 'compgen -A shopt'    'compgen -A shopt'
check 'shopt count'         'shopt | wc -l'
check 'shopt -o listing'    'shopt -o'

# ── ...and still identical once huck's own option has been TOGGLED ────────────
# The row that would catch the option leaking into the shared table only when
# set, or the extension storage aliasing a real one.
check 'listing after -u'    'shopt -u syntax_highlight 2>/dev/null; shopt'
check 'listing after -s'    'shopt -s syntax_highlight 2>/dev/null; shopt'
check 'compgen after -u'    'shopt -u syntax_highlight 2>/dev/null; compgen -A shopt'
check 'neighbours after -u' 'shopt -u syntax_highlight 2>/dev/null; shopt checkwinsize sourcepath xpg_echo'
check 'set -o after -u'     'shopt -u syntax_highlight 2>/dev/null; set -o'

# ── every OTHER unknown name is rejected exactly as bash rejects it ───────────
check 'unknown -s'          'shopt -s no_such_option_xyz'
check 'unknown -u'          'shopt -u no_such_option_xyz'
check 'unknown query'       'shopt no_such_option_xyz'
check 'unknown -q'          'shopt -q no_such_option_xyz; echo rc=$?'
check 'near miss'           'shopt -s syntax_highlightx'
check 'prefix of it'        'shopt -s syntax_high'

harness_summary
