# v330 — `caller` builtin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `caller` builtin (bash-compatible), reading huck's existing `call_stack` — a useful standalone feature and the fourth `dbg-support` sub-arc step (509 → 475).

**Architecture:** Register `caller` in the builtins list + dispatch, and a `builtin_caller` that reads `shell.call_stack` (same indexing that backs the correct `FUNCNAME`/`BASH_LINENO`/`BASH_SOURCE`).

**Tech Stack:** Rust; huck-engine builtins; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-23-caller-builtin-design.md`
Issue: [#281](https://github.com/jdstanhope/huck/issues/281)

## Global Constraints

- bash 5.2.21 fidelity: `caller` → `LINE FILE` rc0 (rc1 at top level); `caller N` → `LINE FUNC FILE` rc0 (rc1 out of range); `caller <non-numeric>` → `caller: <arg>: invalid number` + `caller: usage: caller [expr]` rc2; extra args ignored. Mapping: `LINE = call_stack[n-1-N].call_line`, `FUNC/FILE = call_stack[n-2-N].{funcname,source}`; valid iff `n >= N+2`. (The prototype validated all cases byte-identical to bash.)
- Commit trailer; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` builtin/function integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in the commit (bare `#N`).

---

### Task 1: Add the `caller` builtin

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (name list + dispatch + `builtin_caller`)
- Create: `tests/scripts/caller_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/caller_diff_check.sh` (model on `trap_zero_diff_check.sh`), comparing `bash --norc --noprofile <file>` vs `"$HUCK_BIN" <file>` (a FILE, so `caller` reports the real filename) byte-identical incl. `EXIT:$?` and stderr. Use a temp-file `check` helper (as `lineno_fidelity_diff_check.sh` does). Cases:
```sh
# g -> f, then various caller forms
'f() {
  caller; echo "rc=$?"
  caller 0; echo "rc=$?"
  caller 1; echo "rc=$?"
  caller 2; echo "rc=$?"
  caller foo; echo "rc=$?"
  caller 0 99; echo "rc=$?"
}
g() { f; }
g
caller; echo "toprc=$?"'
# a stack-trace loop (dbg-support.sub shape)
'trace() { local i; for ((i=0; i<${#FUNCNAME[@]}; i++)); do caller $i || break; done; }
a() { trace; }
b() { a; }
b'
```
Build (`cargo build -p huck`) and run — huck FAILs (`caller: command not found`).

- [ ] **Step 2: Register `caller`**

In `builtins.rs`: add `"caller",` to the `BUILTIN_NAMES` list, and a dispatch arm `"caller" => builtin_caller(args, out, err, shell),` (next to `"enable" => …`, before the `_ => unreachable!`).

- [ ] **Step 3: Implement `builtin_caller`** (from the spec, verbatim):
```rust
fn builtin_caller(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let n = shell.call_stack.len();
    match args.first() {
        None => {
            if n >= 2 {
                let line = shell.call_stack[n - 1].call_line;
                let file = shell.call_stack[n - 2].source.clone();
                let _ = writeln!(out, "{line} {file}");
                ExecOutcome::Continue(0)
            } else {
                ExecOutcome::Continue(1)
            }
        }
        Some(a) => {
            let k: usize = match a.parse::<u64>() {
                Ok(v) => v as usize,
                Err(_) => {
                    crate::sh_error_to!(shell, err, Some("caller"), "{a}: invalid number");
                    let _ = writeln!(err, "caller: usage: caller [expr]");
                    return ExecOutcome::Continue(2);
                }
            };
            if n >= k + 2 {
                let line = shell.call_stack[n - 1 - k].call_line;
                let func = shell.call_stack[n - 2 - k].funcname.clone();
                let file = shell.call_stack[n - 2 - k].source.clone();
                let _ = writeln!(out, "{line} {func} {file}");
                ExecOutcome::Continue(0)
            } else {
                ExecOutcome::Continue(1)
            }
        }
    }
}
```
Confirm the `caller foo` stderr is byte-identical to bash (the `sh_error_to!` prefix + `caller: foo: invalid number`, then `caller: usage: caller [expr]`). If the `writeln!(out/err, …)` idiom differs from the file's convention (some builtins use an `e!` macro), match the surrounding style.

- [ ] **Step 4: Confirm the harness passes** vs bash (all forms + the stack-trace loop). Also spot-check `type caller` / `command -v caller` now report a builtin.

- [ ] **Step 5: Regression + dbg-support measurement**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1     # green
ulimit -v 1500000; bash tests/scripts/caller_diff_check.sh        # PASS
for t in trap_integration functions_integration funcname type_integration command_bare_form_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
ulimit -v 2000000
HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 200 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"   # PASS
HUCK_BASH_TEST_CATEGORY=dbg-support HUCK_TEST_TIMEOUT=120 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 240 bash tests/bash-test-suite/runner.sh > /tmp/dbgs.md 2>&1
SC=$(grep -oE "/tmp/huck-bash-tests[^ ]*" /tmp/dbgs.md | head -1)
echo "dbg-support diff: $(wc -l < $SC/dbg-support.diff) lines (was 509; expect ~475)"
```
dbg-support2 MUST stay PASS.

- [ ] **Step 6: Full sweep**
```bash
cargo build --release -p huck
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # green (coproc flake pre-existing)
```

- [ ] **Step 7: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/builtins.rs tests/scripts/caller_diff_check.sh
git commit -m "$(cat <<'EOF'
v330: add the caller builtin (#281)

Add `caller`/`caller N` reading the existing call_stack (LINE [FUNC] FILE;
rc1 out-of-range/top-level; invalid-number error rc2; extra args ignored),
byte-identical to bash. A useful standalone builtin; shrinks the dbg-support
bash-suite diff 509 -> 475 (its residual is dominated by a separate
DEBUG-output/command-sub line-ordering issue, not caller).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** registration + `builtin_caller` + harness + dbg-support measurement + regressions — all Task 1.
- **Placeholders:** none; the builtin is complete (prototype-validated). The output macro (`writeln!` vs `e!`) is flagged to match the file's convention.
- **Type consistency:** builtin signature `(&[String], &mut dyn Write, &mut dyn Write, &mut Shell) -> ExecOutcome`; `Frame { funcname, source, call_line }`.
- **Scope:** `caller` only; arithmetic args / `0 NULL` quirk / the deeper interleaving residual are out of scope.
