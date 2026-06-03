# huck v81 — `select` loops (M-24) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Each task is a fresh subagent with spec-compliance + code-quality review between tasks.

**Goal:** Implement bash's `select NAME [in WORDS]; do BODY; done` menu loop with byte-for-byte fidelity to bash 5.2's multi-column menu, and fix the related no-`in` `for` positional-fallback bug (M-24a).

**Architecture:** `select` mirrors the existing `for` machinery (new `Keyword::Select`, `SelectClause` AST, `parse_select_command`, `run_select`) and reuses v79's loop infrastructure (`Shell.loop_depth`, `ExecOutcome::LoopBreak(u32,i32)`/`LoopContinue(u32)`, the decrement-and-bubble pattern). The numbered menu is produced by a pure helper `format_select_menu` that ports bash's `print_select_list`/`indent`/`print_index_and_element` exactly. Input is read via the existing `read` builtin (sets `REPLY`). The `for` no-`in` form gains a `has_in` flag and falls back to `"$@"`.

**Tech Stack:** Rust 1.85+, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-03-huck-select-loops-design.md` (read it; it contains the verified bash algorithm and behavior table).

**Branch:** `v81-select-loops` (create from `main` in Preamble).

**Commit trailer (every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1:** `git status && git rev-parse --abbrev-ref HEAD` → expect clean tree on `main`.
- [ ] **Step 2:** `git checkout -b v81-select-loops` → "Switched to a new branch".
- [ ] **Step 3:** Baseline: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "Baseline:", s}'` → expect **2315**.
- [ ] **Step 4:** `cargo clippy --all-targets 2>&1 | tail -2` → clean.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/executor.rs` | `format_select_menu` + helpers (`number_len`, `select_indent`, `select_displen`); `run_select` loop runner; `Command::Select` dispatch; `run_for_inner` no-`in` fallback. Unit tests for the formatter + executor. | 1, 3 |
| `src/command.rs` | `Keyword::Select`; `SelectClause`; `Command::Select`; `parse_select_command`; `has_in` on `ForClause`; parser unit tests. | 2 |
| `tests/select_integration.rs` | NEW. Binary-driven integration tests. | 4 |
| `tests/scripts/select_diff_check.sh` | NEW. huck's 8th bash-diff harness (incl. a `for`-no-`in` fragment). | 4 |
| `tests/pty_interactive.rs` | One new pty `select` test. | 4 |
| `docs/bash-divergences.md`, `README.md` | M-24 → `[fixed v81]`, M-24a note, changelog, summary stamp, README row. | 4 |

---

## Task 1: Menu formatter (`format_select_menu`) — pure, byte-exact

This is the hardest, most self-contained piece. Pure string-building, fully unit-testable. Port of bash `print_select_list` + `indent` + `print_index_and_element` (see spec).

**Files:**
- Modify: `src/executor.rs` — add the formatter + helpers + a `mod select_menu_tests`.

- [ ] **Step 1: Write the failing tests** (add at the bottom of `src/executor.rs`):

```rust
#[cfg(test)]
mod select_menu_tests {
    use super::{format_select_menu, number_len, select_indent};

