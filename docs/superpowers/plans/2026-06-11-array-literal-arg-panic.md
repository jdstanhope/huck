# v136 — fix the array-literal-as-argument panic (M-114) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the panic when an array-literal / assignment-prefix word reaches `expand()`/`expand_assignment()` as a command argument (`eval x=(a b)`, `eval arr+=(x y)`, `eval a[1]=v`, `eval a[2]+=z`). Reconstruct it to `name…=…` text so `eval` re-parses it — matching bash. M-114 is the last open Tier-1 bug.

**Architecture:** Replace the four `unreachable!` arms (`AssignPrefix` + `ArrayLiteral`, in both `expand` and `expand_assignment`) with reconstruction via shared helpers (`render_assign_target`, `reconstruct_array_literal`, `render_elem_value`). Route v130's xtrace array render through the same `reconstruct_array_literal` (de-dup). The reconstructed text is pushed as ONE field/string.

**Tech Stack:** Rust. Tests: cargo integration (compare to bash) + a bash-diff harness.

**GIT SAFETY:** Do NOT `git checkout <sha>` — stay on `v136-array-literal-arg-panic`; edit, build, commit in place. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec `docs/superpowers/specs/2026-06-11-array-literal-arg-panic-design.md`. Key locations: `expand()` per-part loop with the `AssignPrefix`/`ArrayLiteral` `unreachable!` arms (expand.rs:~977-988); `expand_assignment()` (expand.rs:1005, `result.push_str(...)`, with the twin `unreachable!` arms ~:1092-1102); `Field::push_str(&mut self, s, quoted: bool)` (expand.rs:83); helpers `expand::word_literal_text(&Word)->Option<&str>` (command.rs:1982 — confirm path), `expand_assignment(&Word,&mut Shell)->String` (expand.rs:1005), `param_expansion::xtrace_quote(&str)->String` (param_expansion.rs:281); types `WordPart::AssignPrefix { target: AssignTarget, append: bool }` + `WordPart::ArrayLiteral(Vec<ArrayLiteralElement>)` (lexer.rs), `AssignTarget::{Bare(String), Indexed{name, subscript: Word}}` (command.rs:277), `ArrayLiteralElement { subscript: Option<Word>, value: Word }` (lexer.rs:192); v130 xtrace array render in `run_exec_single` + `array_literal_elements` (executor.rs:2920+).

---

### Task 1: Reconstruct instead of panic (the fix)

**Files:**
- Create: `tests/array_literal_arg_integration.rs`
- Modify: `src/expand.rs` (helpers + the 4 arms), `src/executor.rs` (v130 xtrace dedup)

