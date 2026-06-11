# v135 — finish `test`/`[[` operators (M-27 + M-14b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the remaining `test`/`[`/`[[ ]]` unary operators — M-27 (`-p -S -b -c -O -G -N -k -u -g -t`) and M-14b (`[[ -v arr[i] ]]` / `test -v 'arr[i]'`). M-26 (`test -v VAR`) already works; v135 deletes its stale entry + locks it with a regression test.

**Architecture:** The two engines share one impl — `[[ ]]`'s `eval_unary` delegates file ops to `test_builtin::evaluate(&["-X", s])`. So add the M-27 operators to `test_builtin` (one place) + their `TestUnaryOp` variants so `[[ ]]` parses them. M-14b adds a subscript-aware `-v` helper used by both engines.

**Tech Stack:** Rust, libc/unix (stat mode bits, geteuid/getegid, isatty). Tests: cargo integration + a bash-diff harness over real filesystem artifacts.

**GIT SAFETY:** Do NOT `git checkout <sha>` — stay on `v135-test-operators`; edit, build, commit in place. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec `docs/superpowers/specs/2026-06-11-test-operators-design.md`. Key locations: `apply_unary` (test_builtin.rs:131, the unary arms); `is_unary_op` (test_builtin.rs:88, the `-a|-e|…` recognizer); `TestUnaryOp` enum (command.rs:389); `try_unary_op` (command.rs:1995); `eval_unary` (executor.rs:1395, delegates to test_builtin); `eval_test_expr` VarSet arm (executor.rs:1336, `shell.is_set(&s)`); `builtin_test` predicate (builtins.rs:6213, `&|n| shell.is_set(n)`); helpers `expand::split_name_subscript(&str)->Option<(String,String)>` (expand.rs:530), `Shell::is_set` (shell_state.rs:546), `Shell::lookup_array_element(name, idx)->Option<String>` (835), `Shell::lookup_associative_element(name, key)->Option<String>` (1068), `Shell::get_associative(name)->Option<&…>` (1056).

---

### Task 1: M-27 operators (`-p -S -b -c -O -G -N -k -u -g -t`)

**Files:**
- Create: `tests/test_operators_integration.rs`
- Modify: `src/test_builtin.rs`, `src/command.rs`, `src/executor.rs`

- [ ] **Step 1: Write the failing integration tests** — create `tests/test_operators_integration.rs`:
```rust
//! v135: M-27 test/[[ file-type/mode/fd operators.
use std::process::{Command, Stdio};
use std::io::Write;
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, i32) {
    let mut c = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn");
    c.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
}

#[test]
fn char_special_dev_null() {
    assert_eq!(run("[ -c /dev/null ] && echo T || echo F\n").0, "T\n");
    assert_eq!(run("[[ -c /dev/null ]] && echo T || echo F\n").0, "T\n");
}
#[test]
fn char_special_regular_file_false() {
    assert_eq!(run("printf x > /tmp/v135reg; [ -c /tmp/v135reg ] && echo T || echo F; rm -f /tmp/v135reg\n").0, "F\n");
}
#[test]
fn fifo_via_mkfifo() {
    // mkfifo is external; create + test + clean.
    let s = "D=$(mktemp -d); mkfifo $D/f; [ -p $D/f ] && echo T || echo F; [ -p /dev/null ] && echo pT || echo pF; rm -rf $D\n";
    assert_eq!(run(s).0, "T\npF\n");
}
#[test]
fn block_special_dev_null_is_not_block() {
    assert_eq!(run("[ -b /dev/null ] && echo T || echo F\n").0, "F\n");
}
#[test]
fn owned_by_euid_true_for_own_file() {
    assert_eq!(run("f=$(mktemp); [ -O $f ] && echo T || echo F; rm -f $f\n").0, "T\n");
}
#[test]
fn sticky_setuid_setgid() {
    let s = "f=$(mktemp); chmod u+s,g+s $f; [ -u $f ] && echo uT || echo uF; [ -g $f ] && echo gT || echo gF; rm -f $f; \
             d=$(mktemp -d); chmod +t $d; [ -k $d ] && echo kT || echo kF; rm -rf $d\n";
    assert_eq!(run(s).0, "uT\ngT\nkT\n");
}
#[test]
fn terminal_fd_false_when_redirected() {
    assert_eq!(run("[ -t 0 ] </dev/null && echo T || echo F\n").0, "F\n");
    assert_eq!(run("[ -t 99 ] && echo T || echo F\n").0, "F\n");      // bad fd
    assert_eq!(run("[ -t abc ] && echo T || echo F\n").0, "F\n");     // non-numeric
}
#[test]
fn missing_file_all_false() {
    for op in ["-p","-S","-b","-c","-O","-G","-k","-u","-g"] {
        let s = format!("[ {op} /no/such/path/v135 ] && echo T || echo F\n");
        assert_eq!(run(&s).0, "F\n", "op {op}");
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --test test_operators_integration 2>&1 | tail -20`. Expected: the `-p/-c/-O/-u/-g/-k/-t` tests FAIL (operators unknown → `[[ ]]` parse error / `[` "unknown operator" rc 2 → wrong output); `block_special_dev_null_is_not_block` and `missing_file_all_false` may partially pass (everything returns F/error). Confirm they fail because the operators aren't implemented.

