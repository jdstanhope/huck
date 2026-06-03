# huck v82 — script-file mode + `-c` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Fresh subagent per task with spec-compliance + code-quality review between tasks.

**Goal:** Run huck non-interactively from a script file (`huck SCRIPT [args]`) or a command string (`huck -c CMD [name [args]]`), with `--` end-of-options; both reuse the existing `run_sourced_contents` engine and free stdin for `read`/`select`.

**Architecture:** `parse_cli` resolves argv into a `RunMode` (Interactive / Command / File). `run()` branches before the rustyline REPL: Command/File modes set `$0`/positionals, mark the shell non-interactive, execute the program via `run_sourced_contents`, and translate the outcome to an exit code. The Interactive path is unchanged.

**Tech Stack:** Rust 1.85+, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-03-huck-script-file-mode-design.md` (read it — has the verified bash 5.2 semantics).

**Branch:** `v82-script-mode` (create from `main` in Preamble).

**Commit trailer (every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1:** `git status && git rev-parse --abbrev-ref HEAD` → clean tree on `main`.
- [ ] **Step 2:** `git checkout -b v82-script-mode` → "Switched to a new branch".
- [ ] **Step 3:** Baseline: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "Baseline:", s}'` → expect **2334**.
- [ ] **Step 4:** `cargo clippy --all-targets 2>&1 | tail -2` → clean.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell.rs` | `RunMode` enum + `parse_cli` resolver (`-c`/`--`/operands/precedence); `run()` branch + `run_program` helper; unit tests for mode resolution | 1, 2 |
| `tests/script_mode_integration.rs` | NEW. Binary-driven integration tests (file mode, `-c`, `--`, exit codes, the read-via-file payoff) | 2 |
| `tests/scripts/script_mode_diff_check.sh` | NEW. huck's 9th bash-diff harness (`-c` fragments + a script-file fragment) | 3 |
| `docs/bash-divergences.md`, `README.md` | M-77 deferred script-mode/`-c` → fixed v82; new entry; changelog; summary stamp; README row | 3 |

---

## Task 1: `RunMode` resolution in `parse_cli` (pure CLI parsing)

Pure logic, fully unit-testable without executing anything. Do this first.

**Files:**
- Modify: `src/shell.rs` — add `RunMode`, extend `CliOptions`, rewrite `parse_cli`'s operand handling; add a `mod cli_tests` (or extend the existing parse_cli tests).

- [ ] **Step 1: Write the failing tests**

Add to `src/shell.rs` tests (there is an existing `parse_cli_positional_errors` test around line 663 — this task changes that behavior, so update/replace it). Use whatever test module `parse_cli` tests already live in:

```rust
#[test]
fn cli_no_args_is_interactive() {
    let o = parse_cli(&[]).unwrap();
    assert_eq!(o.mode, RunMode::Interactive);
}

#[test]
fn cli_file_mode_sets_path_and_args() {
    let o = parse_cli(&["s.sh".into(), "a".into(), "b".into()]).unwrap();
    assert_eq!(o.mode, RunMode::File { path: "s.sh".into(), args: vec!["a".into(), "b".into()] });
}

#[test]
fn cli_dash_c_first_operand_is_argv0() {
    let o = parse_cli(&["-c".into(), "echo hi".into(), "name".into(), "x".into()]).unwrap();
    assert_eq!(o.mode, RunMode::Command {
        command: "echo hi".into(),
        argv0: Some("name".into()),
        args: vec!["x".into()],
    });
}

#[test]
fn cli_dash_c_no_operands_argv0_none() {
    let o = parse_cli(&["-c".into(), "echo hi".into()]).unwrap();
    assert_eq!(o.mode, RunMode::Command { command: "echo hi".into(), argv0: None, args: vec![] });
}

#[test]
fn cli_dash_c_requires_argument() {
    assert!(parse_cli(&["-c".into()]).is_err());
}

#[test]
fn cli_double_dash_ends_options_for_file() {
    // `--` lets a dash-leading name be the script path.
    let o = parse_cli(&["--".into(), "-weird".into(), "a".into()]).unwrap();
    assert_eq!(o.mode, RunMode::File { path: "-weird".into(), args: vec!["a".into()] });
}