- [ ] **Step 1: Write the failing integration tests** — create `tests/array_literal_arg_integration.rs`:
```rust
//! v136: an array-literal / assign-prefix word as a command ARGUMENT (e.g.
//! `eval x=(a b)`) reconstructs to text instead of panicking (M-114).
use std::process::{Command, Stdio};
use std::io::Write;
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut c = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn");
    c.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into_owned(),
     String::from_utf8_lossy(&o.stderr).into_owned(),
     o.status.code().unwrap_or(-1))
}

#[test]
fn eval_array_literal_assignment() {
    let (o, _e, _c) = run("eval x=(a b); declare -p x\n");
    assert_eq!(o, "declare -a x=([0]=\"a\" [1]=\"b\")\n", "o: {o:?}");
}
#[test]
fn eval_array_append() {
    let (o, _e, _c) = run("arr=(p); eval arr+=(x y); echo \"${arr[@]}\"\n");
    assert_eq!(o, "p x y\n", "o: {o:?}");
}
#[test]
fn eval_indexed_element() {
    let (o, _e, _c) = run("eval a[1]=v; echo \"${a[1]}\"\n");
    assert_eq!(o, "v\n", "o: {o:?}");
}
#[test]
fn eval_indexed_append() {
    let (o, _e, _c) = run("a[2]=Q; eval a[2]+=z; echo \"${a[2]}\"\n");
    assert_eq!(o, "Qz\n", "o: {o:?}");
}
#[test]
fn eval_subscript_and_quoted_value() {
    // a quoted element with a space must stay ONE element after re-parse.
    let (o, _e, _c) = run("eval x=([3]=\"a b\" c); echo \"${x[3]}|${x[4]}\"\n");
    assert_eq!(o, "a b|c\n", "o: {o:?}");
}
#[test]
fn eval_empty_array() {
    let (o, _e, _c) = run("eval x=(); echo \"len=${#x[@]}\"\n");
    assert_eq!(o, "len=0\n", "o: {o:?}");
}
#[test]
fn escaped_form_still_works() {
    let (o, _e, _c) = run("f(){ eval $1=\\(p q\\); }; f arr; echo \"${arr[@]}\"\n");
    assert_eq!(o, "p q\n", "o: {o:?}");
}
#[test]
fn quoted_form_still_works() {
    let (o, _e, _c) = run("eval \"x=(a b)\"; echo \"${x[@]}\"\n");
    assert_eq!(o, "a b\n", "o: {o:?}");
}
#[test]
fn declaration_array_unchanged() {
    let (o, _e, _c) = run("declare d=(a b); echo \"${d[@]}\"\n");
    assert_eq!(o, "a b\n", "o: {o:?}");
}
#[test]
fn non_eval_arg_does_not_panic() {
    // bash syntax-errors here; huck must NOT panic (documented divergence:
    // huck reconstructs the arg string).
    let (o, _e, c) = run("echo x=(a b)\n");
    assert_ne!(c, 101, "must not panic (rc 101)");
    assert_eq!(o, "x=(a b)\n", "o: {o:?}");
}
```
IMPORTANT: before relying on the exact `declare -p x` / `${arr[@]}` strings, run each fragment through bash and set the assertion to bash's EXACT output (e.g. `bash -c 'eval x=(a b); declare -p x'`). The `eval_array_append`/`eval_subscript_and_quoted_value` expected values especially — confirm against bash and adjust if huck's `declare -p`/echo format differs in a pre-existing way (use `echo "${x[@]}"` forms which are format-stable rather than `declare -p` where possible).

