# v122 — populate `BASH_REMATCH` after `[[ … =~ … ]]` (M-14 sub-feature) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `[[ STRING =~ REGEX ]]` populate the `BASH_REMATCH` array (`[0]`=whole match, `[1..]`=capture groups, non-participating→`""`, cleared on no-match), so `_longopt`-style completers (and the many scripts that use `BASH_REMATCH`) work — fixing `ls -<TAB>` showing no options.

**Architecture:** Change one arm. `eval_test_expr`'s `TestExpr::Regex` arm (`src/executor.rs:1227`, already `&mut Shell`) swaps `re.is_match(&l)` for `re.captures(&l)` and sets `BASH_REMATCH` via `shell.replace_array`. No signature changes.

**Tech Stack:** Rust (`regex` crate). `src/executor.rs`. Tests: `cargo test`, a new integration test, a new `tests/scripts/*_diff_check.sh` harness.

**Spec:** `docs/superpowers/specs/2026-06-09-bash-rematch-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `eval_test_expr` `TestExpr::Regex` arm (`src/executor.rs:1227-1233`):
  ```rust
          TestExpr::Regex { lhs, pattern } => {
              let l = expand_assignment(lhs, shell);
              let p = expand_assignment(pattern, shell);
              let p = if shell.nocasematch() { format!("(?i){p}") } else { p };
              let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
              Ok(re.is_match(&l))
          }
  ```
- `shell.replace_array(name: &str, map: BTreeMap<usize, String>) -> Result<(), AssignErr>` (used elsewhere in `executor.rs` / `shell_state.rs`).

**Verified bash contract (probed):** `[[ abcdef =~ b(c)(d) ]]` → `BASH_REMATCH=(bcd c d)` (n=3); `BASH_REMATCH=(stale x y); [[ xyz =~ nomatch ]]` → rc 1, n=0 (cleared); `[[ ab =~ (a)|(b) ]]` → `(a a "")` (non-participating group 2 = empty); `[[ foobar =~ o+ ]]` → `[0]=oo`; `[[ "a.b" =~ "a.b" ]]` → rc 0, `[0]=a.b`.

---

## Task 1: populate `BASH_REMATCH` in the Regex arm

**Files:**
- Modify: `src/executor.rs` (`TestExpr::Regex` arm)
- Create: `tests/bash_rematch_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/bash_rematch_integration.rs` (file-arg `run` helper, pid+atomic-counter temp path; verify each expected value against bash FIRST):
```rust
//! v122: BASH_REMATCH array population after [[ =~ ]] (M-14 sub-feature).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v122_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test]
fn whole_match_and_groups() {
    assert_eq!(
        run("[[ abcdef =~ b(c)(d) ]]\necho \"n=${#BASH_REMATCH[@]} 0=[${BASH_REMATCH[0]}] 1=[${BASH_REMATCH[1]}] 2=[${BASH_REMATCH[2]}]\"\n"),
        "n=3 0=[bcd] 1=[c] 2=[d]\n"
    );
}
#[test]
fn no_match_clears() {
    assert_eq!(
        run("BASH_REMATCH=(stale x y)\n[[ xyz =~ nomatch ]]\necho \"rc=$? n=${#BASH_REMATCH[@]} 0=[${BASH_REMATCH[0]}]\"\n"),
        "rc=1 n=0 0=[]\n"
    );
}
#[test]
fn nonparticipating_group_is_empty() {
    assert_eq!(
        run("[[ ab =~ (a)|(b) ]]\necho \"0=[${BASH_REMATCH[0]}] 1=[${BASH_REMATCH[1]}] 2=[${BASH_REMATCH[2]}]\"\n"),
        "0=[a] 1=[a] 2=[]\n"
    );
}
#[test]
fn matched_substring() {
    assert_eq!(run("[[ foobar =~ o+ ]]\necho \"[${BASH_REMATCH[0]}]\"\n"), "[oo]\n");
}
#[test]
fn quoted_regex_sets_rematch() {
    assert_eq!(run("[[ \"a.b\" =~ \"a.b\" ]]\necho \"rc=$? [${BASH_REMATCH[0]}]\"\n"), "rc=0 [a.b]\n");
}
#[test]
fn longopt_style_extraction() {
    // The bash-completion _longopt pattern: extract each option via BASH_REMATCH.
    let s = "for w in --all -x --almost-all; do [[ $w =~ (--[a-z-]+) ]] && printf '%s\\n' \"${BASH_REMATCH[1]}\"; done\n";
    assert_eq!(run(s), "--all\n--almost-all\n");
}
```

- [ ] **Step 2: Run — confirm fail**

Run: `cargo build --bin huck && cargo test --test bash_rematch_integration 2>&1 | tail -20`
Expected: the BASH_REMATCH tests FAIL (empty `BASH_REMATCH`); `no_match_clears` shows huck leaving the stale value (n=3) instead of clearing.

- [ ] **Step 3: Implement — `captures` + populate/clear `BASH_REMATCH`**

In `src/executor.rs`, replace the `TestExpr::Regex` arm's final line `Ok(re.is_match(&l))` so the arm reads:
```rust
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            let p = expand_assignment(pattern, shell);
            let p = if shell.nocasematch() { format!("(?i){p}") } else { p };
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            match re.captures(&l) {
                Some(caps) => {
                    // BASH_REMATCH[0] = whole matched substring; [1..] = capture
                    // groups (a non-participating group is "" but still indexed).
                    let map: std::collections::BTreeMap<usize, String> = (0..caps.len())
                        .map(|i| {
                            (
                                i,
                                caps.get(i)
                                    .map(|m| m.as_str().to_string())
                                    .unwrap_or_default(),
                            )
                        })
                        .collect();
                    let _ = shell.replace_array("BASH_REMATCH", map);
                    Ok(true)
                }
                None => {
                    // bash clears BASH_REMATCH to an empty array on no match.
                    let _ = shell.replace_array("BASH_REMATCH", std::collections::BTreeMap::new());
                    Ok(false)
                }
            }
        }
