# Braced Special Parameters `${-}`/`${?}`/`${$}`/`${!}` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `${-}`, `${?}`, `${$}`, `${!}` (and with modifiers, e.g. `${-#*e}`, `${?:-x}`) parse and evaluate. Unblocks nvm's `nvm()` flag tests (line 2963).

**Architecture:** Lexer — `read_braced_param_expansion` routes `-`/`?`/`$` (and bare `${!}`) through the existing `dispatch_braced_modifier` (handles bare `}` + modifiers). Eval — one arm in `Shell::lookup_var` for `?` (the others are already resolved). No new modifier/AST machinery.

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/lexer.rs` — `read_braced_param_expansion`: dispatch `-`/`?`/`$` → `dispatch_braced_modifier`; bare `${!}` → `$!` in the `!` branch.
- `src/shell_state.rs` — `lookup_var`: add `"?"` arm.
- `tests/braced_special_params_integration.rs`, `tests/scripts/braced_special_params_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — new Tier-2 entry `[fixed v102]` + `${!<modifier>}` edge note + changelog + README row.

---

### Task 1: Lexer dispatch + `lookup_var` `?` + tests

**Files:** `src/lexer.rs`, `src/shell_state.rs`, `tests/braced_special_params_integration.rs` (NEW)

- [ ] **Step 1: Write the failing integration test**

Create `tests/braced_special_params_integration.rs`:

```rust
//! v102: braced single-char special params ${-}/${?}/${$}/${!} (+ modifiers).
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
fn braced_status() {
    assert_eq!(run("false; echo \"${?}\"\n").0, "1\n");
    assert_eq!(run("true; echo \"${?}\"\n").0, "0\n");
}

#[test]
fn braced_status_with_modifier() {
    // ${?:-x}: status is set (0), so :- yields the status, not the default.
    assert_eq!(run("true; echo \"${?:-na}\"\n").0, "0\n");
}

#[test]
fn braced_pid_equals_dollar_dollar() {
    assert_eq!(run("[ \"${$}\" = \"$$\" ] && echo same || echo diff\n").0, "same\n");
}

#[test]
fn braced_dash_equals_unbraced() {
    assert_eq!(run("[ \"${-}\" = \"$-\" ] && echo same || echo diff\n").0, "same\n");
}

#[test]
fn braced_dash_remove_prefix_nvm_shape() {
    // nvm's errexit test. Under default (no -e): ${-#*e} == $- -> "no".
    assert_eq!(run("f() { if [ \"${-#*e}\" != \"$-\" ]; then echo yes; else echo no; fi; }\nf\n").0, "no\n");
    // With -e set: removing up-to-e changes it -> "yes".
    assert_eq!(run("set -e\nf() { if [ \"${-#*e}\" != \"$-\" ]; then echo yes; else echo no; fi; }\nf\n").0, "yes\n");
}

#[test]
fn braced_bgpid_empty_then_set() {
    assert_eq!(run("[ -z \"${!}\" ] && echo empty\n").0, "empty\n");
    assert_eq!(run("sleep 0 &\n[ -n \"${!}\" ] && echo set\nwait\n").0, "set\n");
}

#[test]
fn regression_braced_count_allargs_indirect_unchanged() {
    assert_eq!(run("set -- a b c\necho \"${#}\"\necho \"${@}\"\nx=hi; r=x; echo \"${!r}\"\n").0,
               "3\na b c\nhi\n");
}
```
Verify each expected output against bash first (`printf '...' | bash`) and adjust to bash's actual output (esp. the `${?:-x}` and `${-#*e}` cases, and confirm `${$}`==`$$`).

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test braced_special_params_integration 2>&1 | tail -20`
Expected: FAIL — `${-}`/`${?}`/`${$}`/`${!}` error `parameter expansion with empty name`; the regression test already passes.

- [ ] **Step 3: Lexer — dispatch `-`/`?`/`$`**

In `read_braced_param_expansion` (`src/lexer.rs:1784`), the function opens with a
`match chars.peek().copied()` handling `Some('@')` and `Some('*')`. Add arms for
the scalar specials, each routing through `dispatch_braced_modifier`:
```rust
        Some('-') => {
            chars.next();
            return dispatch_braced_modifier("-".to_string(), quoted, None, chars, parts, false);
        }
        Some('?') => {
            chars.next();
            return dispatch_braced_modifier("?".to_string(), quoted, None, chars, parts, false);
        }
        Some('$') => {
            chars.next();
            return dispatch_braced_modifier("$".to_string(), quoted, None, chars, parts, false);
        }
```
(Place beside the existing `Some('@')`/`Some('*')` arms. `dispatch_braced_modifier`
emits `ParamExpansion { name, modifier: None }` for a bare `}` and the right
modifier otherwise — so `${-}` and `${-#*e}` both work.)

- [ ] **Step 4: Lexer — bare `${!}` → `$!`**

In the `!` branch (`src/lexer.rs:~1888`), right after `chars.next()` consumes the
`!` and BEFORE the digit/`read_braced_name` indirect handling, add:
```rust
        // Bare `${!}` is the `$!` special param (last bg pid), NOT indirect.
        if chars.peek() == Some(&'}') {
            chars.next(); // consume `}`
            parts.push(WordPart::Var { name: "!".to_string(), quoted });
            return Ok(());
        }
```
Everything after stays the v95 indirect path (`${!var}`, `${!arr[@]}`, `${!N}`).
(`${!<modifier>}` like `${!:-x}` remains the indirect path — documented edge.)