- [ ] **Step 2: Run to verify the panic** — `cargo test --test array_literal_arg_integration 2>&1 | tail -25`. Expected: the `eval_*` and `non_eval_arg_does_not_panic` tests FAIL (panic → the child aborts, rc≠expected); `escaped_form_still_works`, `quoted_form_still_works`, `declaration_array_unchanged` PASS (those paths don't reach the panic).

- [ ] **Step 3: Add the reconstruction helpers in `src/expand.rs`.**
```rust
/// Render an `AssignTarget` LHS back to text: `name` or `name[<subscript>]`.
fn render_assign_target(target: &crate::command::AssignTarget, shell: &mut Shell) -> String {
    use crate::command::AssignTarget;
    match target {
        AssignTarget::Bare(name) => name.clone(),
        AssignTarget::Indexed { name, subscript } => {
            format!("{name}[{}]", expand_assignment(subscript, shell))
        }
    }
}

/// Render ONE array-literal element value re-parse-safely: a purely-literal
/// value verbatim (the common `a`/`b` case), else expanded + quote-when-meta.
fn render_elem_value(v: &crate::lexer::Word, shell: &mut Shell) -> String {
    match crate::command::word_literal_text(v) {   // confirm the actual path of word_literal_text
        Some(t) => t.to_string(),
        None => crate::param_expansion::xtrace_quote(&expand_assignment(v, shell)),
    }
}

/// Reconstruct an array literal to re-parseable `(e1 e2 [k]=v …)` text.
pub(crate) fn reconstruct_array_literal(
    elems: &[crate::lexer::ArrayLiteralElement],
    shell: &mut Shell,
) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(elems.len());
    for e in elems {
        match &e.subscript {
            Some(sub) => parts.push(format!("[{}]={}", expand_assignment(sub, shell), render_elem_value(&e.value, shell))),
            None => parts.push(render_elem_value(&e.value, shell)),
        }
    }
    format!("({})", parts.join(" "))
}
```
(Confirm the real module path of `word_literal_text` — it was reported at command.rs:1982; call it accordingly, e.g. `crate::command::word_literal_text`.)

- [ ] **Step 4: Replace the `unreachable!` arms in `expand()` (expand.rs:~983).**
```rust
            WordPart::AssignPrefix { target, append } => {
                let mut lhs = render_assign_target(target, shell);
                lhs.push_str(if *append { "+=" } else { "=" });
                current.push_str(&lhs, true);  // one field; not word-split
            }
            WordPart::ArrayLiteral(elems) => {
                let rendered = reconstruct_array_literal(elems, shell);
                current.push_str(&rendered, true);
            }
```

- [ ] **Step 5: Replace the twin arms in `expand_assignment()` (expand.rs:~1098).**
```rust
            WordPart::AssignPrefix { target, append } => {
                result.push_str(&render_assign_target(target, shell));
                result.push_str(if *append { "+=" } else { "=" });
            }
            WordPart::ArrayLiteral(elems) => {
                result.push_str(&reconstruct_array_literal(elems, shell));
            }
```

- [ ] **Step 6: De-dup the v130 xtrace render (`src/executor.rs`).** The xtrace block renders an array-literal decl arg inline (around `array_literal_elements`, ~executor.rs:2920+). Replace that inline element loop with a call to `crate::expand::reconstruct_array_literal(elems, shell)` so there is ONE renderer. The xtrace output must be UNCHANGED — verify the v130 `set_x`/`setx_trace_fidelity` tests still pass byte-for-byte. (If the xtrace render produced a subtly different format than `reconstruct_array_literal`, prefer the shared helper's output and update any v130 test that asserted the old inline format ONLY if bash agrees with the new output; otherwise keep them aligned — do NOT regress xtrace.)

- [ ] **Step 7: Run the tests** — `cargo test --test array_literal_arg_integration 2>&1 | tail -20` → all pass.

- [ ] **Step 8: Build + FULL regression + clippy** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (none — array/`declare`/`local`/inline-assign/xtrace tests stay green); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 9: Sanity vs bash** (report):
```
for f in 'eval x=(a b); declare -p x' 'arr=(p); eval arr+=(x y); echo "${arr[@]}"' 'eval a[1]=v; echo "${a[1]}"' 'eval x=([3]="a b" c); echo "${x[3]}|${x[4]}"'; do
  b=$(bash -c "$f" 2>&1); h=$(./target/debug/huck -c "$f" 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=$b"; echo " h=$h"; }
done
echo "no-panic echo:"; ./target/debug/huck -c 'echo x=(a b)'; echo "rc=$?"
```

- [ ] **Step 10: Commit**
```bash
git add src/expand.rs src/executor.rs tests/array_literal_arg_integration.rs
git commit -m "$(cat <<'EOF'
fix(v136): reconstruct array-literal arg instead of panicking (M-114)

An array-literal / assign-prefix word reaching expand()/expand_assignment() as a
command argument (eval x=(a b), arr+=(...), a[i]=v, a[i]+=z) hit unreachable! and
panicked. Reconstruct it to `name...=(...)` text (positional/subscripted elements,
literal-fast-path else expanded+requoted) so eval/declare re-parse it — matching
bash. The v130 xtrace array render now routes through the shared
reconstruct_array_literal helper.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Bash-diff harness + docs (resolve M-114)

**Files:**
- Create: `tests/scripts/array_literal_arg_diff_check.sh`
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Bash-diff harness** — create `tests/scripts/array_literal_arg_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v136: array-literal / assign-prefix word
# as a command argument via eval (and declaration builtins) reconstructs + re-parses
# like bash. Does NOT test the non-eval `echo x=(a b)` case — bash syntax-errors
# there at parse time; huck reconstructs the arg (documented divergence).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "eval array"        'eval x=(a b); declare -p x'
check "eval append"       'arr=(p); eval arr+=(x y); echo "${arr[@]}"'
check "eval idx elem"     'eval a[1]=v; echo "${a[1]}"'
check "eval idx append"   'a[2]=Q; eval a[2]+=z; echo "${a[2]}"'
check "eval quoted elem"  'eval x=([3]="a b" c); echo "${x[3]}|${x[4]}"'
check "eval empty"        'eval x=(); echo "len=${#x[@]}"'
check "eval var elem"     'v=Z; eval x=($v b); echo "${x[@]}"'
check "escaped form"      'f(){ eval $1=\(p q\); }; f arr; echo "${arr[@]}"'
check "quoted form"       'eval "x=(a b)"; echo "${x[@]}"'
check "declare array"     'declare d=(a b); echo "${d[@]}"'
check "local array"       'f(){ local l=(a b); echo "${l[@]}"; }; f'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/array_literal_arg_diff_check.sh`. Run: `cargo build 2>&1 | tail -2; bash tests/scripts/array_literal_arg_diff_check.sh` → expect `Fail: 0`. If `eval var elem` (the `$v` splitting case) FAILS, that's the M-102 overlap — report it; if it's a genuine splitting divergence, DROP that one row with a comment citing M-102 (do NOT mask a crash or a non-splitting failure).

- [ ] **Step 2: Delete M-114 + add divergences in docs/bash-divergences.md.**
  - DELETE the M-114 entry from Tier-1. Decrement the Tier-1 count: `| Bugs (Tier 1) | 1 | … (M-114). |` → `| Bugs (Tier 1) | 0 | None open. |` (🎉 no open bugs). If removing M-114 empties Tier-1's body, leave the section header with a "None currently." line (match how other empty sections read, if any) or a brief note.
  - ADD a Tier-3 (Intentional) or Tier-4 (low) entry for the non-eval syntax-error divergence, e.g.: **`name=(…)` array-literal as an argument to a NON-declaration/non-eval command**: bash is a parse-time syntax error; huck reconstructs the arg to its `name=(…)` text (so `echo x=(a b)` prints `x=(a b)`). Replicating bash's parse-time gating needs command-context-aware lexing; the reconstruction is harmless and makes `eval x=(a b)` work. `[intentional]`/low. Bump that tier's count.
  - On the existing **M-102** entry, add a one-line note that the `eval x=($v …)` reconstruction path shares M-102's element-word-splitting best-effort behavior.

- [ ] **Step 3: Verify docs** — `grep -n "M-114" docs/bash-divergences.md` → none; `grep -n "Bugs (Tier 1) | 0" docs/bash-divergences.md` → present; the new divergence entry present.

- [ ] **Step 4: Full regression + clippy** — `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean); smoke the v135 harness: `bash tests/scripts/test_operators_diff_check.sh | tail -1`.

- [ ] **Step 5: Commit**
```bash
git add tests/scripts/array_literal_arg_diff_check.sh docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
test+docs(v136): array-literal-arg harness; resolve M-114 (no open bugs)

Add the bash-diff harness (eval/declare array-literal arg forms, byte-identical).
Delete M-114 (Tier-1 1->0 — no open bugs); document the non-eval syntax-error
reconstruction divergence + note the M-102 element-splitting overlap.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-review notes
- **Spec coverage:** Task 1 = the 4 arms + helpers + v130 xtrace dedup + integration tests; Task 2 = harness + delete M-114 + document divergences.
- **One renderer:** `reconstruct_array_literal` is shared by `expand`, `expand_assignment`, and the v130 xtrace path.
- **Re-parse safety:** reconstructed text pushed as ONE quoted field in `expand()`; element values literal-fast-path else expanded + `xtrace_quote`d so spaced elements survive.
- **No-regress:** declaration/leading-assignment paths (try_split_assignment) never reach the arms; element values have no nested ArrayLiteral (no recursion); xtrace output unchanged (verify v130 tests).
- **Bash-fidelity caveat (documented):** non-eval syntax-error case + `$v`-element splitting (M-102 overlap) are best-effort; the canonical literal `eval` forms are exact.
