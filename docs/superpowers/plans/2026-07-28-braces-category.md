# v341 — flip the `braces` category Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip the bash-suite `braces` category to 0-diff PASS (runner PASS 29 → 30) by fixing four brace-expansion divergences.

**Architecture:** Three fixes in `crates/huck-syntax/src/brace_expand.rs` (`parse_range` negative step + backslash char range; `expand_into` nested non-comma body) and one post-pass in `crates/huck-syntax/src/lexer.rs` (`brace_expand_parts` bare-`$name` merge).

**Tech Stack:** Rust (`huck-syntax`), bash-vs-huck diff-check harnesses, the bash test-suite runner.

**Design reference:** `docs/superpowers/specs/2026-07-28-braces-category-design.md`. Issues: [#44](https://github.com/jdstanhope/huck/issues/44) (Root 1), [#318](https://github.com/jdstanhope/huck/issues/318) (Roots 2–4).

## Global Constraints

- **Branch:** all work on `v341-braces` (off `main`). Do NOT push to `main` or merge; hand the PR to the user.
- **Commit trailer:** every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit (CI enforces `--check`).
- **This box OOMs on `cargo test --workspace`.** Test per-crate single-threaded ONLY: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (and `-p huck-engine` where noted). Build the binary with `cargo build -p huck` / `cargo build --release --locked --bin huck`. Guard sweeps with `ulimit -v 1500000` + `timeout`.
- **Bash source** at `/tmp/bash-5.2.21`; `export BASH_SOURCE_DIR=/tmp/bash-5.2.21` for the runner.
- **Harness:** `tests/scripts/brace_expansion_diff_check.sh` uses `check "<label>" '<fragment>'` (runs the fragment through both `bash -c` and `huck -c`, asserts byte-identical incl. rc).
- **Empirical bash rules:** (Root 4) step-sign is ignored — `|step|`, direction from endpoints. (Root 2) only `\` (0x5C) is emptied in a char range; both endpoints must be ASCII letters. (Root 3) an outer `{body}` with no top-level comma/range has LITERAL braces but inner braces still expand. (Root 1) only bare `$name` (not `${name}`) merges a following name-continuation run.

---

### Task 1: Root 4 (negative step) + Root 2 (backslash char range) in `parse_range`

**Files:**
- Modify: `crates/huck-syntax/src/brace_expand.rs` (`parse_range`, integer step ~201, char step ~270, char loop ~284)
- Test: `tests/scripts/brace_expansion_diff_check.sh` (extend)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Add failing harness cases**

In `tests/scripts/brace_expansion_diff_check.sh`, add before the final total line:

```bash
# v341 (#318) Root 4: negative step (sign ignored; direction from endpoints).
check "neg step int desc"  'echo {10..1..-2}'
check "neg step int asc"   'echo {-1..-10..-2}'
check "neg step big"       'echo {100..0..-5}'
check "neg step char"      'echo {z..a..-2}'
check "pos step desc"      'echo {10..1..2}'
# v341 (#318) Root 2: backslash char range → empty element.
check "backslash range Aa" 'echo {A..a}'
check "backslash range Za" 'echo {Z..a}'
```

- [ ] **Step 2: Build and run the harness; confirm the new cases FAIL**

Run:
```bash
cargo build -p huck
bash tests/scripts/brace_expansion_diff_check.sh
```
Expected: the neg-step and backslash-range cases FAIL — huck leaves `{10..1..-2}` literal and includes a literal `\` in `{A..a}`.

- [ ] **Step 3: Root 4 — accept negative step (integer arm)**

In `parse_range`, the INTEGER arm's step match (~201) currently:
```rust
Some(s) => match s.parse::<i64>() {
    Ok(0) => return None,
    Ok(n) if n > 0 => {
        if r >= l { n } else { -n }
    }
    _ => return None,
},
```
Replace with:
```rust
Some(s) => match s.parse::<i64>() {
    Ok(0) => return None,
    Ok(n) => {
        // bash ignores the step's SIGN — magnitude only, direction from the
        // endpoints (`{10..1..-2}` == `{10..1..2}` → 10 8 6 4 2). (#318)
        let m = n.abs();
        if r >= l { m } else { -m }
    }
    Err(_) => return None,
},
```

- [ ] **Step 4: Root 4 — accept negative step (char arm)**

Apply the identical replacement to the CHAR arm's step match (~270) — same before/after as Step 3.

- [ ] **Step 5: Root 2 — empty the backslash in the char loop**

In the char-range loop (~284), the push:
```rust
if let Some(c) = char::from_u32(cur as u32) {
    out.push(c.to_string());
} else {
    return None;
}
```
becomes:
```rust
if let Some(c) = char::from_u32(cur as u32) {
    // bash emits an EMPTY element for `\` (0x5C) in a char range (#318);
    // every other char in the byte span is emitted literally.
    if c == '\\' {
        out.push(String::new());
    } else {
        out.push(c.to_string());
    }
} else {
    return None;
}
```

- [ ] **Step 6: Build and run the harness; confirm all cases PASS**

Run:
```bash
cargo build -p huck
bash tests/scripts/brace_expansion_diff_check.sh
```
Expected: `Fail: 0`.

- [ ] **Step 7: Per-crate lib tests**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 brace`
Expected: PASS — existing `brace_expand` unit tests unaffected (they use positive steps / letter ranges without `\`). If a test encoded a negative-step-returns-literal expectation, update it to the expanded form (bash-correct).

- [ ] **Step 8: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-syntax/src/brace_expand.rs tests/scripts/brace_expansion_diff_check.sh
git commit -m "$(cat <<'EOF'
v341: brace expansion negative step + backslash char range (#318)

parse_range rejected a negative explicit step; bash ignores the step sign
(magnitude only, direction from endpoints), so {10..1..-2} → 10 8 6 4 2.
And a char range emits an EMPTY element for `\` (0x5C), matching bash's
{A..a} quirk.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Root 3 — recurse into a nested non-comma brace body (`expand_into`)

**Files:**
- Modify: `crates/huck-syntax/src/brace_expand.rs` (`expand_into` None branch ~59-72)
- Test: `tests/scripts/brace_expansion_diff_check.sh` (extend)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Add failing harness cases**

Add before the final total line:

```bash
# v341 (#318) Root 3: nested non-comma brace — inner still expands.
check "nested non-comma"   'echo a-{b{d,e}}-c'
check "nested deeper"      'echo a-{b{c{d,e}}}-f'
check "nested plain body"  'echo x-{foo}-y'
check "brace spaces body"  'echo {a b}'
```

- [ ] **Step 2: Build and run the harness; confirm `nested non-comma`/`nested deeper` FAIL**

Run: `cargo build -p huck && bash tests/scripts/brace_expansion_diff_check.sh`
Expected: `nested non-comma` and `nested deeper` FAIL (huck leaves them literal); `nested plain body` and `brace spaces body` already PASS.

- [ ] **Step 3: Recurse into the body in the `None` branch**

In `expand_into`, replace the `None => { … }` branch (~59-72) with:

```rust
None => {
    // Outer {body} is not a brace expr (no top-level comma/range) → the
    // braces are LITERAL, but inner braces inside body still expand
    // (bash: `a-{b{d,e}}-c` → `a-{bd}-c a-{be}-c`). Recurse into body and
    // suffix and cross them, re-wrapping body in literal braces. Do NOT
    // re-feed `{be}` through expand_into — the literal braces would be
    // re-parsed as a top-level brace with no comma/range and recurse forever.
    let mut body_exp = Vec::new();
    expand_into(body, &mut body_exp)?;
    let mut suffix_exp = Vec::new();
    expand_into(suffix, &mut suffix_exp)?;
    for be in &body_exp {
        for se in &suffix_exp {
            out.push(format!("{prefix}{{{be}}}{se}"));
            if out.len() > MAX_ELEMENTS {
                return Err(BraceError::TooManyElements);
            }
        }
    }
    return Ok(());
}
```

- [ ] **Step 4: Build and run the harness; confirm all cases PASS**

Run: `cargo build -p huck && bash tests/scripts/brace_expansion_diff_check.sh`
Expected: `Fail: 0`. In particular `a-{b{d,e}}-c` → `a-{bd}-c a-{be}-c`; `x-{foo}-y` → `x-{foo}-y` (unchanged); `{a b}` → `{a b}` (unchanged).

- [ ] **Step 5: Per-crate lib tests**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 brace`
Expected: PASS — literal-brace-body tests unchanged.

- [ ] **Step 6: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-syntax/src/brace_expand.rs tests/scripts/brace_expansion_diff_check.sh
git commit -m "$(cat <<'EOF'
v341: expand inner braces of a non-comma outer body (#318)

`a-{b{d,e}}-c`: the outer {b{d,e}} has no top-level comma so its braces are
literal, but bash still expands the inner {d,e} → `a-{bd}-c a-{be}-c`. The
None branch now recurses into body (and suffix) and crosses them, re-wrapping
body in literal braces (without re-parsing them).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Root 1 — bare `$var{x,y}` name-merge (`brace_expand_parts`)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`brace_expand_parts` ~6825; new merge helper near it)
- Test: `tests/scripts/brace_expansion_diff_check.sh` (extend)

**Interfaces:**
- Consumes: `WordPart::{Var, Literal}` (lexer.rs:569).
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Add failing harness cases**

Add before the final total line:

```bash
# v341 (#44) Root 1: bare $var{x,y} merges the brace suffix into the name;
# braced ${var} does NOT (structurally distinct: Var vs ParamExpansion).
check "bare name merge"    'var=baz; varx=vx; vary=vy; echo $var{x,y}'
check "braced no merge"    'var=baz; varx=vx; vary=vy; echo ${var}{x,y}'
check "quoted braced"      'var=baz; varx=vx; vary=vy; echo "${var}"{x,y}'
check "merge non-namechar" 'var=baz; echo $var{-,+}'
check "merge digits"       'v1=one; v2=two; var=baz; echo $var{1,2}'
```

- [ ] **Step 2: Build and run the harness; confirm `bare name merge` FAILs**

Run: `cargo build -p huck && bash tests/scripts/brace_expansion_diff_check.sh`
Expected: `bare name merge` and `merge digits` FAIL — huck emits `bazx bazy` / `baz1 baz2` where bash emits `vx vy` / `one two`. (`braced no merge`, `quoted braced`, `merge non-namechar` already PASS.)

- [ ] **Step 3: Add the merge post-pass in `brace_expand_parts`**

Read `brace_expand_parts` (`lexer.rs:6825`): it builds a sentinel concat, calls `brace_expand::expand`, and maps each expanded string through `split_on_sentinels` into a `Vec<WordPart>`. Add a helper and apply it to each reconstructed `Vec<WordPart>` before returning:

```rust
/// bash brace-expands textually before parameter expansion, so `$var{x,y}`
/// becomes `$varx $vary` — the brace suffix's leading name-continuation run
/// merges into a BARE `$name`. huck reconstructs `[Var{var}, Literal{"x"}]`;
/// this merges the leading `[A-Za-z0-9_]` run of an unquoted Literal into an
/// immediately-preceding bare `WordPart::Var{quoted:false}`. Only bare `$name`
/// (Var) merges — braced `${name}` is a ParamExpansion and is left alone
/// (bash: `${var}{x,y}` → `bazx bazy`). v341 (#44).
fn merge_brace_name_suffix(parts: &mut Vec<WordPart>) {
    let mut i = 0;
    while i + 1 < parts.len() {
        // Compute the merge (name-continuation run to move + the remaining
        // literal) under a SCOPED immutable borrow, so it ends before the
        // mutation below — avoids holding `&parts[i+1]` across `&mut parts[i]`.
        let merge: Option<(String, String)> = {
            let bare_var = matches!(&parts[i], WordPart::Var { quoted: false, .. });
            match (bare_var, &parts[i + 1]) {
                (true, WordPart::Literal { text, quoted: false }) => {
                    let run_len = text
                        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                        .unwrap_or(text.len());
                    if run_len > 0 {
                        Some((text[..run_len].to_string(), text[run_len..].to_string()))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        };
        match merge {
            Some((run, rest)) => {
                if let WordPart::Var { name, .. } = &mut parts[i] {
                    name.push_str(&run);
                }
                if rest.is_empty() {
                    parts.remove(i + 1);
                } else {
                    parts[i + 1] = WordPart::Literal {
                        text: rest,
                        quoted: false,
                    };
                }
                // Do not advance i — a further literal may now be adjacent.
            }
            None => i += 1,
        }
    }
}
```

Apply it inside `brace_expand_parts` to each `Vec<WordPart>` produced by `split_on_sentinels` (map or a loop calling `merge_brace_name_suffix(&mut v)`), so the returned `Vec<Vec<WordPart>>` has merged names.

- [ ] **Step 4: Build and run the harness; confirm all cases PASS**

Run: `cargo build -p huck && bash tests/scripts/brace_expansion_diff_check.sh`
Expected: `Fail: 0`. `$var{x,y}` → `vx vy`; `${var}{x,y}` → `bazx bazy`; `$var{-,+}` → `baz- baz+`; `$var{1,2}` → `one two`.

- [ ] **Step 5: Per-crate lib tests + a targeted no-merge check**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
Expected: PASS. Then a manual spot-check that non-brace words are untouched (they never reach `brace_expand_parts`):
```bash
./target/debug/huck -c 'var=baz; echo $var-x $var.y "$var"z'
bash                  -c 'var=baz; echo $var-x $var.y "$var"z'
```
Expected: identical (`baz-x baz.y bazz`).

- [ ] **Step 6: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-syntax/src/lexer.rs tests/scripts/brace_expansion_diff_check.sh
git commit -m "$(cat <<'EOF'
v341: merge bare $name brace suffix into the variable name (#44)

bash brace-expands textually before param expansion, so `$var{x,y}` →
`$varx $vary`. huck reconstructs [Var{var}, Literal{"x"}]; a post-pass in
brace_expand_parts now merges the leading name-continuation run of an
unquoted Literal into an immediately-preceding bare Var. Only bare $name
(Var) merges; braced ${name} (ParamExpansion) is untouched.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Verify the flip, prove no-regression, update docs + memory

**Files:**
- Modify: `docs/bash-test-suite-baseline.md`
- Modify: `/home/john/.claude/projects/-home-john-projects-huck/memory/project_huck_iterations.md`
- Modify: `/home/john/.claude/projects/-home-john-projects-huck/memory/MEMORY.md`

**Interfaces:** none (verification + docs).

- [ ] **Step 1: Build release**

Run: `cargo build --release --locked --bin huck`
Expected: clean.

- [ ] **Step 2: Category runner — 0-diff PASS**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
HUCK_BASH_TEST_CATEGORY=braces bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E '^\| braces '
```
Expected: `| braces | PASS |`. If FAIL, inspect the fresh `/tmp/huck-bash-tests-*/braces.diff` (must be empty) and map any residual to Root 1–4.

- [ ] **Step 3: No-regression baseline**

```bash
git worktree add -q /tmp/huck-v341-base origin/main
( cd /tmp/huck-v341-base && ulimit -v 3000000 && cargo build --release --locked --bin huck )
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
for c in braces dollars more-exp new-exp exp-tests array assoc nquote nquote1; do
  b=$(HUCK_BASH_TEST_CATEGORY=$c bash /tmp/huck-v341-base/tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "^\| $c " | awk -F'|' '{gsub(/ /,"",$3);print $3}')
  v=$(HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "^\| $c " | awk -F'|' '{gsub(/ /,"",$3);print $3}')
  flag=""; [ "$b" != "$v" ] && flag="  <-- CHANGED ($b->$v)"
  printf "%-10s base=%-5s v341=%-5s%s\n" "$c" "$b" "$v" "$flag"
done
git worktree remove --force /tmp/huck-v341-base
```
Expected: `braces` FAIL→PASS; every other category holds its status (PASS stays PASS; any FAIL not worse — if a FAIL's status is unchanged, spot-check its diff line count didn't grow).

- [ ] **Step 4: Full diff-check sweep**

Run:
```bash
cargo build -p huck
( ulimit -v 1500000; timeout 600 bash tests/scripts/run_diff_checks.sh )
```
Expected: all harnesses PASS (green), incl. `brace_expansion`, `braced_special_params`, `param_*`, `array_*`.

- [ ] **Step 5: Touched integration bins (single-threaded)**

Run:
```bash
for t in brace_expansion_integration braced_special_params_integration special_params_integration; do
  ( ulimit -v 1500000; cargo test -p huck --test $t --jobs 1 -- --test-threads 1 ) 2>&1 | grep -E 'test result|error\[' || echo "MISSING/FAILED: $t"
done
```
Expected: each `test result: ok`.

- [ ] **Step 6: Confirm only braces flipped (full runner)**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E 'PASS:|FAIL:'
```
Expected: `PASS: 30`, `FAIL: 52`. Cross-check the PASS list gained exactly `braces`.

- [ ] **Step 7: Update `docs/bash-test-suite-baseline.md`**

Add a dated `**Updated by v341 (#44/#318, 2026-07-28 UTC):**` note at the top (mirroring the v340 note's style): `braces` flipped to PASS; the four roots (negative step, nested non-comma, backslash char-range, bare-`$name` merge); Summary PASS 29→30, FAIL 53→52; only `braces` flipped, no regressions. Update the `## Summary` count block (PASS 29→30, FAIL 53→52) and its PASS-category list. Replace the `| braces | FAIL | … |` row with a PASS row. Note #44 stays open for the broader brace-before-param ordering.

- [ ] **Step 8: Update memory files**

Append a v341 entry to `project_huck_iterations.md` (newest at top) and a one-line hook to the top of `MEMORY.md`'s iteration list: FLIPS `braces` 29→30 (4 brace-expansion roots — neg-step sign-ignored, nested non-comma inner-expand, backslash-char-range→empty, bare-`$var{x,y}`→`$varx` name-merge (targeted #44 slice, Var-not-ParamExpansion)); durable lessons — (a) at PASS 29 the clean single-root flips are gone; every near-miss now has a fundamental blocker (L-44 assoc order, L-11 char-vs-byte, `$0` prog-name, regex-engine, by-design) or is multi-root; (b) braces Root 1 looked architectural (#44) but scoped down to a bounded reconstruction post-pass because bare `$var`=Var and `${var}`=ParamExpansion are structurally distinct. Note #44 stays open.

- [ ] **Step 9: Commit docs**

```bash
git add docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v341: baseline — braces flipped to PASS (29->30) (#44/#318)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files live outside the repo — save them via the Write tool, not git.)

---

## Final review & PR (after all tasks)

- [ ] Review the whole branch diff (`git diff main...v341-braces`) for stray edits and formatting.
- [ ] Confirm `cargo fmt --all --check` clean and a fresh `cargo build --workspace --locked` (build only) succeeds.
- [ ] Push `v341-braces`, open a PR targeting `main` with body `Closes #318` (and noting #44 partially addressed — do NOT auto-close #44), a summary of the four roots + verification evidence. Hand to the user; wait for CI green before calling it ready (do NOT self-merge).