    fn items(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn number_len_digit_counts() {
        assert_eq!(number_len(1), 1);
        assert_eq!(number_len(9), 1);
        assert_eq!(number_len(10), 2);
        assert_eq!(number_len(99), 2);
        assert_eq!(number_len(100), 3);
    }

    #[test]
    fn indent_emits_tab_across_stop_else_space() {
        let mut s = String::new();
        select_indent(&mut s, 6, 11); // crosses the 8-boundary once → tab + 3 spaces
        assert_eq!(s, "\t   ");
        let mut s2 = String::new();
        select_indent(&mut s2, 20, 22); // same tab block → 2 spaces
        assert_eq!(s2, "  ");
    }

    #[test]
    fn single_item() {
        assert_eq!(format_select_menu(&items(&["only"]), 80), "1) only\n");
    }

    #[test]
    fn three_items_single_column() {
        // 3 short items, COLS=80: cols=80/large, rows becomes 1 → flip to 1 col.
        assert_eq!(
            format_select_menu(&items(&["a", "b", "c"]), 80),
            "1) a\n2) b\n3) c\n"
        );
    }

    #[test]
    fn ten_items_cols80_multicolumn() {
        let got = format_select_menu(
            &items(&["one", "two", "three", "four", "five",
                     "six", "seven", "eight", "nine", "ten"]),
            80,
        );
        // Verified byte-for-byte against bash 5.2 (COLUMNS=80, cat -A):
        let expected = "1) one\t    3) three   5) five\t  7) seven   9) nine\n\
                        2) two\t    4) four    6) six\t  8) eight  10) ten\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn ten_items_cols40() {
        let got = format_select_menu(
            &items(&["one", "two", "three", "four", "five",
                     "six", "seven", "eight", "nine", "ten"]),
            40,
        );
        let expected = "1) one\t    5) five    9) nine\n\
                        2) two\t    6) six    10) ten\n\
                        3) three    7) seven\n\
                        4) four\t    8) eight\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn ten_items_cols110_single_column_flip() {
        let got = format_select_menu(
            &items(&["one", "two", "three", "four", "five",
                     "six", "seven", "eight", "nine", "ten"]),
            110,
        );
        // Wide COLS → rows==1 flip → single column, numbers right-justified to 2.
        let expected = " 1) one\n 2) two\n 3) three\n 4) four\n 5) five\n\
                        \x20 6) six\n 7) seven\n 8) eight\n 9) nine\n10) ten\n";
        assert_eq!(got, expected);
    }
}
```

> NOTE: the `\x20` at a line-continuation boundary is a literal leading space (` 6) six`). If transcription is uncertain, regenerate any `expected` with
> `printf '1\n' | COLUMNS=<n> bash -c '<frag>' 2>&1 >/dev/null | cat -A` and translate `^I`→`\t`, `$`→`\n`.

- [ ] **Step 2: Run, expect failure**

Run: `cargo test --quiet select_menu_tests 2>&1 | tail -5`
Expected: compile error (functions not defined) / FAIL.

- [ ] **Step 3: Implement the formatter** (place near the other loop runners in `src/executor.rs`):

```rust
/// Default screen width when $COLUMNS is unset/invalid (bash uses 80).
const SELECT_DEFAULT_COLS: usize = 80;
const SELECT_TABSIZE: usize = 8;

/// Decimal digit count of `n` (bash NUMBER_LEN). n>=1 in practice.
fn number_len(n: usize) -> usize {
    let mut len = 1;
    let mut v = n;
    while v >= 10 {
        v /= 10;
        len += 1;
    }
    len
}

/// Display width of a menu item. ASCII-exact (codepoint count); wide-char
/// width is a documented sub-divergence (see spec).
fn select_displen(s: &str) -> usize {
    s.chars().count()
}

/// Pad column position `from` up to `to` exactly as bash's `indent()`:
/// emit a tab when crossing an 8-column tab stop, else a space.
fn select_indent(out: &mut String, mut from: usize, to: usize) {
    while from < to {
        if to / SELECT_TABSIZE > from / SELECT_TABSIZE {
            out.push('\t');
            from += SELECT_TABSIZE - (from % SELECT_TABSIZE);
        } else {
            out.push(' ');
            from += 1;
        }
    }
}

/// Render the numbered `select` menu byte-for-byte like bash 5.2's
/// `print_select_list`. `cols_width` is the screen width (COLS). The returned
/// string (with a trailing newline per row) is written to stderr by the caller.
fn format_select_menu(items: &[String], cols_width: usize) -> String {
    let mut out = String::new();
    let list_len = items.len();
    if list_len == 0 {
        out.push('\n');
        return out;
    }
    let indices_len = number_len(list_len);
    let max_item = items.iter().map(|s| select_displen(s)).max().unwrap_or(0);
    // RP_SPACE_LEN (") ") = 2, plus bash's extra +2 gap.
    let max_elem_len = max_item + indices_len + 2 + 2;

    let mut cols = if max_elem_len != 0 { cols_width / max_elem_len } else { 1 };
    if cols == 0 {
        cols = 1;
    }
    let mut rows = list_len.div_ceil(cols);
    cols = list_len.div_ceil(rows);
    if rows == 1 {
        rows = cols;
        cols = 1;
    }
    let first_col_iw = number_len(rows);
    let other_iw = indices_len;

    for row in 0..rows {
        let mut ind = row;
        let mut pos = 0usize;
        loop {
            let iw = if pos == 0 { first_col_iw } else { other_iw };
            let item = &items[ind];
            // bash print_index_and_element: "%*d" + ") " + item
            out.push_str(&format!("{:>width$}) {}", ind + 1, item, width = iw));
            let elem_len = select_displen(item) + iw + 2;
            ind += rows;
            if ind >= list_len {
                break;
            }
            select_indent(&mut out, pos + elem_len, pos + max_elem_len);
            pos += max_elem_len;
        }
        out.push('\n');
    }
    out
}
```

> If `div_ceil` is unavailable on the MSRV, use `(a + b - 1) / b`.

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test --quiet select_menu_tests 2>&1 | tail -5`
Expected: all tests PASS. If a multi-column `expected` mismatches, regenerate it from bash (Step 1 note) — the algorithm is authoritative; transcription is the likely error.

