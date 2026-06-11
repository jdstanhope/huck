# huck v141 — cmdsub + arith + backticks in prompt expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `$PS1`/`$PS2`/`$PS4`/`${var@P}` prompt expansion run `$(…)` command substitution, `$((…))` arithmetic, and `` `…` `` backticks (closing most of L-29), so oh-my-posh's `PS1='$(_omp_get_primary)'` renders.

**Architecture:** Extend `expand_prompt` (`src/prompt.rs`) with self-contained byte scanners for the three forms, reusing `lexer::tokenize`+`command::parse`+`expand::run_substitution` (cmdsub/backtick) and `arith::parse`+`arith::eval` (arith). Change its signature to `&mut Shell` and thread that through the 3 callers. The two callers that render a prompt as a side effect (REPL render, `ps4`) snapshot/restore `$?`; `${var@P}` does not.

**Tech Stack:** Rust; `src/prompt.rs`, `src/shell.rs` (REPL), `src/executor.rs` (`ps4`); reuse `lexer`/`command`/`expand`/`arith`. `Shell::last_status()`/`set_last_status` (pub) + `last_cmd_sub_status()`/`set_last_cmd_sub_status` (pub(crate)).

**Reference:** spec at `docs/superpowers/specs/2026-06-11-prompt-cmdsub-arith-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits before). Stay on `v141-prompt-cmdsub-arith`. Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** BINARY crate — `cargo test --bin huck <one filter>`, `cargo test --test <name>`, `cargo clippy --all-targets` (NOT `--lib`). Builds take minutes.

---

### Task 1: Change `expand_prompt` to `&mut Shell` (mechanical; no behavior change)

**Files:**
- Modify: `src/prompt.rs` (signature + ~12 test call-sites)
- Modify: `src/executor.rs` (`ps4` → `&mut Shell`; hoist its 3 call-sites)
- Modify: `src/shell.rs` (REPL render: `borrow_mut`)
- (`src/param_expansion.rs` `@P` site needs NO change — it already passes `shell: &mut Shell`.)

- [ ] **Step 1: Change the signature** — `src/prompt.rs` line ~13:
```rust
pub fn expand_prompt(template: &str, shell: &mut Shell) -> String {
```

- [ ] **Step 2: Build to see every breakage**

Run: `cargo build 2>&1 | tail -40`
Expected: errors at the test call-sites (`&shell` → need `&mut`), `ps4` (passes `shell` to `expand_prompt` — `ps4` itself takes `&Shell`), and the REPL render (`cell.borrow()` immutable). Record the list.

- [ ] **Step 3: Fix `ps4`** — `src/executor.rs` ~2914. Change its signature and hoist its 3 call-sites:
```rust
fn ps4(shell: &mut Shell) -> String {
```
At the three call-sites, change `ps4(shell)` to a hoisted local (avoids any borrow-in-`format!` issue and is uniform):
- ~2801: replace `xtrace_emit(&format!("{}{name}={}", ps4(shell), crate::param_expansion::xtrace_quote(&val)));` with:
```rust
                    let p4 = ps4(shell);
                    xtrace_emit(&format!("{p4}{name}={}", crate::param_expansion::xtrace_quote(&val)));
```
- ~3072 already does `let p4 = ps4(shell);` — leave as-is.
- ~4954: replace `xtrace_emit(&format!("{}{}", ps4(shell), xtrace_command_line(&[], &resolved.program, &resolved.args)));` with:
```rust
        let p4 = ps4(shell);
        xtrace_emit(&format!("{p4}{}", xtrace_command_line(&[], &resolved.program, &resolved.args)));
```

- [ ] **Step 4: Fix the REPL render** — `src/shell.rs` ~418-429. Change the borrow to mutable:
```rust
        let expanded = {
            let mut shell = cell.borrow_mut();
            let (var_name, default) = if pending.is_none() {
                ("PS1", DEFAULT_PS1)
            } else {
                ("PS2", DEFAULT_PS2)
            };
            let template = shell
                .lookup_var(var_name)
                .unwrap_or_else(|| default.to_string());
            crate::prompt::expand_prompt(&template, &mut shell)
        };
```

- [ ] **Step 5: Fix the prompt.rs unit-test call-sites** — every test does `let shell = Shell::new(); ... expand_prompt(X, &shell)`. Change each to `let mut shell = Shell::new(); ... expand_prompt(X, &mut shell)`. `cargo build` will flag all (~12); fix until it compiles.

- [ ] **Step 6: Build + run prompt/xtrace tests (no behavior change)**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --bin huck prompt 2>&1 | tail -10` → existing prompt tests PASS.
Run: `cargo test --bin huck xtrace 2>&1 | tail -10` (PS4/ps4 tests, if any) → PASS.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 7: Commit**

```bash
git add src/prompt.rs src/executor.rs src/shell.rs
git commit -m "$(printf 'refactor: expand_prompt takes &mut Shell (prep for prompt cmdsub/arith)\n\nNo behavior change; threads &mut through ps4 + the REPL render.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: `$((…))` arithmetic in prompt expansion

**Files:**
- Modify: `src/prompt.rs` (the `$`-branch + a scan helper + unit tests)

- [ ] **Step 1: Write the failing tests** — add to `mod tests` in `src/prompt.rs`:
```rust
#[test]
fn expand_arith_simple() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("$((40+2))", &mut shell), "42");
}
#[test]
fn expand_arith_nested_parens() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("[$(( (1+2)*3 ))]", &mut shell), "[9]");
}
#[test]
fn expand_arith_unterminated_is_literal() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("$((1+2", &mut shell), "$((1+2");
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck expand_arith 2>&1 | tail -15`
Expected: FAIL — current output is the literal `$((40+2))` etc.

- [ ] **Step 3: Add the arith scanner + branch** — `src/prompt.rs`.

Add this helper near the other module-level helpers:
```rust
/// Finds the matching `))` for a `$((…))` whose `$((` ends at `start`.
/// Returns `(body_end_exclusive, next_index_after_closing))`. Mirrors the
/// lexer's `scan_arith_block` close rule (a `)` at depth 0 followed by `)`).
fn scan_arith_close(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut k = start;
    let mut depth: i32 = 0;
    while k < bytes.len() {
        match bytes[k] {
            b'(' => depth += 1,
            b')' => {
                if depth == 0 && k + 1 < bytes.len() && bytes[k + 1] == b')' {
                    return Some((k, k + 2));
                }
                depth -= 1;
            }
            _ => {}
        }
        k += 1;
    }
    None
}
```

In `expand_prompt`, inside the `$` branch (the `else { // bytes[i] == b'$' ... }` block), at the VERY TOP (before the `if bytes[i + 1] == b'{'` check), add the `$((` case:
```rust
            // $((...)) arithmetic.
            if bytes[i + 1] == b'(' && i + 2 < bytes.len() && bytes[i + 2] == b'(' {
                match scan_arith_close(bytes, i + 3) {
                    Some((body_end, next_i)) => {
                        let body = &template[i + 3..body_end];
                        if let Ok(expr) = crate::arith::parse(body)
                            && let Ok(n) = crate::arith::eval(&expr, shell)
                        {
                            out.push_str(&n.to_string());
                        }
                        i = next_i;
                        continue;
                    }
                    None => {
                        // Unterminated: emit the rest literally.
                        out.push_str(&template[i..]);
                        break;
                    }
                }
            }
```
(Note: `i + 1` is known in-bounds here — the enclosing branch already handled `i + 1 >= bytes.len()`. The `continue` re-enters the outer `while i < bytes.len()` loop.)

- [ ] **Step 4: Run the tests + clippy**

Run: `cargo test --bin huck expand_arith 2>&1 | tail -15` → 3 PASS.
Run: `cargo test --bin huck prompt 2>&1 | tail -8` → existing prompt tests still green.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 5: Commit**

```bash
git add src/prompt.rs
git commit -m "$(printf 'feat: \$((...)) arithmetic in prompt expansion\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: `$(…)` command substitution + `` `…` `` backticks

**Files:**
- Modify: `src/prompt.rs` (two scanners + the `$(` branch + a backtick branch + unit tests)

- [ ] **Step 1: Write the failing tests** — add to `mod tests`:
```rust
#[test]
fn expand_cmdsub_simple() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("$(echo hi)", &mut shell), "hi");
}
#[test]
fn expand_cmdsub_nested_and_mixed() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("a$(echo $(echo x))b", &mut shell), "axb");
}
#[test]
fn expand_cmdsub_strips_trailing_newlines() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("$(printf 'a\\n\\n')", &mut shell), "a");
}
#[test]
fn expand_cmdsub_paren_in_quotes() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("$(echo \")\")", &mut shell), ")");
}
#[test]
fn expand_backtick() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("`echo y`", &mut shell), "y");
}
#[test]
fn expand_cmdsub_unterminated_is_literal() {
    let mut shell = Shell::new();
    assert_eq!(expand_prompt("$(echo", &mut shell), "$(echo");
}
#[test]
fn expand_cmdsub_passes_ansi_and_markers_through() {
    // \[ \] -> \x01 \x02, ANSI escape literal, cmdsub in the middle.
    let mut shell = Shell::new();
    assert_eq!(
        expand_prompt("\\[\\e[31m\\]$(echo R)\\[\\e[0m\\]", &mut shell),
        "\x01\x1b[31m\x02R\x01\x1b[0m\x02"
    );
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck expand_cmdsub expand_backtick 2>&1 | tail -20`
Expected: FAIL — literal pass-through (cmdsub) / `` ` `` not handled (backtick).

- [ ] **Step 3: Add the scanners + a shared run helper** — `src/prompt.rs`:
```rust
/// Finds the matching `)` for a `$(…)` whose `$(` ends at `start` (depth starts
/// at 1). Quote-aware so a `)` inside `'…'`/`"…"` does not close early.
/// Returns `(body_end_exclusive, next_index_after_close)`.
fn scan_cmdsub_close(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut k = start;
    let mut depth: i32 = 1;
    let mut in_single = false;
    let mut in_double = false;
    while k < bytes.len() {
        let b = bytes[k];
        if in_single {
            if b == b'\'' { in_single = false; }
        } else if in_double {
            if b == b'"' { in_double = false; }
        } else {
            match b {
                b'\'' => in_single = true,
                b'"' => in_double = true,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((k, k + 1));
                    }
                }
                _ => {}
            }
        }
        k += 1;
    }
    None
}

/// Finds the next unescaped backtick after `start`. Returns its index.
fn scan_backtick_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut k = start;
    while k < bytes.len() {
        match bytes[k] {
            b'\\' => k += 2,
            b'`' => return Some(k),
            _ => k += 1,
        }
    }
    None
}

