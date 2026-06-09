# v125 — Redirections on Function-Call Commands (M-117) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply a redirect attached to a function-call command (`func >file`, `func 2>&1`, `func >&2`, `func <file`, …) to the function body — fixing nvm's `→ ∞`.

**Architecture:** Reuse the v97 compound-redirect machinery. Extract the real-fd redirect-scope core of `run_redirected` into a closure-based `with_redirect_scope` helper (making `run_redirected` a behavior-preserving one-line wrapper), then route `run_exec_single`'s function-call branch through it when the call carries a redirect.

**Tech Stack:** Rust; the existing `CompoundRedirectScope` (dup2 + restore) + `apply_out_redirect` + the capture→Terminal sink-switch.

Spec: `docs/superpowers/specs/2026-06-09-function-call-redirects-design.md`.

**Conventions:**
- Build/test: `cargo build` (debug) → `target/debug/huck`; harness uses it.
- Commit trailer EXACTLY (keep "(1M context)"): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Bash-diff harness fragments run as FILE-ARG scripts (L-27).
- Branch: `v125-function-call-redirects` (from `main` before Task 1).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/executor.rs` | Extract `with_redirect_scope` (Task 1); wire function-call branch (Task 2) | 1, 2 |
| `tests/function_redirect_integration.rs` | NEW — function-call redirects vs bash | 2 |
| `tests/scripts/function_redirect_diff_check.sh` | NEW — 48th bash-diff harness | 2 |
| `README.md`, `docs/bash-divergences.md` | harness 47→48; delete M-117 | 3 |

---

### Task 1: Extract `with_redirect_scope` (behavior-preserving refactor)

**Files:**
- Modify: `src/executor.rs` — `run_redirected` (`:498-597`); it has ONE caller (`:441`, the `Command::Redirected` arm).

Context: `run_redirected(inner, stdin, stdout, stderr, shell, sink)` flushes stdout, builds a `CompoundRedirectScope`, applies the stdin redirect, then `apply_out_redirect` for stdout/stderr, then runs the inner command — forcing a `Terminal` inner sink when a stdout redirect is present (so the redirect wins over an outer `$()` capture), then flushes + drops the scope. We generalize "runs the inner command" into a caller-supplied closure so a function call can reuse the exact same scope logic.

- [ ] **Step 1: Add the closure-based helper**

In `src/executor.rs`, directly ABOVE the current `fn run_redirected(` (`:498`), add `with_redirect_scope`. Its body is the CURRENT `run_redirected` body **moved verbatim**, with only the inner-run line changed. Concretely:

```rust
/// Applies stdin/stdout/stderr redirects at the real-fd level (saved/restored
/// via `CompoundRedirectScope`), forcing a `Terminal` inner sink when a stdout
/// redirect is present so the redirect wins over an outer capture, then runs
/// `run_inner(shell, inner_sink)` and returns its status. A redirect-open
/// failure prints `huck: <target>: <err>` and returns `Continue(1)` WITHOUT
/// running `run_inner`. (Shared by `run_redirected` for compounds and by the
/// function-call branch.)
fn with_redirect_scope<F>(
    stdin: &Option<Redirect>,
    stdout: &Option<Redirect>,
    stderr: &Option<Redirect>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    run_inner: F,
) -> ExecOutcome
where
    F: FnOnce(&mut Shell, &mut StdoutSink) -> ExecOutcome,
{
    use std::os::unix::io::IntoRawFd;

    // Flush buffered terminal/builtin output BEFORE swapping fds so prior
    // output is not diverted into the redirect target.
    let _ = io::stdout().flush();

    let mut scope = CompoundRedirectScope::new();

    // --- stdin (fd 0) ---  [MOVE the existing block from run_redirected verbatim]
    // --- stdout (fd 1) --- [MOVE verbatim: apply_out_redirect onto STDOUT_FILENO]
    // --- stderr (fd 2) --- [MOVE verbatim: apply_out_redirect onto STDERR_FILENO]

    let mut terminal_sink = StdoutSink::Terminal;
    let inner_sink: &mut StdoutSink = if stdout.is_some() {
        &mut terminal_sink
    } else {
        sink
    };
    let outcome = run_inner(shell, inner_sink);
    let _ = io::stdout().flush();
    drop(scope);
    outcome
}
```
Move the three redirect-application blocks (the stdin `match`, the stdout `if let Some(r) = stdout { apply_out_redirect(...) }`, the stderr equivalent) from the old `run_redirected` body verbatim into the marked spots. The ONLY logic change versus the original is `run_command(inner, shell, inner_sink)` → `run_inner(shell, inner_sink)`.

- [ ] **Step 2: Make `run_redirected` a thin wrapper**

Replace the ENTIRE old `run_redirected` body with:
```rust
fn run_redirected(
    inner: &Command,
    stdin: &Option<Redirect>,
    stdout: &Option<Redirect>,
    stderr: &Option<Redirect>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    with_redirect_scope(stdin, stdout, stderr, shell, sink, |shell, inner_sink| {
        run_command(inner, shell, inner_sink)
    })
}
```
The `Command::Redirected` caller at `:441` is unchanged.

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean. If the borrow checker complains that `inner` is captured by the closure while `shell` is also passed — note the closure is `FnOnce` and `inner` is an immutable `&Command` borrow that doesn't alias `shell`; this compiles (the closure borrows `inner`, the call passes `shell`/`inner_sink`). If `IntoRawFd` is now imported in both functions, drop the duplicate `use` from the wrapper (the wrapper doesn't need it).

- [ ] **Step 4: Verify compound-redirect behavior is unchanged**

Run the existing redirect-on-compound tests (find them: `grep -rl "redirect" tests/ | head` and the in-file ones):
`cargo test redirect 2>&1 | tail -15`
Then a manual spot check (compound `>file`, capture of a brace group, heredoc-on-done):
```bash
printf 'd=$(mktemp -d); { echo A; echo B; } > "$d/f"; cat "$d/f"\nx=$( { echo CAP; } ); echo "[$x]"\nwhile read l; do echo "got=$l"; done <<< "hi"\n' > /tmp/v125_c.sh
./target/debug/huck /tmp/v125_c.sh   # expect: A / B / [CAP] / got=hi
```
Expected: identical to pre-refactor (A, B, `[CAP]`, got=hi).

- [ ] **Step 5: Add a unit test pinning the refactor**

Add to the `#[cfg(test)] mod tests` in `src/executor.rs` a test that drives a compound `{ … } >file` through the public execution entrypoint and asserts the file got the output (proves `run_redirected`→`with_redirect_scope` still applies the redirect). Use whatever in-module helper the existing executor tests use to run a script string (e.g. a `run_script`/`exec_str` helper — mirror a neighboring test). If no such helper exists, SKIP the unit test and rely on Step 4 + Task 2's integration tests; note the skip in the commit.

```rust
#[test]
fn compound_redirect_still_works_after_extraction() {
    // { echo HI; } > <tmpfile>  should write HI to the file.
    let dir = std::env::temp_dir().join(format!("huck_v125_c_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("f.txt");
    let _ = std::fs::remove_file(&p);
    // <run `{ echo HI; } > p` via the in-module script-exec helper>
    // assert_eq!(std::fs::read_to_string(&p).unwrap().trim_end(), "HI");
    let _ = std::fs::remove_file(&p);
}
```
(Fill the run-helper line to match the neighboring tests; if none fits cleanly, delete this test and note the skip.)

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs
git commit -m "refactor(v125): extract with_redirect_scope from run_redirected

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Apply redirects to function-call bodies + tests

**Files:**
- Modify: `src/executor.rs` — the function-call branch in `run_exec_single` (`:2945-2946`)
- Create: `tests/function_redirect_integration.rs`
- Create: `tests/scripts/function_redirect_diff_check.sh`

Context: the function-call branch currently is:
```rust
    } else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned() {
        call_function(&resolved.program.clone(), body, resolved.args, shell, sink)
    } else if builtins::is_builtin(&resolved.program) {
```
`cmd` (the `&ExecCommand`) carries `cmd.stdin`/`cmd.stdout`/`cmd.stderr: Option<Redirect>`. This branch ignores them — the fix routes through `with_redirect_scope` when any is present.

- [ ] **Step 1: Write the failing integration test**

Create `tests/function_redirect_integration.rs` (copy the binary-invocation helper idiom from `tests/builtin_stdout_dup_integration.rs`):
```rust
//! v125 (M-117): a redirect on a function-call command applies to the body.
//! File-arg execution (L-27).

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn run_huck_frag(frag: &str) -> (String, String, i32) {
    let path = std::env::temp_dir().join(format!(
        "huck_v125_{}_{}.sh", std::process::id(), N.fetch_add(1, Ordering::SeqCst)
    ));
    let mut f = std::fs::File::create(&path).expect("create temp script");
    f.write_all(frag.as_bytes()).expect("write temp script");
    drop(f);
    let out = Command::new(env!("CARGO_BIN_EXE_huck")).arg(&path).output().expect("run huck");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn func_redirect_to_file_writes_body_output() {
    let (out, _e, _c) = run_huck_frag(
        r#"f(){ printf '%s\n' BODY; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x""#,
    );
    assert_eq!(out.trim_end(), "BODY", "{out:?}");
}

#[test]
fn func_redirect_to_stderr_not_captured() {
    let (out, _e, _c) = run_huck_frag(
        r#"f(){ printf '%s\n' BODY; }; a=$(f >&2 2>/dev/null); echo "[$a]""#,
    );
    assert_eq!(out.trim_end(), "[]", "{out:?}");
}

#[test]
fn func_2to1_captures_stderr() {
    let (out, _e, _c) = run_huck_frag(
        r#"g(){ printf '%s\n' E >&2; }; b=$(g 2>&1); echo "[$b]""#,
    );
    assert_eq!(out.trim_end(), "[E]", "{out:?}");
}

#[test]
fn func_stderr_suppressed() {
    let (_o, err, _c) = run_huck_frag(
        r#"g(){ printf '%s\n' OOPS >&2; }; g 2>/dev/null"#,
    );
    assert!(!err.contains("OOPS"), "stderr should be suppressed: {err:?}");
}

#[test]
fn func_redirect_with_inline_assignment() {
    // V=1 f >file : inline assign visible in body AND redirect applied.
    let (out, _e, _c) = run_huck_frag(
        r#"f(){ printf '%s\n' "v=$V"; }; d=$(mktemp -d); V=1 f >"$d/x"; cat "$d/x""#,
    );
    assert_eq!(out.trim_end(), "v=1", "{out:?}");
}

#[test]
fn func_body_builtin_and_external_both_redirected() {
    // Both the builtin (echo) and the external (/bin/echo) in the body go to the file.
    let (out, _e, _c) = run_huck_frag(
        r#"f(){ echo BUILTIN; command echo EXTERNAL; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x""#,
    );
    assert!(out.contains("BUILTIN") && out.contains("EXTERNAL"), "{out:?}");
}
```
Run: `cargo test --test function_redirect_integration 2>&1 | tail -20`
Expected: `func_redirect_to_file_writes_body_output`, `func_redirect_to_stderr_not_captured`, `func_2to1_captures_stderr`, `func_redirect_with_inline_assignment`, `func_body_builtin_and_external_both_redirected` FAIL (redirect ignored); `func_stderr_suppressed` may already pass or fail.

- [ ] **Step 2: Wire the function-call branch**

Replace the function-call branch (`:2945-2946`) with:
```rust
    } else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned() {
        let name = resolved.program.clone();
        let args = resolved.args;
        if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
            with_redirect_scope(&cmd.stdin, &cmd.stdout, &cmd.stderr, shell, sink,
                move |shell, inner_sink| call_function(&name, body, args, shell, inner_sink))
        } else {
            call_function(&name, body, args, shell, sink)
        }
    } else if builtins::is_builtin(&resolved.program) {
```
Notes:
- `resolved.args` is moved into `args` (it was moved into `call_function` before, so this is fine — but ensure `resolved.args` isn't used after this branch; it isn't, the branch returns).
- The `move` closure owns `name`, `body`, `args`; `shell`/`inner_sink` are the closure params.
- If the borrow checker objects to `&cmd.stdin` being borrowed while the `move` closure also runs, note `cmd` is `&ExecCommand` (not owned by the closure) and the redirect refs are passed as `with_redirect_scope` args (borrowed for the call), not captured by the closure — this composes.

- [ ] **Step 3: Build + run the integration tests**

Run: `cargo build 2>&1 | tail -3 && cargo test --test function_redirect_integration 2>&1 | tail -12`
Expected: all 6 pass.

- [ ] **Step 4: Add the 48th bash-diff harness**

Create `tests/scripts/function_redirect_diff_check.sh` (model on `tests/scripts/builtin_stdout_dup_diff_check.sh`; compare stdout only — `2>/dev/null` both sides — so the `huck:`/`bash:` error-prefix divergence is irrelevant):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v125 (M-117): redirections on a
# function-call command apply to the body. File-arg execution (L-27).
# stdout-only compare (2>/dev/null both sides).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "func >file"            'f(){ printf "%s\n" BODY; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x"'
check "func >&2 not captured" 'f(){ printf "%s\n" BODY; }; a=$(f >&2); echo "[$a]"'
check "func 2>&1 captures"    'g(){ printf "%s\n" E >&2; }; b=$(g 2>&1); echo "[$b]"'
check "func >>file append"    'f(){ printf "%s\n" L; }; d=$(mktemp -d); f >"$d/x"; f >>"$d/x"; cat "$d/x"'
check "inline-assign + redir" 'f(){ printf "%s\n" "v=$V"; }; d=$(mktemp -d); V=9 f >"$d/x"; cat "$d/x"'
check "builtin+external body" 'f(){ echo B; command echo X; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x"'
check "func <herestring"      'r(){ read a; echo "got=$a"; }; r <<< "hi"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
Run: `chmod +x tests/scripts/function_redirect_diff_check.sh && cargo build 2>&1|tail -1 && ./tests/scripts/function_redirect_diff_check.sh`
Expected: `Total: 7, Pass: 7, Fail: 0`. If `func <herestring` reveals stdin-on-function isn't wired (the `<<<` should reach the body's `read`), that's in-scope — `with_redirect_scope` handles the stdin arm, so it should pass; if it fails, debug rather than drop it.