- [ ] **Step 5: clippy + commit**

```bash
cargo clippy --all-targets 2>&1 | tail -2
git add src/executor.rs
git commit -m "$(cat <<'EOF'
v81 task 1: byte-exact select menu formatter (port of bash print_select_list)

Pure format_select_menu + number_len/select_indent/select_displen helpers,
porting bash 5.2's print_select_list/indent/print_index_and_element. Unit
tests assert exact bytes (incl. tabs) for single-item, single-column,
multi-column at COLUMNS=80/40, and the wide-COLUMNS single-column flip.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: AST + lexer keyword + parser (+ `has_in` on ForClause)

**Files:**
- Modify: `src/command.rs` — `Keyword::Select`, `SelectClause`, `Command::Select`, `parse_select_command`, wire-in; add `has_in: bool` to `ForClause`; parser unit tests.
- Modify: `src/executor.rs` — minimal `Command::Select` dispatch (stub `run_select` for now) so the crate compiles; thread `has_in` (no behavior change yet — Task 3 adds the fallback).

- [ ] **Step 1: Add `has_in` to `ForClause` (compile-first, behavior unchanged)**

In `src/command.rs`, add the field to `ForClause`:
```rust
pub struct ForClause {
    pub var: String,
    pub words: Vec<Word>,
    /// True when an explicit `in WORDS` clause was present. The no-`in`
    /// form (`has_in == false`) iterates the positional params (Task 3).
    pub has_in: bool,
    pub body: Sequence,
}
```
In `parse_for_after_keyword`, set `has_in` based on whether the `in` keyword was consumed (the existing `if iter.peek()... == Some(Keyword::In)` branch → `has_in = true`; else `false`). Update every `ForClause { .. }` literal in the codebase (parser + tests) to include `has_in`. Find them: `grep -rn "ForClause {" src/ tests/`.

- [ ] **Step 2: Build to surface all `ForClause` construction sites**

Run: `cargo build 2>&1 | grep -E "missing field|ForClause" | head`
Fix each (set `has_in: true` where the test/parse path represents `for x in ...`, `false` for no-`in`). Re-build clean.

- [ ] **Step 3: Add the `Keyword::Select` variant**

In `src/command.rs` `enum Keyword`, add `Select`. Add to the `as_str` match: `Keyword::Select => "select"`. Add to `keyword_of`: `"select" => Some(Keyword::Select)`.

- [ ] **Step 4: Add `SelectClause` + `Command::Select`**

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SelectClause {
    /// Loop variable name — a validated identifier.
    pub var: String,
    /// None => no `in` clause (iterate the positional params "$@").
    /// Some(words) => explicit `in WORDS` (Some(vec![]) = empty `in`).
    pub words: Option<Vec<Word>>,
    pub body: Sequence,
}
```
Add `Select(Box<SelectClause>)` to `enum Command`.

- [ ] **Step 5: Write the failing parser tests**

Add to `src/command.rs` tests (mirror existing `for` parser tests; use the same helpers the for-tests use to lex+parse a string):

```rust
#[test]
fn parses_select_with_in() {
    let cmd = parse_one("select x in a b c; do echo $x; done");
    match cmd {
        Command::Select(s) => {
            assert_eq!(s.var, "x");
            assert_eq!(s.words.as_ref().map(|w| w.len()), Some(3));
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn parses_select_without_in_is_none() {
    let cmd = parse_one("select x; do echo $x; done");
    match cmd {
        Command::Select(s) => {
            assert_eq!(s.var, "x");
            assert!(s.words.is_none(), "no-`in` select must have words == None");
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn parses_select_empty_in_is_some_empty() {
    let cmd = parse_one("select x in; do echo $x; done");
    match cmd {
        Command::Select(s) => assert_eq!(s.words, Some(vec![])),
        other => panic!("expected Select, got {other:?}"),
    }
}
```

> Use whatever single-command parse helper the existing `for`/`case` tests use (e.g. a local `parse_one`/`first_command` helper). If none exists, replicate the lex→parse call the other parser tests make.

