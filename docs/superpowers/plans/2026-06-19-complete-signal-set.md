# Complete Standard Signal Set Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck name, send, trap, and list all standard signals (1–31), matching bash, instead of only the ~16 job-control signals it knows today.

**Architecture:** Replace the two hand-maintained signal tables in `src/traps.rs` with one `libc::SIG*`-backed builder (cfg-guarded for macOS) and derive the trappable/killable views from it. The `kill -l` and `trap -l` listings adopt bash's `SIG`-prefixed/tab/5-column format. No change to the trap dispatch / pending-bitmask (`AtomicU32` already covers signals 1–31).

**Tech Stack:** Rust, the `libc` crate. Files: `src/traps.rs` (signal table), `src/builtins.rs` (`kill -l` / `trap -l` printers + tests), `tests/scripts/kill_signals_diff_check.sh` (new harness), `tests/kill_signals_integration.rs` (new integration test).

**Spec:** `docs/superpowers/specs/2026-06-19-complete-signal-set-design.md`

**Background the implementer needs:**
- `src/traps.rs` has two `const` tables: `TRAPPABLE` (14 entries, job-control signals minus KILL/STOP) and `KILLABLE` (16 = TRAPPABLE + KILL + STOP). Public accessors `name_table()` → `TRAPPABLE` and `killable_signals()` → `KILLABLE`, both returning `&'static [(&'static str, i32)]` (name WITHOUT `SIG` prefix, libc number).
- Internal `TRAPPABLE` uses in `traps.rs`: `ignored_at_startup_set` (line ~30, `for (_, signum) in TRAPPABLE`), `parse_trap_signal` (line ~315, `TRAPPABLE.iter().any(...)`; line ~331, `for (n, sig) in TRAPPABLE`).
- `src/builtins.rs`: `handle_kill_l` (number↔name via `killable_signals()`; empty → `print_killable_table`), `signal_by_name` (sending, `killable_signals()`), `kill_with_n_flag`, `print_killable_table` (the `kill -l` no-arg listing — currently 4-col, bare names), `print_signal_table` (the `trap -l` listing — currently 4-col bare via `name_table()`), `signal_number_to_name` (`name_table()`), `signal_names()` (SIG-prefixed, `name_table()`).
- The trap pending bitmask is an `AtomicU32` (bit N = signal N) — covers signals 1–31, so trapping the new signals needs NO infra change. (RT signals 34–64 are out of scope precisely because they'd overflow it.)
- Bash-diff harnesses live in `tests/scripts/*_diff_check.sh`; run with `bash tests/scripts/<name>.sh` (build huck first). Integration tests are `tests/*.rs` using a `huck` helper that runs the binary.

---

## Task 1: Unify the signal table in `traps.rs`

**Files:**
- Modify: `src/traps.rs` — add `standard_signals()` builder + `OnceLock` views; remove `TRAPPABLE`/`KILLABLE` consts; repoint `name_table()`/`killable_signals()` and the 3 internal uses.
- Test: `src/traps.rs` `mod tests`.

- [ ] **Step 1: Up-front grep for tests/code that encode the OLD signal set.**

Run:
```bash
grep -rn "TRAPPABLE\|KILLABLE\|killable_signals\|name_table\|signal_by_name\|print_killable\|print_signal_table" src/ tests/ | grep -iv "fn name_table\|fn killable_signals" | head -40
grep -rn "vec!\[\"HUP\"\|14\b.*signal\|16\b.*signal\|\.len(), *14\|\.len(), *16\|chunks(4)\|cols = 4" src/ tests/ | head
```
Classify: tests asserting specific common signals (HUP/INT/KILL/TERM/STOP/WINCH) stay valid (those signals remain). Flag any asserting a COUNT (14/16) or the OLD 4-column/bare format — those get updated in this task (table) or Task 2 (format). Report the list.

- [ ] **Step 2: Write the failing table tests** in `src/traps.rs` `mod tests`:

```rust
    #[test]
    fn name_table_has_full_standard_set_minus_kill_stop() {
        let t = name_table();
        // newly-added standard signals are present (trappable)
        for name in ["ABRT", "SEGV", "BUS", "FPE", "ILL", "TRAP", "SYS", "URG", "XCPU"] {
            assert!(t.iter().any(|(n, _)| *n == name), "trappable missing {name}");
        }
        // KILL and STOP are NOT trappable
        assert!(!t.iter().any(|(n, _)| *n == "KILL"), "KILL must not be trappable");
        assert!(!t.iter().any(|(n, _)| *n == "STOP"), "STOP must not be trappable");
        // all numbers fit the AtomicU32 pending mask (1..=31)
        assert!(t.iter().all(|(_, num)| (1..=31).contains(num)), "signal out of 1..=31");
    }

    #[test]
    fn killable_includes_kill_stop_and_new_signals() {
        let k = killable_signals();
        assert!(k.iter().any(|(n, _)| *n == "KILL"));
        assert!(k.iter().any(|(n, _)| *n == "STOP"));
        assert!(k.iter().any(|(n, _)| *n == "ABRT"));
        assert!(k.iter().any(|(n, _)| *n == "SEGV"));
        // number<->name agrees with libc
        assert_eq!(k.iter().find(|(n, _)| *n == "ABRT").map(|(_, x)| *x), Some(libc::SIGABRT));
        assert_eq!(k.iter().find(|(n, _)| *n == "SEGV").map(|(_, x)| *x), Some(libc::SIGSEGV));
    }
```

- [ ] **Step 3: Run to confirm they FAIL.**

Run: `cargo test --lib name_table_has_full_standard_set killable_includes_kill_stop 2>&1 | tail -15`
Expected: FAIL — current `TRAPPABLE`/`KILLABLE` lack ABRT/SEGV/etc.

- [ ] **Step 4: Replace the tables with the unified builder + views.**

In `src/traps.rs`, add (near the top, after the existing `use` lines — needs `use std::sync::OnceLock;` if not already imported):

```rust
/// Every standard (non-real-time) signal this platform names, as
/// (name without SIG prefix, libc number). Built from libc constants so numbers
/// are correct per platform; platform-specific signals are cfg-gated so the
/// crate builds on macOS as well as Linux. Real-time signals (SIGRTMIN..) are
/// intentionally excluded — the trap pending bitmask is an AtomicU32 (bits
/// 1..=31 only).
fn standard_signals() -> Vec<(&'static str, i32)> {
    let mut v = vec![
        ("HUP", libc::SIGHUP), ("INT", libc::SIGINT), ("QUIT", libc::SIGQUIT),
        ("ILL", libc::SIGILL), ("TRAP", libc::SIGTRAP), ("ABRT", libc::SIGABRT),
        ("BUS", libc::SIGBUS), ("FPE", libc::SIGFPE), ("KILL", libc::SIGKILL),
        ("USR1", libc::SIGUSR1), ("SEGV", libc::SIGSEGV), ("USR2", libc::SIGUSR2),
        ("PIPE", libc::SIGPIPE), ("ALRM", libc::SIGALRM), ("TERM", libc::SIGTERM),
        ("CHLD", libc::SIGCHLD), ("CONT", libc::SIGCONT), ("STOP", libc::SIGSTOP),
        ("TSTP", libc::SIGTSTP), ("TTIN", libc::SIGTTIN), ("TTOU", libc::SIGTTOU),
        ("URG", libc::SIGURG), ("XCPU", libc::SIGXCPU), ("XFSZ", libc::SIGXFSZ),
        ("VTALRM", libc::SIGVTALRM), ("PROF", libc::SIGPROF), ("WINCH", libc::SIGWINCH),
        ("IO", libc::SIGIO), ("SYS", libc::SIGSYS),
    ];
    #[cfg(target_os = "linux")]
    {
        v.push(("STKFLT", libc::SIGSTKFLT));
        v.push(("PWR", libc::SIGPWR));
    }
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
    {
        v.push(("EMT", libc::SIGEMT));
        v.push(("INFO", libc::SIGINFO));
    }
    v
}

static FULL_TABLE: OnceLock<Vec<(&'static str, i32)>> = OnceLock::new();
static TRAPPABLE_VIEW: OnceLock<Vec<(&'static str, i32)>> = OnceLock::new();
```

Replace the `TRAPPABLE` and `KILLABLE` `const` definitions and the two accessor
bodies with:

```rust
/// Returns the trappable signal table (name → number) — every standard signal
/// except KILL and STOP (which POSIX says cannot be trapped).
pub fn name_table() -> &'static [(&'static str, i32)] {
    TRAPPABLE_VIEW
        .get_or_init(|| {
            killable_signals()
                .iter()
                .copied()
                .filter(|(_, n)| *n != libc::SIGKILL && *n != libc::SIGSTOP)
                .collect()
        })
        .as_slice()
}

/// Returns every signal huck can SEND via `kill` (the full standard set,
/// including KILL and STOP). Used by `kill` send + `kill -l` number↔name.
pub fn killable_signals() -> &'static [(&'static str, i32)] {
    FULL_TABLE.get_or_init(standard_signals).as_slice()
}
```

Then repoint the 3 internal `TRAPPABLE` uses to `name_table()`:
- `ignored_at_startup_set`: `for (_, signum) in TRAPPABLE {` → `for (_, signum) in name_table() {`
- `parse_trap_signal` numeric branch: `if TRAPPABLE.iter().any(|(_, s)| *s == n) {` → `if name_table().iter().any(|(_, s)| *s == n) {`
- `parse_trap_signal` name branch: `for (n, sig) in TRAPPABLE {` → `for (n, sig) in name_table() {`

(The `IGNORED_AT_STARTUP` `OnceLock` and `init_pending_bitmask` are unchanged.)

- [ ] **Step 5: Run the new tests + the existing traps tests.**

Run: `cargo test --lib -p huck traps:: 2>&1 | tail -15 && cargo test --lib name_table_has_full killable_includes 2>&1 | tail -6`
Expected: PASS. Existing `parse_trap_signal` tests (TERM/KILL) still pass (KILL still rejected for trap; TERM still trappable).

- [ ] **Step 6: Update any count/old-set assertions found in Step 1.**

If Step 1 found a test asserting `TRAPPABLE.len() == 14` / `KILLABLE.len() == 16` (or similar), update the expectation to the new set (or assert membership instead of exact count). Verify against bash where relevant. If none found, note that and skip.

- [ ] **Step 7: Commit.**

```bash
git add src/traps.rs
git commit -m "$(cat <<'EOF'
v189: unify signal table to the full standard set (1-31)

Replace the hand-maintained TRAPPABLE/KILLABLE tables with one libc-constant
builder (cfg-guarded for macOS); derive name_table() (trappable = all minus
KILL/STOP) and killable_signals() (all) from it. Adds the 15 missing standard
signals (ABRT, SEGV, BUS, FPE, ILL, TRAP, SYS, URG, XCPU, XFSZ, VTALRM, PROF,
IO, PWR, STKFLT). No trap-runtime change (AtomicU32 covers 1..=31).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: bash-format the `kill -l` / `trap -l` listings

**Files:**
- Modify: `src/builtins.rs` — `print_killable_table` and `print_signal_table`.
- Test: `src/builtins.rs` `mod tests`.

- [ ] **Step 1: Write the failing format test** in `src/builtins.rs` `mod tests`:

```rust
    #[test]
    fn kill_l_listing_matches_bash_format() {
        let mut buf = Vec::new();
        print_killable_table(&mut buf);
        let s = String::from_utf8(buf).unwrap();
        // bash: ` 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP\n…`
        let first = s.lines().next().unwrap();
        assert_eq!(first, " 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP");
        // SIG prefix everywhere, 5 columns per full row
        assert!(s.contains("SIGABRT"), "missing SIGABRT: {s}");
        assert!(s.contains("11) SIGSEGV"));
    }
```

- [ ] **Step 2: Run to confirm it FAILS.**

Run: `cargo test --lib kill_l_listing_matches_bash_format 2>&1 | tail -12`
Expected: FAIL — current output is 4-col, bare names (`1) HUP   2) INT …`).

- [ ] **Step 3: Reformat the printers.** Replace `print_killable_table`'s body with the bash format:

```rust
fn print_killable_table(out: &mut dyn Write) {
    print_sig_listing(out, crate::traps::killable_signals());
}