```
(If `BTreeMap` is already imported in `executor.rs`, use the bare name; otherwise the fully-qualified `std::collections::BTreeMap` as shown is fine. `regex::Captures::len()` = groups + 1, so `0..caps.len()` covers index 0 plus every declared group.)

- [ ] **Step 4: Run — confirm green**

Run: `cargo build --bin huck && cargo test --test bash_rematch_integration 2>&1 | tail -12`
Expected: all 6 tests PASS. If `no_match_clears` still shows a stale value, `replace_array` with an empty map may be a no-op — in that case `shell.unset("BASH_REMATCH")` before/instead in the `None` arm (verify the result is `${#BASH_REMATCH[@]}=0`); confirm against bash.

- [ ] **Step 5: Spot check vs bash + full regression + clippy**

```bash
cargo build --bin huck
for f in '[[ abcdef =~ b(c)(d) ]]; echo "${BASH_REMATCH[@]}"' \
         'BASH_REMATCH=(s x y); [[ z =~ q ]]; echo "n=${#BASH_REMATCH[@]}"' \
         '[[ ab =~ (a)|(b) ]]; echo "[${BASH_REMATCH[2]}]"' \
         '[[ foobar =~ o+ ]]; echo "${BASH_REMATCH[0]}"'; do
  printf '%s\n' "$f" > /tmp/t.sh
  b=$(bash --norc --noprofile /tmp/t.sh 2>&1); h=$(./target/debug/huck /tmp/t.sh 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b] h=[$h]"; }
done
cargo test 2>&1 | grep -E "test result: FAILED" || echo "no failures"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: all MATCH; no FAILED; clippy clean. Watch `dbracket`/regex suites (the `=~` truth value must be unchanged — `captures().is_some()` ⇔ `is_match()`).

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs tests/bash_rematch_integration.rs
git commit -m "$(cat <<'EOF'
feat: populate BASH_REMATCH after [[ =~ ]] (M-14)

The =~ arm now uses re.captures (not is_match) and sets BASH_REMATCH as an
indexed array: [0]=whole matched substring, [1..]=capture groups (non-
participating group => "" but still indexed); a failed match clears it to an
empty array, matching bash. eval_test_expr is already &mut Shell, so the change
is localized to the Regex arm. Unblocks _longopt-style completion option
extraction (${BASH_REMATCH[0]}).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the rewritten Regex arm, the integration pass line (6), the MATCH spot-check, the `=~`-truth-value non-regression (dbracket suite green), full-suite green, clippy status. Note if you used `unset` vs empty `replace_array` for the no-match clear, and any deviation.

---

## Task 2: 45th harness + payoff + docs

**Files:**
- Create: `tests/scripts/bash_rematch_diff_check.sh`
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/bash_rematch_diff_check.sh` (file-arg execution per L-27):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v122: BASH_REMATCH population after
# [[ =~ ]] (M-14 sub-feature). File-arg execution (L-27).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "whole+groups"   '[[ abcdef =~ b(c)(d) ]]; echo "n=${#BASH_REMATCH[@]} [${BASH_REMATCH[0]}][${BASH_REMATCH[1]}][${BASH_REMATCH[2]}]"'
check "no-match clears" 'BASH_REMATCH=(stale x y); [[ xyz =~ nomatch ]]; echo "rc=$? n=${#BASH_REMATCH[@]}"'
check "nonpart group"  '[[ ab =~ (a)|(b) ]]; echo "[${BASH_REMATCH[1]}][${BASH_REMATCH[2]}]"'
check "substring"      '[[ foobar =~ o+ ]]; echo "[${BASH_REMATCH[0]}]"'
check "quoted regex"   '[[ "a.b" =~ "a.b" ]]; echo "rc=$? [${BASH_REMATCH[0]}]"'
check "anchored"       '[[ hello =~ ^h.*o$ ]]; echo "[${BASH_REMATCH[0]}]"'
check "digits group"   '[[ "v1.2.3" =~ ([0-9]+)\.([0-9]+) ]]; echo "[${BASH_REMATCH[1]}][${BASH_REMATCH[2]}]"'
check "longopt extract" 'for w in --all -x --almost-all; do [[ $w =~ (--[a-z-]+) ]] && printf "%s\n" "${BASH_REMATCH[1]}"; done'
check "rematch indices" '[[ abcdef =~ b(c)(d) ]]; echo "${!BASH_REMATCH[@]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
(Verify each fragment byte-identical; adjust if a fragment's quoting is awkward, keeping coverage.)

- [ ] **Step 2: Make executable, run it, run ALL harnesses**

```bash
chmod +x tests/scripts/bash_rematch_diff_check.sh && cargo build --bin huck
bash tests/scripts/bash_rematch_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo all-harnesses-done
```
Expected: `Total: 9, Pass: 9, Fail: 0`; `count: 45`; no `FAIL` lines.

- [ ] **Step 3: Payoff (pty) — `ls -<TAB>` now shows options**

```bash
cargo build --release 2>&1 | tail -1
python3 - <<'PY'
import os, pty, select, time
BIN=os.path.abspath("target/release/huck")
pid,fd=pty.fork()
if pid==0:
    os.environ["PS1"]="HK> "; os.execv(BIN,[BIN]); os._exit(127)
def drain(t):
    b=b""; e=time.time()+t
    while time.time()<e:
        r,_,_=select.select([fd],[],[],0.3)
        if r:
            try: d=os.read(fd,8192)
            except OSError: break
            if not d: break
            b+=d
    return b
def send(s): os.write(fd,s.encode())
time.sleep(0.5); drain(1.0)
send("source /usr/share/bash-completion/bash_completion\n"); drain(3.0)
send("ls --\t\t"); out=drain(6.0)   # double-tab to list candidates
print("offered --all:", b"--all" in out, "| --almost-all:", b"almost" in out)
send("\x15"); send("echo PAYOFF_OK\n"); resp=drain(4.0)
print("responsive:", b"PAYOFF_OK" in resp)
os.write(fd,b"\nexit\n")
try: os.close(fd)
except OSError: pass
PY
```
Expected: `offered --all: True` (or at least some `--`options listed), `responsive: True`. Report the result. (If no PTY in the sandbox, note it; the integration test + harness still gate the BASH_REMATCH behavior.)

- [ ] **Step 4: Commit the harness**

```bash
git add tests/scripts/bash_rematch_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 45th bash-diff harness for BASH_REMATCH (M-14)

9 byte-identical fragments (whole+groups, no-match clears, non-participating
group, substring, quoted, anchored, version-digits, _longopt extraction,
indices). Payoff: ls -<TAB> now offers options via _longopt's BASH_REMATCH.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Docs**

Get the test count: `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.
In `docs/bash-divergences.md`:
- **M-14 entry**: append a clause noting `BASH_REMATCH` array population shipped v122 — `[[ =~ ]]` now sets `BASH_REMATCH[0]`=whole match, `[1..]`=capture groups (non-participating→`""`), cleared on no-match; via `re.captures` in the Regex arm.
- **M-82 and M-83 deferred lists**: change `BASH_REMATCH array population` (still pending) → note it as `fixed v122`.
- **"Last updated"** line → v122 (BASH_REMATCH population — unblocks `_longopt`-style completion option extraction; `ls -<TAB>` now offers options).
- **Change log**: append a `2026-06-09` v122 entry (the `is_match`→`captures` switch, the array semantics, the `_longopt`/`ls -<TAB>` payoff, the 45th harness + `<N>` tests, the honest note that mise candidates still need the 2.12 bash-completion env).
- **README**: add a v122 row after v121.

- [ ] **Step 6: Verify + commit**

```bash
grep -n 'BASH_REMATCH.*v122\|fixed v122\|v122' docs/bash-divergences.md README.md | head
grep -n '<N>' docs/bash-divergences.md README.md && echo "PLACEHOLDER LEFT" || echo "no placeholders"
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v122 — BASH_REMATCH array population (M-14 sub-feature)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA(s), the `Total: 9, Pass: 9` + `count: 45` lines, the payoff result (options offered + responsive), the docs greps, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 45 files).
- [ ] **Payoff**: `ls -<TAB>` offers `--`options (Task 2 Step 3).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`; MEMORY.md is near its cap — compress older entries while updating). **Tell the user to re-test `ls -<TAB>` live: options should now appear. mise candidates still need the 2.12 bash-completion API (env).**
