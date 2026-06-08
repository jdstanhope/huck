# v113 — `printf -v` array-element target Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `printf -v NAME[SUBSCRIPT] FORMAT ARGS…` write the formatted result into an array element (indexed or associative), fixing the `mise ` + `<TAB>` `printf: 'words[0]': not a valid identifier` error and its downstream `_upvars` cascade.

**Architecture:** Reuse, don't reimplement. `builtin_printf` accepts a `name[sub]` `-v` target (via the existing `split_name_subscript`, promoted to `pub(crate)`), then routes the formatted result through `apply_one_assignment` with an `AssignTarget::Indexed` — so the subscript is arith-evaluated (indexed) / string-keyed (associative), the array is created/promoted, and readonly is enforced, all by reuse. Plain-name `-v` is unchanged.

**Tech Stack:** Rust. `src/expand.rs` (one visibility change), `src/builtins.rs`. Tests: `cargo test --bin huck`, `cargo test --test printf_v_array_integration`, `bash tests/scripts/printf_v_array_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-08-printf-v-array-element-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `fn split_name_subscript(n: &str) -> Option<(String, String)>` (`src/expand.rs:519`) — private; promote to `pub(crate)`. Returns `Some((name, sub))` only when `n` ends with `]`, contains `[`, and the name part is non-empty.
- `builtin_printf` `-v` validation (`src/builtins.rs:~2664`, the `if !is_valid_name(&args[i]) { … } v_var = Some(args[i].clone());`).
- `builtin_printf` write site (`src/builtins.rs:~2755`, `if let Some(var) = v_var { … shell.try_set(&var, s) … }`).
- `pub(crate) fn apply_one_assignment(a: &Assignment, shell) -> Result<(), ()>` (`src/executor.rs`); its `(AssignTarget::Indexed { name, subscript }, None)` arm `eval_subscript`s the subscript and `expand_assignment`s the value (`~:4090`).
- Types: `crate::lexer::Word(pub Vec<WordPart>)`; `crate::lexer::WordPart::Literal { text: String, quoted: bool }`; `crate::command::Assignment { target: AssignTarget, value: Word, append: bool }`; `crate::command::AssignTarget::Indexed { name: String, subscript: Word }`.

**Verified bash contract:** `printf -v "x[2]" %s hi` (x unset) → `declare -a x=([2]="hi")`; `words=(); printf -v "words[0]" %s a; printf -v "words[1]" %s b` → `words=(a b)`; `j=2; printf -v "x[j+1]" %s X` → element 3; `declare -A m; printf -v "m[key]" %s V` → `m[key]=V`; `printf -v plain %s hello` → `plain=hello`; `printf -v 'x[]' …` → bash error.

---

## Task 1: accept `name[sub]` `-v` target + route through `apply_one_assignment` + integration tests

**Files:**
- Modify: `src/expand.rs` (promote `split_name_subscript` to `pub(crate)`)
- Modify: `src/builtins.rs` (`-v` validation + write-site routing)
- Create: `tests/printf_v_array_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/printf_v_array_integration.rs` (the `run` helper returns `(stdout, stderr, exit_code)`):
```rust
//! v113: `printf -v` array-element target integration tests (M-109).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn printf_v_indexed_elements() {
    let (out, err, _c) = run(
        "words=()\nprintf -v \"words[0]\" %s a\nprintf -v \"words[1]\" %s b\n\
         echo \"${words[0]}/${words[1]}\"\n");
    assert_eq!(out, "a/b\n", "out: {out} err: {err}");
    assert!(!err.contains("not a valid identifier"), "rejected target: {err}");
}

#[test]
fn printf_v_arith_subscript() {
    let (out, _e, _c) = run("j=2\nprintf -v \"x[j+1]\" %s X\ndeclare -p x\n");
    assert!(out.contains("[3]=\"X\""), "out: {out}");
}

#[test]
fn printf_v_unset_var_promotes_to_indexed_array() {
    let (out, _e, _c) = run("printf -v \"y[2]\" %s hi\ndeclare -p y\n");
    assert_eq!(out, "declare -a y=([2]=\"hi\")\n", "out: {out}");
}