- [ ] **Step 6: Implement `parse_select_command`**

Mirror `parse_for_after_keyword` exactly, but:
- Build a `SelectClause`, not a `ForClause`.
- `words` is `None` when there is no `in` keyword; `Some(collected_words)` when `in` is present (even if the list is empty).
- Reuse the same identifier validation and the same `do … done` body parsing + the same error variants `for` uses (`UnterminatedFor`/missing-`do`/missing-`done` equivalents; reuse them rather than adding new variants).

Wire it in:
- In `parse_command`: `Some(Keyword::Select) => parse_select_command(iter),`
- In `parse_next_stage` (pipeline position): add the same `Some(Keyword::Select) => ...` arm next to the existing `for`/`while`/`case` arms.

- [ ] **Step 7: Add a stub `run_select` + dispatch so the crate compiles**

In `src/executor.rs`, add a temporary stub and dispatch arm (Task 3 replaces the body):
```rust
fn run_select(_clause: &crate::command::SelectClause, _shell: &mut Shell, _sink: &mut StdoutSink) -> ExecOutcome {
    // Task 3 implements this.
    ExecOutcome::Continue(0)
}
```
Add the dispatch arm wherever `Command::For(..)` is matched in `run_command`:
```rust
Command::Select(clause) => run_select(clause, shell, sink),
```

- [ ] **Step 8: Run parser tests + full build**

Run: `cargo test --quiet parses_select 2>&1 | tail -6` → 3 PASS.
Run: `cargo build 2>&1 | tail -3` → clean.

- [ ] **Step 9: Full suite + clippy + commit**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "After Task 2:", s}'   # 2315 + new parser tests
cargo clippy --all-targets 2>&1 | tail -2
git add -A
git commit -m "$(cat <<'EOF'
v81 task 2: select AST + keyword + parser; has_in flag on ForClause

New Keyword::Select, SelectClause { var, words: Option<Vec<Word>>, body },
Command::Select, and parse_select_command (mirrors parse_for) wired into
parse_command and parse_next_stage. words==None distinguishes no-`in`
(iterate "$@") from Some(vec![]) (explicit empty `in`). ForClause gains
has_in (set by the parser) ahead of the Task-3 positional fallback.
Stub run_select returns Continue(0) so the crate compiles.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `run_select` execution + `for` no-`in` positional fallback (M-24a)

**Files:**
- Modify: `src/executor.rs` — real `run_select` (loop runner using v79 infra + the read loop + menu); `run_for_inner` no-`in` fallback; executor unit/integration-style tests.

Read `run_for`/`run_for_inner` (around `src/executor.rs:309-380`) first — `run_select` shares its structure (loop_depth wrapper, the four-arm `LoopBreak`/`LoopContinue` decrement-and-bubble, SIGINT check, `try_set` for the var, `expand` + `glob_expand_fields` for words). Read `builtin_read` (`src/builtins.rs:1957`) — it reads one line from stdin and, with no NAME args, stores the raw line in `REPLY`, returning `Continue(0)` on success and a non-zero `Continue` on EOF.

- [ ] **Step 1: Fix `run_for_inner` no-`in` fallback (M-24a) + failing test**

Add the integration-style test (drives `process_line`):
```rust
#[test]
fn for_without_in_iterates_positionals() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line("set -- a b c", &mut sh, false);
    let (out, _code) = crate::executor::execute_capturing(
        &crate::test_support_parse("for x; do printf '%s ' \"$x\"; done"), &mut sh);
    assert_eq!(out, "a b c ");
}
```
> Use whatever existing capture/parse helpers the executor tests already use to run a fragment and capture stdout (mirror an existing `run_for` executor/integration test). If executor unit tests can't easily capture stdout, put this assertion in `tests/select_integration.rs` (Task 4) instead and keep Task 3's for-fix covered by the diff harness — but prefer an inline test here.

Then change `run_for_inner` so that when `!clause.has_in` it iterates the positional parameters instead of the (empty) `words`:
```rust
let mut values: Vec<String> = Vec::new();
if clause.has_in {
    for word in &clause.words {
        values.extend(glob_expand_fields(expand(word, shell)));
    }
} else {
    values = shell.positional_args.clone();
}
```
(Explicit empty `in` keeps `has_in == true` with empty `words` → iterates nothing, matching bash.)

- [ ] **Step 2: Run the for-fix test + regression**

