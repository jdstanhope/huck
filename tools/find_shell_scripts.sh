#!/usr/bin/env bash
# find_shell_scripts.sh — discover shell scripts on the system.
#
# READ-ONLY: this script only LISTS files and reads each candidate's first line
# (the shebang). It never executes, sources, modifies, or deletes anything.
#
# Usage:
#   tools/find_shell_scripts.sh [ROOT ...]            # roots to search
#   tools/find_shell_scripts.sh                       # default roots
#   tools/find_shell_scripts.sh / >scripts.tsv        # whole system (slow)
#
# Output (stdout): one TAB-separated record per script:
#   <shell>\t<how>\t<path>
#     shell : sh | bash | dash | ksh | zsh | ...   (the interpreter)
#     how   : shebang | ext                        (how it was identified)
# A summary (counts per shell) is printed to stderr at the end.
#
# A candidate is a regular, readable file that is EITHER executable OR named
# *.sh / *.bash. Each candidate's first line is checked for a shell shebang;
# files with no shell shebang but a .sh/.bash name are reported as shell-by-ext.
set -u

roots=("$@")
if [ "${#roots[@]}" -eq 0 ]; then
    # Sensible defaults; pass your own roots to override (e.g. `/` for everything).
    roots=(/usr /etc /opt /bin /sbin /usr/local /lib /lib64 "$HOME")
fi

# Keep only roots that exist (a missing default root shouldn't abort find).
existing=()
for r in "${roots[@]}"; do [ -e "$r" ] && existing+=("$r"); done
[ "${#existing[@]}" -gt 0 ] || { echo "no existing roots to search" >&2; exit 1; }

# Classify a shebang's interpreter. Echoes the shell name, or nothing if the
# first line is not a recognized shell shebang. Handles `#!/usr/bin/env shell`.
classify_shebang() {
    local line=$1 rest cmd base args
    [[ $line == '#!'* ]] || return 1
    rest=${line#'#!'}
    rest=${rest#"${rest%%[![:space:]]*}"}     # left-trim whitespace
    cmd=${rest%%[[:space:]]*}                  # first token = interpreter path
    base=${cmd##*/}                            # its basename
    if [[ $base == env ]]; then               # `env shell ...`: take next token
        args=${rest#"$cmd"}
        args=${args#"${args%%[![:space:]]*}"}
        base=${args%%[[:space:]]*}
        base=${base##*/}
    fi
    case "$base" in
        sh|bash|dash|ksh|ksh93|mksh|pdksh|zsh|ash|busybox) printf '%s' "$base"; return 0 ;;
        *) return 1 ;;
    esac
}

# Virtual / risky filesystems to prune (never descend into these).
find "${existing[@]}" \
        \( -path /proc -o -path /sys -o -path /dev -o -path /run \
           -o -fstype proc -o -fstype sysfs -o -fstype devtmpfs \) -prune \
        -o -type f -readable \( -executable -o -name '*.sh' -o -name '*.bash' \) -print0 \
        2>/dev/null |
while IFS= read -r -d '' f; do
    firstline=""
    IFS= read -r firstline < "$f" 2>/dev/null || true
    if shell=$(classify_shebang "$firstline"); then
        printf '%s\tshebang\t%s\n' "$shell" "$f"
    else
        case "$f" in
            *.bash) printf 'bash\text\t%s\n' "$f" ;;
            *.sh)   printf 'sh\text\t%s\n'   "$f" ;;
        esac
    fi
done | sort -u | tee >(
    # Summary to stderr (counts per shell), without consuming stdout.
    awk -F'\t' '{c[$1]++; total++} END {
        for (s in c) printf "  %-8s %d\n", s, c[s] > "/dev/stderr";
        printf "total shell scripts found: %d\n", total > "/dev/stderr";
    }'
)