#[test]
fn cli_operands_after_c_are_verbatim_including_dashdash() {
    // After `-c CMD`, operands are taken verbatim: `--` becomes $0, `-x` becomes $1.
    let o = parse_cli(&["-c".into(), "cmd".into(), "--".into(), "-x".into()]).unwrap();
    assert_eq!(o.mode, RunMode::Command {
        command: "cmd".into(), argv0: Some("--".into()), args: vec!["-x".into()],
    });
}

#[test]
fn cli_unknown_leading_flag_errors() {
    assert!(parse_cli(&["-x".into()]).is_err());
}

#[test]
fn cli_dash_c_precedence_over_file() {
    // `-c` wins; the operand is $0, not a script path.
    let o = parse_cli(&["-c".into(), "cmd".into(), "file.sh".into()]).unwrap();
    assert_eq!(o.mode, RunMode::Command { command: "cmd".into(), argv0: Some("file.sh".into()), args: vec![] });
}

#[test]
fn cli_norc_then_file_still_parses() {
    let o = parse_cli(&["--norc".into(), "s.sh".into()]).unwrap();
    assert!(o.norc);
    assert_eq!(o.mode, RunMode::File { path: "s.sh".into(), args: vec![] });
}
```

- [ ] **Step 2: Run, expect failure**

Run: `cargo test --quiet --bin huck cli_ 2>&1 | tail -6`
Expected: compile error (`RunMode` undefined) / FAIL.

- [ ] **Step 3: Implement `RunMode` + rewrite `parse_cli`**

Add near `CliOptions` in `src/shell.rs` (add `use std::path::PathBuf;` if not present):

```rust
#[derive(Debug, PartialEq, Eq)]
enum RunMode {
    /// REPL (tty) or piped-stdin command reading — current behavior.
    Interactive,
    /// `-c COMMAND [NAME [ARG...]]`: argv0 = NAME (None → keep the shell's
    /// default $0), args = the rest.
    Command { command: String, argv0: Option<String>, args: Vec<String> },
    /// `SCRIPT [ARG...]`: $0 = path, args = the rest.
    File { path: PathBuf, args: Vec<String> },
}

struct CliOptions {
    rcfile_path: Option<PathBuf>,
    norc: bool,
    mode: RunMode,
}

fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
    let mut rcfile_path: Option<PathBuf> = None;
    let mut norc = false;
    let mut command: Option<String> = None;
    let mut i = 0;

    // Scan leading options until the first operand, `--`, or `-c`.
    while i < args.len() {
        match args[i].as_str() {
            "--norc" => {
                norc = true;
                i += 1;
            }
            "--rcfile" => {
                i += 1;
                if i >= args.len() {
                    return Err("--rcfile: requires an argument".to_string());
                }
                rcfile_path = Some(PathBuf::from(&args[i]));
                i += 1;
            }
            s if s.starts_with("--rcfile=") => {
                rcfile_path = Some(PathBuf::from(&s["--rcfile=".len()..]));
                i += 1;
            }
            "-c" => {
                i += 1;
                if i >= args.len() {
                    return Err("-c: option requires an argument".to_string());
                }
                command = Some(args[i].clone());
                i += 1;
                break; // remaining args are operands, taken verbatim
            }
            "--" => {
                i += 1;
                break;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(format!("unrecognized option: {s}"));
            }
            _ => break, // first operand (script path)
        }
    }

    let rest = &args[i..];
    let mode = if let Some(command) = command {
        RunMode::Command {
            command,
            argv0: rest.first().cloned(),
            args: rest.get(1..).map(|s| s.to_vec()).unwrap_or_default(),
        }
    } else if let Some(path) = rest.first() {
        RunMode::File {
            path: PathBuf::from(path),
            args: rest[1..].to_vec(),
        }
    } else {
        RunMode::Interactive
    };

    Ok(CliOptions { rcfile_path, norc, mode })
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test --quiet --bin huck cli_ 2>&1 | tail -8` → all pass.
(The crate is a binary, so target `--bin huck`.)

- [ ] **Step 5: Build + clippy + full suite + commit**

```bash
cargo build 2>&1 | tail -2
cargo clippy --all-targets 2>&1 | tail -2
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "After Task 1:", s}'
```
Expected: builds clean (run() still uses `opts.rcfile_path`/`opts.norc`; the new `opts.mode` is unused for now → it may warn `dead_code` on the `RunMode` variants; add `#[allow(dead_code)]` on `RunMode` for this task, removed in Task 2). Suite = 2334 + new cli tests.