Run: `cargo test --quiet for_without_in_iterates_positionals 2>&1 | tail -4` → PASS.
Run the existing for tests to confirm no regression: `cargo test --quiet for_ 2>&1 | grep -E "^test result"`.

- [ ] **Step 3: Write failing `select` execution tests**

Add executor-level tests driving `process_line` with piped stdin is awkward at unit level; put the behavioral tests in `tests/select_integration.rs` (Task 4). Here, add ONE executor test that exercises the loop-flow wiring without real stdin by checking `loop_depth` restoration and empty-list short-circuit:
```rust
#[test]
fn select_empty_list_runs_no_body_and_restores_depth() {
    let mut sh = Shell::new();
    // `select x in ; do exit 7; done` — empty `in` → body never runs → status 0.
    let outcome = crate::shell::process_line("select x in; do exit 7; done", &mut sh, false);
    assert_eq!(sh.loop_depth, 0, "loop_depth must be restored");
    // The shell did not exit 7 (body never ran):
    assert!(!matches!(outcome, ExecOutcome::Exit(7)));
}
```

- [ ] **Step 4: Implement `run_select`**

Replace the Task-2 stub. Structure (mirror `run_for`'s wrapper + the bash `select_query`/outer loop from the spec):

```rust
fn run_select(clause: &crate::command::SelectClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_select_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_select_inner(clause: &crate::command::SelectClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    use std::sync::atomic::Ordering;

    // 1. Build the item list (expand `in WORDS`, or positionals for no-`in`).
    let items: Vec<String> = match &clause.words {
        Some(words) => {
            let mut v = Vec::new();
            for w in words {
                v.extend(glob_expand_fields(expand(w, shell)));
            }
            v
        }
        None => shell.positional_args.clone(),
    };
    // 2. Empty list → exit immediately, body never runs (bash).
    if items.is_empty() {
        return ExecOutcome::Continue(0);
    }

    // 3. Screen width: $COLUMNS if a positive integer, else 80.
    let cols_width = shell
        .lookup_var("COLUMNS")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(SELECT_DEFAULT_COLS);

    let mut last = ExecOutcome::Continue(0);
    let mut show_menu = true;

    loop {
        // 3a. PS3 (default "#? ").
        let ps3 = shell.lookup_var("PS3").unwrap_or_else(|| "#? ".to_string());

        // 3b. select_query: (re)print menu when show_menu, prompt, read until a
        //     non-empty line or EOF. Empty line reprints the menu.
        let selection: Option<String> = loop {
            if show_menu {
                eprint!("{}", format_select_menu(&items, cols_width));
            }
            eprint!("{ps3}");
            use std::io::Write;
            let _ = std::io::stderr().flush();

            // Read one line into REPLY via the read builtin (no NAME args).
            let mut devnull: Vec<u8> = Vec::new();
            let r = read_line_into_reply(shell, &mut devnull);
            if !matches!(r, ExecOutcome::Continue(0)) {
                // EOF / read failure → terminate the select loop.
                println!(); // bash writes a newline to stdout on EOF
                return last_or_failure(last);
            }
            let reply = shell.lookup_var("REPLY").unwrap_or_default();
            if reply.is_empty() {
                show_menu = true;
                continue; // reprint menu, re-prompt
            }
            // Parse as a 1-based index; invalid / out-of-range → empty selection.
            match reply.trim().parse::<usize>() {
                Ok(n) if n >= 1 && n <= items.len() => break Some(items[n - 1].clone()),
                _ => break Some(String::new()),
            }
        };

        let sel = selection.expect("select_query returns Some unless it returned early");

        // 3c. Bind NAME (honor readonly like the other loop runners).
        if shell.try_set(&clause.var, sel).is_err() {
            eprintln!("huck: {}: readonly variable", clause.var);
            return ExecOutcome::Continue(1);
        }

        // 3d. SIGINT check (mirror run_for).
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }

        // 3e. Run the body; handle flow with the v79 decrement-and-bubble pattern.
        match execute_sequence_body(&clause.body, shell, sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak(1, st) => {
                last = ExecOutcome::Continue(st);
                break;
            }
            ExecOutcome::LoopBreak(n, st) => return ExecOutcome::LoopBreak(n - 1, st),
            ExecOutcome::LoopContinue(1) => { /* fall through to next prompt */ }
            ExecOutcome::LoopContinue(n) => return ExecOutcome::LoopContinue(n - 1),
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Continue(c) => last = ExecOutcome::Continue(c),
        }

        // 3f. Menu suppressed next iteration unless the last REPLY was empty
        //     (KSH_COMPATIBLE_SELECT). REPLY empty was handled inside the inner
        //     loop, so here always suppress.
        show_menu = false;
    }
    last
}
```

Implement the two small helpers used above:
```rust
/// Reads one line from stdin into REPLY using the read builtin's no-NAME path.
/// Returns Continue(0) on success, a non-zero Continue on EOF.
fn read_line_into_reply(shell: &mut Shell, out: &mut Vec<u8>) -> ExecOutcome {
    crate::builtins::run_builtin("read", &[], out, shell)
}

fn last_or_failure(last: ExecOutcome) -> ExecOutcome {
    // bash returns the read's failure status on EOF; surface the loop's last
    // body status if any iteration ran, else the read failure (1).
    match last {
        ExecOutcome::Continue(_) => last,
        other => other,
    }
}
```
> CONFIRM during implementation: the exact path to invoke the `read` builtin with no names. If `run_builtin` is the dispatcher, `run_builtin("read", &[], out, shell)` matches `builtin_read(&[], out, shell)`. If `read` returns its EOF status differently than `Continue(nonzero)`, adjust the `!matches!(r, Continue(0))` check accordingly (verify by reading `builtin_read`). The behavioral contract to satisfy: empty line → reprint+re-prompt; EOF → loop ends; valid number → that item; invalid/out-of-range → empty NAME, body still runs.

- [ ] **Step 5: Build + run executor tests**

Run: `cargo build 2>&1 | tail -3` → clean.
Run: `cargo test --quiet select_empty_list_runs_no_body for_without_in 2>&1 | tail -6` → PASS.

- [ ] **Step 6: Smoke-test from the binary**

```bash
printf '2\n' | ./target/debug/huck -c 2>/dev/null <<'SH' || \
printf '2\n' | ./target/debug/huck <<'SH'
select x in alpha beta gamma; do echo "got=$x reply=$REPLY"; break; done
SH
```
Expected stdout: `got=beta reply=2`. (huck has no `-c`; pipe via stdin. The menu/prompt go to stderr.)
Also: `printf '9\n' | ./target/debug/huck <<'SH'\nselect x in a b; do echo "x=[$x]"; break; done\nSH` → `x=[]` (invalid index, body runs).

- [ ] **Step 7: Full suite + clippy + commit**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "After Task 3:", s}'
cargo clippy --all-targets 2>&1 | tail -2
git add -A
git commit -m "$(cat <<'EOF'
v81 task 3: run_select execution + for no-in positional fallback (M-24a)

