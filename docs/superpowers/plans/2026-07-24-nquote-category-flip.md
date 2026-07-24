# v333 — Flip the `nquote` bash-suite category Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two `$'…'` (ANSI-C) quoting divergences so the `nquote` bash-suite category reaches 0-diff (Summary PASS 21→22, FAIL 61→60).

**Architecture:** Root A is a one-arm fix in the ANSI-C escape decoder (`\c\\` off-by-one). Root B threads a parser-set `is_pattern` flag onto `Mode::ParamWordOperand` so the lexer's `$'…'` arm suppresses ANSI-C only for a heredoc VALUE word (patterns still expand) — parser-driven, no lexer lookahead, no lexer→parser dependency.

**Tech Stack:** Rust; huck-syntax (`lexer.rs`, `parser.rs`); bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-24-nquote-category-flip-design.md`
Issue: [#289](https://github.com/jdstanhope/huck/issues/289)

## Global Constraints

- bash 5.2.21 fidelity — byte-identical incl. stderr + exit:
  - `$'\c\\'` → 1c; `$'\c\a'` → 1c 61; `$'\c\\\c]'` → 1c 1d; `$'\c[\c\\\c]\c^\c_\c?'` → 1b 1c 1d 1e 1f 7f.
  - In a heredoc: value words (`${x-…}`/`${x=…}`/`${x?…}`/`${x+…}`) keep `$'…'` LITERAL; patterns (`${x%…}`/`${x#…}`/`${x/…}`/case `${x^…}`) EXPAND it. Outside a heredoc (dquote/unquoted) BOTH expand.
- **Architecture rule (hard):** the lexer must NOT depend on the parser or add any forward scan. `is_pattern` is set by the PARSER when it pushes `Mode::ParamWordOperand`; the lexer only READS it from the current mode (like the existing `in_dquote`/`enclosing_dquote`). Do not add lookahead to distinguish value-vs-pattern.
- Do NOT change existing `$'…'` decoding for non-`\c\\` escapes, the dquote/unquoted `${x-word}` expansion, or the `EPOCHSECONDS`/other unrelated code.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace`; guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in commits (bare `#N`).

---

### Task 1: Root A — `\c\\` control-char escape

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`decode_ansi_c_escape`, the `Some('c')` arm)
- Create: `tests/scripts/nquote_ansi_c_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/nquote_ansi_c_diff_check.sh` (model on `tests/scripts/syntax_error_diag_diff_check.sh`; a reusable `check "label" 'frag'` comparing `bash --norc --noprofile -c` vs `"$HUCK_BIN" -c`, byte-identical stdout+stderr+exit, huck path normalized). Task 2 appends heredoc cases — keep the helper reusable. Root A cases (compare the raw bytes; the fragments `printf '%s' $'…' | od -An -tx1`):
```sh
check "cc backslash"   "printf '%s' \$'\\c\\\\' | od -An -tx1"          # 1c
check "cc bs then a"    "printf '%s' \$'\\c\\a' | od -An -tx1"           # 1c 61
check "cc bs then c]"   "printf '%s' \$'\\c\\\\\\c]' | od -An -tx1"      # 1c 1d
check "cc full run"     "printf '%s' \$'\\c[\\c\\\\\\c]\\c^\\c_\\c?' | od -An -tx1"  # 1b 1c 1d 1e 1f 7f
```
Build (`cargo build -p huck`) and run — the `\c\\`-containing cases FAIL. Confirm each against `bash --norc --noprofile` first.

- [ ] **Step 2: Add the `Some('\\')` arm**

In `crates/huck-syntax/src/lexer.rs`, `decode_ansi_c_escape`, the `Some('c') => match chars.next() { … }` block currently has `None`/`Some('?')`/`Some('@')`/`Some(c)` arms. Insert a `Some('\\')` arm before the generic `Some(c)`:
```rust
Some('\\') => {
    // `\c\` is Ctrl-\ (0x1C). The escaped-backslash form `\c\\`
    // consumes the SECOND backslash as part of the control target;
    // `\c\X` for any other X yields Ctrl-\ and leaves X. Matches
    // bash: `\c\\` -> 1c, `\c\a` -> 1c 61, `\c\\\c]` -> 1c 1d.
    if chars.peek() == Some(&'\\') {
        chars.next();
    }
    out.push('\x1C');
}
```