- [ ] **Step 3: `apply_unary` in src/test_builtin.rs** — add `use std::os::unix::fs::MetadataExt;` at the top, a small helper, and the 11 arms before the final `_ => Err(...)`:
```rust
        "-p" => Ok(file_mode(operand).map(|m| m & libc::S_IFMT == libc::S_IFIFO).unwrap_or(false)),
        "-S" => Ok(file_mode(operand).map(|m| m & libc::S_IFMT == libc::S_IFSOCK).unwrap_or(false)),
        "-b" => Ok(file_mode(operand).map(|m| m & libc::S_IFMT == libc::S_IFBLK).unwrap_or(false)),
        "-c" => Ok(file_mode(operand).map(|m| m & libc::S_IFMT == libc::S_IFCHR).unwrap_or(false)),
        "-k" => Ok(file_mode(operand).map(|m| m & libc::S_ISVTX != 0).unwrap_or(false)),
        "-u" => Ok(file_mode(operand).map(|m| m & libc::S_ISUID != 0).unwrap_or(false)),
        "-g" => Ok(file_mode(operand).map(|m| m & libc::S_ISGID != 0).unwrap_or(false)),
        "-O" => Ok(std::fs::metadata(operand).map(|m| m.uid() == unsafe { libc::geteuid() }).unwrap_or(false)),
        "-G" => Ok(std::fs::metadata(operand).map(|m| m.gid() == unsafe { libc::getegid() }).unwrap_or(false)),
        "-N" => Ok(std::fs::metadata(operand).map(|m| m.mtime() > m.atime()).unwrap_or(false)),
        "-t" => Ok(operand.parse::<i32>().map(|fd| unsafe { libc::isatty(fd) } == 1).unwrap_or(false)),
```
with the helper (near `access`):
```rust
/// mode bits of `path` (follows symlinks), or None if it can't be stat'd.
fn file_mode(path: &str) -> Option<libc::mode_t> {
    std::fs::metadata(path).ok().map(|m| m.mode() as libc::mode_t)
}
```
NOTE: `MetadataExt::mode()` returns `u32`; cast to `libc::mode_t` for the `&` with `S_IFMT`/`S_IFIFO` etc. The `libc` `S_IF*`/`S_IS*` constants are `mode_t` (often `u32` on Linux). Make the casts compile cleanly — if a type mismatch, cast both sides to `u32`.

- [ ] **Step 4: `is_unary_op` in src/test_builtin.rs (~88)** — extend the `matches!(s, "-a" | … | "-v")` list to also include `"-p" | "-S" | "-b" | "-c" | "-O" | "-G" | "-N" | "-k" | "-u" | "-g" | "-t"`.

- [ ] **Step 5: `TestUnaryOp` + `try_unary_op` in src/command.rs** — add 11 variants to the enum (command.rs:389) with comments:
```rust
    IsFifo,        // -p
    IsSocket,      // -S
    IsBlockDev,    // -b
    IsCharDev,     // -c
    OwnedByEuid,   // -O
    OwnedByEgid,   // -G
    NewerThanRead, // -N
    IsSticky,      // -k
    IsSetuid,      // -u
    IsSetgid,      // -g
    IsTerminal,    // -t
```
and the matching `try_unary_op` arms (command.rs:1995):
```rust
        "-p" => Some(TestUnaryOp::IsFifo),
        "-S" => Some(TestUnaryOp::IsSocket),
        "-b" => Some(TestUnaryOp::IsBlockDev),
        "-c" => Some(TestUnaryOp::IsCharDev),
        "-O" => Some(TestUnaryOp::OwnedByEuid),
        "-G" => Some(TestUnaryOp::OwnedByEgid),
        "-N" => Some(TestUnaryOp::NewerThanRead),
        "-k" => Some(TestUnaryOp::IsSticky),
        "-u" => Some(TestUnaryOp::IsSetuid),
        "-g" => Some(TestUnaryOp::IsSetgid),
        "-t" => Some(TestUnaryOp::IsTerminal),
```