run_select is a loop runner (v79 loop_depth wrapper + decrement-and-bubble):
expands `in WORDS` (or "$@" for no-`in`), prints the byte-exact menu + PS3 to
stderr, reads REPLY via the read builtin, handles empty-line reprint, EOF
termination, and invalid/out-of-range -> empty NAME (body still runs), binds
NAME, runs the body, and suppresses the menu on subsequent iterations
(KSH_COMPATIBLE_SELECT). run_for_inner now iterates positional params for the
no-`in` form (M-24a); explicit empty `in` still iterates nothing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Integration tests + bash-diff harness + pty test + docs

**Files:**
- Create: `tests/select_integration.rs`
- Create: `tests/scripts/select_diff_check.sh` (executable)
- Modify: `tests/pty_interactive.rs`
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: `tests/select_integration.rs`** (binary-driven via stdin; mirror an existing `tests/*_integration.rs` harness for the `run_huck` helper):

```rust
//! Integration tests for v81 `select` loops (M-24) + for no-`in` (M-24a).
use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str, stdin: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    {
        let si = child.stdin.as_mut().unwrap();
        si.write_all(script.as_bytes()).unwrap();
        si.write_all(b"\n").unwrap();
        si.write_all(stdin.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(), String::from_utf8_lossy(&o.stderr).into(), o.status.code().unwrap_or(-1))
}

#[test]
fn valid_selection_sets_name_and_reply() {
    // NOTE: script and the user's input both arrive on stdin; the select read
    // consumes the line(s) AFTER the script line. Keep the program on one line.
    let (out, _e, _c) = run_huck("select x in alpha beta gamma; do echo \"got=$x reply=$REPLY\"; break; done", "2\n");
    assert_eq!(out, "got=beta reply=2\n");
}

#[test]
fn invalid_index_sets_empty_name_but_runs_body() {
    let (out, _e, _c) = run_huck("select x in a b; do echo \"x=[$x]\"; break; done", "9\n");
    assert_eq!(out, "x=[]\n");
}

#[test]
fn nonnumeric_sets_empty_name_but_runs_body() {
    let (out, _e, _c) = run_huck("select x in a b; do echo \"x=[$x] r=$REPLY\"; break; done", "foo\n");
    assert_eq!(out, "x=[] r=foo\n");
}

#[test]
fn eof_terminates_loop() {
    // No input → immediate EOF → body never runs; reach the echo after.
    let (out, _e, _c) = run_huck("select x in a b; do echo ran; done\necho after", "");
    assert!(out.contains("after"), "stdout: {out:?}");
    assert!(!out.contains("ran"), "body should not run on immediate EOF: {out:?}");
}

#[test]
fn no_in_uses_positionals() {
    let (out, _e, _c) = run_huck("set -- p q; select x; do echo \"x=$x\"; break; done", "2\n");
    assert_eq!(out, "x=q\n");
}

#[test]
fn break_2_from_nested_loop_exits_both() {
    let (out, _e, _c) = run_huck(
        "for i in 1 2; do select x in a b; do echo \"$i$x\"; break 2; done; done",
        "1\n");
    assert_eq!(out, "1a\n");
}

#[test]
fn for_no_in_iterates_positionals() {
    let (out, _e, _c) = run_huck("set -- a b c; for x; do printf '%s ' \"$x\"; done; echo", "");
    assert_eq!(out, "a b c \n");
}
```
Run: `cargo test --test select_integration 2>&1 | grep -E "^test result"` → 7 PASS. (Adjust the stdin-sharing detail if huck consumes the program and the select input from the same stream differently — verify against the binary; the contract is what matters.)

