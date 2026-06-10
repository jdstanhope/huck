# v130 — `set -x` trace fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck's `set -x` (xtrace) output byte-match bash 5.x for the common cases — bash-style per-word quoting, inline + bare assignment lines, `local`/`declare` args, the `command` prefix, and tracing of external pipeline stages.

**Architecture:** A new `xtrace_quote()` in `param_expansion.rs` replicates bash's `sh_contains_shell_metas` "quote-only-if-needed" rule (distinct from `@Q`, which always quotes). The trace block in `run_exec_single` is rewritten (quoting, `command` prefix, decl args, inline-assignment lines); bare-assignment tracing is added to the `Assign` arm; external pipeline stages are traced in `spawn_external_with_fds`.

**Tech Stack:** Rust. Tests: cargo integration tests (`CARGO_BIN_EXE_huck`, capture STDERR) + a bash-diff harness comparing stderr only.

**GIT SAFETY:** Do NOT `git checkout <sha>` — stay on `v130-setx-trace-fidelity`; edit, build, commit in place. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec `docs/superpowers/specs/2026-06-10-setx-trace-fidelity-design.md`. Key types: `Assignment { target: AssignTarget, value: Word, append }` (command.rs:297); `DeclArg::Plain(String) | Assign(Assignment)` (command.rs:311); `ResolvedCommand { program, args, decl_args: Option<Vec<DeclArg>>, .. }` (executor.rs:2108); helpers `command.rs::word_literal_text(&Word)->Option<&str>` (1982), `builtins::escape_alias_value(&str)->String` (5563, `pub(crate)`), `param_expansion::ansi_c_quote(&str)->String` (pub(crate)), `expand::expand_assignment(&Word,&mut Shell)->String`.

---

### Task 1: `xtrace_quote` quoting helper

**Files:**
- Create: `tests/xtrace_quote_unit.rs` — NO; unit-test inside the module instead (the fn is `pub(crate)`). Add `#[cfg(test)]` tests in `src/param_expansion.rs`.
- Modify: `src/param_expansion.rs`

- [ ] **Step 1: Write the failing unit tests** — append to the `#[cfg(test)] mod tests` in `src/param_expansion.rs` (if none exists, add one at end of file):

```rust
#[cfg(test)]
mod xtrace_quote_tests {
    use super::xtrace_quote;
    #[test]
    fn bare_safe_words() {
        for s in ["hello", "a-b", "a/b", "a.b", "a:b", "a=b", "a,b", "a%b", "a+b", "a@b", "a_b", "aZ9", "a#b", "a~b"] {
            assert_eq!(xtrace_quote(s), s, "{s} should be bare");
        }
    }
    #[test]
    fn empty_is_two_quotes() {
        assert_eq!(xtrace_quote(""), "''");
    }
    #[test]
    fn metas_get_single_quoted() {
        assert_eq!(xtrace_quote("a b"), "'a b'");
        assert_eq!(xtrace_quote("; foo"), "'; foo'");
        assert_eq!(xtrace_quote("["), "'['");
        assert_eq!(xtrace_quote("]"), "']'");
        assert_eq!(xtrace_quote("a!b"), "'a!b'");
        assert_eq!(xtrace_quote("a^b"), "'a^b'");
        assert_eq!(xtrace_quote("a*b"), "'a*b'");
        assert_eq!(xtrace_quote("a$b"), "'a$b'");
        assert_eq!(xtrace_quote("%s\\n"), "'%s\\n'"); // backslash is a meta
    }
    #[test]
    fn leading_tilde_and_hash_are_meta_but_not_mid_word() {
        assert_eq!(xtrace_quote("~x"), "'~x'");      // leading ~
        assert_eq!(xtrace_quote("#x"), "'#x'");      // leading #
        assert_eq!(xtrace_quote("a~b"), "a~b");      // mid ~ safe
        assert_eq!(xtrace_quote("a#b"), "a#b");      // mid # safe
        assert_eq!(xtrace_quote("x=~y"), "'x=~y'");  // ~ after = is meta
    }
    #[test]
    fn single_quote_in_value_is_escaped() {
        // 'it'\''s' style: escape_alias_value rewrites ' -> '\''
        assert_eq!(xtrace_quote("it's"), "'it'\\''s'");
    }
    #[test]
    fn control_chars_use_ansi_c() {
        assert_eq!(xtrace_quote("a\tb"), "$'a\\tb'");
        assert_eq!(xtrace_quote("a\nb"), "$'a\\nb'");
    }
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test --lib xtrace_quote 2>&1 | tail -20`. Expected: fail to compile (`xtrace_quote` not found).