```bash
git add src/shell.rs
git commit -m "$(cat <<'EOF'
v82 task 1: RunMode resolution in parse_cli (-c / -- / script operand)

parse_cli now resolves argv into RunMode { Interactive | Command | File }
instead of rejecting positionals. `-c` consumes the next arg as the command
string then takes remaining operands verbatim (first = $0/NAME, rest = $1..);
a bare operand becomes the script path; `--` ends option scanning. Unit
tests cover every mode, the -c-first-operand-is-argv0 quirk, precedence,
`--`, and the -c-missing-argument / unknown-flag errors.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `run()` branch + `run_program` helper (execution)

**Files:**
- Modify: `src/shell.rs` — branch in `run()` for Command/File modes; add `run_program`; remove the Task-1 `#[allow(dead_code)]`.
- Create: `tests/script_mode_integration.rs`.

Read `run()` (`src/shell.rs:139`+) and the exit-handling tail (≈238-280) first; reuse its `fire_exit_trap` + `hangup_jobs` cleanup pattern.

- [ ] **Step 1: Add `run_program` + branch `run()`**

Add the helper (near `run()` / `maybe_source_rc_file`):

```rust
/// Executes a non-interactive program (a `-c` string or a script file's
/// contents) and returns the process exit code. Sets $0 and the positional
/// parameters, marks the shell non-interactive (so the rc file is skipped),
/// runs the program through the shared `run_sourced_contents` engine, fires the
/// EXIT trap, and hangs up jobs. Does NOT touch interactive history or the
/// line editor.
fn run_program(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 {
    let mut shell = shell_cell.borrow_mut();
    shell.is_interactive = false;
    if let Some(a0) = argv0 {
        shell.shell_argv0 = a0;
    }
    shell.positional_args = args;

    let outcome = crate::builtins::run_sourced_contents(
        contents,
        std::path::Path::new(label),
        &mut shell,
    );
    let code = match outcome {
        ExecOutcome::Exit(n) => n,
        ExecOutcome::FunctionReturn(n) => n,
        ExecOutcome::Continue(s) => shell.take_pending_fatal_pe_error().unwrap_or(s),
        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
    };
    crate::traps::fire_exit_trap(&mut shell);
    shell.hangup_jobs();
    code
}
```

In `run()`, after `parse_cli`, after `install_job_control_signals()`, create the
shell cell + install the SIGINT/SIGCHLD handlers, THEN branch BEFORE building the
editor. Concretely, restructure the top of `run()` so the order is:

```rust
pub fn run(args: &[String]) -> i32 {
    let opts = match parse_cli(args) {
        Ok(o) => o,
        Err(e) => { eprintln!("huck: {e}"); return 2; }
    };

    install_job_control_signals();

    let shell_cell = Rc::new(RefCell::new(Shell::new()));
    {
        let shell = shell_cell.borrow();
        install_sigint_handler(Arc::clone(&shell.sigint_flag));
        install_sigchld_handler(Arc::clone(&shell.sigchld_flag));
    }

    // Non-interactive program modes bypass the REPL entirely.
    match opts.mode {
        RunMode::Command { command, argv0, args } => {
            let label = argv0.clone().unwrap_or_else(|| shell_cell.borrow().shell_argv0.clone());
            return run_program(&command, argv0, args, &label, &shell_cell);
        }
        RunMode::File { path, args } => {
            let contents = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => {
                    eprintln!("huck: {}: No such file or directory", path.display());
                    return 127;
                }
            };
            let label = path.display().to_string();
            return run_program(&contents, Some(label.clone()), args, &label, &shell_cell);
        }
        RunMode::Interactive => {}
    }

    // ----- interactive / piped-stdin REPL (unchanged below this line) -----
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut editor: Editor<HuckHelper, FileHistory> = match Editor::with_config(config) {
        // ... existing body unchanged ...
```