- [ ] **Step 6: `eval_unary` in src/executor.rs (~1395)** — add 11 delegating arms before the closing `}` (mirroring the FileExists pattern):
```rust
        TestUnaryOp::IsFifo        => test_builtin::evaluate(&["-p".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSocket      => test_builtin::evaluate(&["-S".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsBlockDev    => test_builtin::evaluate(&["-b".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsCharDev     => test_builtin::evaluate(&["-c".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::OwnedByEuid   => test_builtin::evaluate(&["-O".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::OwnedByEgid   => test_builtin::evaluate(&["-G".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::NewerThanRead => test_builtin::evaluate(&["-N".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSticky      => test_builtin::evaluate(&["-k".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSetuid      => test_builtin::evaluate(&["-u".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSetgid      => test_builtin::evaluate(&["-g".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsTerminal    => test_builtin::evaluate(&["-t".to_string(), s.to_string()]).unwrap_or(false),
```
(Any `match` on `TestUnaryOp` elsewhere — e.g. a Debug/exhaustive match — must also gain the new arms; the compiler will flag non-exhaustive matches. Fix them.)

- [ ] **Step 7: Run the tests** — `cargo test --test test_operators_integration 2>&1 | tail -15` → all pass.

- [ ] **Step 8: Build + FULL regression + clippy** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (none — existing test/`[[ ]]` tests stay green); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 9: Sanity vs bash** (report):
```
for op in -p -S -b -c -O -G -k -u -g; do
  f=$(mktemp); b=$(bash -c "[ $op $f ] && echo T || echo F"); h=$(./target/debug/huck -c "[ $op $f ] && echo T || echo F"); rm -f $f
  [ "$b" = "$h" ] && echo "MATCH $op" || echo "DIFF $op: bash=$b huck=$h"
done
echo "-c /dev/null:"; bash -c '[ -c /dev/null ] && echo T'; ./target/debug/huck -c '[ -c /dev/null ] && echo T'
```