/// Prints a signal listing in bash's `kill -l` format: signals sorted by number,
/// `SIG`-prefixed names, 5 columns per row, tab-separated, number right-aligned
/// to width 2. (huck lists the standard signals 1–31; bash additionally appends
/// the real-time tail 34–64, deferred.)
fn print_sig_listing(out: &mut dyn Write, table: &[(&str, i32)]) {
    let mut sigs: Vec<&(&str, i32)> = table.iter().collect();
    sigs.sort_by_key(|(_, n)| *n);
    let last = sigs.len().saturating_sub(1);
    for (i, (name, num)) in sigs.iter().enumerate() {
        let sep = if i % 5 == 4 || i == last { "\n" } else { "\t" };
        let _ = write!(out, "{num:>2}) SIG{name}{sep}");
    }
}
```

And repoint `print_signal_table` (the `trap -l` listing) to the same helper:

```rust
fn print_signal_table(out: &mut dyn Write) {
    print_sig_listing(out, crate::traps::name_table());
}
```

- [ ] **Step 4: Run the format test + any existing listing tests.**

Run: `cargo test --lib kill_l_listing_matches_bash_format 2>&1 | tail -6 && cargo test --lib -p huck 2>&1 | grep "test result:" | grep -v "0 failed" || echo OK`
Expected: PASS; `OK` (no other failures). If an existing test asserted the old 4-col/bare listing, update it to the bash format now (verify against `bash -c 'kill -l'`).

- [ ] **Step 5: Commit.**

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
v189: kill -l / trap -l listing in bash's SIG-prefixed 5-col format

print_sig_listing emits `N) SIGNAME` tab-separated, 5 per row, matching bash for
the standard signals (the RT tail beyond 31 is deferred).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: bash-diff harness

**Files:**
- Create: `tests/scripts/kill_signals_diff_check.sh`

- [ ] **Step 1: Create the harness** (mirrors `tests/scripts/process_sub_diff_check.sh`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v189: the full standard signal set
# (kill -l number<->name, the kill -l listing format, and kill -SIG sending).
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

# number -> name for every standard signal
check "num->name 1..31" 'for n in $(seq 1 31); do kill -l $n; done'
# name -> number (bare, SIG-prefixed, lowercase)
check "name->num"       'kill -l ABRT SIGSEGV bus 11 KILL'
# 128+signo exit-status form
check "exit-form 137"   'kill -l 137'
# full listing: first 30 signals = 6 complete rows, byte-identical to bash
# (bash appends the RT tail beyond 31, excluded by head -6)
check "listing head -6" 'kill -l | head -6'
# send a real (non-job-control) signal to a DIRECT-child sleep and capture the
# wait status (128+signo). sleep must be a child of THIS shell so `wait` reports
# the termination signal (a non-child would return 127 in both shells).
check "send ABRT"       'sleep 30 & p=$!; kill -ABRT $p; wait $p; echo "rc=$?"'
check "send -s SEGV"    'sleep 30 & p=$!; kill -s SEGV $p; wait $p; echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable and run.**

Run:
```bash
chmod +x tests/scripts/kill_signals_diff_check.sh
cargo build 2>&1 | tail -1
bash tests/scripts/kill_signals_diff_check.sh
```
Expected: all `PASS`, `Total: 6, Pass: 6, Fail: 0`, exit 0. If "send ABRT/SEGV" diverges due to `wait` rc reporting differences, simplify to asserting the signal is *accepted* (rc 0 of the `kill` itself): `kill -ABRT "$s"; echo "kill-rc=$?"; kill -KILL "$s"` — keep it byte-identical; do NOT weaken to hide a real divergence.

- [ ] **Step 3: Prove non-tautological** (fails pre-fix):

```bash
BASE=$(git merge-base HEAD main)
git worktree add -d /tmp/huck-prefix "$BASE" 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/kill_signals_diff_check.sh | tail -4
git worktree remove --force /tmp/huck-prefix 2>&1 | tail -1
```
Expected: multiple FAILs pre-fix (`num->name 1..31`, `name->num`, `listing head -6`, `send ABRT`). Report how many.

- [ ] **Step 4: Commit.**

```bash
git add tests/scripts/kill_signals_diff_check.sh
git commit -m "$(cat <<'EOF'
v189: bash-diff harness for the full standard signal set

