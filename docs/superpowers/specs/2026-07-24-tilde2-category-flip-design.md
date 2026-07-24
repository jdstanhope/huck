# v335 — Flip the `tilde2` bash-suite category to PASS

Issue: [#294](https://github.com/jdstanhope/huck/issues/294) — tilde
mis-expanded in an assignment value (after an inner `=`) and not expanded in an
unquoted `${x:-word}` / `${x:=word}` default.

Follow-up (separate iteration): [#295](https://github.com/jdstanhope/huck/issues/295)
— move tilde-prefix *recognition* out of the lexer into the word-expansion phase.

## Problem

The `tilde2` bash-suite category is a near-miss (diff 25 lines). Its **complete
residual is exactly two lexer-side tilde-recognition roots**; the posix-`eval`
tail (`tilde2.tests` output lines 21–24) is proven downstream of Root A (with
`h` corrected, huck matches bash byte-for-byte). Fixing both takes the category
to **0-diff → PASS** (Summary PASS 23→24, FAIL 59→58).

Architecture note (unchanged by this iteration): the lexer only *recognizes*
whether a `~` is a tilde-prefix and emits `TokenKind::Tilde { spec, assign_ctx }`
vs a `Lit`; the actual HOME substitution already happens later in the expander
(`expand.rs`, `WordPart::Tilde`, honoring `posix`/`assign_ctx`). Both fixes below
adjust *recognition* only.

### Root A — over-expands `~` after an *inner* `=` in an assignment RHS

```
HOME=/usr/xyz
h=HOME=~ ;           echo $h        # bash: HOME=~          huck: HOME=/usr/xyz
ADDPATH=PATH=~/bin ; echo $ADDPATH  # bash: PATH=~/bin      huck: PATH=/usr/xyz/bin
```

bash tilde-expands the prefix right after the *assigning* `=` (word start) and
after each unquoted `:`, but **never** after a second, embedded `=`. huck
re-enables tilde eligibility after any `=`/`:` in the value.

### Root B — no expansion of a word-start `~` in an unquoted `${x:-word}`/`${x:=word}`

```
HOME=/usr/xyz
echo  ${TPATH:-~}       # bash: /usr/xyz      huck: ~
echo "${TPATH:-~}"      # bash: ~             huck: ~   (agree — quoted stays literal)
: ${A:=~/bin:~/bin2:$XPATH}; echo $A
                        # bash: /usr/xyz/bin:~/bin2:...   huck: ~/bin:~/bin2:...
```

The default word is expanded through the tilde-resolving `expand_assignment`, but
`scan_step_param_operand` emits a literal `~` rather than a `WordPart::Tilde`.
bash expands the **leading (word-start)** tilde only — not one after `:` (line 2
above: `~/bin` expands, `~/bin2` does not) — and only for VALUE operands, not
patterns.

Verified bash rules (targets):
- Assignment value: word-start tilde expands; tilde after an unquoted `:` expands;
  tilde after an embedded `=` does **not**. (`foo=~:~` → both expand; `h=HOME=~`
  → literal.)
- `${x:-word}`/`${x:=word}`/`${x:+word}` (and colon-less `-`/`=`/`+`) default
  word: only a **word-start** tilde expands, and only when the `${…}` is
  **unquoted**. Quoted `"${x:-~}"` and inner-quoted `${x:-"~"}` stay literal.
- Pattern operands (`#`/`##`/`%`/`%%`/`/`) are **not** tilde-expanded.

## Design

Two edits, both in `crates/huck-syntax/src/lexer.rs`, prototype-verified against
bash 5.2.21.

### 1. Root A — assignment-value tilde eligibility re-enables on `:` only

In the assignment-value literal-run scan (`scan_command_word_atom`, ~line 5265),
the per-char eligibility update is:

```rust
boundary = self.in_assignment_value && matches!(ch, '=' | ':');   // before
boundary = self.in_assignment_value && matches!(ch, ':');         // after
```

The initial eligibility (a tilde right after the assigning `=`) already comes
from `begin_assignment_value` / `try_scan_assign_prefix` seeding
`assign_val_tilde_ok = true`, so a word-start tilde still expands; only an
*embedded* `=` stops re-triggering. `foo=~:~` (tilde3.sub) still expands both —
the `:` re-enables — so no regression.

### 2. Root B — recognize a word-start `~` in an unquoted value-operand

In `scan_step_param_operand`, the **unquoted** branch, at **operand-word-start**
(no operand chars emitted yet this word), when
`!in_dquote && !enclosing_dquote && !is_pattern`, recognize `~` via
`try_parse_tilde` and emit `TokenKind::Tilde { spec, assign_ctx: false }`
(word-start → `assign_ctx=false`, matching the existing `~` command-word arm);
otherwise fall through to the current literal handling. Gates:
- `!in_dquote && !enclosing_dquote` → quoted defaults stay literal (`"${x:-~}"`,
  `${x:-"~"}`).
- `!is_pattern` (the v333 discriminator) → `#`/`%`/`/` operands stay literal.
- word-start only → a tilde after `:` in the operand stays literal (bash rule).

Because `expand_word_to_string` already routes the operand word through the
tilde-resolving `expand_assignment`, emitting a `WordPart::Tilde` is sufficient —
no engine change.

## Testing

Gate = bash 5.2.21 fidelity + `tilde2` at 0 diff + no per-category regressions.

1. **Bash-diff harness** — extend `tests/scripts/tilde_diff_check.sh` (currently
   27/27) with the tilde2 cases, byte-identical incl. stderr + exit:
   - Root A: `h=HOME=~`, `ADDPATH=PATH=~/bin:$XPATH`, `export h=HOME=~`, and the
     regression `foo=~:~` (both expand) and `foo=a:~` (after `:` expands) /
     `foo=a=~` (after inner `=` literal).
   - Root B: `${x:-~}`, `${x:=~}`, `${x:+~}` unquoted (expand) vs quoted
     `"${x:-~}"` / `${x:-"~"}` (literal); `${A:=~/bin:~/bin2}` (first expands,
     second literal); pattern operands `${x#~}`/`${x%~}` (literal).
   - The posix / non-posix `eval` tail from `tilde2.tests` (guards the Root A
     cascade).
2. **`tilde2` category** flips: `HUCK_BASH_TEST_CATEGORY=tilde2` → PASS, 0 diff.
3. **Regression**: huck-syntax lib green; huck-engine lib green; the tilde /
   assignment / expansion `-p huck` integration bins green; full
   `run_diff_checks.sh` sweep green; previously-flipped categories
   (`tilde`/`array2`/`nquote`/`dynvar`/`parser`/`rhs-exp`) stay PASS. Both edits
   are lexer word-recognition changes, so compare per-category diff-LINE counts
   against the saved baseline scratch dir — watch `array`, `assoc`, `new-exp`,
   `exp-tests`, `quote`, `posixexp` (any word-lexing-adjacent category) for
   within-category regressions the PASS table would hide.

Per repo constraints: build the binary with `cargo build -p huck`; per-crate
tests single-threaded (`cargo test -p <crate> --jobs 1 -- --test-threads 1`);
NEVER `cargo test --workspace`; guard runner/sweeps with `ulimit -v` + `timeout`;
run the `-p huck` integration bins single-threaded before push; NO GPL bash text
copied into the repo.

## Scope

**In scope.** The two lexer tilde-recognition fixes (Root A eligibility, Root B
operand word-start); the extended harness; the `tilde2` flip; regressions.

**Out of scope (tracked).** The architectural move of tilde-prefix recognition
into the expansion phase — filed as #295, a separate iteration. Named-user tilde
`~user:` before `:` in a command word (#72, pre-existing, not in `tilde2`).

## Documentation

- Removes a divergence (no new intentional one). #294 auto-closes via the PR
  (`Closes #294`); `docs/bash-divergences.md` unchanged. #295 stays open.
- Update the bash-test-suite baseline doc (`tilde2` PASS, Summary PASS 23→24,
  FAIL 59→58) if present; record in `project_huck_iterations.md` + `MEMORY.md`.