- [ ] **Step 3: Confirm the harness passes** (Root A cases) byte-identical to bash.

- [ ] **Step 4: Regression**
```bash
cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1   # green (475)
ulimit -v 1500000; HUCK_BIN=./target/debug/huck bash tests/scripts/nquote_ansi_c_diff_check.sh && echo PASS
HUCK_BIN=./target/debug/huck bash tests/scripts/ansi_c_quoting_diff_check.sh 2>/dev/null || true
cargo test -p huck --test ansi_c_quoting_integration --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
```

- [ ] **Step 5: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-syntax/src/lexer.rs tests/scripts/nquote_ansi_c_diff_check.sh
git commit -m "$(cat <<'EOF'
v333: fix $'\c\\' control-char escape off-by-one (#289)

`\c` followed by an escaped backslash (`\c\\`) is Ctrl-\ and must consume BOTH
backslashes; huck consumed only `\c\`, leaving a stray `\` that corrupted the
next escape. Add an explicit `Some('\\')` arm to the ANSI-C `\c` decoder.
Byte-identical to bash. Part of the nquote category flip.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Root B — heredoc value-word `$'…'` + category flip

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`Mode::ParamWordOperand` field; dispatch; `scan_step_param_operand` signature + `$'` arm; test-module constructions)
- Modify: `crates/huck-syntax/src/parser.rs` (set `is_pattern` at all `Mode::ParamWordOperand` sites)
- Modify: `crates/huck-syntax/src/parser/tests.rs` (test construction)
- Modify: `tests/scripts/nquote_ansi_c_diff_check.sh` (add heredoc cases)

- [ ] **Step 1: Add the harness cases (red)**

Append heredoc value-word (LITERAL) + pattern (EXPAND) + regression cases:
```sh
# value words in a heredoc -> $' LITERAL
check "hd default lit" 'unset none; cat <<EOF
[${none-X$'"'"'\t'"'"'Y}]
EOF'
# ...(also ${none=…}, ${set+…})
# patterns in a heredoc -> $' EXPAND
check "hd suffix exp"  'V=X$'"'"'\t'"'"'; cat <<EOF
[${V%$'"'"'\t'"'"'}]
EOF'
# regression: dquote/unquoted still EXPAND
check "dq value exp"   'unset none; printf "[%s]" "${none-X$'"'"'\t'"'"'Y}"'
```
(Quoting `$'\t'` inside the single-quoted `check` arg is fiddly — verify each fragment's expected bytes against `bash --norc --noprofile` FIRST, then encode. Use `od -An -c` in the fragment if literal-vs-expanded is easier to compare as chars.) These FAIL before the fix (huck expands the heredoc value-word `$'`).

- [ ] **Step 2: Add `is_pattern` to the mode + dispatch**

In `lexer.rs`, add `is_pattern: bool` to `Mode::ParamWordOperand`. In the mode-dispatch match, destructure it for `ParamWordOperand` and pass to `scan_step_param_operand`; for `ParamSubstPatternOperand`/`ParamSubstringOffsetOperand`/`ParamSubscriptOperand` pass `true`:
```rust
Mode::ParamWordOperand { in_dquote, enclosing_dquote, is_pattern }
    => self.scan_step_param_operand(None, '}', in_dquote, enclosing_dquote, is_pattern),
Mode::ParamSubstPatternOperand { in_dquote, enclosing_dquote }
    => self.scan_step_param_operand(Some('/'), '}', in_dquote, enclosing_dquote, true),
Mode::ParamSubstringOffsetOperand { in_dquote, enclosing_dquote }
    => self.scan_step_param_operand(Some(':'), '}', in_dquote, enclosing_dquote, true),
Mode::ParamSubscriptOperand { in_dquote, enclosing_dquote }
    => self.scan_step_param_operand(None, ']', in_dquote, enclosing_dquote, true),
```

- [ ] **Step 3: `scan_step_param_operand` param + gate**

Add `is_pattern: bool` to the signature. Change the `$'…'` arm (the one that calls `scan_ansi_c_quoted` and emits `QuoteRun{AnsiC}` in the "outside dquote" branch) to:
```rust
Some('\'') if !(self.emitting_heredoc.is_some() && !is_pattern) => {
```
(Unchanged body.) When the guard fails — a heredoc VALUE word — control falls to the lone-`$` `_ =>` literal arm, emitting `$` as a Lit; the `'` then scans as an ordinary run char (a bare `'…'` in a heredoc operand already does this, so no `'`-handling change is needed).

- [ ] **Step 4: Set `is_pattern` at every parser construction site**

In `parser.rs`, every `Mode::ParamWordOperand { in_dquote, enclosing_dquote }` gains `is_pattern`:
- `false` — `UseDefault`, `AssignDefault`, `ErrorIfUnset`, `UseAlternate` (value family).
- `true` — `RemovePrefix`, `RemoveSuffix`, the `/`-replacement word, `Case`, the substring length operand, and the two bad-substitution cleanup operands.

Also fix the test-module constructions (`is_pattern: false`) in `lexer.rs`'s `#[cfg(test)]` and `parser/tests.rs` so `cargo test` compiles. (The compiler's E0063 lists every site.)

- [ ] **Step 5: Confirm the harness passes** — value words LITERAL, patterns EXPAND, dquote/unquoted still EXPAND. Spot-check each operator against bash.

- [ ] **Step 6: `nquote` flips + family regression**
```bash
cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1   # green (fix test constructors)
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
cargo build --release -p huck
for c in nquote nquote1 nquote5 iquote cprint rhs-exp; do
  HUCK_BASH_TEST_CATEGORY=$c HUCK_TEST_TIMEOUT=60 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
    timeout 150 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "$c \|"
done
```
Expect: `nquote` PASS 0-diff; the others stay PASS (nquote2/3/4 are a separate class, unaffected).

- [ ] **Step 7: Broad regression**
```bash
# param/quote/heredoc integration bins (is_pattern touches the shared operand scanner):
for t in param_substitution_integration ansi_c_quoting_integration heredoc_integration \
         here_string_integration array_literal_expansion_integration braced_special_params_integration \
         indirect_expansion_integration alternate_word_quoting_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result" || echo "(no bin: $t)"
done
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # green (coproc flake pre-existing)
```

- [ ] **Step 8: Docs + memory**
  - `docs/bash-test-suite-baseline.md`: prepend "Updated by v333 (#289, 2026-07-24 UTC): `nquote` flipped to PASS (0-diff). Summary PASS 21→22, FAIL 61→60."
  - `project_huck_iterations.md` + `MEMORY.md`: record v333 (nquote flip; Root A `\c\\`; Root B the parser-set `is_pattern` value-vs-pattern discriminator — the clean way to give the lexer parser-known context without lookahead; the debt-zone dip stayed bounded).

- [ ] **Step 9: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs crates/huck-syntax/src/parser/tests.rs tests/scripts/nquote_ansi_c_diff_check.sh docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v333: $'…' literal in heredoc value-word operands; flips nquote to PASS (#289)

bash disables $'…' ANSI-C quoting inside a heredoc body, including in a ${x-word}
value-word operand, but a pattern operand (${x%…}/${x#…}/${x/…}/case) still
expands it. The value-vs-pattern distinction is parser-only, so add an is_pattern
flag to Mode::ParamWordOperand (set by the parser, read by the lexer's $' arm to
suppress ANSI-C only for a heredoc value word). Parser-driven, no lexer lookahead.
Completes the nquote category flip (12 -> 0 diff, Summary PASS 21->22).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files live outside the repo — update in the same session, not this commit.)

---

## Self-Review

- **Spec coverage:** Root A (Task 1); Root B mode/dispatch/gate + parser setters (Task 2); harness (Task 1, extended Task 2); category flip + regressions (Task 2). Both roots map to a task.
- **Placeholders:** none — exact code for the lexer edits. The harness heredoc-fragment quoting is flagged as fiddly (verify against bash first); the parser `is_pattern` values are enumerated by operator.
- **Type consistency:** `decode_ansi_c_escape(chars, out)`; `Mode::ParamWordOperand { in_dquote, enclosing_dquote, is_pattern }`; `scan_step_param_operand(sep, end, in_dquote, enclosing_dquote, is_pattern)`; `emitting_heredoc: Option<HeredocEmit>`.
- **Scope:** two roots only; no param-expansion refactor beyond the one flag; nquote2/3/4 and `$"…"`-in-heredoc explicitly out of scope. The review must confirm (a) the lexer reads `is_pattern` without any lookahead, and (b) the per-operator `is_pattern` values match bash (value=literal, pattern=expand in a heredoc).