kill -l number<->name (1..31), the listing format (head -6), and kill -SIG /
-s sending; non-tautological (fails pre-fix).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: integration test — trapping new signals

**Files:**
- Create: `tests/kill_signals_integration.rs` (model the `huck` helper on an existing integration test, e.g. `tests/trap_integration.rs`).

- [ ] **Step 1: Look at an existing integration test for the run helper.**

Run: `sed -n '1,30p' tests/trap_integration.rs` (or any `tests/*_integration.rs`) to copy the exact `huck`/`run` helper (binary invocation via `CARGO_BIN_EXE_huck`, stdin-fed script).

- [ ] **Step 2: Write the test** using that helper:

```rust
// Uses the same in-crate helper pattern as the other integration tests.
#[test]
fn trap_fires_on_newly_supported_signal() {
    // trap a non-job-control signal (SEGV) and raise it via kill -SEGV $$.
    let out = huck("trap 'echo HIT' SEGV\nkill -SEGV $$\nexit\n");
    assert!(out.contains("HIT"), "SEGV trap did not fire: {out:?}");
}

#[test]
fn kill_l_round_trips_new_signal() {
    assert_eq!(huck("kill -l ABRT\n").trim(), &libc::SIGABRT.to_string());
    assert_eq!(huck(&format!("kill -l {}\n", libc::SIGABRT)).trim(), "ABRT");
}
```