- [ ] **Step 5: clippy + commit**

Run: `cargo clippy --all-targets 2>&1 | tail -5` (clean).
```bash
git add src/executor.rs tests/function_redirect_integration.rs tests/scripts/function_redirect_diff_check.sh
git commit -m "feat(v125): apply redirects to function-call bodies (M-117; fixes nvm ->∞)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Docs + nvm payoff

**Files:**
- Modify: `docs/bash-divergences.md` (delete M-117, Summary Bugs 2→1)
- Modify: `README.md` (harness 47→48)

- [ ] **Step 1: Verify the nvm payoff (non-interactive — no `→ ∞`)**

```bash
cargo build 2>&1 | tail -1
printf '. "$HOME/.nvm/nvm.sh"\nnvm alias 2>/dev/null | sed -n "1,4p"\n' > /tmp/v125_nvm.sh
timeout 30 ./target/debug/huck /tmp/v125_nvm.sh
```
EXPECTED: alias lines show REAL versions (e.g. `default -> lts/* (-> v24.16.0)`), NOT `-> ∞`. Capture the output. If still `→ ∞`, report BLOCKED (do not commit).

- [ ] **Step 2: Verify the nvm ls payoff (interactive PTY)**

Build release (`cargo build --release 2>&1 | tail -1`) and run a python PTY harness (huck has no `-i`; tty stdin = interactive) that sources `~/.nvm/nvm.sh` and runs `nvm ls`, confirming it completes AND the alias section shows real versions (no `→ ∞`). (Reuse the harness pattern from v124's Task 3: `pty.fork` → `os.execv(HUCK,[HUCK])` → send `. "$HOME/.nvm/nvm.sh"; echo SRC_OK`, drain, send `nvm ls; echo LS_DONE`, drain ~20s, assert `LS_DONE` present and output has no `→ ∞`.) Do NOT source the user's `~/.bashrc` (PG* creds). Capture the alias lines.

- [ ] **Step 3: Delete M-117 from the divergences doc**

In `docs/bash-divergences.md`: delete the entire `### M-117: …` entry from Tier 1. In the Summary table, change `| Bugs (Tier 1) | 2 | Open bugs to fix (M-114, M-117). |` to `| Bugs (Tier 1) | 1 | Open bug to fix (M-114). |`. Verify: `grep -n "M-117" docs/bash-divergences.md` → nothing.

- [ ] **Step 4: Bump the README harness count**

In `README.md`, change "**47 bash-diff harnesses**" → "**48 bash-diff harnesses**". Verify: `grep -n "bash-diff harness" README.md`.

- [ ] **Step 5: Full regression sanity**

```bash
cargo test 2>&1 | grep -E "test result: FAILED|error\[" | head    # none
cargo clippy --all-targets 2>&1 | tail -3                         # clean
for h in tests/scripts/*_diff_check.sh; do bash "$h" >/dev/null 2>&1 || echo "FAIL $h"; done  # silent
```

- [ ] **Step 6: Commit**

```bash
git add README.md docs/bash-divergences.md
git commit -m "docs(v125): drop M-117; harness count 48

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` + `cargo clippy --all-targets` clean.
- [ ] `cargo test 2>&1 | grep -E "test result: FAILED|error\["` → none.
- [ ] all 48 harnesses pass.
- [ ] compound-redirect + v124 subshell/builtin PTY suites green.
- [ ] `nvm alias` / `nvm ls`: real versions, no `→ ∞`.

## Self-review notes (plan author)
- **Spec coverage:** extraction → Task 1 (with the behavior-preserving wrapper + compound regression check); function-branch wire-in → Task 2 (gated on any redirect, fast path otherwise, inline-assign preserved, builtin+external body); tests (integration + 48th harness) → Task 2; docs + payoff → Task 3.
- **Type consistency:** `with_redirect_scope<F: FnOnce(&mut Shell, &mut StdoutSink) -> ExecOutcome>(stdin, stdout, stderr, shell, sink, run_inner)`; `run_redirected` wrapper signature unchanged; `call_function(&str, Box<Command>, Vec<String>, &mut Shell, &mut StdoutSink)`.
- **Zero-regression hinges:** Task 1 is a pure extraction (the single `run_redirected` caller + compound tests prove it); Task 2's no-redirect fast path is the verbatim old `call_function(...)` call. `>&-`/`>&1`/`2>&1`/`>>`/`<` reuse the existing `apply_out_redirect`/stdin arms (verify `>&-` matches v124's `/dev/null` choice during impl).
