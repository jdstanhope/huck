# Zero-error `~/.bashrc` (M-90 / export -a / `${arr[@]±word}`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clear the last 6 leaked errors when sourcing the user's `~/.bashrc` (all from mise) — three independent gaps: M-90 (builtin stderr honors `2>`), `export -a`, and `${arr[@]±word}` array set/unset modifier.

**Architecture:** Three isolated fixes in three files: `src/executor.rs` (M-90 fd guard), `src/builtins.rs` (export flag prelude), `src/expand.rs` (array modifier arms). No parser/AST change.

**Tech Stack:** Rust. Tests: `cargo test --bin huck`, `cargo test --test <name>`, `bash tests/scripts/*_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-08-bashrc-zero-errors-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- Stdin guard model: `BuiltinStdinGuard` / `prepare_builtin_stdin` `src/executor.rs:~2200-2259`; `StageFiles { stdin, stdout, stderr }` `~2194`; `open_stage_files` opens `files.stderr` `~2339`; builtin arm `~2659`; control-builtin arm `~2643`.
- `builtin_export_decl` `src/builtins.rs:~1156-1238` (per-arg loop `~1176`, reject `~1195`); `builtin_local_decl` flag-loop model `~1244`.
- `expand_array_param` match `src/expand.rs:~537`, catch-all reject `~639`; `collect_values` `~518`; no-modifier all-elements `(PM::None, SK::All)` `~539`; `expand_assoc_param` catch-all `~391`.

ALWAYS verify expected outputs against the SYSTEM bash first (the harness is byte-diff).

---

## Task 1: M-90 — builtin error output honors `2>`

**Files:** `src/executor.rs`, tests.

- [ ] **Step 1: Failing integration tests**

Create `tests/bashrc_zero_errors_integration.rs` (copy the `run(script)->(stdout,stderr,code)` helper from `tests/set_x_integration.rs`):
```rust
#[test]
fn builtin_stderr_2_devnull_suppressed() {
    // declare -p of an unset var errors to stderr; 2>/dev/null must suppress it.
    let (out, err, _c) = run("declare -p NOPE_VAR 2>/dev/null\necho after\n");
    assert_eq!(out, "after\n");
    assert!(!err.contains("NOPE_VAR"), "stderr leaked: {err}");
}
#[test]
fn builtin_stderr_2_file_captured() {
    let (out, _e, _c) = run("declare -p NOPE2 2>/tmp/huck_m90_$$.err; echo \"got=$(cat /tmp/huck_m90_$$.err); rm -f /tmp/huck_m90_$$.err\"\n");
    assert!(out.contains("NOPE2"), "stderr not captured to file: {out}");
}
#[test]
fn builtin_stderr_unredirected_still_reaches_stderr() {
    let (_o, err, _c) = run("declare -p NOPE3\n");
    assert!(err.contains("NOPE3"), "stderr should still appear when unredirected: {err}");
}
```
Verify the expected values against bash first (`bash -c 'declare -p NOPE 2>/dev/null'` → nothing; without redirect → stderr message). Run `cargo test --test bashrc_zero_errors_integration builtin_stderr 2>&1 | tail` → `builtin_stderr_2_devnull_suppressed` + `_2_file_captured` FAIL (leak), `_unredirected` passes.

- [ ] **Step 2: Add `BuiltinStderrGuard` + `prepare_builtin_stderr`**

In `src/executor.rs`, near `BuiltinStdinGuard`/`prepare_builtin_stdin` (~2200-2259), add the stderr analog:
```rust
/// RAII guard that restores STDERR_FILENO from a saved dup'd fd on drop.
/// Used to apply a `2>`/`2>>` redirect around an in-process builtin so its
/// `eprintln!` output lands in the redirect target (M-90).
struct BuiltinStderrGuard { saved_fd: RawFd }
impl Drop for BuiltinStderrGuard {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved_fd, libc::STDERR_FILENO);
            libc::close(self.saved_fd);
        }
    }
}