(Add `libc` to `[dev-dependencies]` if integration tests don't already use it —
check `Cargo.toml`; the lib already depends on `libc`, and integration tests can
use it as a dev-dep. If adding friction, hardcode `6`/`"ABRT"` and a comment.)

- [ ] **Step 3: Run the integration test.**

Run: `cargo test --test kill_signals_integration 2>&1 | tail -12`
Expected: PASS. If `trap … SEGV` does not fire, STOP and report (it would mean the pending-bitmask path needs the new signal registered — investigate `install`).

- [ ] **Step 4: Commit.**

```bash
git add tests/kill_signals_integration.rs Cargo.toml
git commit -m "$(cat <<'EOF'
v189: integration test for trapping a newly-supported signal

trap '…' SEGV fires on kill -SEGV $$; kill -l round-trips ABRT<->6.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: full regression, macOS cfg sanity, memory

**Files:**
- Modify (memory): `project_huck_iterations.md`, `MEMORY.md` (paths under `/home/john/.claude/projects/-home-john-projects-shuck/memory/`).

- [ ] **Step 1: Full test suite (0 failures).**

Run: `cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"`
Expected: `ALL GREEN`. If a previously-passing test now fails, it encoded the old set/format — update it to match bash (verify against `bash -c '…'`) and re-run.