- [ ] **Step 2: `tests/scripts/select_diff_check.sh`** (huck's 8th harness; byte-identical to bash, menu compared via `cat -A`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck diff harness for v81 `select` (M-24) and the
# no-`in` `for` fix (M-24a). Menu + prompt go to stderr; we compare merged
# stdout+stderr through `cat -A` so tabs/newlines are explicit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() { # label, columns, input, program
  local label="$1" cols="$2" input="$3" prog="$4" b h
  b=$(printf '%s' "$input" | COLUMNS="$cols" bash -c "$prog" 2>&1 | cat -A; echo "EXIT:${PIPESTATUS[1]:-0}")
  h=$(printf '%s' "$input" | COLUMNS="$cols" "$HUCK_BIN" <<<"$prog" 2>&1 | cat -A; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# NOTE: huck reads the program from stdin (no -c). bash uses -c with input piped.
# For huck, the program is the heredoc and the menu input must follow it on stdin.
# If this dual-mode is awkward, write per-fragment .sh files run identically by both.
check "menu COLUMNS=80" 80 $'1\n' 'select x in one two three four five six seven eight nine ten; do break; done'
check "menu COLUMNS=40" 40 $'1\n' 'select x in one two three four five six seven eight nine ten; do break; done'
check "menu COLUMNS=110 single-col" 110 $'1\n' 'select x in one two three four five six seven eight nine ten; do break; done'
check "mixed widths" 80 $'1\n' 'select x in aaa bbbbbbbb cc dddddddddddd ee ff; do break; done'
check "12 items 2-digit" 80 $'1\n' 'select x in i1 i2 i3 i4 i5 i6 i7 i8 i9 i10 i11 i12; do break; done'
check "selection + reply" 80 $'2\n' 'select x in a b c; do echo "got=$x r=$REPLY"; break; done'
check "invalid index" 80 $'9\n' 'select x in a b; do echo "x=[$x]"; break; done'
check "empty then valid (reprint)" 80 $'\n2\n' 'select x in a b c; do echo "x=$x"; break; done'
check "custom PS3" 80 $'1\n' 'PS3="choose> "; select x in a b; do echo $x; break; done'
check "no-in positionals" 80 $'2\n' 'set -- p q; select x; do echo "x=$x"; break; done'
check "for no-in (M-24a)" 80 '' 'set -- a b c; for x; do printf "%s " "$x"; done; echo'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
Make executable: `chmod +x tests/scripts/select_diff_check.sh`.

- [ ] **Step 3: Build + run the harness; iterate to 11/11**

```bash
cargo build --quiet
tests/scripts/select_diff_check.sh
```
Expected: `Total: 11, Pass: 11, Fail: 0`. If a menu fragment fails, `diff` shows the exact tab/space mismatch — fix `format_select_menu`/`select_indent` until byte-identical. **Common pitfalls:** stdin sharing between the program text and the select input (huck reads both from stdin — the select read must consume only the trailing menu-input lines, not the program); `$COLUMNS` not honored; PS3 default glyphs. If the dual stdin/`-c` invocation can't be made identical, switch to writing each fragment to a temp `.sh` file and running `bash file.sh` and `huck < file.sh`-style identically for both (document in the harness header).

- [ ] **Step 4: One pty interactive test** in `tests/pty_interactive.rs` (apply the v80 lesson — `settle()` after any post-transition prompt before sending input):

```rust
#[test]
fn pty_select_menu_and_pick() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else { return; };
    expect(&mut session, "huck> ");
    send(&mut session, "select x in alpha beta; do echo picked=$x; break; done");
    send(&mut session, ENTER);
    expect(&mut session, "1) alpha");        // menu to the pty
    expect(&mut session, "2) beta");
    expect(&mut session, "#? ");             // PS3 default
    settle();                                 // v80: prompt is not a safe write barrier under load
    send(&mut session, "2");
    send(&mut session, ENTER);
    expect(&mut session, "picked=beta");
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
```
Run: `cargo test --test pty_interactive pty_select_menu_and_pick 2>&1 | tail -4` → PASS (or skip if no PTY).

- [ ] **Step 5: Docs — `docs/bash-divergences.md`**

Flip M-24:
```
- **M-24: `select` loops** — `[fixed v81]` medium. `select NAME [in WORDS]; do BODY; done` ...
```
Write a full entry describing: menu to stderr byte-identical to bash 5.2's `print_select_list` (column-major, `$COLUMNS`-aware, tab-packed via the ported `indent`, `max_elem_len = longest + NUMBER_LEN(count) + 4`, the `rows==1` single-column flip, first-column vs other-column number widths); `PS3` (default `#? `); `REPLY` raw line via the `read` builtin; empty-line reprint; EOF termination; invalid/out-of-range → empty `NAME` + body runs; no-`in` → `"$@"`; empty list → no body; `break`/`continue N` via v79. Add an **M-24a** entry (`[fixed v81]` low): no-`in` `for` now iterates `"$@"` (was a no-op); explicit empty `in` still iterates nothing. Add a `2026-06-03` change-log entry. Update the Summary table (Tier-2 Notes: `M-24 fixed by v81`; add M-24a) and the "Last updated" stamp.

- [ ] **Step 6: `README.md`** — add after the v80 row:
```
| v81       | `select` loops (M-24) + no-`in` `for` positionals (M-24a)        |
```

- [ ] **Step 7: Final full suite + all 8 harnesses + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
cargo build --quiet
for h in arrays ifs test_combinators completion function_keyword arith_for loop_levels select; do
  echo -n "$h: "; tests/scripts/${h}_diff_check.sh 2>&1 | tail -1
done
```
Expected: FAIL=0; all 8 harnesses `Fail: 0`.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v81 task 4: select integration tests + bash-diff harness + pty + docs

tests/select_integration.rs (7 tests), tests/scripts/select_diff_check.sh
(huck's 8th harness, 11 fragments byte-identical to bash 5.2 incl. menu
layouts via cat -A and the M-24a for-no-in fragment), one pty select test
(with the v80 settle() discipline). Docs: M-24 -> [fixed v81] with the full
menu-algorithm description, new M-24a entry, change-log, summary stamp,
README row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist (before merge)

- [ ] All tests pass (`PASS=<2315+new>`, `FAIL=0`); clippy clean.
- [ ] All 8 bash-diff harnesses `Fail: 0` (no regression in the prior 7).
- [ ] Menu byte-identical to bash at COLUMNS 80 / 40 / 110 / mixed widths / 12 items.
- [ ] `REPLY` = raw line; `NAME` = word or empty; invalid/out-of-range runs body; empty line reprints; EOF ends loop.
- [ ] `select` with no `in` iterates `"$@"`; empty `in` runs nothing.
- [ ] `for` with no `in` iterates `"$@"` (M-24a); explicit empty `in` runs nothing.
- [ ] `break`/`continue`/`break N` from `select` work; `shell.loop_depth` is 0 after exit.
- [ ] `select` works as a pipeline stage (parses, runs).
- [ ] pty test passes (or cleanly skips without a PTY).

## Merge

`AskUserQuestion` before merging (per CLAUDE.md). Then `git merge --no-ff` into `main`, push, delete branch; update memory files.