/// If `stderr` is Some, save fd 2, dup2 the target onto fd 2, and return a guard
/// that restores it on drop. The builtin's `eprintln!`/`eprint!` (libc fd 2) is
/// then redirected for the call's duration.
fn prepare_builtin_stderr(stderr: Option<File>) -> Option<BuiltinStderrGuard> {
    use std::os::unix::io::AsRawFd;
    let f = stderr?;
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved < 0 { return None; }
        // Flush Rust's stderr buffer (if any) before swapping the fd.
        let _ = std::io::Write::flush(&mut std::io::stderr());
        libc::dup2(f.as_raw_fd(), libc::STDERR_FILENO);
        // `f` is dropped here, closing its fd — but the dup2'd fd 2 keeps the
        // file open until the guard restores. (Match prepare_builtin_stdin's
        // handling of the File lifetime exactly — if it keeps the File alive,
        // do the same here; the key is fd 2 points at the file during the call.)
        Some(BuiltinStderrGuard { saved })
    }
}
```
IMPORTANT: mirror `prepare_builtin_stdin`'s exact handling of the `File`'s
lifetime (whether it `into_raw_fd()`s and closes later, or keeps the `File`). The
target fd must stay valid on fd 2 until the guard restores. Read
`prepare_builtin_stdin` and copy its lifetime discipline precisely.

- [ ] **Step 3: Apply the guard in both builtin arms**

In `run_exec_single`'s regular builtin arm (`~2659`) and the control-builtin arm
(`~2643`), after `open_stage_files` and alongside the existing
`prepare_builtin_stdin(...)` call, add:
```rust
        let _stderr_guard = prepare_builtin_stderr(files.stderr.take());
```
(Take `files.stderr` so it isn't double-handled; hold `_stderr_guard` for the full
duration of the `run_builtin`/control-builtin call, dropping AFTER it returns so
fd 2 is restored. Place it so its scope wraps the builtin call exactly like the
stdin guard.) Flush stderr before dropping the guard if the builtin buffers.

- [ ] **Step 4: `2>&1` for builtins (best-effort, verify vs bash)**

`2>&1` (`Redirect::Dup{fd:2,source:1}`) currently resolves to `stderr: None` and is
only dup2'd in `run_subprocess`. For the in-process builtin path, handle it: when
the command's resolved stderr is a Dup to fd 1, the guard should `dup2(1, 2)` (so
stderr follows the current stdout). Apply redirects in SOURCE ORDER vs the stdout
setup. Test `declare -p NOPE 2>&1 | grep NOPE` → matches bash. If the
stdout-writer-vs-fd split makes exact `{ builtin; } 2>&1` parity hard, scope to the
fd-level `cmd 2>&1` case and add a test for what works; note the residual for the
docs L-note. Do NOT block the file-redirect fix on this.

- [ ] **Step 5: Run tests + clippy**
- `cargo test --test bashrc_zero_errors_integration builtin_stderr 2>&1 | tail` → pass.
- `cargo test --bin huck 2>&1 | tail -3` and `cargo test 2>&1 | tail -10` → no regression (builtins WITHOUT redirects still error to stderr; external-command redirects unchanged; capture-mode `$()` unchanged).
- `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**
```bash
git add src/executor.rs tests/bashrc_zero_errors_integration.rs
git commit -m "fix: builtin error output honors 2> / 2>> redirection (M-90)

open_stage_files opened files.stderr but the in-process builtin arms never applied
it, so builtins' eprintln! always hit the real fd 2 (e.g. declare -p UNSET
2>/dev/null leaked). New BuiltinStderrGuard / prepare_builtin_stderr mirrors the
stdin guard: dup2 the 2>file/2>>file target onto fd 2 around the builtin call and
restore on drop. Applied in both builtin arms; 2>&1 handled best-effort.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `export -a` (M-89)

**Files:** `src/builtins.rs`, tests.

- [ ] **Step 1: Failing tests**

First verify bash on THIS system: `bash -c 'export -a FOO=bar; declare -p FOO; echo rc=$?'` and `bash -c 'export -a chpwd_functions; echo rc=$?'`. Match those. Add to `tests/bashrc_zero_errors_integration.rs`:
```rust
#[test]
fn export_a_flag_accepted() {
    // mise shape: export -a NAME (no value) — must not error.
    let (_o, err, c) = run("export -a chpwd_functions\necho rc=$?\n");
    assert!(!err.contains("not a valid identifier"), "export -a leaked: {err}");
    assert_eq!(c, 0);
}
#[test]
fn export_a_with_assignment_exports() {
    let (out, _e, _c) = run("export -a FOO=bar\ndeclare -p FOO\n");
    assert!(out.contains("FOO") && out.contains("bar"), "{out}");
}
```
(If `bash -c 'export -a'` on this system actually ERRORS with `invalid option`, change the assertions to match bash exactly and report it — the goal is huck==bash.) Run `cargo test --test bashrc_zero_errors_integration export_a 2>&1 | tail` → FAIL.

- [ ] **Step 2: Add the leading-flag prelude to `builtin_export_decl`**

In `builtin_export_decl` (`src/builtins.rs:~1156`), before the per-arg name/assign
loop (`~1176`), consume leading flag args (model on `builtin_local_decl`'s flag
loop ~1244). Accept `-a` (no-op), and tolerate `-p`/`-n`/`-f`/`--` if trivially
done (else just `-a`):
```rust
    let mut rest_start = 0;
    for (i, arg) in args.iter().enumerate() {
        if let DeclArg::Plain(s) = arg {
            if s == "--" { rest_start = i + 1; break; }
            if s.starts_with('-') && s.len() > 1 && s[1..].chars().all(|c| "apnf".contains(c)) {
                // -a (allexport, no-op here), and -p/-n/-f tolerated; clustered ok.
                rest_start = i + 1;
                continue;
            }
        }
        rest_start = i;
        break;
    }
    let names = &args[rest_start..];