- [ ] **Step 2: All bash-diff harnesses + clippy green.**

Run:
```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -iv "fail: 0" || echo "ALL HARNESSES GREEN"
cargo clippy 2>&1 | tail -3
```
Expected: `ALL HARNESSES GREEN`; clippy clean.

- [ ] **Step 3: macOS cfg-build sanity.**

Run:
```bash
rustup target list --installed | grep -q apple-darwin && cargo check --target x86_64-apple-darwin 2>&1 | tail -5 || echo "darwin target not installed — verify by inspection: SIGSTKFLT/SIGPWR are #[cfg(linux)], SIGEMT/SIGINFO are #[cfg(macos/bsd)]"
```
Expected: builds, or the inspection note. Confirm no `libc::SIGSTKFLT`/`SIGPWR` reference is reachable on macOS (they're Linux-only) and no `SIGEMT`/`SIGINFO` on Linux.

- [ ] **Step 4: Record the iteration in memory.**

Prepend a v189 entry to `project_huck_iterations.md` (newest-first): coverage-sweep-found gap; unified the signal table to the full standard set (1–31) from `libc` constants with cfg guards; fixed `kill -l` number↔name, `kill -SIG`/`-s` sending, `trap … SEGV`, and the `kill -l`/`trap -l` listing format (bash SIG-prefix/tab/5-col); trap pending `AtomicU32` already covered 1–31 so no dispatch change (and is why RT signals are deferred); merge SHA (fill after merge). Update the `MEMORY.md` index line + the coverage-divergence note (kill -l RESOLVED; declare-no-args + bind -p still pending). Note the two deferred follow-ons (RT signals; `kill -l` RT tail).

- [ ] **Step 5: Commit the memory update.**

```bash
git add /home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md
git commit -m "v189: record complete-signal-set iteration in memory

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Report-back (Task 5)

Report: STATUS, all commit SHAs, the Step 1 grep classification (Task 1), the full `cargo test` summary, harness results (incl. the new `kill_signals_diff_check.sh` and its pre-fix FAIL count), clippy status, the macOS cfg-build result, and confirmation the trap-on-SEGV integration test passes.
