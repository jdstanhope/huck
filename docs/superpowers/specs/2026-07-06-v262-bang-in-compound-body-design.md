# v262 — F2 fix: leading `!` in a compound body / condition

**Status:** design approved (2026-07-06)
**Arc:** post-reconciliation cleanup before the finale. The A→B→C reconciliation
(v259/v260/v261) is done; F2 is one of the two remaining small must-fix
carry-forwards surfaced by the v259 whole-branch review. From the verified
carry-forward inventory (`huck_carryforward_inventory.md`).

## Summary

On the dormant atom-command path (`new_seq`, `command_atoms` default `false`), a
leading `!` that is preceded by an inter-token `Blank` atom is swallowed into the
command word instead of counting as a pipeline negation. This happens wherever a
`!` follows a compound opener, a keyword, or a connector — anything that routes
through `parse_pipeline` with a `Blank` in front of the `!`:

- `{ ! a; }` → atom `Simple(program:"!", args:["a"])` vs oracle
  `Pipeline{negate:true,[Simple(a)]}`
- `{ ! ! a; }`, `if x; then ! ! a; fi`
- `while ! a; do :; done` (the **condition**), `while x; do ! a; done` (body),
  `for i in 1; do ! a; done` (body)
- `{ ! a && b; }`, `{ ! a | b; }`

**Root cause:** `parse_pipeline` (parser.rs) counts leading `!` words with a
`while iter.peek_kind()?.map(is_bang_word)…` loop. The loop already skips
inter-token `Blank`s *after* each bang (for `! ! a`), but its FIRST `peek_kind`
sees the `Blank` the atom scanner emits after a compound opener / keyword /
`&&` / `|`. `is_bang_word(Blank)` is false, so the loop never starts, `bangs`
stays 0, and the `!` falls through into `parse_command` as the program word.

**Already correct (unaffected), confirmed by probe:** `case x in a) ! b;; esac`
(bespoke case-item path pre-skips the blank), `( ! a )` (bespoke
`parse_subshell_sequence`), top-level `! a` (`parse_sequence` pre-skips),
`{ a; }` (no bang), `!a` (glued — not a bang word).

**Fix (approved):** skip leading `Blank`(s) at the top of `parse_pipeline`,
before the bang-count loop — the same skip the loop already performs after each
bang. Single DRY fix point; a no-op for the already-correct paths (they arrive
with no leading blank). `command.rs` EMPTY-diff; `command_atoms` stays `false`.

## Background — why the single fix point is comprehensive AND safe (probed)

Probed `old_seq` vs `new_seq` across compound types and bang parity:

| input | before fix |
|---|---|
| `{ ! a; }` / `{ ! ! a; }` | EQ=false (bug) |
| `if x; then ! ! a; fi` | EQ=false |
| `while ! a; do :; done` (cond) | EQ=false |
| `while x; do ! a; done` (body) | EQ=false |
| `for i in 1; do ! a; done` | EQ=false |
| `{ ! a && b; }` / `{ ! a \| b; }` | EQ=false |
| `case x in a) ! b;; esac` | EQ=true (bespoke path) |
| `( ! a )` | EQ=true (bespoke `parse_subshell_sequence`) |
| `! a` (top-level) | EQ=true (`parse_sequence` pre-skips) |
| `{ a; }` / `!a` | EQ=true |

Every divergent case routes through `parse_pipeline`; the already-correct cases
either don't (case items, subshell) or arrive with no leading blank (top-level).
A command never meaningfully begins with a `Blank`, so skipping zero-or-more
leading blanks at the top of `parse_pipeline` is a no-op where there are none and
the correct behavior where there is one — comprehensive and safe.

Once the bang loop sees the `!`, everything downstream is already correct: `bangs`
is counted, `negate = bangs % 2 == 1` and `had_bangs = bangs > 0` flow into
`finish_pipeline`, whose existing wrapping rule produces the oracle's shape:
- odd bang, simple stage (`{ ! a; }`): `Pipeline{negate:true,[Simple(a)]}`
- even bang, simple stage (`{ ! ! a; }`): `Pipeline{negate:false,[Simple(a)]}`
  (the simple-stage arm always wraps; the v259 CF3 `had_bangs` path covers the
  even-bang compound-stage case).

## Architecture

**Files:**
- `crates/huck-syntax/src/parser.rs` — one `while`-skip added at the top of
  `parse_pipeline`; new `diff_cmd` corpus in `mod tests`.
- `crates/huck-syntax/src/command.rs` — UNTOUCHED (EMPTY diff).
- `crates/huck-syntax/src/lexer.rs` — UNTOUCHED.

### The change

At the very start of `parse_pipeline` (before `let mut bangs = 0usize;`):
```rust
    // Skip any leading inter-token Blank the atom scanner emits after a compound
    // opener / keyword / connector (`{ ! a; }`, `while ! a`, `then ! a`), so the
    // bang-count loop below sees the `!` rather than the Blank in front of it.
    // (The loop already skips blanks BETWEEN successive bangs; this covers the
    // one before the FIRST bang.) A command never begins with a meaningful Blank,
    // so this is a no-op for the paths that already arrive blank-free.
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
```

No other change. `finish_pipeline`, the wrapping rule, and every compound-body /
condition caller are unchanged.

## Differential corpus

**Fixed — new `diff_cmd` (EQ=false today, must become byte-identical):**
`{ ! a; }`, `{ ! ! a; }`, `if x; then ! ! a; fi`, `if ! a; then :; fi`
(if-condition), `while ! a; do :; done` (while-condition),
`until ! a; do :; done` (until-condition), `while x; do ! a; done` (body),
`for i in 1; do ! a; done` (body), `{ ! a && b; }`, `{ ! a || b; }`,
`{ ! a | b; }`. (Probed: all EQ=false today — conditions AND bodies of every
compound routed through `parse_pipeline` diverge.)

**Regression guards — stay `diff_cmd` byte-identical:** `! a` (top-level),
`( ! a )` (subshell), `case x in a) ! b;; esac` (bespoke case-item path),
`{ a; }` (no bang), `!a` (glued, not a bang word).

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` for the fixed corpus
  and the regression guards.
- Full `huck-syntax` lib suite is the non-regression net.
- `command.rs` diff-vs-main = EMPTY; `lexer.rs` untouched.
- Both `command_atoms` sites stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green (box is
  1 core/1.9GB — never `--workspace`, never multi-threaded).
- `cargo build -p huck-syntax` → 0 warnings.

## Task decomposition (SDD)

- **T1 — the leading-Blank skip + the corpus.** Single task: the one-line fix in
  `parse_pipeline` plus the `diff_cmd` corpus (fixed cases + regression guards).
  Too small to split; the fix and its parity test belong together.

## Live-flip carry-forwards

RESOLVES F2. No new pin expected (the fix is a strict superset-of-correct: it
only changes cases that were wrong). Whole-branch review to probe for any sibling
routed through `parse_pipeline` not in the corpus (e.g. `until` conditions,
`&&`/`||`/`|`-chained bangs, nested compounds). After merge, mark F2 resolved and
record v262 in the iteration log. Remaining before the finale: **only**
array-lit-subscript-bare-dquote (`a=(["k"]=v)`); then flip `command_atoms` +
delete the forward-scanning scanners. No `bash-divergences.md` change (dormant).