```
Then run the existing name/assign loop over `names` instead of `args`. `-a` does
nothing beyond being consumed. (Adapt to the real `DeclArg` variants and the
function's actual arg type — read it first; `builtin_local_decl` shows the exact
pattern for parsing `-a`/`-A` from `DeclArg::Plain`.)

- [ ] **Step 3: Run tests + clippy**
- `cargo test --test bashrc_zero_errors_integration export_a 2>&1 | tail` → pass.
- `cargo test --bin huck 2>&1 | tail -3` → no regression (existing export tests: `export NAME=v`, bare `export`, `export -p` if supported, readonly+export).
- clippy clean.

- [ ] **Step 4: Commit**
```bash
git add src/builtins.rs tests/bashrc_zero_errors_integration.rs
git commit -m "feat: export -a (accept the flag, no-op like bash) (M-89)

builtin_export_decl had no flag parsing, so export -a NAME errored \"-a: not a
valid identifier\". Add a leading-flag prelude (mirrors builtin_local_decl) that
consumes -a (no-op) and continues exporting the names. Clears mise's
export -a chpwd_functions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `${arr[@]±word}` array set/unset modifier (M-82)

**Files:** `src/expand.rs`, tests.

- [ ] **Step 1: Failing tests**

Verify vs bash first. Add to `tests/bashrc_zero_errors_integration.rs`:
```rust
#[test]
fn array_all_plus_modifier() {
    assert_eq!(run("a=(x y z); echo \"${a[@]+SET}\"\n").0, "SET\n");
    assert_eq!(run("echo \"[${b[@]+SET}]\"\n").0, "[]\n"); // unset
    assert_eq!(run("e=(); echo \"[${e[@]+SET}]\"\n").0, "[]\n"); // empty array
}
#[test]
fn array_all_minus_modifier() {
    assert_eq!(run("a=(x y z); echo \"${a[@]-DEF}\"\n").0, "x y z\n");
    assert_eq!(run("echo \"[${b[@]-DEF}]\"\n").0, "[DEF]\n");
}
#[test]
fn array_safe_expand_idiom() {
    // ${arr[@]+"${arr[@]}"} — expand only if set (safe under set -u).
    assert_eq!(run("set -u; a=(1 2); printf '<%s>' \"${a[@]+\"${a[@]}\"}\"; echo\n").0, "<1><2>\n");
}
#[test]
fn assoc_all_plus_modifier() {
    assert_eq!(run("declare -A m=([k]=v); echo \"${m[@]+SET}\"\n").0, "SET\n");
}
```
Run `cargo test --test bashrc_zero_errors_integration array_ assoc_ 2>&1 | tail` → FAIL ("not supported on array").

- [ ] **Step 2: Add the modifier arms in `expand_array_param`**

In `src/expand.rs`, replace the catch-all reject (`~639`) so `UseAlternate`/`UseDefault`
on `SK::All`/`SK::Star` are handled (keep the reject for other modifiers):
```rust
        // ${arr[@]+word} / ${arr[@]-word} (and :+/:-): whole-array set/unset test.
        (PM::UseAlternate { word, .. }, SK::All | SK::Star) => {
            if !collect_values(shell).is_empty() {
                // array is set -> substitute `word`
                /* expand `word` like the scalar UseAlternate path does */
            } else {
                ExpansionResult::Value(String::new())
            }
        }
        (PM::UseDefault { word, .. }, SK::All | SK::Star) => {
            if !collect_values(shell).is_empty() {
                // array set -> the all-elements expansion (reuse the
                // (PM::None, SK::All/Star) result exactly: WordList for @, joined Value for *)
            } else {
                /* expand `word` like the scalar UseDefault path */
            }
        }
        // Other scalar modifiers on @/* — still unsupported (v71 scope).
        (other, SK::All | SK::Star) => { /* existing eprintln! reject */ }
```
- For substituting `word`, reuse the same word-expansion the scalar
  `expand_modifier_with_value` UseAlternate/UseDefault path uses (read the
  `(modif, SK::Index(w))` arm ~625 and the scalar `${var+word}` path) so quoting/
  splitting matches. The `word` should be expanded in the current quote context.
- For the "array set" all-elements result, reuse the EXACT logic of the existing
  `(PM::None, SK::All)` (→ `WordList(collect_values(shell))`) and `(PM::None, SK::Star)`
  (→ joined `Value`) arms — factor a tiny helper if needed so `-`-when-set returns
  identical output to a bare `${arr[@]}`.