/// Parses `body` as a command and runs it as a command substitution, returning
/// its output (trailing newlines already stripped by `run_substitution`). On a
/// lex/parse error returns an empty string.
fn run_prompt_cmdsub(body: &str, shell: &mut Shell) -> String {
    match crate::lexer::tokenize(body) {
        Ok(toks) => match crate::command::parse(toks) {
            Ok(Some(seq)) => crate::expand::run_substitution(&seq, shell),
            _ => String::new(),
        },
        Err(_) => String::new(),
    }
}
```

In `expand_prompt`'s `$` branch, AFTER the `$((` arith block from Task 2 and BEFORE `if bytes[i + 1] == b'{'`, add the `$(` cmdsub case:
```rust
            // $(...) command substitution.
            if bytes[i + 1] == b'(' {
                match scan_cmdsub_close(bytes, i + 2) {
                    Some((body_end, next_i)) => {
                        let body = template[i + 2..body_end].to_string();
                        out.push_str(&run_prompt_cmdsub(&body, shell));
                        i = next_i;
                        continue;
                    }
                    None => {
                        out.push_str(&template[i..]);
                        break;
                    }
                }
            }
```

Add a backtick branch to the OUTER loop. First extend the fast-path terminator (line ~20) to also stop on `` ` ``:
```rust
        while j < bytes.len() && bytes[j] != b'\\' && bytes[j] != b'$' && bytes[j] != b'`' {
            j += 1;
        }
```
Then, in the dispatch after the fast-path (where it currently does `if bytes[i] == b'\\' { … } else { /* $ */ … }`), make it a three-way: add a `` ` `` arm. Change the structure to:
```rust
        if bytes[i] == b'`' {
            match scan_backtick_close(bytes, i + 1) {
                Some(close) => {
                    let body = template[i + 1..close].to_string();
                    out.push_str(&run_prompt_cmdsub(&body, shell));
                    i = close + 1;
                }
                None => {
                    out.push_str(&template[i..]);
                    break;
                }
            }
        } else if bytes[i] == b'\\' {
            // ... existing escape handling unchanged ...
        } else {
            // ... existing $ handling (now incl. the Task-2 $(( and the $( above) ...
        }
```
(Keep the existing `\\` and `$` blocks verbatim; just add the `` ` `` arm in front.)

