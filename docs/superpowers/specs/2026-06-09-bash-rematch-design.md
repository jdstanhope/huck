# huck v122 — populate `BASH_REMATCH` after `[[ … =~ … ]]` (M-14 sub-feature) Design

**Status:** approved design, ready for implementation plan.
**Implements:** `BASH_REMATCH` array population on a successful (and the clearing
on a failed) `[[ STRING =~ REGEX ]]` match — the long-deferred sub-feature
listed under M-82/M-83, owned by M-14 (the `[[ ]]` `=~` regex test).
**Why now:** it is the reason `ls -<TAB>` (and most `_longopt`-style
completions) show **no options** even though v121 fixed the hang.
bash-completion's `_longopt` extracts each option with
`[[ $line =~ --… ]] && printf '%s\n' ${BASH_REMATCH[0]}`; huck's `=~` evaluates
the boolean but never populates `BASH_REMATCH`, so the extraction emits nothing.
Beyond completion, `BASH_REMATCH` is used pervasively in shell scripts.
**Branch (impl):** `v122-bash-rematch`.

## Background — the gap (probed against bash this session)

The `=~` eval site (`src/executor.rs:1227-1232`, in `eval_test_expr`, which
already takes `&mut Shell`) is:
```rust
TestExpr::Regex { lhs, pattern } => {
    let l = expand_assignment(lhs, shell);
    let p = …;                                  // expanded regex pattern
    let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
    Ok(re.is_match(&l))                          // <-- boolean only; no capture
}
```
`re.is_match` returns the match boolean but never captures, so `BASH_REMATCH`
stays empty → `${BASH_REMATCH[0]}` is empty.

Probed bash semantics:

| fragment | bash result |
|---|---|
| `[[ abcdef =~ b(c)(d) ]]; … BASH_REMATCH` | `n=3 [0]=bcd [1]=c [2]=d` |
| `BASH_REMATCH=(stale x y); [[ xyz =~ nomatch ]]` | rc 1, `n=0 [0]=` (CLEARED) |
| `[[ ab =~ (a)\|(b) ]]` | `[0]=a [1]=a [2]=` (non-participating group → empty, still indexed) |
| `[[ foobar =~ o+ ]]` | `[0]=oo` (matched SUBSTRING; `=~` is an unanchored search) |
| `[[ "a.b" =~ "a.b" ]]` | rc 0, `[0]=a.b` (quoted regex set normally) |

Rules: on match, `BASH_REMATCH[0]` = whole matched substring, `[1..]` = capture
groups (non-participating → `""`, still occupying their index). On no match,
`BASH_REMATCH` is cleared to an empty array. Only the `=~` test touches it —
other `[[ ]]` tests leave any prior value intact.

## Architecture — set `BASH_REMATCH` in the Regex arm (no signature change)

`eval_test_expr` already has `&mut Shell`, so the fix is localized to the
`TestExpr::Regex` arm: replace `re.is_match(&l)` with `re.captures(&l)` and set
the `BASH_REMATCH` indexed array from the result.

### Component — the `TestExpr::Regex` arm (`src/executor.rs:1227`)
```text
let re = regex::Regex::new(&p)?;
match re.captures(&l) {
    Some(caps) => {
        // index i in 0..caps.len(): the matched substring, or "" for a
        // non-participating group (caps.get(i) == None).
        let map: BTreeMap<usize,String> = (0..caps.len())
            .map(|i| (i, caps.get(i).map(|m| m.as_str().to_string()).unwrap_or_default()))
            .collect();
        let _ = shell.replace_array("BASH_REMATCH", map);  // replaces any prior value
        Ok(true)
    }
    None => {
        let _ = shell.replace_array("BASH_REMATCH", BTreeMap::new());  // clear (empty array)
        Ok(false)
    }
}
```
- `replace_array` replaces any prior `BASH_REMATCH` (scalar/indexed/assoc) with
  the new indexed array; on a readonly `BASH_REMATCH` it returns `Err` — ignored
  (rare edge; bash would error, but the match boolean is what matters). Verify
  `replace_array`'s behavior on a pre-existing SCALAR `BASH_REMATCH` during
  implementation (it should replace; if it type-errors, `unset` first).