- [ ] **Step 10: Commit**
```bash
git add src/test_builtin.rs src/command.rs src/executor.rs tests/test_operators_integration.rs
git commit -m "$(cat <<'EOF'
feat(v135): test/[[ file-type/mode/fd operators (M-27)

Add -p -S -b -c (file types), -k -u -g (sticky/setuid/setgid), -O -G (owned by
euid/egid), -N (modified since read), -t (fd is a terminal) to the shared
test_builtin engine + TestUnaryOp so both `test`/`[` and `[[ ]]` support them
(the [[ ]] path delegates to test_builtin). Was a parse error in [[ ]] before.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: M-14b — subscript-aware `-v` (+ M-26 regression)

**Files:**
- Modify: `src/shell_state.rs` (`element_or_var_is_set`), `src/builtins.rs` (predicate), `src/executor.rs` (VarSet arm)
- Modify: `tests/test_operators_integration.rs` (add `-v` tests)

- [ ] **Step 1: Add failing tests** — append:
```rust
#[test]
fn v_plain_name_regression() { // M-26 still works
    assert_eq!(run("x=1; [ -v x ] && echo T || echo F; [ -v NOPE ] && echo T || echo F\n").0, "T\nF\n");
    assert_eq!(run("x=1; [[ -v x ]] && echo T || echo F\n").0, "T\n");
}
#[test]
fn v_indexed_array_element() {
    assert_eq!(run("arr=(a b c); [[ -v arr[1] ]] && echo T || echo F; [[ -v arr[9] ]] && echo T || echo F\n").0, "T\nF\n");
    // builtin form
    assert_eq!(run("arr=(a b c); [ -v 'arr[1]' ] && echo T || echo F\n").0, "T\n");
}
#[test]
fn v_indexed_array_arith_subscript() {
    assert_eq!(run("arr=(a b c); i=2; [[ -v arr[i] ]] && echo T || echo F; [[ -v arr[i+5] ]] && echo T || echo F\n").0, "T\nF\n");
}
#[test]
fn v_associative_element() {
    assert_eq!(run("declare -A m; m[key]=1; [[ -v m[key] ]] && echo T || echo F; [[ -v m[nope] ]] && echo T || echo F\n").0, "T\nF\n");
}
#[test]
fn v_subscript_on_unset_base() {
    assert_eq!(run("[[ -v nope_arr[0] ]] && echo T || echo F\n").0, "F\n");
}
```

- [ ] **Step 2: Run to verify** — `cargo test --test test_operators_integration v_ 2>&1 | tail -15`. Expected: `v_plain_name_regression` PASSES (M-26 done); the array-element tests FAIL (huck treats `arr[1]` as a plain name → false).

- [ ] **Step 3: `Shell::element_or_var_is_set` in src/shell_state.rs** — add (read-only `&self`):
```rust
/// `-v` target: a bare name / positional / special param, OR an array element
/// `name[sub]`. For the element form, report whether THAT element is set:
/// associative arrays use `sub` as a literal key; indexed arrays arith-evaluate
/// `sub` to an index. Falls back to `is_set` for the no-subscript form.
pub fn element_or_var_is_set(&self, target: &str) -> bool {
    if let Some((name, sub)) = crate::expand::split_name_subscript(target) {
        if self.get_associative(&name).is_some() {
            return self.lookup_associative_element(&name, &sub).is_some();
        }
        // indexed array: arith-evaluate the subscript (read-only).
        let idx = match crate::arith::eval_to_i64_readonly(&sub, self) {
            Ok(n) if n >= 0 => n as usize,
            _ => return false,
        };
        return self.lookup_array_element(&name, idx).is_some();
    }
    self.is_set(target)
}
```
IMPORTANT on the subscript arith: find how huck already evaluates an array subscript for indexed access (grep for where `${arr[i]}` / `arr[i]=` resolve the subscript — likely `arith::parse` + an eval with `&Shell`). REUSE that exact path so `[[ -v arr[i] ]]` matches `${arr[i]}` semantics. If the only arith entry needs `&mut Shell`, either (a) make `element_or_var_is_set` take `&self` and use a read-only subscript evaluator (preferred — a `-v` subscript with a command substitution is a rare edge), or (b) evaluate the subscript at the call sites (which have `&mut Shell` in the `[[ ]]` path) and pass the resolved index/key in. Pick the lowest-churn correct option; if you add a `eval_to_i64_readonly`, keep it tiny (parse + eval variables via `&self`, no command subs). Document the chosen approach.

- [ ] **Step 4: Wire `builtin_test` (src/builtins.rs:6213)** — change the predicate from `&|n| shell.is_set(n)` to `&|n| shell.element_or_var_is_set(n)`.

- [ ] **Step 5: Wire `[[ ]]` VarSet (src/executor.rs:1336)** — change `return Ok(shell.is_set(&s));` to `return Ok(shell.element_or_var_is_set(&s));`.

- [ ] **Step 6: Run the tests** — `cargo test --test test_operators_integration 2>&1 | tail -15` → all pass (Task 1 + Task 2).

- [ ] **Step 7: Build + FULL regression + clippy** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none — existing `-v` tests stay green); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 8: Sanity vs bash** (report):
```
for f in 'arr=(a b c); [[ -v arr[1] ]] && echo T || echo F' 'arr=(a b c); [[ -v arr[9] ]] && echo T || echo F' 'declare -A m; m[k]=1; [[ -v m[k] ]] && echo T || echo F' 'declare -A m; m[k]=1; [[ -v m[x] ]] && echo T || echo F'; do
  b=$(bash -c "$f"); h=$(./target/debug/huck -c "$f"); [ "$b" = "$h" ] && echo "MATCH" || echo "DIFF: $f (b=$b h=$h)"
done
```

- [ ] **Step 9: Commit**
```bash
git add src/shell_state.rs src/builtins.rs src/executor.rs tests/test_operators_integration.rs
git commit -m "$(cat <<'EOF'
feat(v135): -v array-element form for test/[[ (M-14b)

`[[ -v arr[i] ]]` / `[ -v 'arr[i]' ]` now check whether the specific array element
is set (indexed: arith subscript; associative: literal key), matching bash —
previously the subscripted name fell through to a plain-name lookup (always false).
New Shell::element_or_var_is_set used by both engines. Plain-name -v (M-26) is
unchanged + locked by a regression test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Bash-diff harness + docs (resolve M-26/M-27/M-14b)

**Files:**
- Create: `tests/scripts/test_operators_diff_check.sh`
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Bash-diff harness** — create `tests/scripts/test_operators_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v135: M-27 file/fd test operators + M-14b
# -v array elements. Builds real artifacts; compares each fragment bash vs huck.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
D="$(mktemp -d)"; trap 'rm -rf "$D"' EXIT
mkfifo "$D/fifo"
: > "$D/reg"
mkdir "$D/sticky"; chmod +t "$D/sticky"
: > "$D/suid"; chmod u+s "$D/suid"
: > "$D/sgid"; chmod g+s "$D/sgid"
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
for form in '[ %s ]' '[[ %s ]]'; do
  T() { printf "$form" "$1"; }
  check "char /dev/null $form"  "$(T '-c /dev/null') && echo T || echo F"
  check "fifo $form"            "$(T "-p $D/fifo") && echo T || echo F"
  check "fifo-on-reg $form"     "$(T "-p $D/reg") && echo T || echo F"
  check "block /dev/null $form" "$(T '-b /dev/null') && echo T || echo F"
  check "sticky $form"          "$(T "-k $D/sticky") && echo T || echo F"
  check "suid $form"            "$(T "-u $D/suid") && echo T || echo F"
  check "sgid $form"            "$(T "-g $D/sgid") && echo T || echo F"
  check "owned $form"           "$(T "-O $D/reg") && echo T || echo F"
  check "group $form"           "$(T "-G $D/reg") && echo T || echo F"
  check "missing-c $form"       "$(T '-c /no/such/v135') && echo T || echo F"
done
check "term fd0 redir"  '[ -t 0 ] </dev/null && echo T || echo F'
check "term bad fd"     '[ -t 99 ] && echo T || echo F'
check "v idx set"       'a=(x y z); [[ -v a[1] ]] && echo T || echo F'
check "v idx unset"     'a=(x y z); [[ -v a[9] ]] && echo T || echo F'
check "v idx arith"     'a=(x y z); i=2; [[ -v a[i] ]] && echo T || echo F'
check "v assoc set"     'declare -A m; m[k]=1; [[ -v m[k] ]] && echo T || echo F'
check "v assoc unset"   'declare -A m; m[k]=1; [[ -v m[x] ]] && echo T || echo F'
check "v builtin idx"   "a=(x y z); [ -v 'a[1]' ] && echo T || echo F"
check "v plain"         'x=1; [ -v x ] && echo T || echo F'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/test_operators_diff_check.sh`. Run: `cargo build 2>&1 | tail -2; bash tests/scripts/test_operators_diff_check.sh` → expect `Fail: 0`. If a row fails, report the diff (do NOT mask). If the runner is root, `-O`/`-G` may differ from a non-root expectation — but the harness compares to bash on the SAME host, so parity holds regardless.

- [ ] **Step 2: Delete M-26, M-27, M-14b from docs/bash-divergences.md** — remove all three Tier-2 entries (M-26 + M-27 in the "Builtins (other)" group; M-14b in "Compound commands"). Decrement the Tier-2 count in the summary table from 24 to 21. Search for any other references to these IDs.

- [ ] **Step 3: Verify docs** — `grep -n "M-26\b\|M-27\b\|M-14b" docs/bash-divergences.md` → none; `grep -n "Missing features (Tier 2) | 21" docs/bash-divergences.md` → present.

- [ ] **Step 4: Full regression + clippy** — `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean); smoke an existing test harness if one exists for `[[ ]]`.

- [ ] **Step 5: Commit**
```bash
git add tests/scripts/test_operators_diff_check.sh docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
test+docs(v135): test-operators harness; resolve M-26/M-27/M-14b

Add the bash-diff harness over real artifacts (fifo/sticky/suid/sgid/char-dev/...)
+ -v array elements. Delete the three resolved divergences (Tier-2 24->21);
M-26 was already implemented (stale entry).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-review notes
- **Spec coverage:** Task 1 = M-27 (11 ops across test_builtin/command/executor) + per-op tests; Task 2 = M-14b subscript-aware `-v` + M-26 regression; Task 3 = bash-diff harness + delete the 3 entries.
- **Shared impl:** the new ops live ONCE in `test_builtin::apply_unary`; `[[ ]]` delegates via `eval_unary`. The `TestUnaryOp` variants exist only so `[[ ]]` PARSES the flags.
- **M-14b subscript arith:** reuse huck's existing array-subscript evaluator so `[[ -v arr[i] ]]` matches `${arr[i]}`; associative keys are literal.
- **No-regress:** existing operators + plain-name `-v` unchanged; bash-diff harness compares parity (handles `-O`/`-G`/`-N` host-dependence).