- [ ] **Step 4: Run the tests + clippy**

Run: `cargo test --bin huck expand_cmdsub expand_backtick 2>&1 | tail -20` → all PASS.
Run: `cargo test --bin huck prompt expand_arith 2>&1 | tail -10` → all green.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 5: Commit**

```bash
git add src/prompt.rs
git commit -m "$(printf 'feat: \$(...) command substitution + backticks in prompt expansion\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Preserve `$?` across the render / ps4 prompt expansion

**Files:**
- Modify: `src/executor.rs` (`ps4` snapshot/restore + a unit test)
- Modify: `src/shell.rs` (REPL render snapshot/restore)

- [ ] **Step 1: Write the failing test** — add to the `#[cfg(test)] mod` in `src/executor.rs` (find one that constructs a `Shell` and can call `ps4`; `ps4` is private to the module). If no such module imports `ps4`, add the test in the same file's test module:
```rust
#[test]
fn ps4_cmdsub_preserves_last_status() {
    let mut shell = Shell::new();
    shell.set_last_status(7);
    shell.set("PS4", "$(false)+ ".to_string());
    let _ = ps4(&mut shell);
    assert_eq!(shell.last_status(), 7, "rendering PS4 must not clobber $?");
}
```
(`Shell::set(name, String)` exists; `false` exits 1 — without the snapshot, `run_substitution` would leave `last_status` = 1.)

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck ps4_cmdsub_preserves 2>&1 | tail -12`
Expected: FAIL — `last_status` is 1 (the `$(false)` clobbered it).

- [ ] **Step 3: Snapshot/restore in `ps4`** — `src/executor.rs` ~2914:
```rust
fn ps4(shell: &mut Shell) -> String {
    let raw = shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string());
    // Rendering a prompt must be transparent to $? (bash saves/restores it).
    let saved_status = shell.last_status();
    let saved_cmd_sub = shell.last_cmd_sub_status();
    let expanded = crate::prompt::expand_prompt(&raw, shell);
    shell.set_last_status(saved_status);
    shell.set_last_cmd_sub_status(saved_cmd_sub);
    // ... the rest of ps4 (first-char repeat etc.) UNCHANGED, using `expanded` ...
