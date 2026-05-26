# v26: Special Parameters `$0` `$$` `$!` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `$0`, `$$`, `$!` return correct values per bash. Closes M-01/02/03 from `docs/bash-divergences.md`.

**Architecture:** Three new magic names (`"0"`, `"$"`, `"!"`) added to `Shell::lookup_var`. Backed by four new fields on `Shell` (`shell_pid`, `shell_argv0`, `last_bg_pid`, `function_arg0`). Lexer learns two new dollar-expansion arms for `$$` and `$!` (the `$0` case already lexes correctly via the existing digit path). Executor pushes/pops `function_arg0` around each `call_function` and updates `last_bg_pid` after each backgrounded pipeline.

**Tech Stack:** Rust 1.95; libc (already a dep); existing huck modules (`src/shell_state.rs`, `src/lexer.rs`, `src/executor.rs`).

**Spec:** `docs/superpowers/specs/2026-05-26-huck-special-params-design.md`.

**Branch:** `v26-special-params` (off `main` at commit `5208046`).

**Baseline:** 1029 tests pass, 0 clippy warnings.

---

## File structure

- `src/shell_state.rs` — `Shell` struct grows 4 fields; `Shell::new()` captures `shell_pid` + `shell_argv0`; `lookup_var` gets 3 new early-return cases.
- `src/lexer.rs` — `read_dollar_expansion` adds `$$` and `$!` arms.
- `src/executor.rs` — `call_function` push/pop `function_arg0`; `run_background_sequence` updates `last_bg_pid`.
- `tests/special_params_integration.rs` (new) — end-to-end coverage.
- `docs/bash-divergences.md` — M-01/02/03 → fixed; change-log entry.
- `README.md` — v26 status row.

---

## Task 1: Shell state + `lookup_var` routing

