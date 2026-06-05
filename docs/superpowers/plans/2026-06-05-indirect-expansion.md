# `${!var}` Indirect Expansion + `[[ ]]` Empty-Integer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support bash `${!var}` indirect parameter expansion (resolve `var`'s value to a name, then expand that name — incl. composition with a trailing modifier), and make `[[ ]]` integer comparisons treat an empty operand as `0` instead of erroring. Clears the entire `bash_completion` error cascade.

**Architecture:** Add an `indirect: bool` field to `WordPart::ParamExpansion` (threaded through `dispatch_braced_modifier`); the lexer's `${!` branch parses the bare/modifier-composed indirect form instead of rejecting it. At eval, a new `expand_indirect` helper in `src/expand.rs` resolves the through-value to an effective name N, then re-expands `${N<modifier>}` via the existing primitives. Separately, the `[[ ]]` integer arm coerces an empty operand to 0.

**Tech Stack:** Rust (binary crate `huck`). Unit tests `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/lexer.rs` — `WordPart::ParamExpansion` gains `indirect: bool`; `dispatch_braced_modifier` takes/sets it; the `${!` branch (~1850) parses the indirect scalar form; ~24 construction sites pass `indirect: false`.
- `src/expand.rs` — new `expand_indirect`; the two `ParamExpansion` match arms (679, 822) destructure `indirect` and call `expand_indirect` when set.
- `src/executor.rs` — `[[ ]]` integer arm (~950): empty operand → 0.
- `tests/indirect_expansion_integration.rs`, `tests/scripts/indirect_expansion_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — fixed note + deferrals + changelog + README row.

---

### Task 1: `${!var}` indirect expansion (lexer + eval, end-to-end)

This is a coupled lexer + eval change; implement together so the feature is testable end-to-end. TDD: integration test first.

**Files:** `src/lexer.rs`, `src/expand.rs` (+ compiler-flagged construction/match sites)

- [ ] **Step 1: Write the failing integration test**

Create `tests/indirect_expansion_integration.rs`:

```rust
//! v95: ${!var} indirect parameter expansion (M-indirect).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn indirect_through_named_var() {
    assert_eq!(run("x=hi\nref=x\necho \"${!ref}\"\n").0, "hi\n");
}

#[test]
fn indirect_through_positional() {
    // OPTIND=2 -> ${2}
    assert_eq!(run("set -- a b c\nOPTIND=2\necho \"${!OPTIND}\"\n").0, "b\n");
}

#[test]
fn indirect_with_default_modifier_unset() {
    assert_eq!(run("ref=missing\necho \"${!ref-fallback}\"\n").0, "fallback\n");
}

#[test]
fn indirect_with_default_modifier_set() {
    assert_eq!(run("x=val\nref=x\necho \"${!ref-fallback}\"\n").0, "val\n");
}

#[test]
fn indirect_source_unset_is_empty() {
    assert_eq!(run("unset ref\necho \"[${!ref}]\"\n").0, "[]\n");
}