```

- [ ] **Step 4: Snapshot/restore in the REPL render** — `src/shell.rs` (the block from Task 1 Step 4). Wrap the `expand_prompt` call:
```rust
        let expanded = {
            let mut shell = cell.borrow_mut();
            let (var_name, default) = if pending.is_none() {
                ("PS1", DEFAULT_PS1)
            } else {
                ("PS2", DEFAULT_PS2)
            };
            let template = shell
                .lookup_var(var_name)
                .unwrap_or_else(|| default.to_string());
            let saved_status = shell.last_status();
            let saved_cmd_sub = shell.last_cmd_sub_status();
            let s = crate::prompt::expand_prompt(&template, &mut shell);
            shell.set_last_status(saved_status);
            shell.set_last_cmd_sub_status(saved_cmd_sub);
            s
        };
```

- [ ] **Step 5: Run the test + prompt suite + clippy**

Run: `cargo test --bin huck ps4_cmdsub_preserves 2>&1 | tail -8` → PASS.
Run: `cargo test --bin huck prompt expand_ 2>&1 | tail -8` → green.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs src/shell.rs
git commit -m "$(printf 'fix: preserve \$? across prompt rendering (PS1/PS2/PS4 cmdsub)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Bash-diff harness (the 61st)

**Files:**
- Create: `tests/scripts/prompt_expansion_diff_check.sh`

- [ ] **Step 1: Write the harness** — create `tests/scripts/prompt_expansion_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v141: cmdsub/arith/backtick in prompt
# expansion, exercised via ${var@P} (the prompt expander) through `-c`.
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
check "cmdsub"        'v='\''$(echo CMDSUB)'\''; echo "${v@P}"'
check "arith"         'v='\''$((6*7))'\''; echo "${v@P}"'
check "arith parens"  'v='\''$(( (1+2)*3 ))'\''; echo "${v@P}"'
check "cmdsub mid"    'v='\''pre-$(echo mid)-post'\''; echo "${v@P}"'
check "cmdsub nested" 'v='\''$(echo $(echo nested))'\''; echo "${v@P}"'
check "backtick"      'v='\''`echo bt`'\''; echo "${v@P}"'
check "cmdsub+var"    'x=VAL; v='\''[$x]$(echo Y)'\''; echo "${v@P}"'
check "trailing nl"   'v='\''$(printf "a\n\n")|'\''; echo "${v@P}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: chmod + build + run**