- `caps.len()` = number of groups + 1, so index `0` (whole match) plus each
  declared group get an entry — matching bash's "every declared group is
  indexed" behavior.

## Scope & correctness
- Only the `=~` arm changes; the match boolean returned is identical to today
  (a `captures().is_some()` ⇔ `is_match()`), so no `[[ =~ ]]` truth-value
  regression.
- The matched CONTENT inherits huck's existing `=~` regex engine (the `regex`
  crate, RE2-style) — the pre-existing **L-09** divergence from POSIX ERE
  (e.g. leftmost-longest vs leftmost-first on alternation). `BASH_REMATCH` now
  reflects whatever that engine matched. For the ASCII option patterns
  `_longopt` uses (`--[A-Za-z0-9]+([-_][A-Za-z0-9]+)*=?`), it matches bash.
- Non-`=~` `[[ ]]` tests do not touch `BASH_REMATCH` (bash behavior).

## Must-not-regress
- `[[ =~ ]]` truth value (match/no-match) and `$?` — unchanged.
- Quoted-regex literal-match behavior (L-23) — unchanged; `BASH_REMATCH` is set
  from whatever matched.
- `[[ ]]` other operators, `(( ))`, `case`, regex error → status 2 path.
- Existing `dbracket`/regex test suites.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | `TestExpr::Regex` arm: `captures` + populate/clear `BASH_REMATCH`; unit tests |
| `tests/bash_rematch_integration.rs` | NEW — the probed cases + `_longopt` extraction, vs bash |
| `tests/scripts/bash_rematch_diff_check.sh` | NEW — 45th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | mark BASH_REMATCH `[fixed v122]` under M-14; drop it from M-82/M-83 deferred lists; changelog; README row |

## Testing
1. **Unit** (`src/executor.rs`): drive `eval_test_expr` on a `TestExpr::Regex`
   and assert `BASH_REMATCH` contents — whole+groups, no-match-clears,
   non-participating-group→`""`. (Constructing a `TestExpr` may be verbose; if
   so, cover via integration and keep a minimal unit check.)
2. **Integration** (`tests/bash_rematch_integration.rs`, binary vs bash,
   file-arg per L-27): the 5 probed cases (byte-identical), plus the real
   extraction `s=$(echo " --all --help" | while read -r w; do [[ $w =~ (--[a-z]+) ]] && echo "${BASH_REMATCH[1]}"; done)` and a `ls --help`-style option extraction → matches bash.
3. **45th bash-diff harness** `tests/scripts/bash_rematch_diff_check.sh` —
   ~8 fragments covering the table + an extraction case, byte-identical.
4. **Regression**: full suite (2882+), all 45 harnesses, clippy `--all-targets`.
   Watch `dbracket`/regex suites.
5. **Payoff (pty)**: source `/usr/share/bash-completion/bash_completion`, drive
   `ls -<TAB>`, confirm option candidates now appear (the `_longopt` extraction
   works). Report before/after. Honest: this unblocks `_longopt`-style
   completers; mise candidates still need the 2.12 bash-completion API (env).

## Edge cases & notes
- **No-match clear**: an empty indexed array (`n=0`), matching bash's observable
  `${#BASH_REMATCH[@]}=0` / `${BASH_REMATCH[0]}` empty. (Whether bash leaves it
  set-but-empty vs unset is unobservable for these probes; an empty array is the
  simplest faithful choice.)
- **Pre-existing scalar `BASH_REMATCH`**: replaced by the indexed array
  (verify `replace_array` converts; else `unset` first).
- **Readonly `BASH_REMATCH`**: `replace_array` Err ignored — the match boolean
  is still correct (rare edge; not worth a diagnostic).
- This does NOT change the regex engine (L-09 stays); it only surfaces the
  match/captures into `BASH_REMATCH`.