#[test]
fn array_keys_still_work() {
    // Regression: ${!a[@]} is array keys, NOT indirect.
    assert_eq!(run("a=(p q r)\necho \"${!a[@]}\"\n").0, "0 1 2\n");
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test indirect_expansion_integration 2>&1 | tail -20`
Expected: FAIL — `${!ref}` currently errors `invalid parameter-expansion modifier: !` (so output is empty / wrong). `array_keys_still_work` should already PASS.

- [ ] **Step 3: Add the `indirect: bool` field to `WordPart::ParamExpansion`**

In `src/lexer.rs`, the struct variant (`:139`):
```rust
    ParamExpansion {
        name: String,
        modifier: ParamModifier,
        quoted: bool,
        subscript: Option<SubscriptKind>,
        /// `${!name}` indirect expansion: resolve `name`'s value to an
        /// effective name, then expand THAT (with `modifier`). v95.
        indirect: bool,
    },
```

- [ ] **Step 4: Thread `indirect` through `dispatch_braced_modifier` and fix construction sites**

Give `dispatch_braced_modifier` (`:2190`) a new `indirect: bool` parameter, and add `indirect` to every `WordPart::ParamExpansion { … }` it builds (sites ~2207, 2223, 2229, 2235, 2241, 2247, 2260). Update all OTHER callers of `dispatch_braced_modifier` to pass `indirect: false`.

Then `cargo build --bin huck 2>&1` and fix every remaining `WordPart::ParamExpansion { … }` construction the compiler flags (the IndirectKeys site ~1862, the no-modifier site ~1822, and any test/expand.rs constructors) by adding `indirect: false` (or `indirect: true` only where semantically indirect — which is just the new `${!` scalar path). For match/destructure sites that don't need it, add `, ..` or `, indirect: _`.

- [ ] **Step 5: Parse the indirect scalar form in the `${!` branch**

In `read_braced_param_expansion`, the `${!` branch (`:1850-1876`). Keep the `[@]`/`[*]` → `IndirectKeys` case unchanged. Replace the `_ =>` arm (currently `Err(InvalidBraceModifier("!"))`) so it parses the indirect scalar form. The `_ =>` arm is reached with the name already read and the (non-`[@]`/`[*]`) `subscript` already scanned; dispatch the trailing modifier with `indirect: true`:

```rust
            _ => {
                // `${!NAME}` / `${!NAME-word}` / `${!NAME[i]}` — indirect
                // scalar expansion (v95): resolve NAME's value to a name,
                // then expand that (with any trailing modifier).
                return dispatch_braced_modifier(name, quoted, subscript, chars, parts, /* indirect */ true);
            }
```

(Here `subscript` is the already-scanned `Option<SubscriptKind>` from the `${!` branch; it is `None` for `${!ref}`, `Some(Index..)` for `${!a[i]}`.)

- [ ] **Step 6: Add lexer unit tests for the parse shapes**

In the lexer `#[cfg(test)] mod tests`, add tests asserting:
- `${!ref}` → a `ParamExpansion { indirect: true, modifier: None, subscript: None, .. }`.
- `${!ref-w}` → `ParamExpansion { indirect: true, modifier: UseDefault{..}, .. }`.
- `${!a[@]}` → `ParamExpansion { indirect: false, modifier: IndirectKeys, .. }` (regression).

Mirror the existing lexer test helper pattern (`tokenize("…").unwrap()` then match `parts[0]`).

- [ ] **Step 7: Add `expand_indirect` to `src/expand.rs`**

Place near the other expansion helpers. It resolves the through-value to an effective name, then re-expands with the carried modifier:

```rust
/// `${!name<modifier>}` indirect expansion: resolve `name`(+`subscript`)'s
/// scalar value to an effective name N, then expand `${N<modifier>}`.
/// Empty/unset through-value → Empty (set -u handled by the caller's normal
/// unset path). N is interpreted as a parameter reference: a plain name, a
/// positional digit / special param, or `name[sub]`.
fn expand_indirect(
    name: &str,
    subscript: Option<&crate::lexer::SubscriptKind>,
    modifier: &crate::lexer::ParamModifier,
    quoted: bool,
    shell: &mut Shell,
) -> crate::param_expansion::ExpansionResult {
    use crate::param_expansion::ExpansionResult;
    // Step 1: through-value = scalar value of (name, subscript).
    let through = match subscript {
        None => shell.lookup_var(name).unwrap_or_default(),
        Some(sub) => {
            // Indirect through an array element: reuse the array-element
            // scalar read. Resolve to a string; empty if absent.
            match expand_array_param(name, &crate::lexer::ParamModifier::None, sub, quoted, shell) {
                ExpansionResult::Value(v) => v,
                _ => String::new(),
            }
        }
    };
    let n = through.trim();
    if n.is_empty() {
        return ExpansionResult::Empty;
    }
    // Step 2: parse N into (effective_name, effective_subscript) and re-expand.
    if let Some((base, sub)) = split_name_subscript(n) {
        // N is `name[sub]` — re-expand that array element with the modifier.
        return expand_array_param(&base, modifier, &sub, quoted, shell);
    }
    crate::param_expansion::expand_modifier(n, modifier, shell)
}
```

Notes for the implementer:
- `shell.lookup_var(name)` is the scalar lookup used elsewhere (resolves named vars, positionals, specials). Confirm its exact name/signature (grep `fn lookup_var`).
- `split_name_subscript(n)` need only handle the simple `name[123]` form: if `n` ends with `]` and contains `[`, split into `base` and a `SubscriptKind::Index`. If parsing the subscript is non-trivial, you MAY scope this to plain-name/positional N (return without the array branch) and document `${!ref}`-resolves-to-`arr[i]` as a deferral — that case is not used by bash_completion. Keep the plain-name/positional path (the 100%-of-bash_completion path) fully working.
- `expand_array_param` is the existing helper (`src/expand.rs:405`).

- [ ] **Step 8: Wire `expand_indirect` into both `ParamExpansion` arms**

Both arms (`src/expand.rs:695` and `:822`) currently destructure `{ name, modifier, quoted, subscript }`; add `indirect` and branch on it:

```rust
            WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect } => {
                let result_pe = if *indirect {
                    expand_indirect(name, subscript.as_ref(), modifier, *quoted, shell)
                } else if let Some(sub) = subscript {
                    expand_array_param(name, modifier, sub, *quoted, shell)
                } else if matches!(/* the existing @/* substring guard */) {
                    expand_positional_substring(name, modifier, *quoted, shell)
                } else {
                    crate::param_expansion::expand_modifier(name, modifier, shell)
                };
                // …existing result_pe handling unchanged…
```

Apply the same `if *indirect { expand_indirect(...) } else { …existing… }` prefix to BOTH arms (the field-split arm at ~695 and the assignment/string arm at ~822). The existing `result_pe` consumption (Value/Empty/WordList) is unchanged.

- [ ] **Step 9: `set -u` handling**

Confirm that `${!ref}` with `ref` unset under `set -u` raises the unbound-variable fatal error like a normal unset reference. In `expand_indirect` Step 1, when `subscript` is `None` and `shell.lookup_var(name)` is `None` (truly unset) and `set -u` is active, trigger the same `pending_fatal_pe_error` path that a normal `${unset}` uses (grep how `expand_modifier`/`ParamModifier::None` raises it — reuse that, don't reinvent). If wiring this cleanly is involved, at minimum match bash's observable behavior; add a test `set -u; unset ref; echo ${!ref}` → nonzero exit. Report what you did.

- [ ] **Step 10: Build, run integration + lexer tests, full suite, clippy**

Run: `cargo build --bin huck && cargo test --test indirect_expansion_integration 2>&1 | tail -20` (all 6 PASS).
Run: `cargo test --bin huck 2>&1 | tail -5` (lexer/param tests green) and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 11: Commit**

```bash
git add src/lexer.rs src/expand.rs tests/indirect_expansion_integration.rs
git commit -m "feat: \${!var} indirect parameter expansion (bare + modifier composition)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: `[[ ]]` empty operand → 0

**Files:** `src/executor.rs` (integer-comparison arm, ~950); unit test in the test module

- [ ] **Step 1: Write the failing test**

Add to the executor `#[cfg(test)] mod tests` (or wherever `[[ ]]` integer tests live — grep `IntEq`/`bad integer` in tests). If executor-level unit testing is awkward, add the assertions to `tests/indirect_expansion_integration.rs` instead (they ship together this iteration):

```rust
#[test]
fn dbracket_empty_operand_is_zero() {
    assert_eq!(run("[[ \"\" -ge 0 ]] && echo Y || echo N\n").0, "Y\n");
    assert_eq!(run("[[ \"\" -eq 0 ]] && echo Y || echo N\n").0, "Y\n");
    assert_eq!(run("[[ 3 -gt \"\" ]] && echo Y || echo N\n").0, "Y\n");
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test indirect_expansion_integration dbracket_empty 2>&1 | tail -10`
Expected: FAIL — current huck prints `[[: bad integer:` and the comparison errors.

- [ ] **Step 3: Coerce empty operand to 0**

In `src/executor.rs` (the integer-comparison arm, `:950-951`):
```rust
            let l: i64 = lhs.parse().map_err(|_| format!("bad integer: {lhs}"))?;
            let r: i64 = rhs.parse().map_err(|_| format!("bad integer: {rhs}"))?;
```
Change so an empty/all-whitespace operand parses as 0 (keep the `bad integer` error for a non-empty non-numeric operand):
```rust
            let parse_int = |s: &str| -> Result<i64, String> {
                let t = s.trim();
                if t.is_empty() { return Ok(0); }
                t.parse().map_err(|_| format!("bad integer: {s}"))
            };
            let l: i64 = parse_int(lhs)?;
            let r: i64 = parse_int(rhs)?;
```
(Place the closure just before these two lines, matching the surrounding style. Confirm the exact variable names / `?`-context.)

- [ ] **Step 4: Verify**

Run: `cargo build --bin huck && cargo test --test indirect_expansion_integration dbracket_empty 2>&1 | tail -10` (PASS).
Manual: `printf '[[ abc -ge 0 ]]; echo $?\n' | ./target/debug/huck 2>&1` — still errors (`bad integer: abc`, rc 2), confirming non-numeric is unchanged.
Run: `cargo test --bin huck 2>&1 | tail -3` (no regressions in `[[ ]]` tests).

- [ ] **Step 5: Commit**

```bash
git add src/executor.rs tests/indirect_expansion_integration.rs
git commit -m "fix: [[ ]] integer comparison treats empty operand as 0 like bash

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: bash-diff harness (20th)

**Files:** `tests/scripts/indirect_expansion_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper (runs a fragment through bash and huck, asserts byte-identical combined stdout+stderr+exit). Fragments:

```bash
check "indirect named"     'x=hi; ref=x; echo "${!ref}"'
check "indirect positional" 'set -- a b c; OPTIND=2; echo "${!OPTIND}"'
check "indirect pos 2"      'set -- a b c; echo "${!2-na}"'
check "indirect default unset" 'ref=missing; echo "${!ref-fallback}"'
check "indirect default set" 'x=val; ref=x; echo "${!ref-fallback}"'
check "indirect source unset" 'unset ref; echo "[${!ref}]"'
check "array keys regress"   'a=(p q r); echo "${!a[@]}"'
check "dbracket empty ge"    '[[ "" -ge 0 ]]; echo $?'
check "dbracket empty eq"    '[[ "" -eq 0 ]]; echo $?'
check "dbracket rhs empty"   '[[ 3 -gt "" ]]; echo $?'
```

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/indirect_expansion_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`. If any FAIL, bash is the oracle — investigate (don't mask). A fragment that exercises something genuinely out of scope (e.g. `${!ref}` resolving to `arr[i]`) should be dropped with a comment only after confirming it's out of scope, not a real bug.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/indirect_expansion_diff_check.sh
git commit -m "test: bash-diff harness for \${!var} indirect + [[ ]] empty-integer

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 4: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'indirect\|IndirectKeys\|${!\|^## Change log\|Missing features (Tier 2)\|Low-impact\|2026-06-05' docs/bash-divergences.md | head -30`. Find the v71 array-keys entry that mentions the deferred bare-`${!NAME}` indirect form (likely M-82 or an L-note), the change-log top, and the README table.

- [ ] **Step 2: Mark the indirect form fixed**

Update the entry that noted bare `${!NAME}` indirect as deferred (from v71) to record it `[fixed v95]`: bare `${!ref}`, indirection through positionals (`${!OPTIND}`/`${!2}`), and composition with a trailing modifier (`${!ref-}`/`${!1#\~}`) now work via an `indirect: bool` field on `ParamExpansion` + an `expand_indirect` two-step (through-value → effective name → re-expand). Note the cleared bash_completion cascade.

- [ ] **Step 3: Note the `[[ ]]` fix + deferrals**

- Add a note (in the `[[ ]]`/test entry — grep `M-14`) that `[[ ]]` integer comparison treats an empty operand as 0 `[fixed v95]`; full arithmetic evaluation of `[[ ]]` integer operands (bare identifiers, `2+3`) remains deferred.
- Add a `[deferred]` note for prefix-name `${!prefix@}` / `${!prefix*}` (list variable names by prefix; not used by the bashrc) — assign the next free `M-` number, OR fold it as a sub-note on the indirect entry. Match the doc's existing convention for such follow-ons.

- [ ] **Step 4: Change-log + README row**

Add a `2026-06-05` v95 change-log entry mirroring v93/v94 style: `${!var}` indirect expansion (bare + positional + modifier composition; `expand_indirect`; the `indirect` field), the bundled `[[ ]]` empty-operand→0 fix, 20th harness `indirect_expansion_diff_check.sh`, the deferrals (prefix `${!pre@}`, full `[[ ]]` arith operands), and that it clears the entire `bash_completion` `${!…}`/`unexpected token after command` cascade. Add a v95 README row after v94.

- [ ] **Step 5: Verify + commit**

`grep -n 'v95\|indirect\|fixed v95' docs/bash-divergences.md README.md` (confirm entries, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v95 \${!var} indirect + [[ ]] empty-integer — changelog, README, deferrals

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 lexer + §2 eval → Task 1; §3 `[[ ]]` → Task 2; testing → Tasks 1/2/3; deferrals + fixed notes → Task 4. Covered.
- **Placeholder scan:** none — all new code (helper, lexer branch, `[[ ]]` closure) shown; the "compiler-guided" steps (4, 8 match sites) are inherent to a field add and name the exact sites from grep.
- **Type consistency:** `WordPart::ParamExpansion { …, indirect: bool }`; `dispatch_braced_modifier(…, indirect: bool)`; `expand_indirect(name, subscript: Option<&SubscriptKind>, modifier, quoted, shell) -> ExpansionResult`; reuses `expand_array_param` / `expand_modifier` / `lookup_var`. `IndirectKeys` (array keys) stays `indirect: false` — distinct from the scalar indirect path.
- **Edge cases:** empty/unset through-value → Empty (+ `set -u` fatal, Step 9); `${!a[@]}` keys regression-tested; non-numeric `[[ ]]` operand still errors; `${!ref}`-resolves-to-`arr[i]` documented as best-effort/deferral if non-trivial.