- `set = !collect_values(shell).is_empty()` (empty array `()` → unset). Colon
  variants behave the same (non-empty array = set-and-non-null).

- [ ] **Step 3: Mirror into `expand_assoc_param`**

Apply the same two arms at the associative catch-all (`src/expand.rs:~391`), using
the assoc value collection for the set predicate.

- [ ] **Step 4: Run tests + clippy**
- `cargo test --test bashrc_zero_errors_integration array_ assoc_ 2>&1 | tail` → pass.
- `cargo test --bin huck 2>&1 | tail -3` → no regression (plain `${a[@]}`/`${a[*]}`/`${#a[@]}`/`${!a[@]}`, single-element `${a[i]±w}`, scalar `${v±w}`, the arrays/assoc integration + unit suites).
- clippy clean.

- [ ] **Step 5: Commit**
```bash
git add src/expand.rs tests/bashrc_zero_errors_integration.rs
git commit -m "feat: \${arr[@]+word} / \${arr[@]-word} whole-array set/unset modifier (M-82)

expand_array_param rejected all modifiers on [@]/[*]. Add UseAlternate (+/:+) and
UseDefault (-/:-) arms (set = non-empty array) for indexed + associative arrays,
enabling the safe-expand idiom \${arr[@]+\"\${arr[@]}\"}. Per-element modifiers stay
deferred. Clears mise's \${__MISE_FLAGS[@]+...}.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: bash-diff harness (33rd)

**Files:** Create `tests/scripts/bashrc_zero_errors_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Copy `tests/scripts/bashrc_builtin_gaps_diff_check.sh`'s structure verbatim. `cargo build` first. Fragments (verify each under `bash --norc --noprofile` first):
```
declare -p NOPE_X 2>/dev/null; echo after
declare -p NOPE_Y 2>/tmp/hk33_$$; printf 'got[%s]\n' "$(cat /tmp/hk33_$$)"; rm -f /tmp/hk33_$$
export -a FOO=bar; declare -p FOO
a=(x y z); echo "${a[@]+SET}"
unset b; echo "[${b[@]+SET}]"
a=(x y z); echo "${a[@]-DEF}"; echo "[${b[@]-DEF}]"
set -u; a=(1 2); printf '<%s>' "${a[@]+"${a[@]}"}"; echo
```
- [ ] **Step 2: Run** → `bash tests/scripts/bashrc_zero_errors_diff_check.sh 2>&1 | tail` → `Total: 7, Pass: 7, Fail: 0`. Drop+comment any fragment that legitimately diverges (e.g. a residual `2>&1`-writer edge); report it. Do NOT mask a real bug.
- [ ] **Step 3: Commit**
```bash
git add tests/scripts/bashrc_zero_errors_diff_check.sh
git commit -m "test: bash-diff harness for M-90 / export -a / \${arr[@]+word} (33rd)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read structure**
`grep -n '^## Change log\|Tier 1\|Tier 2\|Last updated\|^- \*\*M-90\|^- \*\*M-89\|^- \*\*M-82\|2026-06-08' docs/bash-divergences.md | head` and `grep -n 'v108' README.md`.

- [ ] **Step 2: Update the three entries**
- **M-90**: flip `[deferred]` → `[fixed v109]`; describe the `BuiltinStderrGuard` fd-2 dup approach (file `2>`/`2>>` primary, `2>&1` best-effort — add an `L-` note if a residual `2>&1`-writer edge remains).
- **M-89**: note `export -a` shipped v109 (no-op flag, matches bash); `-f`/other export-flag forms per what was done.
- **M-82**: note the whole-array `${arr[@]+word}`/`${arr[@]-word}` (`+`/`-`/`:+`/`:-`) modifier shipped v109 (indexed + associative); per-element modifiers still deferred.
- Bump Tier counts as appropriate; "Last updated" → 2026-06-08.

- [ ] **Step 3: Change-log + README row**
`2026-06-08` v109 change-log entry (style of v108): the three fixes, the **zero-error `~/.bashrc`** payoff (the 6 mise leaks → 0), 33rd harness, test count, any L-note. v109 README iteration row after v108.

- [ ] **Step 4: Verify + commit**
`grep -n 'fixed v109\|v109' docs/bash-divergences.md README.md` → real numbers, no placeholders.
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v109 — zero-error bashrc (M-90 / export -a / \${arr[@]+word})

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | tail -5` (green), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass).
- [ ] **Payoff smoke**: `mise activate bash > /tmp/mi.sh; printf 'source /tmp/mi.sh\n_mise_hook\necho END\n' | ./target/debug/huck 2>&1 | grep -icE 'error|not a valid|not supported|not found'` → `0` (was 6). Report it.
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files.
```
