#!/usr/bin/env bash
# Parse-only compatibility sweep: run `bash -n` and `huck -n` over a corpus of
# real scripts and categorize how the two PARSERS agree or disagree. SAFE — both
# shells are invoked with -n (noexec), so nothing in any script is ever executed
# (verified: huck -n produces zero side effects). Each parse is wrapped in a
# timeout so a pathological input can only waste CPU, never hang the sweep.
#
# Usage: tools/parse_sweep.sh [tsv] [out]
#   LIMIT=N           only the first N rows (0 = all; default all)
#   PARSE_TIMEOUT=S   per-parse timeout seconds (default 10)
#   HUCK_BIN=path     huck binary (default target/debug/huck)
set -u
HUCK="${HUCK_BIN:-$(pwd)/target/debug/huck}"
TSV="${1:-tools/scripts.tsv}"
OUT="${2:-tools/parse_results.tsv}"
LIMIT="${LIMIT:-0}"
TIMEOUT="${PARSE_TIMEOUT:-10}"
[ -x "$HUCK" ] || { echo "build huck first: $HUCK" >&2; exit 1; }
[ -f "$TSV" ]  || { echo "no corpus tsv: $TSV" >&2; exit 1; }

RC=0; STDERR=""
parse_one() {  # $1=shell $2=path  -> sets RC, STDERR (stderr only; stdout discarded)
    STDERR=$(timeout -k 2 "$TIMEOUT" "$1" -n "$2" 2>&1 >/dev/null)
    RC=$?
}

# classify huck's result into one of: ok | fail | timeout | crash
huck_class() {  # $1=rc $2=stderr
    case "$1" in
        124|137) echo timeout; return ;;
        101|134|139) echo crash; return ;;
    esac
    if printf '%s' "$2" | grep -qi 'panic'; then echo crash; return; fi
    # huck can return rc 0 while still printing a syntax error on a later
    # statement, so treat any syntax-error text as a parse failure regardless.
    if [ "$1" = 0 ] && ! printf '%s' "$2" | grep -q 'syntax error'; then echo ok; else echo fail; fi
}

printf 'dialect\thow\tcategory\tbash_rc\thuck_rc\tpath\thuck_msg\n' > "$OUT"
declare -A CAT
n=0
while IFS=$'\t' read -r dialect how path; do
    [ "$LIMIT" -gt 0 ] && [ "$n" -ge "$LIMIT" ] && break
    [ -r "$path" ] || continue
    n=$((n+1))

    parse_one bash   "$path"; b_rc=$RC
    parse_one "$HUCK" "$path"; h_rc=$RC; h_err=$STDERR

    [ "$b_rc" = 124 ] || [ "$b_rc" = 137 ] && bclass=timeout || bclass=$([ "$b_rc" = 0 ] && echo ok || echo fail)
    hclass=$(huck_class "$h_rc" "$h_err")

    case "$hclass:$bclass" in
        crash:*)        cat=HUCK_CRASH ;;
        timeout:*)      cat=HUCK_TIMEOUT ;;
        *:timeout)      cat=BASH_TIMEOUT ;;
        ok:ok)          cat=AGREE_OK ;;
        fail:fail)      cat=AGREE_FAIL ;;
        fail:ok)        cat=HUCK_GAP ;;       # bash parses, huck rejects — the gold
        ok:fail)        cat=HUCK_LENIENT ;;   # huck parses, bash rejects
    esac

    CAT[$cat]=$(( ${CAT[$cat]:-0} + 1 ))
    msg=$(printf '%s' "$h_err" | head -1 | cut -c1-160)
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$dialect" "$how" "$cat" "$b_rc" "$h_rc" "$path" "$msg" >> "$OUT"
done < "$TSV"

echo "=== parsed $n scripts (timeout ${TIMEOUT}s each) ==="
for k in AGREE_OK AGREE_FAIL HUCK_GAP HUCK_LENIENT HUCK_CRASH HUCK_TIMEOUT BASH_TIMEOUT; do
    printf '  %-13s %s\n' "$k" "${CAT[$k]:-0}"
done
echo "full results -> $OUT"