- [ ] **Step 5: Eval — `lookup_var` resolves `?`**

In `Shell::lookup_var` (`src/shell_state.rs:431`), add to the special-params
`match name` (beside `"0"`/`"$"`/`"!"`/`"-"`):
```rust
            "?" => return Some(self.last_status().to_string()),
```
(Confirm the accessor name — `self.last_status()` per the existing codebase; match
the exact method used elsewhere.)

- [ ] **Step 6: Build + lexer unit tests**

`cargo build --bin huck`. Add lexer unit tests: `${-}` → a `ParamExpansion` with
`name == "-"` (modifier `None`); `${-#*e}` → `name == "-"`, modifier `RemovePrefix`;
`${?}`/`${$}`/bare `${!}` tokenize without error; `${!var}` still parses indirect
(`indirect == true`). Mirror neighboring braced-param lexer tests.

- [ ] **Step 7: Run integration + full suite + clippy**

Run: `cargo build --bin huck && cargo test --test braced_special_params_integration 2>&1 | tail -20` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures — ESPECIALLY param-expansion / special-params / indirect-v95 / `${var@OP}`-v96 suites).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).
Manual: `printf 'false; echo "${?}"\n' | ./target/debug/huck` → `1`; `printf '[ "${-}" = "$-" ] && echo ok\n' | ./target/debug/huck` → `ok`.

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs src/shell_state.rs tests/braced_special_params_integration.rs
git commit -m "feat: braced special params \${-}/\${?}/\${\$}/\${!} with modifiers

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: bash-diff harness (27th)

**Files:** `tests/scripts/braced_special_params_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper. Use
DETERMINISTIC fragments (compare-and-echo — never byte-compare raw `$-`/`$$`/`$!`
values, which vary across shells/runs):
```bash
check "status"         'false; echo "${?}"'
check "status zero"    'true; echo "${?}"'
check "status default" 'true; echo "${?:-na}"'
check "pid eq"         '[ "${$}" = "$$" ] && echo same || echo diff'
check "dash eq"        '[ "${-}" = "$-" ] && echo same || echo diff'
check "dash noe"       'f() { [ "${-#*e}" = "$-" ] && echo no || echo yes; }; f'
check "bgpid empty"    '[ -z "${!}" ] && echo empty || echo set'
check "nvm shape"      'f() { if [ "${-#*e}" != "$-" ]; then echo yes; else echo no; fi; }; f'
check "count regress"  'set -- a b c; echo "${#}"'
check "indirect regress" 'x=hi; r=x; echo "${!r}"'
```
After writing, RUN it and confirm fragments are well-formed. (`${-#*e}` under
huck's default flags vs bash's may differ in WHICH letters are present, but the
`= "$-"` comparison is self-relative, so the yes/no output is stable in each
shell — verify they agree.)

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/braced_special_params_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`. If `dash noe`/`nvm shape` diverges because
huck's default `$-` differs from bash's in a way that flips the `*e` test (e.g.
one shell has `e` by default and the other doesn't), STOP and report — that's a
real `$-` content difference to investigate, not a fragment bug. Otherwise
investigate any FAIL (bash is the oracle).

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/braced_special_params_diff_check.sh
git commit -m "test: bash-diff harness for braced special params (27th)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'special param\|^## Change log\|Missing features (Tier 2)\|2026-06-0\|^- \*\*L-[0-9]' docs/bash-divergences.md | head` and `grep -n '| v10' README.md`. Match v100/v101 style; next free `M-` number (highest is M-97).

- [ ] **Step 2: Add the Tier-2 entry**

New Tier-2 entry, next free `M-`, `[fixed v102]`: braced single-char special params `${-}`/`${?}`/`${$}`/`${!}` (+ modifiers) — lexer routes `-`/`?`/`$` through `dispatch_braced_modifier` and handles bare `${!}`→`$!`; `lookup_var` gained `?`. Note the nvm `${-#*e}` driver and the `${!<modifier>}`-vs-indirect edge (out of scope). Bump the Tier-2 count + roster narrative.

- [ ] **Step 3: Change-log + README row**

`2026-06-06` v102 change-log entry mirroring v100/v101 style (lexer dispatch + `?` eval; nvm `nvm()` flag tests now parse; 27th harness; nvm advances past 2963). v102 README row after v101.

- [ ] **Step 4: Verify + commit**

`grep -n 'v102\|fixed v102' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v102 braced special params fixed — changelog, README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 lexer + §2 eval → Task 1; testing → Tasks 1/2; new Tier-2 entry → Task 3. Covered.
- **Placeholder scan:** none — the dispatch arms, the bare-`${!}` block, and the `lookup_var` arm are shown in full.
- **Type consistency:** `dispatch_braced_modifier(name: String, quoted, None, chars, parts, false)` (6-arg signature per v95/v96); `WordPart::Var { name, quoted }`; `lookup_var` `"?" => last_status`. Reuses the modifier eval + special-param resolution.
- **Edge cases:** `${!}` bare → `$!`, `${!var}` indirect unchanged, `${!<modifier>}` documented out-of-scope; specials non-subscriptable (`None`); `${#}`/`${@}`/`${*}`/`${0}`/digit/`${!var}` paths untouched; harness compares self-relative (no raw `$-`/`$$`/`$!` byte-compare).