#[test]
fn printf_v_associative_element() {
    let (out, _e, _c) = run("declare -A m\nprintf -v \"m[key]\" %s V\necho \"${m[key]}\"\n");
    assert_eq!(out, "V\n", "out: {out}");
}

#[test]
fn printf_v_plain_name_unchanged() {
    let (out, _e, _c) = run("printf -v plain %s hello\necho \"$plain\"\n");
    assert_eq!(out, "hello\n", "out: {out}");
}

#[test]
fn printf_v_reassemble_loop_shape() {
    // The bash_completion __reassemble shape: build a `words` array element by
    // element with printf -v "words[i]". Must populate, no identifier error.
    let (out, err, _c) = run(
        "COMP_WORDS=(mise \"\")\nwords=()\n\
         for ((i=0; i<${#COMP_WORDS[@]}; i++)); do printf -v \"words[i]\" %s \"${COMP_WORDS[i]}\"; done\n\
         echo \"n=${#words[@]} w0=${words[0]}\"\n");
    assert_eq!(out, "n=2 w0=mise\n", "out: {out} err: {err}");
    assert!(!err.contains("not a valid identifier"), "leak: {err}");
}
```
Verify each `assert_eq!`/`contains` against the system bash first (run the same script through `bash --norc --noprofile`).

- [ ] **Step 2: Run the integration tests — confirm they fail**

Run: `cargo build --bin huck && cargo test --test printf_v_array_integration 2>&1 | tail -20`
Expected: the element-target tests FAIL — huck prints `printf: 'words[0]': not a valid identifier`; `printf_v_plain_name_unchanged` PASSES.

- [ ] **Step 3: Promote `split_name_subscript` to `pub(crate)`**

In `src/expand.rs` (`~:519`), change:
```rust
fn split_name_subscript(n: &str) -> Option<(String, String)> {
```
to:
```rust
pub(crate) fn split_name_subscript(n: &str) -> Option<(String, String)> {
```
(Body unchanged.)

- [ ] **Step 4: Accept a `name[sub]` `-v` target**

In `builtin_printf` (`src/builtins.rs:~2664`), replace:
```rust
                if !is_valid_name(&args[i]) {
                    eprintln!("huck: printf: `{}': not a valid identifier", args[i]);
                    return ExecOutcome::Continue(1);
                }
                v_var = Some(args[i].clone());
```
with:
```rust
                let target = &args[i];
                let valid = is_valid_name(target)
                    || crate::expand::split_name_subscript(target)
                        .map(|(name, sub)| is_valid_name(&name) && !sub.is_empty())
                        .unwrap_or(false);
                if !valid {
                    eprintln!("huck: printf: `{target}': not a valid identifier");
                    return ExecOutcome::Continue(1);
                }
                v_var = Some(target.clone());
```

- [ ] **Step 5: Route an element target through `apply_one_assignment`**

In `builtin_printf`'s write site (`src/builtins.rs:~2755`), replace:
```rust
    if let Some(var) = v_var {
        let s = String::from_utf8_lossy(&buf).into_owned();
        if shell.try_set(&var, s).is_err() {
            eprintln!("huck: printf: {var}: readonly variable");
            return ExecOutcome::Continue(1);
        }
    } else if let Err(e) = out.write_all(&buf) {
```
with:
```rust
    if let Some(var) = v_var {
        let s = String::from_utf8_lossy(&buf).into_owned();
        if let Some((name, sub)) = crate::expand::split_name_subscript(&var) {
            // Array-element target: write via the same path as `name[sub]=value`,
            // so the subscript is arith-evaluated (indexed) / string-keyed
            // (associative), the array is created/promoted, and readonly is
            // enforced — all by reuse. (M-109)
            let assignment = crate::command::Assignment {
                target: crate::command::AssignTarget::Indexed {
                    name,
                    subscript: crate::lexer::Word(vec![
                        crate::lexer::WordPart::Literal { text: sub, quoted: false },
                    ]),
                },
                value: crate::lexer::Word(vec![
                    crate::lexer::WordPart::Literal { text: s, quoted: true },
                ]),
                append: false,
            };
            if crate::executor::apply_one_assignment(&assignment, shell).is_err() {
                // apply_one_assignment already printed the specific diagnostic
                // (readonly / type mismatch / bad subscript).
                return ExecOutcome::Continue(1);
            }
        } else if shell.try_set(&var, s).is_err() {
            eprintln!("huck: printf: {var}: readonly variable");
            return ExecOutcome::Continue(1);
        }
    } else if let Err(e) = out.write_all(&buf) {
```
(Leave the trailing `out.write_all` error arm and the final `ExecOutcome::Continue(exit)` exactly as they are — you are only splitting the `Some(var)` branch.)

- [ ] **Step 6: Run the integration tests — confirm green**

Run: `cargo build --bin huck && cargo test --test printf_v_array_integration 2>&1 | tail -10`
Expected: all 6 tests PASS.

- [ ] **Step 7: Verify byte-identical to bash**

```bash
cargo build --bin huck
for f in 'words=(); printf -v "words[0]" %s a; printf -v "words[1]" %s b; declare -p words' \
         'j=2; printf -v "x[j+1]" %s X; declare -p x' \
         'printf -v "y[2]" %s hi; declare -p y' \
         'declare -A m; printf -v "m[key]" %s V; echo "${m[key]}"' \
         'printf -v plain %s hello; echo "$plain"'; do
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | ./target/debug/huck 2>&1; echo "EXIT:$?")
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b]"; echo " h=[$h]"; }
done
```
Expected: five `MATCH` lines.

- [ ] **Step 8: Regression + clippy**

Run: `cargo test --bin huck 2>&1 | tail -3 && cargo clippy --bin huck 2>&1 | tail -3`
Expected: unit suite green; clippy clean (no new warnings — `split_name_subscript` now has an external caller, so its prior `#[allow(dead_code)]` if any is no longer needed; remove it only if clippy flags it as unnecessary, else leave).

- [ ] **Step 9: Commit**

```bash
git add src/expand.rs src/builtins.rs tests/printf_v_array_integration.rs
git commit -m "$(cat <<'EOF'
feat: printf -v writes array elements (M-109)

`printf -v NAME[SUBSCRIPT] …` now writes the formatted result into an array
element instead of erroring `not a valid identifier`. The -v target accepts a
`name[sub]` form (via split_name_subscript, now pub(crate)); the write routes
through apply_one_assignment with an AssignTarget::Indexed, so the subscript is
arith-evaluated (indexed) / string-keyed (associative), the array is
created/promoted, and readonly is enforced — all by reuse. Plain-name -v
unchanged. Fixes bash_completion's __reassemble_comp_words_by_ref (mise<TAB>).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the visibility change + the two builtin_printf edits, the 6 integration-test pass line, the five bash MATCH lines, unit-suite + clippy status.

---

## Task 2: 37th bash-diff harness + payoff smoke

**Files:**
- Create: `tests/scripts/printf_v_array_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/printf_v_array_diff_check.sh`, modeled on `tests/scripts/arith_comma_diff_check.sh` (same `set -u`, `HUCK_BIN`, `check()` combined-output pattern). Use `declare -p` / `echo "${arr[k]}"` readouts so output is deterministic:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v113: `printf -v NAME[SUBSCRIPT]` writes
# an array element (M-109). Indexed (arith subscript) + associative (string key);
# plain-name -v unchanged.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "two indexed elements"  'words=(); printf -v "words[0]" %s a; printf -v "words[1]" %s b; echo "${words[0]}/${words[1]}"'
check "arith subscript"       'j=2; printf -v "x[j+1]" %s X; declare -p x'
check "unset promotes"        'printf -v "y[2]" %s hi; declare -p y'
check "associative key"       'declare -A m; printf -v "m[key]" %s V; echo "${m[key]}"'
check "plain name"            'printf -v plain %s hello; echo "$plain"'
check "element overwrite"     'a=(p q r); printf -v "a[1]" %s Z; echo "${a[*]}"'
check "loop build"            'c=(one two three); w=(); for ((i=0;i<${#c[@]};i++)); do printf -v "w[i]" %s "${c[i]}"; done; echo "${w[*]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable and run it + all harnesses**

Run:
```bash
chmod +x tests/scripts/printf_v_array_diff_check.sh && cargo build --bin huck
bash tests/scripts/printf_v_array_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo done
```
Expected: `Total: 7, Pass: 7, Fail: 0`; `count: 37`; no `FAIL` lines. (If a fragment FAILs, fix Task 1's code — do NOT weaken the fragment to mask a divergence.)

- [ ] **Step 3: Payoff smoke**

Run:
```bash
cargo build --bin huck
echo "=== printf -v element works ==="
./target/debug/huck -c 'words=(); printf -v "words[0]" %s hi; echo "PRINTF_OK=${words[0]}"' 2>&1
echo "=== reassemble loop shape ==="
printf '%s\n' 'COMP_WORDS=(mise ""); words=(); for ((i=0;i<${#COMP_WORDS[@]};i++)); do printf -v "words[i]" %s "${COMP_WORDS[i]}"; done; echo "REASSEMBLE_OK=${words[0]}"' | ./target/debug/huck 2>&1
```
Expected: `PRINTF_OK=hi` and `REASSEMBLE_OK=mise`, with NO `not a valid identifier`.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/printf_v_array_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 37th bash-diff harness for printf -v array element (M-109)

7 byte-identical fragments: two indexed elements, arith subscript, unset
promotion, associative key, plain-name regression, element overwrite, loop
build. Payoff verified (the __reassemble_comp_words_by_ref shape populates).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the `Total: 7, Pass: 7` line, the `count: 37` + no-FAIL line, the payoff-smoke output (`PRINTF_OK` / `REASSEMBLE_OK`).

---

## Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|Missing features (Tier 2) |\|^## Change log\|2026-06-08.*v112' docs/bash-divergences.md | head
grep -n '| v112 ' README.md
```
Confirm next free Tier-2 number is **M-109**.

- [ ] **Step 2: Add the M-109 entry**

In `docs/bash-divergences.md` Tier-2 (Missing features) section (e.g. after the M-108 entry from v112), add an **M-109** entry `[fixed v113]`: `printf -v NAME[SUBSCRIPT] …` writes the formatted result into an array element — indexed (arith-evaluated subscript) or associative (string key), creating/promoting the array, with the same semantics as `NAME[SUBSCRIPT]=value` (readonly enforced). Mechanism: `builtin_printf` accepts a `name[sub]` `-v` target (via `split_name_subscript`, promoted to `pub(crate)`) and routes the write through `apply_one_assignment` with an `AssignTarget::Indexed`. Plain-name `-v` unchanged. Driver: bash_completion's `__reassemble_comp_words_by_ref` (`printf -v "$2[i]" …`, reached by `mise<TAB>`) — fixed `printf: 'words[0]': not a valid identifier` and the downstream `_upvars` `invalid option` cascade (the `words` array was never built). Notes: `printf -v 'arr[@]'`/`'x[]'` rejected/error like bash; element-case readonly/type-mismatch message comes from `apply_one_assignment` (no `printf:` prefix) — a trivial stderr divergence. 6 integration tests + the 37th harness.

- [ ] **Step 3: Bump the Tier-2 count + summary note**

In the Summary table **Missing features (Tier 2)** row: increment the count by 1 (M-109) and append to the note: `; M-109 printf -v array-element target fixed by v113`. Update the **Last updated** line to mention v113 (the `printf -v` array-element target).

- [ ] **Step 4: Change-log entry + README row**

`docs/bash-divergences.md` change log (after the v112 entry): a `2026-06-08` v113 entry — the `printf -v` array-element target (the reuse-`apply_one_assignment` mechanism), the `mise<TAB>` payoff (`printf: …not a valid identifier` + the `_upvars` cascade cleared), the verified semantics (indexed/arith, associative/string-key, unset-promotion, plain-name unchanged), the trivial readonly-message note, the 37th harness + the test count from Task 2's full-suite run. Add a v113 README iteration row after v112 in the same compact style. Use the REAL test count: `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.

- [ ] **Step 5: Verify (no placeholders) + commit**

```bash
grep -n 'M-109\|fixed v113\|v113' docs/bash-divergences.md README.md | head
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v113 — printf -v array-element target (M-109)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the grep output proving real M-number/version, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 37 files).
- [ ] **Payoff**: the `__reassemble_comp_words_by_ref` `printf -v "words[i]"` shape populates with no identifier error (Task 2 Step 3).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`).
