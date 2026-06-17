# v178: arithmetic-expansion errors are fatal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an arithmetic-expansion error (`$((1;2))`, `x=$((1+))`, `${v:1+:2}`) abort the command with a nonzero status like bash, instead of huck's current "print + expand to empty + continue, exit 0".

**Architecture:** Three expansion sites swallow arith eval errors (print, `set_last_status(1)`, continue). Route each through huck's existing fatal-expansion mechanism — `shell.pending_fatal_pe_error = Some(1)` / `ExpansionResult::Fatal { status: 1 }` — which the executor already turns into a command abort (the same path `${x:?}` / `set -u` use). No new types, no executor change.

**Tech Stack:** Rust (`src/expand.rs`, `src/param_expansion.rs`); bash-diff harness.

**Spec:** `docs/superpowers/specs/2026-06-17-arith-expansion-fatal-design.md`

**Branch:** `v178-arith-expansion-fatal`

**Background facts (verified):**
- huck today: `echo $((1;2)); echo SECOND` → prints `huck: arithmetic: …`, runs both echos, exit 0. bash → error, exit 1, SECOND does NOT run.
- huck ALREADY aborts on other expansion errors via `pending_fatal_pe_error`: `echo ${x:?bad}; echo SECOND` → rc 1, SECOND absent (matches bash's abort). The arith arms just don't set the flag.
- Sites: `expand.rs:901` (`$((…))` in `expand` → `Vec<Field>`), `expand.rs:1116` (`$((…))` in `expand_assignment` → `String`), `param_expansion.rs:207` & `:212` (substring offset/length, `ExpansionResult` return).
- The sibling pattern to mirror: `expand.rs:994-997` — `ExpansionResult::Fatal { status } => { shell.pending_fatal_pe_error = Some(status); return result; }`. `expand_assignment` has the same arm. `param_expansion.rs:222` already returns `ExpansionResult::Fatal { status: 1 }` for the substring-range error.
- bash's status for an arith-EXPANSION error is **1** (not the 127 it uses for `${x:?}`), so huck's `Some(1)` matches bash here.
- Standalone `(( ))` / `let` already return rc 1 (non-fatal) — DO NOT touch. Array *read* `${a[1+]}` already fatal — DO NOT touch. Array *assignment* `a[1+]=5` is out of scope (separate follow-on).

---

### Task 1: Make `$((…))` expansion errors fatal (both expand sites)

**Files:** Modify `src/expand.rs` (the `WordPart::Arith` `Err` arms at ~line 901 in `expand` and ~line 1116 in `expand_assignment`).

- [ ] **Step 1: Fix the `expand` arith arm (~line 901)**

The current `Err` arm:
```rust
                    Err(e) => {
                        eprintln!("huck: arithmetic: {}", e);
                        shell.set_last_status(1);
                        has_emitted = true;
                        // Append nothing; the field stays empty if no other parts.
                    }
```
Replace it with (mirroring the `ExpansionResult::Fatal` arm in the same function):
```rust
                    Err(e) => {
                        eprintln!("huck: arithmetic: {}", e);
                        shell.pending_fatal_pe_error = Some(1);
                        return result;
                    }
```

- [ ] **Step 2: Fix the `expand_assignment` arith arm (~line 1116)**

The current `Err` arm:
```rust
                    Err(e) => {
                        eprintln!("huck: arithmetic: {}", e);
                        shell.set_last_status(1);
                        // Append nothing.
                    }
```
Replace it with:
```rust
                    Err(e) => {
                        eprintln!("huck: arithmetic: {}", e);
                        shell.pending_fatal_pe_error = Some(1);
                        return result;
                    }
```
(In `expand_assignment`, `result` is the `String` built so far — confirm the function returns `result` and that its own `ExpansionResult::Fatal` arm uses the same `shell.pending_fatal_pe_error = Some(status); return result;` shape.)

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`. (`pending_fatal_pe_error` is an existing pub field on `Shell`; `result` is the in-scope accumulator. `set_last_status` may now be unused in these arms — that's fine, it's used elsewhere.)

- [ ] **Step 4: Verify (vs bash) — abort + exit 1, next command doesn't run**

Run:
```bash
H="$(pwd)/target/debug/huck"
chk() { # label ; fragment   — compare STDOUT+EXIT (stderr wording differs by design)
  local b bo h ho
  bo=$(bash -c "$2" 2>/dev/null); b=$?
  ho=$("$H" -c "$2" 2>/dev/null); h=$?
  [ "$bo" = "$ho" ] && [ "$b" = "$h" ] && echo "  OK  $1  (rc=$h out=[$ho])" || echo "  DIFF $1  bash(rc=$b [$bo]) huck(rc=$h [$ho])"
}
echo "--- must now abort (empty stdout, exit 1, no SECOND) ---"
chk 'expansion 1;2'   'echo $((1;2)); echo SECOND'
chk 'expansion 1+'    'echo $((1+)); echo SECOND'
chk 'expansion 1 2'   'echo $((1 2)); echo SECOND'
chk 'assign x=1+'     'x=$((1+)); echo SECOND'
chk 'arith arr idx'   'a=(x y); echo $((a[1+])); echo SECOND'
echo "--- controls: must NOT abort ---"
chk 'valid 1+2'       'echo $((1+2)); echo SECOND'
chk 'standalone ((1+))' '(( 1+ )); echo SECOND'
```
Expected: every line prints `OK` (the 5 bad cases: empty stdout, rc 1, no SECOND in both shells; the controls: valid prints `3` then `SECOND`, standalone prints `SECOND` — standalone stays non-fatal).

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs
git commit -m "v178: arithmetic expansion \$((…)) errors are fatal (abort, exit 1)

The two \$((…)) expansion arms (expand / expand_assignment) printed the error,
set \$?=1, and continued with an empty expansion (exit 0). Route them through the
existing fatal-PE mechanism (pending_fatal_pe_error = Some(1); return) like
\${x:?}/set -u, so an arith expansion error aborts the command with exit 1 and the
rest of the list does not run — matching bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Make substring-index (`${v:off:len}`) arith errors fatal

**Files:** Modify `src/param_expansion.rs` (the `Substring` arm, ~lines 207 & 212).

- [ ] **Step 1: Fix both `eval_substring_index` error returns**

In the `ParamModifier::Substring { offset, length }` arm, the offset and length errors currently swallow:
```rust
            let off_n = match eval_substring_index(offset, shell) {
                Ok(n) => n,
                Err(()) => return ExpansionResult::Empty,
            };
            let len_n = match length {
                Some(w) => match eval_substring_index(w, shell) {
                    Ok(n) => Some(n),
                    Err(()) => return ExpansionResult::Empty,
                },
                None => None,
            };
```
Change BOTH `Err(()) => return ExpansionResult::Empty,` to:
```rust
                Err(()) => return ExpansionResult::Fatal { status: 1 },
```
(`eval_substring_index` already printed `huck: arithmetic: …`; `Fatal` is the variant the caller turns into `pending_fatal_pe_error` — same as the substring-RANGE error a few lines below, `ExpansionResult::Fatal { status: 1 }`.)

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | tail -2` → `Finished`.

- [ ] **Step 3: Verify (vs bash)**

Run:
```bash
H="$(pwd)/target/debug/huck"
chk() { local b bo h ho; bo=$(bash -c "$2" 2>/dev/null); b=$?; ho=$("$H" -c "$2" 2>/dev/null); h=$?
  [ "$bo" = "$ho" ] && [ "$b" = "$h" ] && echo "  OK  $1 (rc=$h out=[$ho])" || echo "  DIFF $1 bash(rc=$b [$bo]) huck(rc=$h [$ho])"; }
echo "--- substring index errors must abort ---"
chk 'offset 1+'   'v=hello; echo ${v:1+:2}; echo SECOND'
chk 'offset bare' 'v=hello; echo ${v:1+}; echo SECOND'
echo "--- control: valid substring must NOT abort ---"
chk 'valid :1:2'  'v=hello; echo ${v:1:2}; echo SECOND'
```
Expected: the two bad cases print `OK` (empty stdout, rc 1, no SECOND in both); the control prints `OK` (`el` then `SECOND`, rc 0).

- [ ] **Step 4: Commit**

```bash
git add src/param_expansion.rs
git commit -m "v178: substring-index \${v:off:len} arith errors are fatal

The substring offset/length arith-error arms returned ExpansionResult::Empty
(swallow); return ExpansionResult::Fatal { status: 1 } instead, like the
substring-range error, so a bad arithmetic index aborts the command (exit 1)
matching bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Bash-diff harness + full regression

**Files:** Create `tests/scripts/arith_error_status_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/arith_error_status_diff_check.sh` with exactly:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v178: an arithmetic-EXPANSION error
# ($((…)), substring index ${v:off:len}) is fatal — the command aborts with a
# nonzero status and the rest of the list does not run, matching bash. Compares
# STDOUT + EXIT CODE only (the error WORDING legitimately differs: `huck:` vs
# bash's text). Each "bad" case is `<bad-arith>; echo SECOND`: if the arith error
# aborts, SECOND never prints.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b bo h ho
    bo=$(bash --norc --noprofile -c "$frag" 2>/dev/null); b=$?
    ho=$("$HUCK_BIN" -c "$frag" 2>/dev/null); h=$?
    if [[ "$bo" == "$ho" && "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash(rc=%s)=[%s]  huck(rc=%s)=[%s]\n' "$label" "$b" "$bo" "$h" "$ho"; FAIL=$((FAIL+1)); fi
}

# --- arithmetic expansion errors must abort (empty stdout, rc 1, no SECOND) ---
check "expansion semicolon"  'echo $((1;2)); echo SECOND'
check "expansion trailing +" 'echo $((1+)); echo SECOND'
check "expansion two terms"  'echo $((1 2)); echo SECOND'
check "assignment bad arith" 'x=$((1+)); echo SECOND'
check "arith with arr index" 'a=(x y); echo $((a[1+])); echo SECOND'
check "embedded in word"     'echo pre$((1 2))post; echo SECOND'
# --- substring index errors must abort ---
check "substring offset+len" 'v=hello; echo ${v:1+:2}; echo SECOND'
check "substring offset only" 'v=hello; echo ${v:1+}; echo SECOND'
# --- controls: must NOT abort ---
check "valid arith"          'echo $((1+2)); echo SECOND'
check "valid substring"      'v=hello; echo ${v:1:2}; echo SECOND'
check "standalone (( )) nonfatal" '(( 1+ )); echo SECOND'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run it**

Run: `chmod +x tests/scripts/arith_error_status_diff_check.sh && cargo build --quiet && bash tests/scripts/arith_error_status_diff_check.sh`
Expected: `Total: 11, Pass: 11, Fail: 0`.
If a "must abort" case FAILs, a site was missed; if a control FAILs, the fatal path wrongly fired on valid input or on the standalone `(( ))` — investigate, do NOT weaken the assertion.

- [ ] **Step 3: Full regression (and check for old-swallow tests)**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN` → `CLEAN`.
Run: `cargo test >/tmp/v178.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v178.log` → `exit: 0`, `0`.
If a unit test FAILS, it likely encodes the OLD swallow behavior (e.g. asserts `expand`/`expand_assignment` of a bad `$((…))` yields an empty field / Ok). Inspect it: if it asserted the pre-fix tolerance, update it to expect the fatal behavior (the expansion now sets `pending_fatal_pe_error`); do not weaken a test that checks something unrelated. Search hint: `grep -nE 'arith|Arith' src/expand.rs | grep -i test` and look near any test feeding a malformed arith body.
Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"` → `0 failed` (count is now 100 with the new harness).

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/arith_error_status_diff_check.sh
# include src/expand.rs or src/param_expansion.rs ONLY if a unit test had to be updated in Step 3
git commit -m "test: v178 bash-diff harness for fatal arithmetic-expansion errors

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `src/expand.rs` (two arith `Err` arms), `src/param_expansion.rs` (two substring returns), the new `tests/scripts/arith_error_status_diff_check.sh` (+ any old-swallow unit-test update). Confirm standalone `(( ))`/`let`, the array *read* path, and the executor are untouched.
- Re-run `arith_error_status_diff_check.sh` (11/11) and the full harness suite (100/100); confirm via a manual `huck -c 'echo $((1;2)); echo SECOND'` that SECOND does not print and `$?`=1.
- Prove the harness is a genuine guard if any doubt arises: build the pre-fix binary in a throwaway `git worktree` and confirm it FAILS the new harness (as in v176).
- Merge `v178-arith-expansion-fatal` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the branch.
- Record in `project_huck_iterations.md` + `MEMORY.md`; note the array-subscript-ASSIGNMENT (`a[1+]=5`) fatal + "bad array subscript" follow-on. No `bash-divergences.md` change. The parse sweep is unaffected (runtime fix), so no HUCK_GAP movement.

---

## Self-review (plan vs spec)

- **Spec coverage:** `$((…))` expansion ×2 (Task 1) ✓; substring index ×2 (Task 2) ✓; both via `pending_fatal_pe_error`/`ExpansionResult::Fatal` (Tasks 1–2) ✓; standalone `(( ))`/`let` + array read untouched (Task 1 facts + final review) ✓; new stdout+exit harness with abort cases + controls (Task 3) ✓; full regression + old-swallow-test check (Task 3 Step 3) ✓; parse-sweep unaffected, array-assign follow-on noted (final review) ✓.
- **Placeholder scan:** none — exact before/after for each arm, full harness, exact commands + expected output. The Step-3 unit-test step gives concrete search hints + a rule (update only if it encodes pre-fix tolerance), not a vague "fix tests".
- **Type/name consistency:** `shell.pending_fatal_pe_error = Some(1); return result;` matches the existing `ExpansionResult::Fatal` arms in `expand`/`expand_assignment`; `ExpansionResult::Fatal { status: 1 }` matches the variant at `param_expansion.rs:222`; `eval_substring_index` returns `Result<i64, ()>` as used; harness filename and `target/debug/huck` consistent across tasks.