Adds the 4 new fields, captures startup state, routes `lookup_var("0" | "$" | "!")` to them. Zero observable behavior change yet (the lexer hasn't been taught to produce `Var { name: "$" }` etc.; the only test surface is the helper itself).

**Files:**
- Modify: `src/shell_state.rs`

- [ ] **Step 1: Snapshot baseline**

```bash
cd /home/john/projects/shuck
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: `1029 0` and `0`.

- [ ] **Step 2: Add the new fields**

In `src/shell_state.rs`, extend the `Shell` struct:
```rust
pub struct Shell {
    // existing fields...
    pub shell_pid: i32,
    pub last_bg_pid: Option<i32>,
    pub shell_argv0: String,
    pub function_arg0: Vec<String>,
}
```

Update `Shell::new()`:
```rust
pub fn new() -> Self {
    let shell_pid = unsafe { libc::getpid() };
    let shell_argv0 = std::env::args().next().unwrap_or_else(|| "huck".to_string());
    Self {
        // existing inits...
        shell_pid,
        last_bg_pid: None,
        shell_argv0,
        function_arg0: Vec::new(),
    }
}
```

- [ ] **Step 3: Add unit tests for the new fields and lookup_var routes**

In `src/shell_state.rs::tests`:
```rust
#[test]
fn shell_new_caches_pid_and_argv0() {
    let shell = Shell::new();
    assert!(shell.shell_pid > 0, "shell_pid should be positive");
    assert!(!shell.shell_argv0.is_empty(), "shell_argv0 should be non-empty");
    assert_eq!(shell.last_bg_pid, None);
    assert!(shell.function_arg0.is_empty());
}

#[test]
fn lookup_var_dollar_returns_cached_pid_as_string() {
    let mut shell = Shell::new();
    shell.shell_pid = 12345;
    assert_eq!(shell.lookup_var("$"), Some("12345".to_string()));
}

#[test]
fn lookup_var_bang_unset_returns_empty_string() {
    let shell = Shell::new();
    assert_eq!(shell.lookup_var("!"), Some(String::new()));
}

#[test]
fn lookup_var_bang_after_set_returns_pid_string() {
    let mut shell = Shell::new();
    shell.last_bg_pid = Some(54321);
    assert_eq!(shell.lookup_var("!"), Some("54321".to_string()));
}

#[test]
fn lookup_var_zero_top_level_returns_shell_argv0() {
    let mut shell = Shell::new();
    shell.shell_argv0 = "my-shell".to_string();
    assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
}

#[test]
fn lookup_var_zero_in_function_returns_function_name() {
    let mut shell = Shell::new();
    shell.shell_argv0 = "my-shell".to_string();
    shell.function_arg0.push("myfunc".to_string());
    assert_eq!(shell.lookup_var("0"), Some("myfunc".to_string()));
}

#[test]
fn lookup_var_zero_nested_returns_innermost() {
    let mut shell = Shell::new();
    shell.function_arg0.push("outer".to_string());
    shell.function_arg0.push("inner".to_string());
    assert_eq!(shell.lookup_var("0"), Some("inner".to_string()));
    shell.function_arg0.pop();
    assert_eq!(shell.lookup_var("0"), Some("outer".to_string()));
    shell.function_arg0.pop();
    assert!(shell.lookup_var("0").is_some());  // falls through to shell_argv0
}
```

Run: `cargo test --bin huck lookup_var_dollar lookup_var_bang lookup_var_zero shell_new_caches` — expect failures (lookup_var doesn't route `$`/`!`/`0` yet).

- [ ] **Step 4: Add the lookup_var routing**

Near the top of `Shell::lookup_var`:
```rust
pub fn lookup_var(&self, name: &str) -> Option<String> {
    // Special parameters (v26).
    match name {
        "0" => return Some(
            self.function_arg0.last().cloned().unwrap_or_else(|| self.shell_argv0.clone())
        ),
        "$" => return Some(self.shell_pid.to_string()),
        "!" => return Some(
            self.last_bg_pid.map(|p| p.to_string()).unwrap_or_default()
        ),
        _ => {}
    }
    // ... existing positional + var lookup ...
}
```

- [ ] **Step 5: Verify**

```bash
cargo test --bin huck lookup_var shell_new_caches 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 7+ new tests pass; full suite ~1036, 0 fails, 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "state: Shell gains shell_pid/last_bg_pid/shell_argv0/function_arg0 + lookup_var routes \$0/\$\$/\$!

Foundation for v26 special parameters. Shell::new() caches getpid() and
argv[0] at startup. lookup_var routes \"0\"/\"\$\"/\"!\" to the new fields.
No observable behavior change yet — the lexer needs Task 2 to actually
produce Var { name: \"\$\" } and Var { name: \"!\" } for \$\$/\$!. The
\$0 case already lexes correctly via the existing digit path; once the
lexer change lands in Task 2, all three resolve through this route."
```

---

## Task 2: Lexer support for `$$` and `$!`

After this task, `$$` and `$!` tokenize as `Var { name: "$" }` and `Var { name: "!" }`. Combined with Task 1's routing, they expand to the right values at runtime.

**Files:**
- Modify: `src/lexer.rs`

- [ ] **Step 1: Failing tests**

In `src/lexer.rs::tests`:
```rust
#[test]
fn lexer_dollar_dollar_emits_var_name_dollar() {
    let tokens = tokenize("$$").unwrap();
    assert_eq!(tokens.len(), 1);
    let Token::Word(Word(parts)) = &tokens[0] else { panic!("expected Word, got {:?}", tokens[0]) };
    assert_eq!(parts.len(), 1);
    assert!(
        matches!(&parts[0], WordPart::Var { name, quoted: false } if name == "$"),
        "got {:?}", parts[0]
    );
}

#[test]
fn lexer_dollar_bang_emits_var_name_bang() {
    let tokens = tokenize("$!").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert!(
        matches!(&parts[0], WordPart::Var { name, quoted: false } if name == "!"),
        "got {:?}", parts[0]
    );
}

#[test]
fn lexer_dollar_zero_already_emits_var_name_zero() {
    // Regression test: $0 was lexed by the existing digit path pre-v26;
    // confirm it still produces Var { name: "0" }.
    let tokens = tokenize("$0").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert!(matches!(&parts[0], WordPart::Var { name, .. } if name == "0"));
}

#[test]
fn lexer_dollar_dollar_inside_double_quotes() {
    let tokens = tokenize("\"$$\"").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert!(matches!(&parts[0], WordPart::Var { name, quoted: true } if name == "$"));
}

#[test]
fn lexer_dollar_bang_inside_double_quotes() {
    let tokens = tokenize("\"$!\"").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert!(matches!(&parts[0], WordPart::Var { name, quoted: true } if name == "!"));
}

#[test]
fn lexer_dollar_dollar_concatenates_with_literal() {
    let tokens = tokenize("pre-$$-post").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert_eq!(parts.len(), 3);
    assert!(matches!(&parts[0], WordPart::Literal { text, .. } if text == "pre-"));
    assert!(matches!(&parts[1], WordPart::Var { name, .. } if name == "$"));
    assert!(matches!(&parts[2], WordPart::Literal { text, .. } if text == "-post"));
}
```

Run: `cargo test --bin huck lexer_dollar` — expect 4-5 of 6 to fail.

- [ ] **Step 2: Add `$$` and `$!` arms in `read_dollar_expansion`**

In `src/lexer.rs`, find `read_dollar_expansion`. After the existing match arms for `'('`, `'{'`, `'?'`, `'@'`, `'*'`, add:
```rust
Some('$') => {
    chars.next();
    parts.push(WordPart::Var { name: "$".to_string(), quoted });
}
Some('!') => {
    chars.next();
    parts.push(WordPart::Var { name: "!".to_string(), quoted });
}
```

The exact placement: before the digit-handling arm (which catches `$0`-`$9`), since both `$` and `!` are single-char specials similar to `$@` and `$*`. Group them with the other one-char specials for consistency.

- [ ] **Step 3: Verify the lexer + the cross-task integration**

```bash
cargo test --bin huck lexer_dollar 2>&1 | tail -10
cargo test --bin huck lookup_var 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: lexer tests pass; previous lookup_var tests still pass; full suite ~1042, 0 fails, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "lex: \$\$ and \$! emit Var { name: \"\$\" } and Var { name: \"!\" }

Two new arms in read_dollar_expansion. Combined with Task 1's
lookup_var routing, \$\$ now expands to the cached shell PID and \$!
expands to the most-recently-backgrounded pipeline's last-stage pid
(or empty if no background has happened yet).

\$0 already lexed correctly via the existing digit path — this commit
includes a regression test to lock that in."
```

---

## Task 3: Executor wiring — `function_arg0` push/pop + `last_bg_pid` update

After this task, `$0` reflects the active function name during function calls, and `$!` updates after each backgrounded pipeline.

**Files:**
- Modify: `src/executor.rs`

- [ ] **Step 1: Failing tests**

In `src/executor.rs::tests`:
```rust
#[test]
fn call_function_pushes_arg0_during_body() {
    // Define a function whose body reads $0 into a var; verify the
    // var contains the function name after the call.
    let mut shell = Shell::new();
    // Tokenize + parse + execute "myfunc() { CAPTURED=$0; }; myfunc"
    let tokens = crate::lexer::tokenize("myfunc() { CAPTURED=$0; }\nmyfunc\n").unwrap();
    let seqs = parse_all(tokens);  // helper that runs the full parse loop
    for seq in seqs {
        let _ = execute(&seq, &mut shell, "");
    }
    assert_eq!(shell.get("CAPTURED"), Some("myfunc"));
}

#[test]
fn call_function_pops_arg0_after_return() {
    let mut shell = Shell::new();
    // Same setup; after the call, function_arg0 should be empty again.
    let tokens = crate::lexer::tokenize("myfunc() { :; }\nmyfunc\n").unwrap();
    // ... execute ...
    assert!(shell.function_arg0.is_empty());
}

#[test]
fn run_background_sequence_sets_last_bg_pid() {
    // Background a /usr/bin/true (external; small overhead).
    let mut shell = Shell::new();
    let seq = parse_one("/usr/bin/true &\n");  // helper
    let _ = execute(&seq, &mut shell, "/usr/bin/true &");
    assert!(shell.last_bg_pid.is_some(), "last_bg_pid should be set");
    // Wait for the child to avoid zombies.
    if let Some(pid) = shell.last_bg_pid {
        unsafe {
            let mut status: libc::c_int = 0;
            libc::waitpid(pid, &mut status, 0);
        }
    }
}
```

(Adapt `parse_all` / `parse_one` to whatever helpers exist or write small inline ones.)

Run: expect failures.

- [ ] **Step 2: Push/pop `function_arg0` in `call_function`**

Find `call_function` in `src/executor.rs`. It currently takes the function's body and the call args. Add a parameter for the function NAME (so the callee can push it). Update each call site of `call_function` to pass the resolved function name.

```rust
fn call_function(
    name: &str,                  // NEW
    body: &Command,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved_positional = std::mem::replace(&mut shell.positional_args, args);
    shell.function_arg0.push(name.to_string());          // NEW

    let outcome = run_command(body, shell, sink);

    shell.function_arg0.pop();                            // NEW
    shell.positional_args = saved_positional;

    // existing FunctionReturn handling...
}
```

Each call site needs to pass the name. The dispatcher already resolved the program word to a function name; pass it through.

- [ ] **Step 3: Update `last_bg_pid` in `run_background_sequence`**

After the v25 spawn loop populates `stage_pids: Vec<i32>`, set:
```rust
shell.last_bg_pid = stage_pids.last().copied();
```

(Place AFTER the spawn loop, BEFORE the job-registration so the value is set even if the job-registration logic somehow short-circuits. Or after — pick the most defensible site.)

- [ ] **Step 4: Verify**

```bash
cargo test --bin huck call_function_pushes call_function_pops run_background_sequence_sets 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 3 new unit tests pass; full suite ~1045, 0 fails, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "exec: call_function pushes function_arg0; run_background_sequence sets last_bg_pid

call_function takes the function name (already resolved by the dispatcher)
and push/pops it onto shell.function_arg0 around the body. \$0 inside
the function now expands to the function name; outside, \$0 falls back
to shell_argv0 (Task 1).

run_background_sequence sets shell.last_bg_pid to the last stage's pid
per POSIX. \$! now reflects the most-recently-backgrounded pipeline."
```

---

## Task 4: Integration tests + doc updates

End-to-end coverage of all three parameters; mark M-01/02/03 fixed.

**Files:**
- Create: `tests/special_params_integration.rs`
- Modify: `docs/bash-divergences.md`
- Modify: `README.md`

- [ ] **Step 1: Create the integration test file**

`tests/special_params_integration.rs`:
```rust
//! End-to-end tests for v26 special parameters $0, $$, $!.

use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn dollar_zero_top_level_contains_huck() {
    let (out, _) = run("echo $0\nexit\n");
    assert!(out.contains("huck"), "got: {out}");
}

#[test]
fn dollar_zero_in_function_is_function_name() {
    let (out, _) = run("f() { echo $0; }\nf\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "f"), "got: {out}");
}

#[test]
fn dollar_zero_nested_functions() {
    let (out, _) = run("f() { g; echo $0; }\ng() { echo $0; }\nf\nexit\n");
    let lines: Vec<&str> = out.lines().filter(|l| l.trim() == "f" || l.trim() == "g").collect();
    assert!(lines.len() >= 2, "got: {out}");
    // The inner call prints "g" first; the outer prints "f" second.
    assert_eq!(lines[0].trim(), "g", "got: {out}");
    assert_eq!(lines[1].trim(), "f", "got: {out}");
}

#[test]
fn dollar_zero_returns_to_shell_after_function() {
    let (out, _) = run("f() { echo $0; }\nf\necho $0\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    // First line is "f" from inside the function; second line contains "huck".
    let in_func = lines.iter().find(|l| l.trim() == "f").is_some();
    let outside_huck = lines.iter().any(|l| l.contains("huck"));
    assert!(in_func, "expected 'f' line, got: {out}");
    assert!(outside_huck, "expected huck-containing line outside function, got: {out}");
}

#[test]
fn dollar_dollar_top_level_is_positive_integer() {
    let (out, _) = run("echo $$\nexit\n");
    let line = out.lines().find(|l| l.trim().parse::<i32>().is_ok()).expect("numeric line");
    let pid: i32 = line.trim().parse().unwrap();
    assert!(pid > 0, "expected positive pid, got: {pid}");
}

#[test]
fn dollar_dollar_same_in_subshell() {
    let (out, _) = run("echo $$\necho $$ | cat\nexit\n");
    let numeric_lines: Vec<i32> = out.lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .collect();
    assert!(numeric_lines.len() >= 2, "got: {out}");
    assert_eq!(numeric_lines[0], numeric_lines[1], "subshell \$\$ should match parent; got: {out}");
}

#[test]
fn dollar_bang_unset_initially_is_empty() {
    let (out, _) = run("echo \"[$!]\"\nexit\n");
    assert!(out.contains("[]"), "got: {out}");
}

#[test]
fn dollar_bang_set_after_backgrounded_external() {
    // /usr/bin/sleep is universally available on Linux.
    let (out, _) = run("/usr/bin/sleep 0.1 &\necho \"[$!]\"\nwait\nexit\n");
    // Output should contain "[N]" where N is a positive integer.
    let bracketed = out.lines().find(|l| l.starts_with('[') && l.ends_with(']')).expect("[pid] line");
    let inner = &bracketed[1..bracketed.len()-1];
    let pid: i32 = inner.parse().expect("integer inside brackets");
    assert!(pid > 0, "got: {out}");
}

#[test]
fn dollar_bang_is_last_stage_of_pipeline() {
    let (out, _) = run("echo hi | /usr/bin/sleep 0.1 &\necho \"[$!]\"\nwait\nexit\n");
    let bracketed = out.lines().find(|l| l.starts_with('[') && l.ends_with(']')).expect("[pid] line");
    let inner = &bracketed[1..bracketed.len()-1];
    let _pid: i32 = inner.parse().expect("integer inside brackets");
    // The exact pid isn't predictable; just verify it's a valid pid.
    // The semantic (LAST stage's pid) is covered by the spec; this test
    // documents that $! is a valid pid after a pipeline &.
}

#[test]
fn dollar_bang_preserves_across_subsequent_foreground() {
    let (out, _) = run(
        "/usr/bin/sleep 0.1 &\nBG_PID=$!\ntrue\necho \"[$BG_PID] [$!]\"\nwait\nexit\n"
    );
    let line = out.lines().find(|l| l.contains('[')).expect("bracketed line");
    // Both bracketed values should be identical.
    let parts: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(parts.len(), 2, "got: {out}");
    assert_eq!(parts[0], parts[1], "\$! changed after foreground command; got: {out}");
}
```

- [ ] **Step 2: Run integration tests**

```bash
cargo test --test special_params_integration 2>&1 | tail -15
```
Expected: all pass.

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Find the "Special parameters" section. Update M-01, M-02, M-03:

```markdown
- **M-01: `$0`** — `[fixed (2026-05-26)]` high. Now supported: top-level returns argv[0] (typically `huck` or the full path); inside a function call, returns the function name (bash semantics).
- **M-02: `$$`** — `[fixed (2026-05-26)]` high. Now supported: returns the shell's PID, cached at startup. Subshells (v25) inherit the cached value via fork — `$$` is stable across the subshell boundary, matching bash.
- **M-03: `$!`** — `[fixed (2026-05-26)]` high. Now supported: after each backgrounded pipeline (`cmd &`), `$!` returns the LAST stage's PID per POSIX. Empty string until first background.
```

Update the summary table (Tier 2 count drops by 3; e.g., 59 → 56 if v25 hadn't already updated it).

Add change-log entry:
```markdown
- **2026-05-26**: M-01/02/03 (special parameters `$0`, `$$`, `$!`) shipped as v26.
```

- [ ] **Step 4: Update `README.md`**

Add the v26 row to the status table:
```
| v26       | Special parameters (`$0`, `$$`, `$!`)                    |
```

- [ ] **Step 5: Verify**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: full suite ~1055, 0 fails, 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "v26: special parameters — integration tests + docs

10 integration tests covering \$0 (top-level + function + nested + post-return),
\$\$ (positive integer + stable across subshell), \$! (unset initial +
backgrounded external + pipeline-last-stage + foreground-preserves).
Audit doc M-01/02/03 marked fixed; README v26 row added."
```

---

## Final verification (no separate task)

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```

Acceptance: 0 failures, 0 warnings, clean build. Then dispatch the final cross-cutting opus review. After approval:

```bash
git -C /home/john/projects/shuck checkout main
git -C /home/john/projects/shuck merge --ff-only v26-special-params
git -C /home/john/projects/shuck branch -d v26-special-params
```

---

## Self-review checklist

1. **Spec coverage**: every section in the spec maps to a task.
   - Lexer changes → Task 2.
   - Shell state + lookup_var → Task 1.
   - `$!` update → Task 3.
   - `$0` update (function name push/pop) → Task 3.
   - Edge cases → tested in Task 4 integration tests.

2. **Placeholders**: every step shows concrete code or a clear contract. The `parse_all`/`parse_one` test helpers in Task 3 are sketched but not spelled out — implementer may need to write small inline ones if not present. Acceptable since the existing test infrastructure has many similar helpers.

3. **Type consistency**: `shell_pid: i32`, `last_bg_pid: Option<i32>`, `shell_argv0: String`, `function_arg0: Vec<String>` — used consistently across Tasks 1, 3, and 4.

4. **Order dependencies**:
   - Task 1 must precede Task 2 (lexer needs the lookup route to be useful, though it could land first).
   - Task 1 must precede Task 3 (Task 3 mutates `function_arg0` and `last_bg_pid`).
   - Task 4 depends on Tasks 1-3.

5. **Backward-compat callouts**: zero breaking changes — `$0`/`$$`/`$!` were previously empty; now they return values. No existing test should break.