Move the existing editor construction, history load, helper set, rc sourcing, and
the REPL `loop` to AFTER the match (they only run in Interactive mode). The
shell-cell creation and handler installation that previously sat between the
editor build and the loop are now above the match — delete the duplicated
originals so each runs once. Remove the `#[allow(dead_code)]` added to `RunMode`
in Task 1.

- [ ] **Step 2: Build + confirm interactive path still works**

```bash
cargo build 2>&1 | tail -3
printf 'echo hi\nexit\n' | ./target/debug/huck   # piped REPL still works → hi
```
Expected: clean build; prints `hi`.

- [ ] **Step 3: Write the integration tests**

Create `tests/script_mode_integration.rs` (mirror an existing `tests/*_integration.rs` for the spawn helper):

```rust
//! Integration tests for v82 script-file mode + `-c`.
use std::io::Write;
use std::process::{Command, Stdio};

const HUCK: &str = env!("CARGO_BIN_EXE_huck");

/// Run huck with CLI args + optional stdin. Returns (stdout, stderr, exit).
fn run(args: &[&str], stdin: &str) -> (String, String, i32) {
    let mut child = Command::new(HUCK)
        .args(args)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(stdin.as_bytes()).unwrap();
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(),
     String::from_utf8_lossy(&o.stderr).into(),
     o.status.code().unwrap_or(-1))
}

fn write_script(body: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn c_mode_first_operand_is_argv0() {
    let (out, _e, c) = run(&["-c", "echo \"0=$0 1=$1 #=$#\"", "name", "a", "b"], "");
    assert_eq!(out, "0=name 1=a #=2\n");
    assert_eq!(c, 0);
}

#[test]
fn c_mode_no_operands_argv0_is_shell_name() {
    let (out, _e, _c) = run(&["-c", "echo \"1=$1 #=$#\""], "");
    assert_eq!(out, "1= #=0\n");
}

#[test]
fn c_mode_multistatement_and_exit_code() {
    let (out, _e, c) = run(&["-c", "x=5; echo $x; exit 3"], "");
    assert_eq!(out, "5\n");
    assert_eq!(c, 3);
}

#[test]
fn c_mode_multiline() {
    let (out, _e, _c) = run(&["-c", "for i in 1 2 3\ndo echo $i\ndone"], "");
    assert_eq!(out, "1\n2\n3\n");
}

#[test]
fn c_mode_empty_command_exit_zero() {
    let (out, _e, c) = run(&["-c", ""], "");
    assert_eq!(out, "");
    assert_eq!(c, 0);
}

#[test]
fn file_mode_argv0_and_positionals() {
    let f = write_script("echo \"0=$0 1=$1 #=$#\"\n");
    let path = f.path().to_str().unwrap();
    let (out, _e, c) = run(&[path, "x", "y"], "");
    assert_eq!(out, format!("0={path} 1=x #=2\n"));
    assert_eq!(c, 0);
}

#[test]
fn file_mode_multiline_and_exit() {
    let f = write_script("greet() { echo \"hi $1\"; }\ngreet world\nexit 4\n");
    let (out, _e, c) = run(&[f.path().to_str().unwrap()], "");
    assert_eq!(out, "hi world\n");
    assert_eq!(c, 4);
}

#[test]
fn file_mode_shebang_line_ignored() {
    let f = write_script("#!/usr/bin/env huck\necho ok\n");
    let (out, _e, c) = run(&[f.path().to_str().unwrap()], "");
    assert_eq!(out, "ok\n");
    assert_eq!(c, 0);
}

#[test]
fn file_mode_missing_file_exits_127() {
    let (_o, err, c) = run(&["/no/such/huck/script-xyz"], "");
    assert_eq!(c, 127);
    assert!(err.contains("No such file or directory"), "stderr: {err:?}");
}

#[test]
fn set_e_propagates_failure_exit() {
    let f = write_script("set -e\nfalse\necho nope\n");
    let (out, _e, c) = run(&[f.path().to_str().unwrap()], "");
    assert!(!out.contains("nope"), "errexit should stop before echo: {out:?}");
    assert_ne!(c, 0, "errexit should exit non-zero");
}

#[test]
fn double_dash_allows_dash_leading_script() {
    // `--` so a leading-dash path isn't treated as an option.
    let f = write_script("echo viadashdash\n");
    let (out, _e, c) = run(&["--", f.path().to_str().unwrap()], "");
    assert_eq!(out, "viadashdash\n");
    assert_eq!(c, 0);
}

#[test]
fn payoff_read_from_file_consumes_real_stdin() {
    // The M-72/L-12 win: program is the file, so `read` gets real stdin.
    let f = write_script("read x\necho \"got=$x\"\n");
    let (out, _e, _c) = run(&[f.path().to_str().unwrap()], "hello\n");
    assert_eq!(out, "got=hello\n");
}
```

