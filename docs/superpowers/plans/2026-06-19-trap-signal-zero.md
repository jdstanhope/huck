# `trap … 0` (signal 0 ≡ EXIT) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `trap '…' 0` register an EXIT trap (numeric `0` ≡ `EXIT`), like bash, instead of erroring.

**Architecture:** One guard in `parse_trap_signal`'s numeric branch maps `0` to `TrapSignal::Exit` — the same value the `EXIT` name produces, so every `trap` form works for free.

**Tech Stack:** Rust. File: `src/traps.rs` (+ unit test). New harness `tests/scripts/trap_zero_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-19-trap-signal-zero-design.md`

**Background:** `parse_trap_signal(name) -> Result<TrapSignal, String>` (`src/traps.rs`) handles `"EXIT"` → `TrapSignal::Exit` near the top. Its numeric branch is `if let Ok(n) = name.parse::<i32>() { … KILL/STOP checks …; if name_table().iter().any(|(_,s)| *s==n) { Ok(Real(n)) } else { Err("{name}: invalid signal specification") } }`. Number `0` isn't in `name_table()`, so it errors. The executor already fires `TrapSignal::Exit` on shell exit, and the `trap -p` formatter renders `TrapSignal::Exit` as `EXIT`.

---

## Task 1: map numeric `0` → EXIT + tests + harness

**Files:**
- Modify: `src/traps.rs` — the guard.
- Test: `src/traps.rs` `mod tests`.
- Create: `tests/scripts/trap_zero_diff_check.sh`.

- [ ] **Step 1: Write the failing unit test** in `src/traps.rs` `mod tests` (near the existing `parse_trap_signal` tests):

```rust
    #[test]
    fn parse_trap_signal_zero_is_exit() {
        assert_eq!(parse_trap_signal("0"), Ok(TrapSignal::Exit));
        // regression: the EXIT name still maps to the same thing.
        assert_eq!(parse_trap_signal("EXIT"), Ok(TrapSignal::Exit));
    }
```

- [ ] **Step 2: Run to confirm it FAILS.**

Run: `cargo test --lib parse_trap_signal_zero_is_exit 2>&1 | tail -8`
Expected: FAIL — `parse_trap_signal("0")` is currently `Err("0: invalid signal specification")`.

- [ ] **Step 3: Add the guard.** In `parse_trap_signal`'s numeric branch, immediately after `if let Ok(n) = name.parse::<i32>() {`:

```rust
        if n == 0 {
            return Ok(TrapSignal::Exit);
        }
```
(Place it before the `SIGKILL`/`SIGSTOP` checks. Keep everything else.)

- [ ] **Step 4: Run to confirm PASS + no regression.**

Run: `cargo test --lib parse_trap_signal_zero_is_exit 2>&1 | tail -6 && cargo test --lib 2>&1 | grep "test result:" | grep -v "0 failed" || echo OK`
Expected: PASS; `OK`. Up-front grep `grep -rn "trap.*0\|invalid signal" tests/ src/ | grep -i trap | head` for any test asserting `trap 0` errors — none expected; update if found. `cargo clippy --lib` clean.

- [ ] **Step 5: Manual byte-check vs bash.**

Run:
```bash
cargo build 2>&1 | tail -1
H=./target/debug/huck
for f in "trap 'echo EX' 0; echo body" "trap '' 0; echo body" "trap 'echo A' 0; trap -p 0"; do
  echo "[$f] bash:[$(bash -c "$f" 2>&1|tr '\n' '/')] huck:[$($H -c "$f" 2>&1|tr '\n' '/')]"
done
```
Expected each pair identical: `body/EX/`, `body/`, and `trap -- 'echo A' EXIT/` (the `-p` form normalizes 0→EXIT). If `trap -p 0` differs, report it.

- [ ] **Step 6: Create the bash-diff harness** `tests/scripts/trap_zero_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v194: `trap … 0` (numeric 0 ≡ EXIT).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "register 0"   "trap 'echo EX' 0; echo body"
check "0 plus sig"   "trap 'echo EX' 0 2; echo body"
check "ignore '' 0"  "trap '' 0; echo body"
check "reset - 0"    "trap 'echo EX' 0; trap - 0; echo body"
check "trap -p 0"    "trap 'echo A' 0; trap -p 0"
check "EXIT name"    "trap 'echo EX' EXIT; echo body"
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 7: Run the harness + prove non-tautological.**

Run:
```bash
chmod +x tests/scripts/trap_zero_diff_check.sh
bash tests/scripts/trap_zero_diff_check.sh
BASE=$(git merge-base HEAD main); git worktree add -d /tmp/huck-prefix "$BASE" 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/trap_zero_diff_check.sh | tail -3
git worktree remove --force /tmp/huck-prefix 2>&1 | tail -1
```
Expected: post-fix all PASS (`Fail: 0`); pre-fix the `0`-spec cases FAIL (old huck errors), the `EXIT name` control passes.

- [ ] **Step 8: Commit.**

```bash
git add src/traps.rs tests/scripts/trap_zero_diff_check.sh
git commit -m "$(cat <<'EOF'
v194: trap with numeric signal 0 registers an EXIT trap

bash treats `trap … 0` as `trap … EXIT`; huck rejected 0 as an invalid signal.
parse_trap_signal now maps n==0 to TrapSignal::Exit (same as the EXIT name), so
register/ignore/reset/-p/mixed all work. Found by the v193 runtime sweep.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: full regression + memory

- [ ] **Step 1: Full suite + harnesses + clippy.**

Run:
```bash
cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -E "Fail: [1-9]" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: `ALL GREEN`; `ALL HARNESSES GREEN`; clippy clean.

- [ ] **Step 2: Record the iteration in memory** (controller does this post-merge): prepend v194 to `project_huck_iterations.md` (first bug from the v193 runtime sweep; `trap … 0` ≡ EXIT via the `n==0` guard in `parse_trap_signal`; runtime-sweep backlog `${!1}`/`$0`-in-function remains), update the `MEMORY.md` index line. No `bash-divergences.md` change.

---

## Report-back (Task 2)

Report: STATUS, the commit SHA, the Step-1 (Task 1) FAIL + Step-4 PASS results, the Step-5 manual byte-check, the harness result + its pre-fix FAIL count, full `cargo test` summary, clippy status.
