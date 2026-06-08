# v115 — bare `local NAME` declares an unset local Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a bare `local NAME` (no `=value`) declare a function-local variable that is **unset** (not set-to-empty), matching bash — fixing the final `mise<TAB>` `bash_completion: : : invalid option` (bash_completion's bare `local … vcword vwords` + `[[ -v ]]`).

**Architecture:** In both `local` code paths, a bare name already takes the local-scope snapshot (which records the outer value for restore-on-return); change the follow-up from `shell.set(name, "")` (set-empty) to `shell.unset(name)` (declared-local-but-unset). `local NAME=`/`NAME=val`/`-a`/`-A` are unchanged.

**Tech Stack:** Rust, `src/builtins.rs`. Tests: `cargo test --bin huck`, `cargo test --test bare_local_unset_integration`, `bash tests/scripts/bare_local_unset_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-08-bare-local-unset-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `builtin_local_decl` bare-scalar `else` arm (`src/builtins.rs:~1359-1363`): the `// Bare local NAME with no value: set empty scalar …` comment + `shell.set(name, String::new());`. `snapshot_for_local_scope(shell, name)` is called just above (`~:1332`); the `-a`/`-A` branches precede the `else`.
- `builtin_local` legacy path (`src/builtins.rs:~688`): `shell.set(name, value.unwrap_or_default());`, where `value: Option<String>` is `None` for a bare name (`arg.find('=')` → `None`). The snapshot block is just above (`~:680-687`).
- `shell.unset(name)` exists (`src/shell_state.rs:~571`).

**Verified bash contract** (in a function): `local x; [[ -v x ]]` → false; `local x; ${x-DEF}` → `DEF`; `local x; ${x:-d}` → `d` (already matched); `local x=` → set; `x=outer; f(){ local x; x=5; echo $x; }; f; echo $x` → `5` then `outer`.

---

## Task 1: bare `local NAME` → unset (both paths) + integration tests

**Files:**
- Modify: `src/builtins.rs` (`builtin_local_decl` bare-scalar arm + `builtin_local` bare-name case)
- Create: `tests/bare_local_unset_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/bare_local_unset_integration.rs` (the `run` helper returns `(stdout, stderr, exit_code)`):
```rust
//! v115: bare `local NAME` declares an unset local (M-111).
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
fn bare_local_is_unset_for_v_test() {
    let (out, _e, _c) = run("f(){ local x; [[ -v x ]] && echo SET || echo UNSET; }\nf\n");
    assert_eq!(out, "UNSET\n", "out: {out}");
}

#[test]
fn bare_local_uses_default_in_non_colon_modifier() {
    let (out, _e, _c) = run("f(){ local x; echo \"[${x-DEF}]\"; }\nf\n");
    assert_eq!(out, "[DEF]\n", "out: {out}");
}

#[test]
fn local_explicit_empty_is_set() {
    let (out, _e, _c) = run("f(){ local x=; [[ -v x ]] && echo SET || echo UNSET; }\nf\n");
    assert_eq!(out, "SET\n", "out: {out}");
}

#[test]
fn local_with_value_is_set() {
    let (out, _e, _c) = run("f(){ local x=v; echo \"$x\"; }\nf\n");
    assert_eq!(out, "v\n", "out: {out}");
}

#[test]
fn bare_local_then_assign_is_local_and_restores_outer() {
    let (out, _e, _c) = run("x=outer\nf(){ local x; x=5; echo \"in=$x\"; }\nf\necho \"out=$x\"\n");
    assert_eq!(out, "in=5\nout=outer\n", "out: {out}");
}

#[test]
fn bare_local_shadows_outer_as_unset() {
    let (out, _e, _c) = run("x=outer\nf(){ local x; echo \"[${x-DEF}]\"; }\nf\n");
    assert_eq!(out, "[DEF]\n", "out: {out}");
}

#[test]
fn get_comp_words_local_v_shape() {
    // The bash_completion shape: bare `local … vcword` then a `[[ -v vcword ]]`
    // gate must be FALSE so no empty arg is appended.
    let (out, _e, _c) = run(
        "f(){ local upvars=() vcur vcword\n\
           vcur=cur\n\
           [[ -v vcur ]] && upvars+=(\"$vcur\")\n\
           [[ -v vcword ]] && upvars+=(\"$vcword\")\n\
           echo \"n=${#upvars[@]} [${upvars[*]}]\"\n\
         }\nf\n");
    assert_eq!(out, "n=1 [cur]\n", "out: {out}");
}
```
Verify each expectation against the system bash first.