- [ ] **Step 4: Run integration tests**

Run: `cargo test --test script_mode_integration 2>&1 | grep -E "^test result"`
Expected: all pass. If `set_e_propagates_failure_exit` fails, that's the spec's flagged verification point — investigate `run_sourced_contents`/`maybe_errexit`: errexit must yield a non-zero process exit in main-program mode. If `run_sourced_contents` doesn't already terminate on errexit, fix the smallest thing (e.g. ensure `process_line`'s errexit path returns `Exit(status)` that `run_sourced_contents` propagates) and note it in the commit. If `payoff_read_from_file_consumes_real_stdin` fails, confirm `read` reads fd 0 and the file path (not stdin) supplied the program.

- [ ] **Step 5: Full suite + clippy + commit**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
git add -A
git commit -m "$(cat <<'EOF'
v82 task 2: run() branch + run_program for -c and file modes

run() resolves opts.mode and, for Command/File, bypasses the rustyline REPL:
run_program sets $0 (shell_argv0) + positional_args, marks the shell
non-interactive (skips rc), runs the program via run_sourced_contents, fires
the EXIT trap, hangs up jobs, and returns the translated exit code. File mode
reads the file (missing → "No such file or directory", exit 127). Interactive
path unchanged. 12 binary-driven integration tests incl. the read-from-file
stdin payoff and errexit propagation.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/script_mode_diff_check.sh` (+x).
- Modify: `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Create the harness** (huck's 9th; mirror `tests/scripts/loop_levels_diff_check.sh` structure for the HUCK_BIN check + PASS/FAIL counting):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v82 script-file mode + `-c`.
# `-c` fragments: compare `bash -c FRAG -- ARGS` vs `huck -c FRAG -- ARGS`.
# File fragment: write a temp script, run `bash FILE ARGS` vs `huck FILE ARGS`.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check_c() { # label, then -c args (FRAG [name args...])
  local label="$1"; shift
  local b h
  b=$(bash -c "$@" 2>&1; echo "EXIT:$?")
  h=$("$HUCK_BIN" -c "$@" 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check_c "argv0 quirk"       'echo "0=$0 1=$1 #=$#"' name a b
check_c "no operands"       'echo "0=$0 1=$1 #=$#"'
check_c "multi-statement"   'x=2; y=3; echo $((x+y))'
check_c "loop"              'for i in 1 2 3; do printf "%s," "$i"; done; echo'
check_c "exit code"         'echo before; exit 7; echo after'
check_c "empty command"     ''

# File-mode fragment: identical script run by both shells.
SCRIPT=$(mktemp)
printf 'echo "0=$(basename "$0") 1=$1 #=$#"\nfor w in "$@"; do printf "[%%s]" "$w"; done; echo\nexit 2\n' > "$SCRIPT"
bo=$(cd / && bash "$SCRIPT" a b 2>&1; echo "EXIT:$?")
ho=$(cd / && "$HUCK_BIN" "$SCRIPT" a b 2>&1; echo "EXIT:$?")
# $0 is the full path (same for both); normalize via basename in the script above.
if [[ "$bo" == "$ho" ]]; then printf 'PASS: %s\n' "file mode"; PASS=$((PASS+1))
else printf 'FAIL: %s\n' "file mode"; diff <(echo "$bo") <(echo "$ho") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
rm -f "$SCRIPT"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/script_mode_diff_check.sh`.

- [ ] **Step 2: Build + run harness; iterate to all-pass**

```bash
cargo build --quiet
tests/scripts/script_mode_diff_check.sh
```
Expected: `Total: 7, Pass: 7, Fail: 0`. If the file-mode `$0` differs, the script uses `basename "$0"` so both shells print the same leaf; if a `-c` fragment differs, `diff` shows the exact mismatch (expect pass — Task 2 matched bash).

- [ ] **Step 3: Docs — `docs/bash-divergences.md`**

Find M-77's deferred list (it mentions "bare-positional script-execution mode (`huck script.sh`), `-c COMMAND`"). Remove those two from M-77's **Deferred** sentence and add a new entry:

```
- **M-77a: script-file mode + `-c`** — `[fixed v82]` medium. `huck SCRIPT [ARG...]`
  runs the file ($0=path, $1..=ARGs); `huck -c COMMAND [NAME [ARG...]]` runs the
  string ($0=NAME or shell name, $1..=ARGs — bash's first-operand-is-$0 quirk);
  `--` ends option scanning. Both are non-interactive (no REPL/rc/history), reuse
  `run_sourced_contents`, and propagate `exit N` / `set -e`. Missing script file →
  "No such file or directory" + exit 127. Shebangs (`#!/usr/bin/env huck`) work.
  Resolves the M-72/L-12 stdin constraint: with the program in a file/`-c` string,
  fd 0 is free, so `huck script < input` feeds `read`/`select`. **Deferred**: `-s`,
  a `-` stdin operand, login-shell / `-i` / `-l`; directory-as-script exits 127
  (bash 126).
```
Add a `2026-06-03` change-log entry. Update the Summary table "Last updated" stamp (and the Tier-2 Notes/count if M-77a is counted as a new entry — keep the count consistent with how follow-ons like M-09a were handled).

- [ ] **Step 4: `README.md`** — add after the v81 row:
```
| v82       | script-file mode (`huck script [args]`) + `-c` + `--` (M-77a)    |
```

- [ ] **Step 5: Final full suite + all 9 harnesses + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
cargo build --quiet
for h in arrays ifs test_combinators completion function_keyword arith_for loop_levels select script_mode; do
  echo -n "$h: "; tests/scripts/${h}_diff_check.sh 2>&1 | tail -1
done
```
Expected: FAIL=0; all 9 harnesses `Fail: 0`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v82 task 3: script-mode bash-diff harness + docs

tests/scripts/script_mode_diff_check.sh (huck's 9th harness): 6 `-c`
fragments + a file-mode fragment, byte-identical to bash. docs: new M-77a
[fixed v82] entry (M-77 deferred list trimmed), change-log, summary stamp;
README v82 row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist (before merge)

- [ ] All tests pass (`FAIL=0`); clippy clean.
- [ ] All 9 bash-diff harnesses `Fail: 0` (no regression in the prior 8).
- [ ] Interactive REPL + piped-stdin REPL unchanged (`printf 'echo hi\nexit\n' | huck` → `hi`).
- [ ] File mode: `$0`=path, positionals, multi-line, `exit N`, shebang ignored, missing→127.
- [ ] `-c`: first-operand-is-$0 quirk, no-operand $0=shell name, multi-statement/multi-line, `exit N`, empty string → exit 0.
- [ ] `--` ends options; operands after `-c CMD` are verbatim.
- [ ] `set -e` / fatal PE propagate as non-zero exit from a script.
- [ ] Payoff: `huck script < input` feeds `read`/`select` real stdin.

## Merge

`AskUserQuestion` before merging (per CLAUDE.md). Then `git merge --no-ff` into `main`, push, delete branch; update memory files.