Run: `chmod +x tests/scripts/prompt_expansion_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/prompt_expansion_diff_check.sh`
Expected: `Total: 8, Pass: 8, Fail: 0`. If any FAILs, paste the diff and STOP (real divergence — do not weaken).

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/prompt_expansion_diff_check.sh
git commit -m "$(printf 'test: 61st bash-diff harness for prompt cmdsub/arith/backtick\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 6: Best-effort oh-my-posh PTY payoff test

**Files:**
- Create: `tests/prompt_omp_pty.rs`

- [ ] **Step 1: Write the PTY test** — FIRST read an existing PTY test for the harness + skip pattern: `ls tests/*_pty.rs`, open `tests/sigint_abort_pty.rs` (or `completion_jobcontrol_pty.rs`) and copy its expectrl/OsSession setup + graceful-skip idiom + per-read timeout. Create `tests/prompt_omp_pty.rs` that:
  - skips gracefully (return early, no fail) if `oh-my-posh` is not on PATH (`which`/`Command::new("oh-my-posh").arg("version")` fails) OR a PTY can't be allocated;
  - spawns interactive huck in a PTY;
  - sources the oh-my-posh init: send `eval "$(oh-my-posh init bash)"\r` (or, equivalently, `source <(oh-my-posh init bash)\r` if process substitution is unsupported, use a tempfile: `oh-my-posh init bash > /tmp/omp.$$; source /tmp/omp.$$`); the test can pre-resolve the init path on the Rust side and send a `source <path>` line;
  - sends `echo READY_$((6*7))\r` and waits to confirm the shell is alive and the marker `READY_42` appears (proves arith-in-`-c`/normal exec works and the shell didn't wedge);
  - sends an empty line to trigger a fresh prompt and asserts the captured output contains an ANSI escape byte (`\x1b[`) — i.e. PS1 RENDERED rather than printing the literal `$(_omp_get_primary)`. Assert the literal string `_omp_get_primary` does NOT appear in the rendered prompt.
  - uses an 8s per-read timeout; `drop(session)` before any panic so a wedged child is killed.

Keep assertions robust: match on the `\x1b[` substring and the absence of `_omp_get_primary`, not exact prompt text.

- [ ] **Step 2: Run**

Run: `cargo test --test prompt_omp_pty 2>&1 | tail -20`
Expected: PASS (oh-my-posh is on PATH on the dev box) or graceful SKIP — never a hang/FAIL. If it hangs, that's a real bug; investigate, do not mask with longer timeouts. If after genuine effort the PTY test can't be made reliable, report DONE_WITH_CONCERNS describing what happens (the deterministic `@P` harness + unit tests are the core guarantee).

- [ ] **Step 3: Commit**

```bash
git add tests/prompt_omp_pty.rs
git commit -m "$(printf 'test: best-effort PTY payoff — oh-my-posh prompt renders (not literal)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 7: Docs — narrow L-29 to `$LINENO`

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Update the L-29 entry**

Find the `L-29` bullet (`grep -n "L-29" docs/bash-divergences.md`). It currently says command substitution / arithmetic / `$LINENO` are not expanded in `$PS4` (and prompts). Rewrite it to reflect that **v141 added cmdsub/arith/backticks** to prompt expansion, so the only remaining gap is `$LINENO` (plus the PS4-self-assign-timing note). Keep it a `[deferred]`/`low` entry, retitled e.g. `L-29: $LINENO not expanded in prompts/$PS4`. Do NOT change the Tier-4 count (L-29 stays one entry, just narrowed).

- [ ] **Step 2: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: narrow L-29 to \$LINENO (cmdsub/arith now expand in prompts, v141)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 8: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL pass (baseline after v140 was 3068 tests; v141 adds ~9 prompt unit + 1 ps4 unit + harness/PTY). Zero failures. Paste any failure.

- [ ] **Step 2: Prompt / xtrace suites explicitly (the paths v141 touches)**

Run: `cargo test --bin huck prompt expand_ 2>&1 | tail -10`
Run: `cargo test --bin huck xtrace 2>&1 | tail -10` (PS4 path — must be unchanged for non-cmdsub PS4).
Run: `cargo test --test prompt_omp_pty 2>&1 | tail -8` (pass or graceful skip).
Run: `cargo test --test pty_interactive 2>&1 | tail -8` (REPL render path — must not regress).

- [ ] **Step 3: All bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" | tail -1; done`
Expected: every harness ends with `Fail: 0` (incl. the new `prompt_expansion_diff_check.sh` → `Pass: 8`).

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -8`
Expected: clean.

- [ ] **Step 5: Manual payoff (recommended)**

Build release: `cargo build --release 2>&1 | tail -2`. Confirm a cmdsub in a prompt renders:
Run: `target/release/huck -c 'v="$(echo RENDERED)"; echo "${v@P}"'` → prints `RENDERED`.
And (interactively, or describe to the controller): in an interactive huck, `eval "$(oh-my-posh init bash)"` then observe the prompt renders the powerline output rather than the literal `$(_omp_get_primary)`.

- [ ] **Step 6: Commit (only if a verification-driven fix was needed)**

If Steps 1-4 surfaced a real issue, make the SMALLEST fix, re-run, commit with the trailer. Otherwise no commit — verification only.

---

## Notes for the implementer
- **`$((` is checked before `$(`** (arith vs cmdsub disambiguation) — bash treats `$((` as arithmetic.
- **Only the explicit `$(`/`$((`/backtick forms are interpreted**; all other prompt bytes (ANSI, `\x01`/`\x02` markers, glyphs, metacharacters as data) pass through untouched. Do NOT re-lex the whole prompt.
- **`run_substitution` already strips trailing newlines** — do not strip again.
- **`$?` is preserved by the RENDER and `ps4` callers only**, never inside `expand_prompt` itself (so `${var@P}` keeps bash's in-command expansion semantics).
- **Use here-strings/`-c`, not pipes** in tests where relevant.
- **`Shell::last_cmd_sub_status()`/`set_last_cmd_sub_status` are `pub(crate)`** — usable from `executor.rs`/`shell.rs` (same crate).