- [ ] **Step 2: Run the integration tests — confirm they fail**

Run: `cargo build --bin huck && cargo test --test bare_local_unset_integration 2>&1 | tail -20`
Expected: `bare_local_is_unset_for_v_test`, `bare_local_uses_default_in_non_colon_modifier`, `bare_local_shadows_outer_as_unset`, `get_comp_words_local_v_shape` FAIL (huck's bare local is set-empty); the `local x=`/`x=v` ones PASS.

- [ ] **Step 3: Fix `builtin_local_decl` bare-scalar arm**

In `src/builtins.rs` (`~:1359`), replace:
```rust
                } else {
                    // Bare `local NAME` with no value: set empty scalar,
                    // matching the legacy builtin_local behavior.
                    shell.set(name, String::new());
                }
```
with:
```rust
                } else {
                    // Bare `local NAME` with no value: declare it function-local
                    // but UNSET (matches bash + `declare NAME`). The snapshot
                    // above records the outer value so it is restored on return;
                    // unsetting here makes `[[ -v NAME ]]` / `${NAME-d}` see it
                    // as unset until assigned. (M-111)
                    shell.unset(name);
                }
```

- [ ] **Step 4: Fix `builtin_local` legacy path bare-name case**

In `src/builtins.rs` (`~:688`), replace:
```rust
        shell.set(name, value.unwrap_or_default());
```
with:
```rust
        match value {
            // `local NAME=` / `local NAME=val`: set (possibly empty).
            Some(v) => shell.set(name, v),
            // Bare `local NAME`: declare local but UNSET (M-111). The snapshot
            // above records the outer value for restore-on-return.
            None => shell.unset(name),
        }
```

- [ ] **Step 5: Run the integration tests — confirm green**

Run: `cargo build --bin huck && cargo test --test bare_local_unset_integration 2>&1 | tail -10`
Expected: all 7 tests PASS.

- [ ] **Step 6: Verify byte-identical to bash**

```bash
cargo build --bin huck
for f in 'f(){ local x; [[ -v x ]] && echo SET || echo UNSET; }; f' \
         'f(){ local x; echo "[${x-DEF}]"; }; f' \
         'f(){ local x=; [[ -v x ]] && echo SET || echo UNSET; }; f' \
         'x=outer; f(){ local x; x=5; echo "in=$x"; }; f; echo "out=$x"' \
         'f(){ local a b; [[ -v a ]] && echo aSET || echo aUNSET; }; f' \
         'f(){ local x; x=hi; echo "$x"; [[ -v x ]] && echo SET; }; f'; do
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | ./target/debug/huck 2>&1; echo "EXIT:$?")
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b]"; echo " h=[$h]"; }
done
```
Expected: six `MATCH` lines.

- [ ] **Step 7: Full regression + clippy**

Run: `cargo test 2>&1 | grep -E "test result: FAILED" ; cargo test 2>&1 | grep -cE "test result: ok"` (no FAILED). Then `cargo clippy --bin huck 2>&1 | tail -3` (clean). Watch the `functions`/`function_keyword`/`declare`/`local`/array suites — if any regresses (e.g. a test that relied on bare `local x` being set-empty), investigate it against bash before proceeding (a test asserting the OLD set-empty behavior is itself wrong and should be corrected to match bash).

- [ ] **Step 8: Commit**

```bash
git add src/builtins.rs tests/bare_local_unset_integration.rs
git commit -m "$(cat <<'EOF'
fix: bare `local NAME` declares an unset local, not set-empty (M-111)

`local NAME` (no value) set NAME to an empty string, so `[[ -v NAME ]]` was true
and `${NAME-default}` substituted nothing — bash leaves a bare local UNSET. Both
local paths (builtin_local_decl + the legacy builtin_local) now unset NAME after
taking the local-scope snapshot, so it is declared-local-but-unset (a later
NAME=val is still local; the outer value is restored on return). `local NAME=` /
`NAME=val` / `-a` / `-A` unchanged. Fixes bash_completion's bare `local … vcword
vwords` + `[[ -v ]]` (the mise<TAB> `_upvars: : invalid option` cascade).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the two edits, the 7 integration-test pass line, the six bash MATCH lines, the full-suite green count (no FAILED), clippy status, and any regressed test (with its bash comparison).

---

## Task 2: 39th bash-diff harness + payoff smoke

**Files:**
- Create: `tests/scripts/bare_local_unset_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/bare_local_unset_diff_check.sh`, modeled on `tests/scripts/alternate_word_quoting_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v115: a bare `local NAME` (no value)
# declares an UNSET local (M-111). `local NAME=`/`=val`/`-a`/`-A` unchanged.
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

check "bare local -v unset"   'f(){ local x; [[ -v x ]] && echo SET || echo UNSET; }; f'
check "bare local -default"   'f(){ local x; echo "[${x-DEF}]"; }; f'
check "bare local +alt"       'f(){ local x; echo "[${x+ALT}]"; }; f'
check "explicit empty is set" 'f(){ local x=; [[ -v x ]] && echo SET || echo UNSET; }; f'
check "local with value"      'f(){ local x=v; echo "$x"; }; f'
check "assign-then-local"     'x=outer; f(){ local x; x=5; echo "in=$x"; }; f; echo "out=$x"'
check "shadow outer as unset" 'x=outer; f(){ local x; echo "[${x-DEF}]"; }; f'
check "colon-default matches" 'f(){ local x; echo "[${x:-d}]"; }; f'
check "multiple bare locals"  'f(){ local a b; [[ -v a ]] && echo aS || echo aU; [[ -v b ]] && echo bS || echo bU; }; f'
check "upvars v-gate shape"   'f(){ local up=() vcur vcword; vcur=cur; [[ -v vcur ]] && up+=("$vcur"); [[ -v vcword ]] && up+=("$vcword"); echo "n=${#up[@]}"; }; f'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable + run it + all harnesses**

Run:
```bash
chmod +x tests/scripts/bare_local_unset_diff_check.sh && cargo build --bin huck
bash tests/scripts/bare_local_unset_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo done
```
Expected: `Total: 10, Pass: 10, Fail: 0`; `count: 39`; no `FAIL` lines.

- [ ] **Step 3: Payoff smoke (the bash_completion `_get_comp_words_by_ref` shape)**

Run:
```bash
cargo build --bin huck
BC=/usr/share/bash-completion/bash_completion
{
  sed -n '/^_upvars()/,/^}/p' "$BC"
  sed -n '/^__reassemble_comp_words_by_ref()/,/^}/p' "$BC"
  sed -n '/^__get_cword_at_cursor_by_ref()/,/^}/p' "$BC"
  sed -n '/^_get_comp_words_by_ref()/,/^}/p' "$BC"
  cat <<'DRIVE'
COMP_WORDBREAKS=$' \t\n"'"'"'><=;|&(:'
COMP_LINE='mise '
COMP_POINT=5
COMP_WORDS=(mise "")
COMP_CWORD=1
_get_comp_words_by_ref -n : cur prev
echo "SMOKE cur=[$cur] prev=[$prev]"
DRIVE
} > /tmp/v115_smoke.sh
echo "=== bash ==="; bash --norc --noprofile /tmp/v115_smoke.sh 2>&1
echo "=== huck (extglob on, as the real bashrc) ==="; sed '1i shopt -s extglob' /tmp/v115_smoke.sh | ./target/debug/huck 2>&1
```
Expected: huck prints `SMOKE cur=[] prev=[mise]` with NO `invalid option` / `not a valid identifier` lines (matching bash's `cur=[] prev=[mise]`).

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/bare_local_unset_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 39th bash-diff harness for bare-local-unset (M-111)

10 byte-identical fragments (bare local unset via -v/-default/+alt, explicit-
empty + valued still set, assign-then-local + restore, multiple bare, the
upvars v-gate shape, colon-default match). Payoff verified: the
_get_comp_words_by_ref -n : cur prev shape resolves with no invalid-option.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the `Total: 10, Pass: 10` line, the `count: 39` + no-FAIL line, and the payoff-smoke output (huck `SMOKE cur=[] prev=[mise]`, no errors).

---

## Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|Bugs (Tier 1) |\|^## Change log\|2026-06-08.*v114\|^### M-110:' docs/bash-divergences.md | head
grep -n '| v114 ' README.md
```
Confirm next free Tier-1 number is **M-111** (correctness bug → Tier 1).

- [ ] **Step 2: Add the M-111 entry (Tier 1)**

In `docs/bash-divergences.md` Tier-1 (Bugs) section (e.g. after the `### M-110:` block), add a `### M-111:` entry `[fixed v115]` (high): a bare `local NAME` (no value) set `NAME` to an empty string, so `[[ -v NAME ]]` was true and `${NAME-default}` / `${NAME+alt}` treated it as set; bash leaves a bare local UNSET. Fix: both `local` paths (`builtin_local_decl` + legacy `builtin_local`) take the local-scope snapshot then `shell.unset(name)` for a bare name (a later `NAME=val` is still local; the outer value is restored on return) — symmetric with the already-correct `declare NAME`. `local NAME=`/`NAME=val`/`-a`/`-A` unchanged. Driver: bash_completion's `_get_comp_words_by_ref` (`local … vcword vwords` + `[[ -v vcword ]]`, reached by `mise<TAB>`) — the spurious set-empty made the `-v` gates true → empty `upvars` elements → `local: '': not a valid identifier` + `_upvars: : invalid option`. Note the colon forms (`${x:-d}`) already matched (they test null-or-unset). Bump the Tier-1 count.

- [ ] **Step 3: Bump Tier-1 count + summary note**

In the Summary table **Bugs (Tier 1)** row: increment the count by 1 (M-111) and append `; M-111 bare local NAME declares an unset local fixed v115` to its note. Update the **Last updated** line to v115 (M-111).

- [ ] **Step 4: Change-log entry + README row**

`docs/bash-divergences.md` change log (after the v114 entry): a `2026-06-08` v115 entry — the bare-local set-empty→unset fix (both paths, snapshot-then-unset), the `mise<TAB>` payoff (the `_get_comp_words_by_ref` `local … vcword` `-v` gates now correct → cascade cleared), the contract (bare unset, `=`/`-a`/`-A` unchanged, colon forms already matched), the 39th harness + test count from Task 2's full-suite run. Add a v115 README iteration row after v114. Use the REAL test count: `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.

- [ ] **Step 5: Verify (no placeholders) + commit**

```bash
grep -n 'M-111\|fixed v115\|v115' docs/bash-divergences.md README.md | head
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v115 — bare local NAME declares an unset local (M-111)

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
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 39 files).
- [ ] **Payoff**: the `_get_comp_words_by_ref -n : cur prev` shape resolves with no `invalid option` (Task 2 Step 3).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`). **This is likely the iteration that makes `mise<TAB>` fully clean — confirm with the user after merge.**