- [ ] **Step 3: Implement** — add to `src/param_expansion.rs` (near `shell_quote`, ~line 266):

```rust
/// Quote `s` the way bash's xtrace (`set -x`) does: leave it bare unless it
/// contains a shell metacharacter, in which case single-quote it (with `'`
/// rewritten `'\''`); empty → `''`; any control char → ANSI-C `$'…'`. Distinct
/// from `shell_quote`/`${v@Q}`, which ALWAYS quotes.
pub(crate) fn xtrace_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars().any(|c| c.is_control()) {
        return ansi_c_quote(s);
    }
    if contains_shell_metas(s) {
        return format!("'{}'", crate::builtins::escape_alias_value(s));
    }
    s.to_string()
}

/// bash `sh_contains_shell_metas`: does `s` contain a character that requires
/// quoting to re-read as a single literal word?
fn contains_shell_metas(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            ' ' | '\t' | '\n' | '\'' | '"' | '\\' | '|' | '&' | ';' | '(' | ')'
            | '<' | '>' | '!' | '{' | '}' | '*' | '[' | '?' | ']' | '^' | '$' | '`' => {
                return true;
            }
            '~' => {
                if i == 0 || chars[i - 1] == '=' || chars[i - 1] == ':' {
                    return true;
                }
            }
            '#' => {
                if i == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
```

- [ ] **Step 4: Run to verify pass** — `cargo test --lib xtrace_quote 2>&1 | tail -20`. Expected: all pass.
- [ ] **Step 5: Build + clippy** — `cargo build 2>&1 | tail -3`; `cargo clippy --all-targets 2>&1 | tail -3` (clean).
- [ ] **Step 6: Commit**

```bash
git add src/param_expansion.rs
git commit -m "$(cat <<'EOF'
feat(v130): xtrace_quote — bash sh_contains_shell_metas quoting

Quote a word only when it contains a shell metacharacter (else bare), empty -> '',
control chars -> $'...'. Distinct from @Q/shell_quote which always quotes. The
arg-quoting primitive for set -x trace fidelity.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Rewrite the `run_exec_single` trace block (quoting, command prefix, decl args, inline-assignment lines)

**Files:**
- Modify: `src/executor.rs` (capture `command_prefix` in the `while resolved.program == "command"` loop ~2835; rewrite the trace block ~2897; add `ps4`/`xtrace_command_line` helpers)
- Modify: `tests/set_x_integration.rs` (update v103 assertions to the new quoted output)
- Create: `tests/setx_trace_fidelity_integration.rs` (simple-command cases)

- [ ] **Step 1: Write the failing integration tests** — create `tests/setx_trace_fidelity_integration.rs`:

```rust
//! v130: set -x trace fidelity — simple-command quoting, command prefix,
//! decl args, inline-assignment lines.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
/// Returns (stdout, stderr, code). xtrace goes to stderr.
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}
fn trace_lines(stderr: &str) -> Vec<String> {
    stderr.lines().filter(|l| l.starts_with("+ ")).map(String::from).collect()
}

#[test]
fn quotes_arg_with_space() {
    let (_o, e, _c) = run("set -x\nx=\"a b\"; echo \"$x\" c\n");
    assert!(trace_lines(&e).contains(&"+ echo 'a b' c".to_string()), "stderr: {e}");
}
#[test]
fn quotes_bracket_command() {
    let (_o, e, _c) = run("set -x\n[ 1 -lt 2 ]\n");
    assert!(trace_lines(&e).contains(&"+ '[' 1 -lt 2 ']'".to_string()), "stderr: {e}");
}
#[test]
fn quotes_empty_and_special() {
    let (_o, e, _c) = run("set -x\necho \"\" \"; foo\"\n");
    assert!(trace_lines(&e).contains(&"+ echo '' '; foo'".to_string()), "stderr: {e}");
}
#[test]
fn safe_words_stay_bare() {
    let (_o, e, _c) = run("set -x\necho hello a-b a/b a=b a,b\n");
    assert!(trace_lines(&e).contains(&"+ echo hello a-b a/b a=b a,b".to_string()), "stderr: {e}");
}
#[test]
fn local_args_rendered() {
    let (_o, e, _c) = run("set -x\nf() { local DEF=x y; }; f\n");
    assert!(trace_lines(&e).contains(&"+ local DEF=x y".to_string()), "stderr: {e}");
}
#[test]
fn command_prefix_kept() {
    let (_o, e, _c) = run("set -x\ncommand printf \"%s\\n\" hi\n");
    assert!(trace_lines(&e).contains(&"+ command printf '%s\\n' hi".to_string()), "stderr: {e}");
}
#[test]
fn inline_assignment_separate_lines() {
    let (_o, e, _c) = run("set -x\nFOO=bar echo hi\n");
    let t = trace_lines(&e);
    let i = t.iter().position(|l| l == "+ FOO=bar").expect("FOO line");
    let j = t.iter().position(|l| l == "+ echo hi").expect("echo line");
    assert!(i < j, "FOO before echo; stderr: {e}");
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test --test setx_trace_fidelity_integration 2>&1 | tail -20`. Expected: `quotes_*`, `local_args_rendered`, `command_prefix_kept`, `inline_assignment_separate_lines` FAIL; `safe_words_stay_bare` PASSES (already bare).

- [ ] **Step 3: Add helpers** — in `src/executor.rs` (near the trace block or top-level helpers):

```rust
fn ps4(shell: &Shell) -> String {
    shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string())
}
/// Join a command's words (prefix ++ program ++ args), each xtrace-quoted, into
/// one trace line body (no PS4).
fn xtrace_command_line(prefix: &[String], program: &str, args: &[String]) -> String {
    use crate::param_expansion::xtrace_quote;
    let mut parts: Vec<String> = prefix.iter().map(|w| xtrace_quote(w)).collect();
    parts.push(xtrace_quote(program));
    parts.extend(args.iter().map(|a| xtrace_quote(a)));
    parts.join(" ")
}
```

- [ ] **Step 4: Capture the `command` prefix** — in the `while resolved.program == "command"` loop (~executor.rs:2835), introduce `let mut command_prefix: Vec<String> = Vec::new();` before the loop, and in the bare-form branch (where it does `resolved.program = new_program`) push the consumed tokens FIRST:

```rust
        Some(_) => {
            // Record the consumed `command` + leading flags for xtrace fidelity.
            command_prefix.push("command".to_string());
            command_prefix.extend(resolved.args[..idx].iter().cloned());
            let new_program = resolved.args[idx].clone();
            // ... (existing rewrite of program/args/decl_args/bypass_functions)
```
(Make sure `command_prefix` is in scope at the trace block below; declare it just before the `while`.)

- [ ] **Step 5: Rewrite the trace block** — replace the existing `if shell.shell_options.xtrace { ... }` block (~2897) with:

```rust
    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        // Inline-assignment prefix: each on its own preceding line (bash).
        for a in &cmd.inline_assignments {
            let name = a.target.name();
            let val = shell.lookup_var(name).unwrap_or_default();
            eprintln!("{p4}{name}={}", crate::param_expansion::xtrace_quote(&val));
        }
        // Command line (only if there is a program word).
        if !resolved.program.is_empty() {
            let body = if let Some(dargs) = &resolved.decl_args {
                // Render declaration args (Plain → quoted; Assign → name=quoted-rhs).
                let mut parts: Vec<String> =
                    command_prefix.iter().map(|w| crate::param_expansion::xtrace_quote(w)).collect();
                parts.push(crate::param_expansion::xtrace_quote(&resolved.program));
                for da in dargs {
                    match da {
                        crate::command::DeclArg::Plain(s) =>
                            parts.push(crate::param_expansion::xtrace_quote(s)),
                        crate::command::DeclArg::Assign(a) => {
                            let name = a.target.name();
                            let rhs = match crate::command::word_literal_text(&a.value) {
                                Some(t) => t.to_string(),
                                None => crate::expand::expand_assignment(&a.value, shell),
                            };
                            parts.push(format!("{name}={}", crate::param_expansion::xtrace_quote(&rhs)));
                        }
                    }
                }
                parts.join(" ")
            } else {
                xtrace_command_line(&command_prefix, &resolved.program, &resolved.args)
            };
            eprintln!("{p4}{body}");
        }
    }
```
NOTE: `word_literal_text` is private to `command.rs` — make it `pub(crate)` (change `fn word_literal_text` → `pub(crate) fn word_literal_text`). If `expand_assignment` is not already reachable as `crate::expand::expand_assignment`, confirm its path (it is used elsewhere in executor.rs — match that call form).

- [ ] **Step 6: Update v103 tests** — open `tests/set_x_integration.rs`. Any assertion that expects the OLD unquoted output (e.g. `+ printf %s\n one`) must become the new bash-matching output (`+ printf '%s\n' one`). Run bash on the same fragment to get the exact expected string. Do NOT weaken assertions — update them to the correct new bytes.

- [ ] **Step 7: Run the new + updated tests** — `cargo test --test setx_trace_fidelity_integration --test set_x_integration 2>&1 | tail -25`. Expected: all pass.

- [ ] **Step 8: Build + clippy + full test** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 9: Commit**

```bash
git add src/executor.rs src/command.rs tests/setx_trace_fidelity_integration.rs tests/set_x_integration.rs
git commit -m "$(cat <<'EOF'
feat(v130): bash-faithful xtrace for simple commands

Rewrite the run_exec_single trace block: xtrace-quote every word, emit each
inline assignment on its own preceding line, preserve the `command` prefix
(captured during the command-collapse), and render local/declare args (decl_args
side-channel) that were previously dropped. Update v103 set_x assertions to the
new quoted output.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Bare-assignment + external-pipeline-stage tracing

**Files:**
- Modify: `src/executor.rs` (`run_single` `SimpleCommand::Assign` arm ~2676; `spawn_external_with_fds`)
- Modify: `tests/setx_trace_fidelity_integration.rs` (add bare-assignment + pipeline-stage tests)

- [ ] **Step 1: Add failing tests** — append to `tests/setx_trace_fidelity_integration.rs`:

```rust
#[test]
fn bare_assignment_traced() {
    let (_o, e, _c) = run("set -x\nA=1\nB=\"x y\"\n");
    let t = trace_lines(&e);
    assert!(t.contains(&"+ A=1".to_string()), "stderr: {e}");
    assert!(t.contains(&"+ B='x y'".to_string()), "stderr: {e}");
}
#[test]
fn external_pipeline_stage_traced() {
    // `cat` is an external stage; both stages must trace.
    let (_o, e, _c) = run("set -x\necho a | cat\n");
    let t = trace_lines(&e);
    assert!(t.contains(&"+ echo a".to_string()), "stderr: {e}");
    assert!(t.contains(&"+ cat".to_string()), "stderr: {e}");
}
```

- [ ] **Step 2: Run to verify fail** — `cargo test --test setx_trace_fidelity_integration bare_assignment_traced external_pipeline_stage_traced 2>&1 | tail -15`. Expected: both FAIL.

- [ ] **Step 3: Trace bare assignments** — in `run_single`’s `SimpleCommand::Assign(items)` arm (~executor.rs:2676), after a successful `apply_one_assignment(a, shell)` (inside the `for a in items` loop, on the success path), emit the trace:

```rust
                if apply_one_assignment(a, shell).is_err() {
                    st = 1;
                    break;
                }
                if shell.shell_options.xtrace {
                    let name = a.target.name();
                    let val = shell.lookup_var(name).unwrap_or_default();
                    eprintln!("{}{name}={}", ps4(shell),
                              crate::param_expansion::xtrace_quote(&val));
                }
```
(Emit AFTER apply so `lookup_var` returns the assigned value; one line per assignment, in order. Array assignments: `lookup_var` yields a scalar/element form — best-effort per spec, must not panic.)

- [ ] **Step 4: Trace external pipeline stages** — in `spawn_external_with_fds`, after the `resolve(exec, shell)` succeeds and before the spawn (near the v129 `flush_stdout()`), add:

```rust
    if shell.shell_options.xtrace {
        eprintln!("{}{}", ps4(shell),
                  xtrace_command_line(&[], &resolved.program, &resolved.args));
    }
```
(External stages carry no `command` prefix / decl_args — declaration builtins and `command` are InProcess. `resolved` is the `ResolvedCommand` already computed in this function.)

- [ ] **Step 5: Run new tests** — `cargo test --test setx_trace_fidelity_integration 2>&1 | tail -20`. Expected: all pass (Task 2 + Task 3 tests).

- [ ] **Step 6: Build + clippy + full test** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs tests/setx_trace_fidelity_integration.rs
git commit -m "$(cat <<'EOF'
feat(v130): trace bare assignments and external pipeline stages

Bare `A=1` (SimpleCommand::Assign) was never traced; emit `+ A=1` per assignment.
External pipeline stages (spawn_external_with_fds) were untraced; emit their
command line so `echo a | cat` traces both stages like bash.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Bash-diff harness + docs (narrow L-21)

**Files:**
- Create: `tests/scripts/setx_trace_fidelity_diff_check.sh`
- Modify: `docs/bash-divergences.md` (narrow L-21)

- [ ] **Step 1: Write the harness** — create `tests/scripts/setx_trace_fidelity_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v130: set -x trace fidelity. Compares
# STDERR only (set -x writes there), with stdout discarded. Default PS4 `+ `.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Compare stderr (trace) of `set -x; <frag>` between bash and huck — exact bytes.
check() {
    local label="$1" frag="$2" b h
    b=$(printf 'set -x\n%s\n' "$frag" | bash 2>&1 >/dev/null)
    h=$(printf 'set -x\n%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Order-independent variant for PIPELINE fragments: in-process stages trace from
# their forked child while external stages trace from the parent, so the strict
# left-to-right order is best-effort (documented). Compare the SET of trace lines.
check_sorted() {
    local label="$1" frag="$2" b h
    b=$(printf 'set -x\n%s\n' "$frag" | bash 2>&1 >/dev/null | sort)
    h=$(printf 'set -x\n%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null | sort)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "arg with space"        'x="a b"; echo "$x" c'
check "bracket test"          '[ 1 -lt 2 ]'
check "empty and special"     'echo "" "; foo"'
check "safe words bare"       'echo hello a-b a/b a.b a:b a=b a,b a%b a+b a@b a_b'
check "local args"            'f() { local DEF=x y; }; f'
check "command prefix"        'command printf "%s\n" hi'
check "inline assignment"     'FOO=bar echo hi'
check "two inline assigns"    'A=1 B=2 echo x'
check "bare assignment"       'A=1'
check "bare assign quoted"    'B="x y"'
check_sorted "pipeline two stages"   'echo a | cat'
check_sorted "pipeline three stages" 'echo a | cat | cat'

# ASCII-punctuation sweep: locks the contains_shell_metas safe set. Assign to a
# var (no execution) with the punct mid-word; compare the traced line.
for c in '!' '#' '%' '+' '-' '.' '/' ':' '=' '@' '^' '_' '~' ',' '*' '?' '[' ']' '{' '}' '(' ')' '<' '>' ';' '|' '&'; do
    check "punct[$c]" "v=a${c}b"
done

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/setx_trace_fidelity_diff_check.sh`.

- [ ] **Step 2: Build + run the harness** — `cargo build 2>&1 | tail -2`; `bash tests/scripts/setx_trace_fidelity_diff_check.sh`. Expected: `Pass: N, Fail: 0`. If a punct case fails, the `contains_shell_metas` set is wrong for that char — fix `contains_shell_metas` in `param_expansion.rs` to match bash (re-run bash on that char to confirm), then rebuild. Do NOT delete a failing fragment to pass.

- [ ] **Step 3: Narrow L-21 in `docs/bash-divergences.md`** — read the L-21 entry (`### L-21: set -x (xtrace) trace-format divergences`, `[intentional]`). It lists five differences (a)–(e). v130 fixes (b) inline-assignment prefix, (c) arg re-quoting, and the bare-assignment clause of (d). Edit the entry to REMOVE those, keeping only the residual: (a) flat `$PS4` (no depth-repeat, no PS4 `$VAR`/escape expansion); the finer-compound clause of (d) (`for`-iteration variable sets, the `case` word, `[[ ]]`/`(( ))` are not separately traced); the decl-RHS-with-command-substitution edge; (e) `2>` does not suppress the trace (M-90); and (f) pipeline-stage trace ORDER is best-effort (lines match bash, but in-process stages trace from a forked child and external stages from the parent, so left-to-right order in a mixed pipeline may differ). Reword the **huck**/**bash**/**Why intentional** prose so it no longer claims the now-fixed gaps. Keep it `[intentional]`. Show the before/after of the entry in your report.

- [ ] **Step 4: Verify** — `grep -n "re-quot\|inline-assignment prefix" docs/bash-divergences.md` should no longer describe these as open (the words may appear only in the "now traces these" sense). Confirm L-21 still exists (narrowed), not deleted.

- [ ] **Step 5: Full regression** — `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean); smoke an existing harness: `bash tests/scripts/async_list_diff_check.sh | tail -1`.

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/setx_trace_fidelity_diff_check.sh docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
test+docs(v130): set -x trace-fidelity harness; narrow L-21

Add the bash-diff harness (quoting ASCII sweep + every fixed divergence row) and
narrow L-21 to the remaining intentional residuals (flat PS4 / no depth-repeat,
finer-compound traces, decl-RHS-cmdsub edge, 2>-no-suppress).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-review notes
- **Spec coverage:** Task 1 = `xtrace_quote` (Component 1); Task 2 = `run_exec_single` rewrite incl. command prefix + decl args + inline-assignment lines (Component 2) + v103 test update; Task 3 = bare assignments (Component 3) + external pipeline stages (Component 4); Task 4 = harness (incl. ASCII sweep locking the meta set) + L-21 narrowing.
- **Type/symbol consistency:** `xtrace_quote`/`contains_shell_metas` (param_expansion.rs); `ps4`/`xtrace_command_line` (executor.rs) defined in Task 2, reused in Task 3; `word_literal_text` made `pub(crate)` in Task 2. `DeclArg::{Plain,Assign}`, `Assignment.target.name()`, `a.value`, `escape_alias_value`, `ansi_c_quote`, `expand_assignment` all exist (see Reference).
- **No double-eval risk** except the documented decl-RHS-command-substitution edge (Task 2 Step 5 uses `word_literal_text` first, falling back to `expand_assignment` only for non-literal RHS).
- **Ordering caveat:** pipeline-stage trace lines come from different processes (parent for external, child for in-process); the harness fragments emit one line per stage and the `check` compares full stderr text — if order proves unstable across runs, keep the assertions to `contains`-style (integration tests already use `contains`).
