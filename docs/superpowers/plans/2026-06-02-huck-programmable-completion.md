# huck v76 — Programmable Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Add bash-style programmable completion: `complete` / `compgen` / `compopt` builtins, `COMP_*` variables, and tab-time `-F` function execution — so real-world completion scripts (`git-completion.bash`, kubectl, systemctl, etc.) can be sourced into huck and fire correctly at the Tab key.

**Architecture:** Three layers. (1) `Rc<RefCell<Shell>>` at the readline boundary so rustyline's `&self` completion callback can mutate shell state; internal `&mut Shell` signatures unchanged. (2) A `CompletionSpec` data layer (`src/completion_spec.rs`) with a pure `resolve_spec()` that turns a spec into candidates. (3) The three builtins (`src/completion_builtins.rs`) that build specs, and the dispatch orchestrator (`src/completion.rs::dispatch`) that decides at Tab time which spec to run.

**Tech Stack:** Rust 1.85+; existing rustyline 14; no new dependencies.

**Branch:** `v76-programmable-completion` (create from `main` in Preamble P.1).

**Spec:** `docs/superpowers/specs/2026-06-02-huck-programmable-completion-design.md`.

**Commit trailer (every commit):**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1: Verify clean tree on main**

Run: `git status && git rev-parse --abbrev-ref HEAD`
Expected: branch `main`, clean working tree.

- [ ] **Step 2: Create the iteration branch**

```bash
git checkout -b v76-programmable-completion
```

Expected: `Switched to a new branch 'v76-programmable-completion'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "Baseline:", sum}'`
Expected: 2149 (current main).

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` with no warnings.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell_state.rs` | Add `completion_specs: CompletionSpecs` and `current_completion_spec: Option<CompletionSpec>` fields | 2, 6 |
| `src/shell.rs` | Wrap `Shell` in `Rc<RefCell<Shell>>` in `run()`; refactor `read_logical_command` to scope borrows around `editor.readline()` | 1 |
| `src/completion.rs` | Restructure `HuckHelper` to hold `Rc<RefCell<Shell>>`; remove snapshot fields and `refresh()`; add `dispatch::resolve()` orchestrator | 1, 5 |
| `src/completion_spec.rs` | NEW. `CompletionSpec`, `CompletionSpecs`, `CompOptions`, `Action`; `resolve_spec()` static generators (`-W`/`-G`/`-A`/`-X`/`-P`/`-S`); `call_completion_function()` glue | 2, 4 |
| `src/completion_builtins.rs` | NEW. `builtin_complete`, `builtin_compgen`, `builtin_compopt`; shared flag parser | 3, 4, 6 |
| `src/builtins.rs` | Add 3 names to `BUILTIN_NAMES`; add 3 match arms in `run_builtin` | 3, 6 |
| `src/executor.rs` | Make `call_function` `pub(crate)`; add a thin `call_function_body` wrapper that takes a name and looks up the body | 4 |
| `src/main.rs` | Add `mod completion_spec;` and `mod completion_builtins;` | 2, 3 |
| `tests/completion_integration.rs` | NEW. ~15 binary-driven completion tests | 7 |
| `tests/scripts/completion_diff_check.sh` | NEW. ~12 bash-diff fragments | 7 |
| `docs/bash-divergences.md` | Flip M-36 to `[fixed v76 partial]`; add change-log entry | 7 |
| `README.md` | New v76 iteration row | 7 |

---

## Task 1: Foundation — `Rc<RefCell<Shell>>` at the readline boundary

**Files:**
- Modify: `src/shell.rs` — wrap Shell in Rc<RefCell>; scope borrows around `editor.readline()`
- Modify: `src/completion.rs` — restructure `HuckHelper` to hold the cell; remove snapshot fields

**Goal:** Pure refactor. No new behavior. All 2149 existing tests pass under the new shape. The latent aliasing issue in `read_logical_command` (holding `&mut Shell` across `editor.readline()`) is eliminated.

### Steps

- [ ] **Step 1: Write a failing unit test that proves HuckHelper holds an Rc<RefCell<Shell>>**

Edit `src/completion.rs`. Add at the bottom of the existing `#[cfg(test)] mod tests` block (before the closing `}`):

```rust
    #[test]
    fn helper_holds_rc_refcell_shell() {
        use std::rc::Rc;
        use std::cell::RefCell;
        let shell = Rc::new(RefCell::new(Shell::new()));
        let helper = HuckHelper::new(Rc::clone(&shell));
        // Mutate shell through the cell; helper must see the change live.
        shell.borrow_mut().set("MY_VAR", "hello".to_string());
        let history = rustyline::history::FileHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (start, pairs) = rustyline::completion::Completer::complete(
            &helper, "echo $MY_V", 10, &ctx,
        ).unwrap();
        assert_eq!(start, 6);
        assert!(pairs.iter().any(|p| p.replacement == "MY_VAR"),
                "live var not visible to helper: {pairs:?}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --quiet helper_holds_rc_refcell_shell 2>&1 | tail -10`
Expected: FAIL — `HuckHelper::new` doesn't take an `Rc<RefCell<Shell>>` yet.

- [ ] **Step 3: Restructure `HuckHelper` in `src/completion.rs`**

Replace the existing `HuckHelper` struct, `impl HuckHelper`, `impl Default`, and `Completer for HuckHelper` blocks (around lines 313-372) with:

```rust
use crate::shell_state::Shell;
use std::cell::RefCell;
use std::rc::Rc;

/// rustyline completion helper. Holds an `Rc<RefCell<Shell>>` so the
/// completion callback can read AND mutate shell state (required by
/// `-F func` execution during Tab). The Rust-borrow discipline is:
/// `complete()` acquires `borrow_mut()` for the duration of the call
/// and releases on return. The main loop must hold NO borrow across
/// `editor.readline()` so this acquisition succeeds.
pub struct HuckHelper {
    shell: Rc<RefCell<Shell>>,
}

impl HuckHelper {
    pub fn new(shell: Rc<RefCell<Shell>>) -> Self {
        Self { shell }
    }
}

impl rustyline::completion::Completer for HuckHelper {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let shell = self.shell.borrow();
        let path = shell.get("PATH").unwrap_or("").to_string();
        let home = shell.get("HOME").unwrap_or("").to_string();
        let var_names: Vec<String> = shell.var_names().map(|s| s.to_string()).collect();
        drop(shell);

        let (start, context) = analyze(line, pos);
        let candidates = match context {
            CompletionContext::Command { prefix } => complete_command(&prefix, &path),
            CompletionContext::Variable { prefix } => complete_variable(&prefix, &var_names),
            CompletionContext::File { dir, prefix } => complete_file(&dir, &prefix, &home),
        };
        let pairs = candidates
            .into_iter()
            .map(|c| rustyline::completion::Pair {
                display: c.display,
                replacement: c.replacement,
            })
            .collect();
        Ok((start, pairs))
    }
}

impl rustyline::hint::Hinter for HuckHelper {
    type Hint = String;
}

impl rustyline::highlight::Highlighter for HuckHelper {}

impl rustyline::validate::Validator for HuckHelper {}

impl rustyline::Helper for HuckHelper {}
```

Note this preserves *current behavior exactly* — it just reads live state from the cell instead of from cached snapshot fields. Task 5 replaces this body with `dispatch::resolve()`.

- [ ] **Step 4: Delete the existing snapshot-based helper tests that no longer compile**

In `src/completion.rs`'s test module, find these three tests and delete them (they construct `HuckHelper { var_names: ..., path: ..., home: ... }` which no longer compiles):
- `helper_complete_command_context`
- `helper_complete_variable_context`
- `helper_complete_file_context`

The new `helper_holds_rc_refcell_shell` test (Step 1) covers the helper-level behavior; the existing unit tests for `complete_command` / `complete_variable` / `complete_file` continue to exercise the underlying functions.

- [ ] **Step 5: Refactor `src/shell.rs::run()` to wrap Shell in Rc<RefCell>**

Edit `src/shell.rs`. At the top of the file, add to the existing `use` block:

```rust
use std::cell::RefCell;
use std::rc::Rc;
```

In `pub fn run(args: &[String]) -> i32`, replace the section from `let mut shell = Shell::new();` through the editor wiring with the cell-based version. The current code (around lines 158-180) looks like:

```rust
    editor.set_helper(Some(HuckHelper::new()));

    let mut shell = Shell::new();
    install_sigint_handler(Arc::clone(&shell.sigint_flag));
    install_sigchld_handler(Arc::clone(&shell.sigchld_flag));

    shell.history.load();
    for (_, command) in shell.history.entries() {
        let _ = editor.add_history_entry(command);
    }

    if let Some(exit_code) = maybe_source_rc_file(&mut shell, &opts) {
        crate::traps::fire_exit_trap(&mut shell);
        shell.hangup_jobs();
        shell.history.save();
        return exit_code;
    }

    loop {
        crate::jobs::reap_and_notify(&mut shell);
        crate::traps::dispatch_pending_traps(&mut shell);
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(&shell);
        }
```

Replace with:

```rust
    let shell_cell = Rc::new(RefCell::new(Shell::new()));

    {
        let shell = shell_cell.borrow();
        install_sigint_handler(Arc::clone(&shell.sigint_flag));
        install_sigchld_handler(Arc::clone(&shell.sigchld_flag));
    }

    {
        let mut shell = shell_cell.borrow_mut();
        shell.history.load();
        for (_, command) in shell.history.entries() {
            let _ = editor.add_history_entry(command);
        }
    }

    editor.set_helper(Some(HuckHelper::new(Rc::clone(&shell_cell))));

    {
        let mut shell = shell_cell.borrow_mut();
        if let Some(exit_code) = maybe_source_rc_file(&mut shell, &opts) {
            crate::traps::fire_exit_trap(&mut shell);
            shell.hangup_jobs();
            shell.history.save();
            return exit_code;
        }
    }

    loop {
        {
            let mut shell = shell_cell.borrow_mut();
            crate::jobs::reap_and_notify(&mut shell);
            crate::traps::dispatch_pending_traps(&mut shell);
        }
        {
            let mut shell = shell_cell.borrow_mut();
            if let Some(exit_code) = fire_prompt_command(&mut shell) {
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.history.save();
                return exit_code;
            }
        }
```

Crucial invariant: no `borrow_mut()` is held across the `editor.readline()` call in `read_logical_command`. Step 6 reworks that function.

- [ ] **Step 6: Refactor `read_logical_command` to take `&RefCell<Shell>` and scope borrows**

In `src/shell.rs`, replace the `fn read_logical_command` signature and body. The current signature (line 250):

```rust
fn read_logical_command(
    editor: &mut Editor<HuckHelper, FileHistory>,
    shell: &mut Shell,
) -> ReadResult {
```

Becomes:

```rust
fn read_logical_command(
    editor: &mut Editor<HuckHelper, FileHistory>,
    cell: &RefCell<Shell>,
) -> ReadResult {
```

Inside the body, every prior direct use of `shell` becomes a scoped borrow. Replace the loop body so the prompt-expansion and history-expansion blocks each acquire a fresh `borrow()` or `borrow_mut()` that DROPS before `editor.readline()`. Here's the full replacement body for the function:

```rust
    use crate::continuation::{classify, joiner_for, Completeness};

    let mut buffer = String::new();
    let mut history = String::new();
    let mut pending: Option<(crate::continuation::ContinuationReason, String)> = None;

    loop {
        let expanded = {
            let shell = cell.borrow();
            let (var_name, default) = if pending.is_none() {
                ("PS1", DEFAULT_PS1)
            } else {
                ("PS2", DEFAULT_PS2)
            };
            let template = shell
                .lookup_var(var_name)
                .unwrap_or_else(|| default.to_string());
            crate::prompt::expand_prompt(&template, &shell)
        };

        match editor.readline(&expanded) {
            Ok(raw) => {
                let line = {
                    let mut shell = cell.borrow_mut();
                    match crate::history::expand(&raw, &shell.history) {
                        Ok(None) => raw,
                        Ok(Some(expanded)) => {
                            println!("{expanded}");
                            expanded
                        }
                        Err(e) => {
                            eprintln!("huck: {e}");
                            shell.set_last_status(1);
                            return ReadResult::Interrupted;
                        }
                    }
                };

                match pending.take() {
                    None => {
                        buffer.push_str(&line);
                        history.push_str(&line);
                    }
                    Some((reason, prev_line)) => {
                        if reason != crate::continuation::ContinuationReason::Backslash {
                            buffer.push('\n');
                        }
                        buffer.push_str(&line);
                        history.push_str(joiner_for(reason, &prev_line));
                        history.push_str(&line);
                    }
                }

                match classify(&buffer) {
                    Completeness::Complete | Completeness::Error => {
                        return ReadResult::Ready { buffer, history };
                    }
                    Completeness::Incomplete(reason) => {
                        if reason == crate::continuation::ContinuationReason::Backslash {
                            buffer.pop();
                            history.pop();
                        }
                        pending = Some((reason, line));
                    }
                }
            }
            Err(ReadlineError::Interrupted) => return ReadResult::Interrupted,
            Err(ReadlineError::Eof) => {
                return if buffer.is_empty() {
                    ReadResult::Eof
                } else {
                    ReadResult::EofMidCommand
                };
            }
            Err(e) => return ReadResult::ReadError(e.to_string()),
        }
    }
```

Note `expand_prompt` takes `&Shell` (already an immutable borrow), so passing `&shell` from a `borrow()` works. `history::expand` takes `&shell.history` (immutable inside our `borrow_mut`).

- [ ] **Step 7: Update the call site in `run()` to pass the cell**

Inside `run()`'s main loop, the `match read_logical_command(&mut editor, &mut shell)` line becomes:

```rust
        match read_logical_command(&mut editor, &shell_cell) {
            ReadResult::Ready { buffer, history } => {
                {
                    let mut shell = shell_cell.borrow_mut();
                    if !history.trim().is_empty() {
                        shell.history.add(history.clone());
                        let _ = editor.add_history_entry(history.as_str());
                    }
                }
                let do_alias = {
                    let shell = shell_cell.borrow();
                    shell.is_interactive
                        || std::env::var("HUCK_EXPAND_ALIASES").is_ok()
                };
                let outcome = {
                    let mut shell = shell_cell.borrow_mut();
                    process_line(&buffer, &mut shell, do_alias)
                };
                match outcome {
                    ExecOutcome::Exit(code) => {
                        let mut shell = shell_cell.borrow_mut();
                        crate::traps::fire_exit_trap(&mut shell);
                        shell.hangup_jobs();
                        shell.history.save();
                        return code;
                    }
                    ExecOutcome::Continue(status) => {
                        let mut shell = shell_cell.borrow_mut();
                        shell.set_last_status(status);
                        if let Some(fatal_status) = shell.take_pending_fatal_pe_error()
                            && !shell.is_interactive
                        {
                            crate::traps::fire_exit_trap(&mut shell);
                            shell.hangup_jobs();
                            shell.history.save();
                            return fatal_status;
                        }
                    }
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
                    | ExecOutcome::FunctionReturn(_) => {
                        let mut shell = shell_cell.borrow_mut();
                        shell.set_last_status(0)
                    }
                }
            }
            ReadResult::Interrupted => continue,
            ReadResult::Eof => {
                let mut shell = shell_cell.borrow_mut();
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.history.save();
                return shell.last_status();
            }
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                let mut shell = shell_cell.borrow_mut();
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.history.save();
                return 2;
            }
            ReadResult::ReadError(msg) => {
                eprintln!("huck: input error: {msg}");
                let mut shell = shell_cell.borrow_mut();
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                return 1;
            }
        }
    }
}
```

The pattern is the same throughout: each block that needs the shell does `let mut shell = shell_cell.borrow_mut()` (or `borrow()` if read-only) and the borrow drops at the block's `}`.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 1:", sum}'`
Expected: 2150 (baseline 2149 + the one new helper test from Step 1, minus 3 deleted helper tests = 2147, then +1 new = 2148; verify the exact count matches what the suite reports).

Actually: 2149 (baseline) - 3 (deleted helper tests) + 1 (new helper test) = 2147. Confirm cargo test reports 2147.

- [ ] **Step 9: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: `Finished` with no warnings. If clippy complains about the new explicit `drop(shell);` in Step 3, replace it with a scoped block.

- [ ] **Step 10: Smoke-test interactive behavior**

Run: `echo 'echo $HOME' | cargo run --quiet 2>&1 | tail -3`
Expected: prints the value of `$HOME` followed by no error.

Then verify Tab completion still works for the existing contexts by running:

```bash
printf 'cd /tm\t\n' | cargo run --quiet 2>&1 | tail -5
```

The `\t` is a tab character — completion should expand `/tm` to `/tmp`. (This won't be a true interactive test, but it exercises the path.)

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v76 task 1: refactor Shell behind Rc<RefCell> at readline boundary

Wraps Shell in Rc<RefCell<Shell>> in src/shell.rs::run(); refactors
read_logical_command to take &RefCell<Shell> and scope all borrows so
no borrow is held across editor.readline(). HuckHelper now stores a
clone of the Rc and reads live state via borrow() inside complete().

Internal &mut Shell signatures across executor/expand/builtins/etc.
unchanged. The latent aliasing in the prior code (mutable borrow held
across the readline call) is eliminated as a side effect.

Pure refactor: no user-visible behavior change. Tab completion, prompt
expansion, history expansion, rc-file sourcing, and trap dispatch all
exercise the same paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `CompletionSpec` data layer + static generators

**Files:**
- Create: `src/completion_spec.rs` — types + `resolve_spec` for static generators
- Modify: `src/shell_state.rs` — add `completion_specs` field
- Modify: `src/main.rs` — add `mod completion_spec;`

**Goal:** All static generators (`-W`, `-G`, `-A action`, `-P`, `-S`, `-X`) work via `resolve_spec`. `-F` is reserved but not wired (returns empty for now). Pure data + pure functions; no shell mutation. Unit-tested in isolation.

### Steps

- [ ] **Step 1: Add the `completion_specs` field to `Shell`**

Edit `src/shell_state.rs`. Find the `pub struct Shell {` declaration (line 116). Add a forward-reference field; we'll define the type after we create the module. At the top of `src/shell_state.rs`, add to the existing `use` block:

```rust
use crate::completion_spec::{CompletionSpec, CompletionSpecs};
```

In the `Shell` struct (somewhere near the existing `dir_stack` field), add:

```rust
    /// Programmable-completion registry (filled by the `complete` builtin).
    pub completion_specs: CompletionSpecs,
    /// Ephemeral slot used by `compopt` inside a `-F` function to mutate
    /// the live spec. Set by `dispatch::resolve` before invoking `-F`;
    /// taken back out afterward.
    pub current_completion_spec: Option<CompletionSpec>,
```

In `impl Shell { pub fn new() -> Self {`, initialize both fields:

```rust
            completion_specs: CompletionSpecs::default(),
            current_completion_spec: None,
```

- [ ] **Step 2: Create `src/completion_spec.rs` with the data types**

Create the new file `src/completion_spec.rs`:

```rust
//! Completion-spec data and the `resolve_spec()` candidate generator.
//!
//! A `CompletionSpec` is what the `complete` builtin builds and stores
//! per command name. `resolve_spec()` is the pure-ish function that
//! turns a spec plus a completion context into a list of candidate
//! strings. It is reused by tab-time dispatch (`completion.rs`) AND by
//! the `compgen` builtin.

use std::collections::HashMap;

use crate::shell_state::Shell;

/// Per-command + default + empty completion specs.
#[derive(Debug, Default, Clone)]
pub struct CompletionSpecs {
    pub by_command: HashMap<String, CompletionSpec>,
    pub default_spec: Option<CompletionSpec>,
    pub empty_spec: Option<CompletionSpec>,
}

/// A single completion spec. Multiple content generators (`-F`, `-W`,
/// `-G`, `-A`) may be set simultaneously; their results are concatenated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionSpec {
    pub function: Option<String>,
    pub wordlist: Option<String>,
    pub glob: Option<String>,
    pub actions: Vec<Action>,

    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub filter: Option<String>,

    pub options: CompOptions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompOptions {
    pub default: bool,
    pub nospace: bool,
    pub filenames: bool,
    pub bashdefault: bool,
    pub dirnames: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    File,
    Directory,
    Command,
    Function,
    Variable,
    Alias,
    Builtin,
    Keyword,
}

impl Action {
    /// Parses the short-form name accepted by `complete -A` / `compgen -A`.
    /// Returns `None` for unsupported actions (caller surfaces the diag).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "file" => Some(Action::File),
            "directory" => Some(Action::Directory),
            "command" => Some(Action::Command),
            "function" => Some(Action::Function),
            "variable" => Some(Action::Variable),
            "alias" => Some(Action::Alias),
            "builtin" => Some(Action::Builtin),
            "keyword" => Some(Action::Keyword),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Action::File => "file",
            Action::Directory => "directory",
            Action::Command => "command",
            Action::Function => "function",
            Action::Variable => "variable",
            Action::Alias => "alias",
            Action::Builtin => "builtin",
            Action::Keyword => "keyword",
        }
    }
}

/// The set of bash shell keywords huck recognizes. Used by `-A keyword`.
const SHELL_KEYWORDS: &[&str] = &[
    "!", "[[", "]]", "case", "do", "done", "elif", "else", "esac",
    "fi", "for", "function", "if", "in", "then", "until", "while", "{", "}",
];

/// Completion context for a single `resolve_spec` call.
#[derive(Debug, Clone)]
pub struct CompletionCtx {
    /// The command name (word 0 of the simple command).
    pub cmd_name: String,
    /// The word the cursor is on (possibly empty).
    pub cur_word: String,
    /// The word at index COMP_CWORD - 1, or "" if cursor is on word 0.
    pub prev_word: String,
    /// Full COMP_WORDS list including separator-words from COMP_WORDBREAKS.
    pub comp_words: Vec<String>,
    /// Index of the cursor word in `comp_words`.
    pub comp_cword: usize,
    /// The full input line.
    pub comp_line: String,
    /// Byte offset of the cursor in the line.
    pub comp_point: usize,
}

/// Runs every generator in the spec, decorates, filters, and returns
/// the resulting candidate strings. `-F` invocation is delegated to
/// `call_completion_function` (filled in by Task 4); for now it returns
/// an empty vec.
///
/// Note: this does NOT apply `-o filenames` rendering or `-o default`/
/// `bashdefault` empty-fallback. Those are the caller's responsibility
/// (Task 5) because they depend on rustyline / dispatch-ladder state.
pub fn resolve_spec(
    spec: &CompletionSpec,
    ctx: &CompletionCtx,
    shell: &mut Shell,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // -F func: deferred to Task 4. For now this stays empty.
    if let Some(func_name) = &spec.function {
        let mut from_func = call_completion_function(func_name, spec, ctx, shell);
        out.append(&mut from_func);
    }

    // -W wordlist: IFS-split the raw wordlist string AT USE TIME and
    // keep entries whose prefix matches cur_word.
    if let Some(wordlist) = &spec.wordlist {
        let ifs = shell.ifs();
        let words = split_wordlist(wordlist, &ifs);
        for w in words {
            if w.starts_with(&ctx.cur_word) {
                out.push(w);
            }
        }
    }

    // -G glob: shell-glob expansion against CWD; keep matches whose
    // basename starts with cur_word.
    if let Some(glob_pat) = &spec.glob {
        for matched in expand_glob(glob_pat) {
            if filename_matches_prefix(&matched, &ctx.cur_word) {
                out.push(matched);
            }
        }
    }

    // -A action: enumerate predefined sources, filtered by cur_word.
    for action in &spec.actions {
        let mut from_action = enumerate_action(*action, &ctx.cur_word, shell);
        out.append(&mut from_action);
    }

    // -X pattern filter. `pat` removes matches; `!pat` keeps only matches.
    if let Some(filter) = &spec.filter {
        let (pattern, invert) = match filter.strip_prefix('!') {
            Some(rest) => (rest, true),
            None => (filter.as_str(), false),
        };
        out.retain(|s| {
            let matches = glob_match(pattern, s);
            if invert { matches } else { !matches }
        });
    }

    // -P prefix / -S suffix decoration.
    if let Some(prefix) = &spec.prefix {
        for s in out.iter_mut() {
            *s = format!("{prefix}{s}");
        }
    }
    if let Some(suffix) = &spec.suffix {
        for s in out.iter_mut() {
            *s = format!("{s}{suffix}");
        }
    }

    out
}

/// Placeholder. Task 4 replaces this with the real -F invocation that
/// sets COMP_*, calls the function via the executor, and reads COMPREPLY.
pub fn call_completion_function(
    _func_name: &str,
    _spec: &CompletionSpec,
    _ctx: &CompletionCtx,
    _shell: &mut Shell,
) -> Vec<String> {
    Vec::new()
}

/// Splits a -W wordlist on the IFS bytes. Whitespace IFS bytes (space,
/// tab, newline) collapse runs and strip leading/trailing; non-whitespace
/// IFS bytes each delimit a single field.
fn split_wordlist(wordlist: &str, ifs: &str) -> Vec<String> {
    if ifs.is_empty() {
        return vec![wordlist.to_string()];
    }
    let ws: Vec<char> = ifs.chars().filter(|c| c.is_ascii_whitespace()).collect();
    let non_ws: Vec<char> = ifs.chars().filter(|c| !c.is_ascii_whitespace()).collect();

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = wordlist.chars().peekable();
    // Strip leading whitespace-IFS.
    while let Some(&c) = chars.peek() {
        if ws.contains(&c) { chars.next(); } else { break; }
    }
    while let Some(c) = chars.next() {
        if ws.contains(&c) {
            // Collapse run of whitespace-IFS.
            while let Some(&n) = chars.peek() {
                if ws.contains(&n) { chars.next(); } else { break; }
            }
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if non_ws.contains(&c) {
            // Each non-ws IFS byte ends a field.
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn expand_glob(pattern: &str) -> Vec<String> {
    match glob::glob(pattern) {
        Ok(paths) => paths
            .filter_map(|p| p.ok())
            .filter_map(|p| p.to_str().map(|s| s.to_string()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn filename_matches_prefix(path: &str, prefix: &str) -> bool {
    let basename = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    basename.starts_with(prefix)
}

fn glob_match(pattern: &str, candidate: &str) -> bool {
    match glob::Pattern::new(pattern) {
        Ok(p) => p.matches(candidate),
        Err(_) => false,
    }
}

fn enumerate_action(action: Action, prefix: &str, shell: &Shell) -> Vec<String> {
    match action {
        Action::File => list_dir_filtered(".", prefix, false),
        Action::Directory => list_dir_filtered(".", prefix, true),
        Action::Command => {
            // Reuse src/completion.rs::complete_command which already
            // walks PATH + builtins. Return just the replacement strings.
            let path = shell.get("PATH").unwrap_or("").to_string();
            crate::completion::complete_command(prefix, &path)
                .into_iter()
                .map(|c| c.replacement)
                .collect()
        }
        Action::Function => {
            let mut names: Vec<String> = shell
                .functions
                .keys()
                .filter(|n| n.starts_with(prefix))
                .cloned()
                .collect();
            names.sort();
            names
        }
        Action::Variable => {
            let mut names: Vec<String> = shell
                .var_names()
                .filter(|n| n.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            names.sort();
            names.dedup();
            names
        }
        Action::Alias => {
            let mut names: Vec<String> = shell
                .aliases
                .keys()
                .filter(|n| n.starts_with(prefix))
                .cloned()
                .collect();
            names.sort();
            names
        }
        Action::Builtin => {
            let mut names: Vec<String> = crate::builtins::BUILTIN_NAMES
                .iter()
                .filter(|n| n.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            names.sort();
            names
        }
        Action::Keyword => SHELL_KEYWORDS
            .iter()
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(),
    }
}

fn list_dir_filtered(dir: &str, prefix: &str, dirs_only: bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let show_hidden = prefix.starts_with('.');
    let mut out: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        if !name.starts_with(prefix) {
            continue;
        }
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        if dirs_only {
            let is_dir = std::fs::metadata(entry.path())
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if !is_dir {
                continue;
            }
        }
        out.push(name.to_string());
    }
    out.sort();
    out
}
```

Note on `Action::File`: bash's `-A file` returns everything in CWD (files AND directories — `-o filenames` is the modifier that adds trailing `/`). The dirs-only variant is `Action::Directory`. The `list_dir_filtered(".", prefix, false)` arm matches that semantic.

- [ ] **Step 3: Add `mod completion_spec;` to `src/main.rs`**

Edit `src/main.rs`. Find the existing `mod ...;` declarations near the top. Add:

```rust
mod completion_spec;
```

Place it alphabetically between the existing `mod completion;` and the next module.

- [ ] **Step 4: Compile to confirm wiring**

Run: `cargo build 2>&1 | tail -20`
Expected: clean build. If `Shell::aliases` doesn't exist, peek at how `M-63: aliases` stores them — likely `shell.aliases: HashMap<String, String>`. Use the same accessor pattern.

If `shell.aliases` doesn't exist as a field but the data is on `Shell`, look up the actual field name with: `grep -n "aliases" src/shell_state.rs | head -5` and adjust the `Action::Alias` arm accordingly.

- [ ] **Step 5: Write failing unit tests for `resolve_spec` static generators**

Add a `#[cfg(test)] mod tests` block at the bottom of `src/completion_spec.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;

    fn ctx(cur: &str) -> CompletionCtx {
        CompletionCtx {
            cmd_name: "cmd".to_string(),
            cur_word: cur.to_string(),
            prev_word: String::new(),
            comp_words: vec!["cmd".to_string(), cur.to_string()],
            comp_cword: 1,
            comp_line: format!("cmd {cur}"),
            comp_point: 4 + cur.len(),
        }
    }

    #[test]
    fn wordlist_filters_by_prefix() {
        let spec = CompletionSpec {
            wordlist: Some("alpha alpine beta".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx("al"), &mut sh);
        assert_eq!(got, vec!["alpha", "alpine"]);
    }

    #[test]
    fn wordlist_with_no_match_is_empty() {
        let spec = CompletionSpec {
            wordlist: Some("alpha beta".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx("z"), &mut sh);
        assert!(got.is_empty());
    }

    #[test]
    fn wordlist_respects_ifs() {
        let spec = CompletionSpec {
            wordlist: Some("alpha:apple:banana".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        sh.set("IFS", ":".to_string());
        let got = resolve_spec(&spec, &ctx("a"), &mut sh);
        assert_eq!(got, vec!["alpha", "apple"]);
    }

    #[test]
    fn action_function_enumerates_functions() {
        use crate::command::Command;
        let mut sh = Shell::new();
        // The simplest valid Command body — an empty Sequence.
        let body: Box<Command> = Box::new(Command::Sequence(crate::command::Sequence {
            pipelines: Vec::new(),
        }));
        sh.functions.insert("alpha".to_string(), body.clone());
        sh.functions.insert("alpine".to_string(), body.clone());
        sh.functions.insert("beta".to_string(), body);

        let spec = CompletionSpec {
            actions: vec![Action::Function],
            ..Default::default()
        };
        let got = resolve_spec(&spec, &ctx("al"), &mut sh);
        assert_eq!(got, vec!["alpha", "alpine"]);
    }

    #[test]
    fn action_builtin_enumerates_builtins() {
        let spec = CompletionSpec {
            actions: vec![Action::Builtin],
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx("ec"), &mut sh);
        assert!(got.contains(&"echo".to_string()), "{got:?}");
    }

    #[test]
    fn action_keyword_enumerates_keywords() {
        let spec = CompletionSpec {
            actions: vec![Action::Keyword],
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx("fo"), &mut sh);
        assert_eq!(got, vec!["for"]);
    }

    #[test]
    fn filter_removes_matches() {
        let spec = CompletionSpec {
            wordlist: Some("alpha apple banana cherry".to_string()),
            filter: Some("a*".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx(""), &mut sh);
        // "a*" removes alpha and apple; banana and cherry remain.
        assert_eq!(got, vec!["banana", "cherry"]);
    }

    #[test]
    fn filter_bang_keeps_only_matches() {
        let spec = CompletionSpec {
            wordlist: Some("alpha apple banana cherry".to_string()),
            filter: Some("!a*".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx(""), &mut sh);
        assert_eq!(got, vec!["alpha", "apple"]);
    }

    #[test]
    fn prefix_suffix_decorate_results() {
        let spec = CompletionSpec {
            wordlist: Some("a b".to_string()),
            prefix: Some("x:".to_string()),
            suffix: Some(":y".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx(""), &mut sh);
        assert_eq!(got, vec!["x:a:y", "x:b:y"]);
    }

    #[test]
    fn function_generator_returns_empty_for_now() {
        // Task 2: -F is reserved but not wired; placeholder returns empty.
        // Task 4 replaces call_completion_function.
        let spec = CompletionSpec {
            function: Some("_nonexistent".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx(""), &mut sh);
        assert!(got.is_empty());
    }

    #[test]
    fn action_parse_recognizes_known() {
        assert_eq!(Action::parse("file"), Some(Action::File));
        assert_eq!(Action::parse("directory"), Some(Action::Directory));
        assert_eq!(Action::parse("keyword"), Some(Action::Keyword));
    }

    #[test]
    fn action_parse_rejects_unknown() {
        assert_eq!(Action::parse("hostname"), None);
        assert_eq!(Action::parse("signal"), None);
    }

    #[test]
    fn split_wordlist_default_ifs_collapses_runs() {
        let got = split_wordlist("  a   b  c ", " \t\n");
        assert_eq!(got, vec!["a", "b", "c"]);
    }

    #[test]
    fn split_wordlist_non_ws_each_delimits() {
        // With IFS=":" (single non-ws byte), each ":" ends a field.
        // "a::b" → ["a", "", "b"].
        let got = split_wordlist("a::b", ":");
        assert_eq!(got, vec!["a", "", "b"]);
    }
}
```

- [ ] **Step 6: Run the new tests and confirm they pass**

Run: `cargo test --quiet completion_spec 2>&1 | tail -15`
Expected: all 14 tests pass.

If any fail, examine the failure and adjust the implementation. Common causes:
- `Shell::var_names` may not yield user-set vars by default — verify with: `grep -n "fn var_names" src/shell_state.rs`.
- `Shell::aliases` field name may be different — if compile fails on that line, run `grep -n "aliases" src/shell_state.rs` to find the actual name.

- [ ] **Step 7: Run the full suite to confirm no regressions**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 2:", sum}'`
Expected: 2161 (2147 from Task 1 + 14 new).

- [ ] **Step 8: Clippy clean**

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: `Finished`, no warnings.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v76 task 2: CompletionSpec data layer + static generators

New src/completion_spec.rs (~400 LOC): CompletionSpec, CompletionSpecs,
CompOptions, Action enum (8 supported actions); CompletionCtx struct
for resolve_spec inputs; resolve_spec() that drives the static
generators (-W wordlist, -G glob, -A action, -P prefix, -S suffix,
-X filter).

Shell gains two fields: completion_specs (the registry) and
current_completion_spec (ephemeral slot for compopt-in-function; used
by Task 6). Both Default-initialized.

-F func is reserved but not wired yet (call_completion_function is a
placeholder returning empty). Task 4 replaces it with the real
COMP_* setup/teardown and executor call.

14 new unit tests cover each generator in isolation plus the
wordlist splitter's IFS handling.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `complete` and `compgen` builtins (static-only)

**Files:**
- Create: `src/completion_builtins.rs` — `builtin_complete`, `builtin_compgen`, shared flag parser
- Modify: `src/builtins.rs` — add `"complete"`, `"compgen"` to `BUILTIN_NAMES`; two match arms in `run_builtin`
- Modify: `src/main.rs` — add `mod completion_builtins;`

**Goal:** `complete -W "..." cmd`, `complete -A action cmd`, `complete -F func cmd`, `complete -p`, `complete -r`, `complete -D`, and `compgen` (all the same generators + final positional arg) all work end-to-end. `-F` is *parsed and stored*; `compgen -F` returns empty (Task 4 wires the actual invocation).

### Steps

- [ ] **Step 1: Create `src/completion_builtins.rs` with the shared flag parser**

Create the new file `src/completion_builtins.rs`:

```rust
//! `complete`, `compgen`, `compopt` builtins. Flag parsing produces a
//! `CompletionSpec`; storage and resolution are delegated to the
//! `completion_spec` module.

use std::io::Write;

use crate::builtins::ExecOutcome;
use crate::completion_spec::{Action, CompOptions, CompletionCtx, CompletionSpec, CompletionSpecs};
use crate::shell_state::Shell;

/// Output of parsing a `complete` / `compgen` flag string.
#[derive(Debug, Default)]
struct ParsedFlags {
    spec: CompletionSpec,
    /// -D: apply to default (no other spec matched).
    is_default: bool,
    /// -E: apply when completing on empty command line.
    is_empty: bool,
    /// -p: print mode.
    print: bool,
    /// -r: remove mode.
    remove: bool,
    /// Trailing positional args (command names for `complete`, optional
    /// word arg for `compgen`).
    positional: Vec<String>,
}

#[derive(Debug)]
enum FlagError {
    Usage(String),
    InvalidAction(String),
    InvalidOption(String),
    MissingArg(char),
}

impl FlagError {
    fn diag(&self, cmd: &str) -> String {
        match self {
            FlagError::Usage(msg) => format!("huck: {cmd}: {msg}"),
            FlagError::InvalidAction(name) => {
                format!("huck: {cmd}: {name}: invalid action name")
            }
            FlagError::InvalidOption(name) => {
                format!("huck: {cmd}: {name}: invalid completion option")
            }
            FlagError::MissingArg(c) => {
                format!("huck: {cmd}: -{c}: option requires an argument")
            }
        }
    }

    fn status(&self) -> i32 {
        match self {
            FlagError::Usage(_) => 2,
            _ => 2,
        }
    }
}

/// Parses the flags. `allow_DE` controls whether `-D`/`-E`/`-p`/`-r`
/// are accepted (true for `complete`, false for `compgen`).
fn parse_flags(args: &[String], allow_d_e: bool) -> Result<ParsedFlags, FlagError> {
    let mut out = ParsedFlags::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') && !arg.starts_with('+') {
            break;
        }
        if arg == "-" || arg == "+" {
            return Err(FlagError::Usage(format!("bad option: {arg}")));
        }
        // Cluster: each character after the leading -/+ is a flag.
        // Flags that take an arg consume the remainder of the current
        // word (inline) OR the next word.
        let leading = arg.chars().next().unwrap();  // - or +
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut ci = 0;
        while ci < chars.len() {
            let c = chars[ci];
            match c {
                'F' | 'W' | 'G' | 'A' | 'P' | 'S' | 'X' | 'o' => {
                    if leading == '+' {
                        return Err(FlagError::Usage(format!("+{c}: not supported")));
                    }
                    // Argument is either the rest of this word or the next word.
                    let arg_value: String = if ci + 1 < chars.len() {
                        let v: String = chars[ci + 1..].iter().collect();
                        ci = chars.len();  // consume rest of this word
                        v
                    } else if i + 1 < args.len() {
                        i += 1;
                        ci = chars.len();
                        args[i].clone()
                    } else {
                        return Err(FlagError::MissingArg(c));
                    };
                    match c {
                        'F' => out.spec.function = Some(arg_value),
                        'W' => out.spec.wordlist = Some(arg_value),
                        'G' => out.spec.glob = Some(arg_value),
                        'A' => {
                            let action = Action::parse(&arg_value)
                                .ok_or_else(|| FlagError::InvalidAction(arg_value.clone()))?;
                            out.spec.actions.push(action);
                        }
                        'P' => out.spec.prefix = Some(arg_value),
                        'S' => out.spec.suffix = Some(arg_value),
                        'X' => out.spec.filter = Some(arg_value),
                        'o' => apply_option(&mut out.spec.options, &arg_value, leading == '+')?,
                        _ => unreachable!(),
                    }
                }
                'D' if allow_d_e => out.is_default = true,
                'E' if allow_d_e => out.is_empty = true,
                'p' if allow_d_e => out.print = true,
                'r' if allow_d_e => out.remove = true,
                other => {
                    return Err(FlagError::Usage(format!("-{other}: invalid option")));
                }
            }
            ci += 1;
        }
        i += 1;
    }
    out.positional = args[i..].to_vec();
    Ok(out)
}

fn apply_option(opts: &mut CompOptions, name: &str, off: bool) -> Result<(), FlagError> {
    let value = !off;
    match name {
        "default" => opts.default = value,
        "nospace" => opts.nospace = value,
        "filenames" => opts.filenames = value,
        "bashdefault" => opts.bashdefault = value,
        "dirnames" => opts.dirnames = value,
        // Recognized-but-rejected: parse error per spec.
        "nosort" | "noquote" | "plusdirs" => {
            return Err(FlagError::InvalidOption(name.to_string()));
        }
        _ => return Err(FlagError::InvalidOption(name.to_string())),
    }
    Ok(())
}

/// `complete` builtin.
pub fn builtin_complete(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_flags(args, true) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.diag("complete"));
            return ExecOutcome::Continue(e.status());
        }
    };

    // Mode: print
    if parsed.print || is_bare(&parsed) {
        return print_complete(&parsed.positional, out, shell);
    }
    // Mode: remove
    if parsed.remove {
        return remove_complete(&parsed.positional, &parsed, shell);
    }
    // Mode: register
    register_complete(&parsed, shell)
}

fn is_bare(parsed: &ParsedFlags) -> bool {
    let spec_empty = parsed.spec == CompletionSpec::default();
    spec_empty
        && !parsed.is_default
        && !parsed.is_empty
        && !parsed.remove
        && parsed.positional.is_empty()
}

fn print_complete(names: &[String], out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    let specs = &shell.completion_specs;
    let mut status: i32 = 0;
    if names.is_empty() {
        // Print all in sorted order: by_command first, then -D, then -E.
        let mut keys: Vec<&String> = specs.by_command.keys().collect();
        keys.sort();
        for k in keys {
            let _ = writeln!(out, "{}", format_spec_for_print(&specs.by_command[k], Some(k.as_str()), None));
        }
        if let Some(d) = &specs.default_spec {
            let _ = writeln!(out, "{}", format_spec_for_print(d, None, Some("-D")));
        }
        if let Some(e) = &specs.empty_spec {
            let _ = writeln!(out, "{}", format_spec_for_print(e, None, Some("-E")));
        }
    } else {
        for n in names {
            match specs.by_command.get(n) {
                Some(s) => {
                    let _ = writeln!(out, "{}", format_spec_for_print(s, Some(n.as_str()), None));
                }
                None => {
                    eprintln!("huck: complete: {n}: no completion specification");
                    status = 1;
                }
            }
        }
    }
    ExecOutcome::Continue(status)
}

fn remove_complete(names: &[String], parsed: &ParsedFlags, shell: &mut Shell) -> ExecOutcome {
    let specs = &mut shell.completion_specs;
    let mut status = 0;
    if parsed.is_default {
        specs.default_spec = None;
    }
    if parsed.is_empty {
        specs.empty_spec = None;
    }
    if names.is_empty() && !parsed.is_default && !parsed.is_empty {
        specs.by_command.clear();
    } else {
        for n in names {
            if specs.by_command.remove(n).is_none() && !parsed.is_default && !parsed.is_empty {
                eprintln!("huck: complete: {n}: no completion specification");
                status = 1;
            }
        }
    }
    ExecOutcome::Continue(status)
}

fn register_complete(parsed: &ParsedFlags, shell: &mut Shell) -> ExecOutcome {
    if (parsed.is_default || parsed.is_empty) && !parsed.positional.is_empty() {
        eprintln!("huck: complete: cannot use -D or -E with command names");
        return ExecOutcome::Continue(2);
    }
    if !parsed.positional.is_empty() && parsed.spec == CompletionSpec::default() {
        eprintln!("huck: complete: nothing to complete");
        return ExecOutcome::Continue(1);
    }
    if parsed.is_default {
        shell.completion_specs.default_spec = Some(parsed.spec.clone());
    }
    if parsed.is_empty {
        shell.completion_specs.empty_spec = Some(parsed.spec.clone());
    }
    for n in &parsed.positional {
        shell.completion_specs.by_command.insert(n.clone(), parsed.spec.clone());
    }
    ExecOutcome::Continue(0)
}

/// Renders a spec for `complete -p` in deterministic re-input form.
fn format_spec_for_print(spec: &CompletionSpec, name: Option<&str>, mode: Option<&str>) -> String {
    let mut parts: Vec<String> = vec!["complete".to_string()];
    if let Some(m) = mode {
        parts.push(m.to_string());
    }
    if let Some(f) = &spec.function {
        parts.push(format!("-F {}", crate::builtins::escape_alias_value(f)));
    }
    if let Some(w) = &spec.wordlist {
        parts.push(format!("-W {}", crate::builtins::escape_alias_value(w)));
    }
    if let Some(g) = &spec.glob {
        parts.push(format!("-G {}", crate::builtins::escape_alias_value(g)));
    }
    for a in &spec.actions {
        parts.push(format!("-A {}", a.as_str()));
    }
    if let Some(p) = &spec.prefix {
        parts.push(format!("-P {}", crate::builtins::escape_alias_value(p)));
    }
    if let Some(s) = &spec.suffix {
        parts.push(format!("-S {}", crate::builtins::escape_alias_value(s)));
    }
    if let Some(x) = &spec.filter {
        parts.push(format!("-X {}", crate::builtins::escape_alias_value(x)));
    }
    let CompOptions { default, nospace, filenames, bashdefault, dirnames } = spec.options;
    if default { parts.push("-o default".to_string()); }
    if nospace { parts.push("-o nospace".to_string()); }
    if filenames { parts.push("-o filenames".to_string()); }
    if bashdefault { parts.push("-o bashdefault".to_string()); }
    if dirnames { parts.push("-o dirnames".to_string()); }
    if let Some(n) = name {
        parts.push("--".to_string());
        parts.push(n.to_string());
    }
    parts.join(" ")
}

/// `compgen` builtin.
pub fn builtin_compgen(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_flags(args, false) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.diag("compgen"));
            return ExecOutcome::Continue(e.status());
        }
    };

    let word = parsed.positional.first().cloned().unwrap_or_default();
    let ctx = CompletionCtx {
        cmd_name: "compgen".to_string(),
        cur_word: word.clone(),
        prev_word: String::new(),
        comp_words: vec![word.clone()],
        comp_cword: 0,
        comp_line: word.clone(),
        comp_point: word.len(),
    };
    let results = crate::completion_spec::resolve_spec(&parsed.spec, &ctx, shell);
    let any = !results.is_empty();
    for r in results {
        let _ = writeln!(out, "{r}");
    }
    ExecOutcome::Continue(if any { 0 } else { 1 })
}
```

- [ ] **Step 2: Confirm `crate::builtins::escape_alias_value` is `pub(crate)`**

Run: `grep -n "fn escape_alias_value" src/builtins.rs | head -3`
Expected: `pub(crate) fn escape_alias_value(...)` or `pub fn escape_alias_value(...)`. If it's currently private (`fn escape_alias_value`), promote it to `pub(crate)`. Edit `src/builtins.rs` at that line to add `pub(crate)` before `fn`.

- [ ] **Step 3: Add `mod completion_builtins;` to `src/main.rs`**

Edit `src/main.rs`. Add alphabetically (after `mod completion_spec;` from Task 2):

```rust
mod completion_builtins;
```

- [ ] **Step 4: Wire `complete` and `compgen` into `run_builtin`**

Edit `src/builtins.rs`. In the `pub const BUILTIN_NAMES` declaration (line 19), add `"complete"` and `"compgen"`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
    "readonly", "read", "printf", "type", "hash",
    "pushd", "popd", "dirs",
    "declare", "typeset",
    "eval",
    "help",
    "complete", "compgen",
];
```

In `pub fn run_builtin` (around line 78), add two match arms after the `"help"` arm:

```rust
        "complete" => crate::completion_builtins::builtin_complete(args, out, shell),
        "compgen" => crate::completion_builtins::builtin_compgen(args, out, shell),
```

(`compopt` is Task 6; not added here yet.)

- [ ] **Step 5: Compile to confirm**

Run: `cargo build 2>&1 | tail -10`
Expected: clean build. If there's a name collision or visibility issue, address it.

- [ ] **Step 6: Write integration test for `complete -p` round-trip**

Edit `src/completion_builtins.rs`. Add at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run_complete(args: &[&str], shell: &mut Shell) -> (String, i32) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::<u8>::new();
        let outcome = builtin_complete(&argv, &mut out, shell);
        let s = String::from_utf8(out).unwrap();
        let code = match outcome {
            ExecOutcome::Continue(n) => n,
            _ => panic!("complete should not return non-Continue"),
        };
        (s, code)
    }

    fn run_compgen(args: &[&str], shell: &mut Shell) -> (String, i32) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::<u8>::new();
        let outcome = builtin_compgen(&argv, &mut out, shell);
        let s = String::from_utf8(out).unwrap();
        let code = match outcome {
            ExecOutcome::Continue(n) => n,
            _ => panic!("compgen should not return non-Continue"),
        };
        (s, code)
    }

    #[test]
    fn complete_registers_and_prints() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-W", "alpha alpine beta", "--", "myc"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command.contains_key("myc"));
        let spec = &sh.completion_specs.by_command["myc"];
        assert_eq!(spec.wordlist, Some("alpha alpine beta".to_string()));

        let (out, code) = run_complete(&["-p", "myc"], &mut sh);
        assert_eq!(code, 0);
        assert!(out.contains("complete"));
        assert!(out.contains("-W"));
        assert!(out.contains("alpha alpine beta"));
        assert!(out.contains("myc"));
    }

    #[test]
    fn complete_unknown_name_for_p_returns_1() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-p", "nope"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn complete_r_removes_spec() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "foo"], &mut sh);
        assert!(sh.completion_specs.by_command.contains_key("foo"));
        let (_, code) = run_complete(&["-r", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert!(!sh.completion_specs.by_command.contains_key("foo"));
    }

    #[test]
    fn complete_r_missing_name_returns_1() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-r", "ghost"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn complete_r_bare_clears_all_by_command() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "a"], &mut sh);
        let _ = run_complete(&["-W", "y", "--", "b"], &mut sh);
        let (_, code) = run_complete(&["-r"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command.is_empty());
    }

    #[test]
    fn complete_D_sets_default_spec() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-D", "-W", "fallback"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.default_spec.is_some());
    }

    #[test]
    fn complete_D_with_names_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-D", "-W", "x", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn complete_invalid_action_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-A", "hostname", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn complete_invalid_option_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-o", "nosort", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn complete_inline_flag_arg() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-Falpha", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(sh.completion_specs.by_command["foo"].function, Some("alpha".to_string()));
    }

    #[test]
    fn complete_nothing_to_complete_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["foo"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn compgen_W_filters_by_prefix_arg() {
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-W", "alpha alpine beta", "al"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(out, "alpha\nalpine\n");
    }

    #[test]
    fn compgen_no_match_returns_1() {
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-W", "a b c", "z"], &mut sh);
        assert_eq!(code, 1);
        assert_eq!(out, "");
    }

    #[test]
    fn compgen_A_builtin() {
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-A", "builtin", "ec"], &mut sh);
        assert_eq!(code, 0);
        assert!(out.contains("echo"));
    }

    #[test]
    fn complete_multiple_actions_accumulate() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-A", "builtin", "-A", "keyword", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        let acts = &sh.completion_specs.by_command["foo"].actions;
        assert_eq!(acts.len(), 2);
        assert!(acts.contains(&Action::Builtin));
        assert!(acts.contains(&Action::Keyword));
    }

    #[test]
    fn complete_print_form_round_trips_wordlist() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "a b c", "--", "myc"], &mut sh);
        let (out, _) = run_complete(&["-p", "myc"], &mut sh);
        // The output should be a complete-form line that, if re-parsed,
        // produces the same spec.
        assert!(out.starts_with("complete "));
        assert!(out.contains("-W 'a b c'") || out.contains("-W \"a b c\""));
        assert!(out.contains("-- myc"));
    }
}
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test --quiet completion_builtins 2>&1 | tail -10`
Expected: 15 tests pass.

- [ ] **Step 8: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 3:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```
Expected: 2176 tests pass (2161 + 15 new). Clippy clean.

- [ ] **Step 9: Smoke-test from the binary**

```bash
cat <<'EOS' | cargo run --quiet
complete -W "alpha alpine beta" -- myc
compgen -W "alpha alpine beta" -- al
complete -p myc
EOS
```

Expected:
```
alpha
alpine
complete -W 'alpha alpine beta' -- myc
```

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v76 task 3: complete + compgen builtins (static-only)

New src/completion_builtins.rs (~600 LOC): shared flag parser
(parse_flags), builtin_complete (register/print/remove modes),
builtin_compgen (thin wrapper around resolve_spec). Three new entries
in BUILTIN_NAMES; two match arms in run_builtin.

Flag surface (Standard tier):
  complete [-DE] [-F func] [-W wordlist] [-G glob] [-A action]...
           [-P prefix] [-S suffix] [-X filter] [-o option]... [--]
           [name ...]
  complete -p [name ...]
  complete -r [name ...]
  compgen  [-F func] [-W wordlist] ... [--] [word]

-F is parsed and stored but its execution is deferred to Task 4
(compgen -F currently returns empty because call_completion_function
is still a placeholder).

complete -p re-input form uses a deterministic flag ordering (-F, -W,
-G, then each -A, -P, -S, -X, then each -o, then -- name).

15 new unit tests cover register/print/remove modes, mutual exclusion,
inline flag args, invalid action/option diagnostics, multi-action
accumulation.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `-F` function invocation

**Files:**
- Modify: `src/executor.rs` — promote `call_function` to `pub(crate)`; add `call_function_body` wrapper
- Modify: `src/completion_spec.rs` — replace placeholder `call_completion_function` with the real implementation

**Goal:** `compgen -F func -- prefix` invokes `func` with `$1=cmd_name`, `$2=cur_word`, `$3=prev_word`, sets `COMP_*` variables, reads `COMPREPLY` back. Positional params and `$?` are restored on return. Unlocks every real-world completion script that uses `-F`.

### Steps

- [ ] **Step 1: Promote `call_function` to `pub(crate)` and add `call_function_body`**

Edit `src/executor.rs`. Find `fn call_function` (line 1572). Change the visibility:

```rust
pub(crate) fn call_function(
```

Below `call_function` (after line 1613), add a new wrapper that accepts a function NAME (rather than the body) — this is what completion needs because the spec only stores the function name:

```rust
/// Invokes a function by name with the given args. Looks up the body
/// from `shell.functions`. Returns `ExecOutcome::Continue(1)` if the
/// function doesn't exist. Stdout from the function goes to the real
/// stdout (matches bash's behavior where completion functions that
/// print produce visible output).
pub(crate) fn call_function_body(
    name: &str,
    args: Vec<String>,
    shell: &mut Shell,
) -> ExecOutcome {
    let body = match shell.functions.get(name) {
        Some(b) => b.clone(),
        None => return ExecOutcome::Continue(1),
    };
    let mut sink = StdoutSink::Real;
    call_function(name, body, args, shell, &mut sink)
}
```

If `StdoutSink::Real` isn't the right variant name, peek at the existing definition: `grep -n "enum StdoutSink\|StdoutSink::" src/executor.rs | head -5`. Use whatever variant represents "write to real stdout" (likely `StdoutSink::Real` or `StdoutSink::Stdout` — match the existing pattern).

- [ ] **Step 2: Replace the placeholder in `src/completion_spec.rs`**

Edit `src/completion_spec.rs`. Replace the entire `pub fn call_completion_function` body with the real implementation:

```rust
/// Invokes a -F completion function. Sets COMP_*, positional params,
/// calls the function via the executor, reads COMPREPLY, then restores
/// positional params and $?. COMP_* are LEFT SET after return (matches
/// bash — they're meant to remain readable until next completion).
pub fn call_completion_function(
    func_name: &str,
    spec: &CompletionSpec,
    ctx: &CompletionCtx,
    shell: &mut Shell,
) -> Vec<String> {
    use crate::shell_state::VarValue;

    // 1. Snapshot variables we'll mutate so we can restore on return.
    //    (Positional args are saved by call_function internally.)
    let saved_last_status = shell.last_status();
    let saved_reply = shell.snapshot_var("COMPREPLY");

    // 2. Set COMP_* shell vars.
    shell.set("COMP_LINE", ctx.comp_line.clone());
    shell.set("COMP_POINT", ctx.comp_point.to_string());
    shell.set("COMP_CWORD", ctx.comp_cword.to_string());

    // COMP_WORDS as indexed array.
    let mut words_map = std::collections::BTreeMap::new();
    for (i, w) in ctx.comp_words.iter().enumerate() {
        words_map.insert(i, w.clone());
    }
    shell.replace_array("COMP_WORDS", VarValue::Indexed(words_map));

    // Clear COMPREPLY so the function can detect "not set yet" if it
    // wants — and so an empty result is unambiguous.
    shell.unset("COMPREPLY");

    // 3. Stash the spec for compopt-in-function mutation (Task 6 reads this).
    let prior_current_spec = shell.current_completion_spec.take();
    shell.current_completion_spec = Some(spec.clone());

    // 4. Build positional args [cmd_name, cur_word, prev_word].
    let pos_args = vec![
        ctx.cmd_name.clone(),
        ctx.cur_word.clone(),
        ctx.prev_word.clone(),
    ];

    // 5. Invoke. ExecOutcome::Exit propagates UP via the result; our
    //    caller (rustyline complete()) doesn't have a graceful path to
    //    propagate Exit, so for now we treat Exit/Continue/Return
    //    identically: the function "finished" and COMPREPLY is read.
    //    Bash actually exits the shell on `exit` inside a completion
    //    function; honoring that requires plumbing through rustyline
    //    which is out of scope. We document as known.
    let _outcome = crate::executor::call_function_body(func_name, pos_args, shell);

    // 6. Take spec back out (compopt may have mutated it).
    let _resolved_spec = shell.current_completion_spec.take();
    shell.current_completion_spec = prior_current_spec;

    // 7. Read COMPREPLY.
    let reply_values: Vec<String> = match shell.get_array("COMPREPLY") {
        Some(map) => {
            // Values in index order.
            let mut items: Vec<(&usize, &String)> = map.iter().collect();
            items.sort_by_key(|(k, _)| **k);
            items.into_iter().map(|(_, v)| v.clone()).collect()
        }
        None => Vec::new(),
    };

    // 8. Restore COMPREPLY (compgen / next completion expects a clean slot).
    shell.restore_var("COMPREPLY", saved_reply);

    // 9. Restore $?. Completion functions do NOT pollute the user's $?.
    shell.set_last_status(saved_last_status);

    // 10. Drain any pending fatal PE error from inside the function so
    //     the next prompt is clean.
    let _ = shell.take_pending_fatal_pe_error();

    reply_values
}
```

NOTE: the spec mentions "honor `exit` from inside `-F`" but rustyline doesn't have a clean way to propagate `ExecOutcome::Exit` out of `complete()` (it returns `rustyline::Result<...>`). We deviate from the spec on this single point: `exit` inside a completion function returns immediately to dispatch with the COMPREPLY-so-far rather than terminating the shell. This is a small known divergence; document in Task 7's bash-divergences.md update.

If `Shell::replace_array` doesn't exist with that signature, look at v71's M-82 commit for the actual array-write API: `grep -n "fn replace_array\|fn set_array\|VarValue::Indexed" src/shell_state.rs | head -10`. The plan above assumes `replace_array(name, VarValue)`; adjust if the actual signature differs.

Similarly for `get_array`: confirm it returns `Option<&BTreeMap<usize, String>>` per `grep -n "fn get_array" src/shell_state.rs`.

- [ ] **Step 3: Write a failing test exercising the real -F path**

Edit `src/completion_spec.rs`. Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn function_invocation_reads_compreply() {
        use crate::command::Command;
        let mut sh = Shell::new();

        // Define a function whose body sets COMPREPLY=(alpha beta).
        // Building the AST by hand is painful; route through process_line.
        let do_alias = false;
        let outcome = crate::shell::process_line(
            "_myf() { COMPREPLY=(alpha beta); }",
            &mut sh,
            do_alias,
        );
        assert!(matches!(outcome, crate::builtins::ExecOutcome::Continue(_)));
        assert!(sh.functions.contains_key("_myf"));

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let got = resolve_spec(&spec, &ctx(""), &mut sh);
        assert_eq!(got, vec!["alpha", "beta"]);
    }

    #[test]
    fn function_invocation_sets_comp_words() {
        let mut sh = Shell::new();
        // Function copies COMP_WORDS[1] into COMPREPLY[0].
        let _ = crate::shell::process_line(
            "_myf() { COMPREPLY=(\"${COMP_WORDS[1]}\"); }",
            &mut sh,
            false,
        );

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let mut c = ctx("");
        c.comp_words = vec!["cmd".to_string(), "expected_word".to_string()];
        c.comp_cword = 1;
        let got = resolve_spec(&spec, &c, &mut sh);
        assert_eq!(got, vec!["expected_word"]);
    }

    #[test]
    fn function_invocation_positional_params() {
        let mut sh = Shell::new();
        // Function copies $1, $2, $3 into COMPREPLY (three elements).
        let _ = crate::shell::process_line(
            "_myf() { COMPREPLY=(\"$1\" \"$2\" \"$3\"); }",
            &mut sh,
            false,
        );

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let c = CompletionCtx {
            cmd_name: "git".to_string(),
            cur_word: "che".to_string(),
            prev_word: "checkout".to_string(),
            comp_words: vec!["git".to_string(), "checkout".to_string(), "che".to_string()],
            comp_cword: 2,
            comp_line: "git checkout che".to_string(),
            comp_point: 16,
        };
        let got = resolve_spec(&spec, &c, &mut sh);
        assert_eq!(got, vec!["git", "che", "checkout"]);
    }

    #[test]
    fn function_invocation_preserves_last_status() {
        let mut sh = Shell::new();
        sh.set_last_status(42);
        // Function exits with 17.
        let _ = crate::shell::process_line(
            "_myf() { COMPREPLY=(x); return 17; }",
            &mut sh,
            false,
        );

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let _ = resolve_spec(&spec, &ctx(""), &mut sh);
        // Completion functions must NOT pollute $?.
        assert_eq!(sh.last_status(), 42);
    }

    #[test]
    fn function_missing_returns_empty() {
        let mut sh = Shell::new();
        let spec = CompletionSpec {
            function: Some("_does_not_exist".to_string()),
            ..Default::default()
        };
        let got = resolve_spec(&spec, &ctx(""), &mut sh);
        assert!(got.is_empty());
    }
```

If `crate::shell::process_line` isn't pub(crate), verify with: `grep -n "pub fn process_line\|fn process_line" src/shell.rs`. If it's only `pub(crate)` it's fine; if it's `pub(super)`-style, adjust accordingly.

- [ ] **Step 4: Run the new tests**

Run: `cargo test --quiet completion_spec::tests::function 2>&1 | tail -15`
Expected: 5 new tests pass.

- [ ] **Step 5: Update Task 3's `function_generator_returns_empty_for_now` test**

This test was correct as of Task 2 but is no longer truthful — a non-existent function still returns empty, but the test description was about the placeholder. Update its name and assertion in `src/completion_spec.rs`:

Find:
```rust
    #[test]
    fn function_generator_returns_empty_for_now() {
        // Task 2: -F is reserved but not wired; placeholder returns empty.
        // Task 4 replaces call_completion_function.
        let spec = CompletionSpec {
            function: Some("_nonexistent".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = resolve_spec(&spec, &ctx(""), &mut sh);
        assert!(got.is_empty());
    }
```

This test is now redundant with `function_missing_returns_empty` from Step 3 (they assert the same thing on the same spec). Delete `function_generator_returns_empty_for_now`.

- [ ] **Step 6: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 4:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```
Expected: 2180 tests pass (2176 - 1 deleted + 5 new = 2180). Clippy clean.

- [ ] **Step 7: Smoke-test from the binary**

```bash
cat <<'EOS' | cargo run --quiet
_myf() { COMPREPLY=(alpha beta gamma); }
complete -F _myf myc
compgen -F _myf -- ""
EOS
```

Expected:
```
alpha
beta
gamma
```

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v76 task 4: -F function invocation

Real call_completion_function in src/completion_spec.rs: sets
COMP_LINE/COMP_POINT/COMP_CWORD/COMP_WORDS, stashes the live spec in
Shell.current_completion_spec (for compopt — Task 6), sets positional
args to [cmd_name, cur_word, prev_word], invokes the function via a
new pub(crate) crate::executor::call_function_body wrapper, reads
COMPREPLY back as a list, restores COMPREPLY and $?.

call_function in src/executor.rs promoted from private to pub(crate)
and a thin call_function_body(name, args, shell) wrapper added that
looks up the function body and uses StdoutSink::Real (function stdout
leaks to terminal; matches bash).

Known divergence: `exit` inside a completion function does NOT
terminate the shell (rustyline complete() can't propagate
ExecOutcome::Exit). Function returns immediately with COMPREPLY-so-far.
Documented in bash-divergences.md (Task 7).

5 new unit tests cover COMPREPLY readback, COMP_WORDS visibility,
positional-param setup, $? preservation, and missing-function path.

Unlocks compgen -F end-to-end. Tab-time dispatch is Task 5.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Tab-completion dispatch

**Files:**
- Modify: `src/completion.rs` — add `dispatch` submodule with `resolve()`; rewrite `HuckHelper::complete` body to call it

**Goal:** When the user presses Tab on `cmd <args>` where `cmd` has a registered spec (or `default_spec`/`empty_spec` applies), the spec fires. COMP_WORDS is tokenized per `COMP_WORDBREAKS`. Empty results fall back per `-o default`/`-o bashdefault`. `-o filenames` renders directories with trailing `/`.

### Steps

- [ ] **Step 1: Add the dispatch submodule scaffold**

Edit `src/completion.rs`. Above the existing `#[cfg(test)] mod tests` block, add:

```rust
pub(crate) mod dispatch {
    //! Tab-time dispatch ladder. Decides which completion source
    //! handles the cursor position: variable, command-pos commands,
    //! a registered -F spec, default-spec fallback, or file completion.

    use super::*;
    use crate::completion_spec::{CompletionCtx, CompletionSpec, resolve_spec};
    use crate::shell_state::Shell;

    /// Entry point. Returns (start_offset, candidates) for rustyline.
    pub fn resolve(line: &str, pos: usize, shell: &mut Shell) -> (usize, Vec<Candidate>) {
        let (start, context) = analyze(line, pos);

        // Path 1: variable context — always wins, no spec lookup.
        if let CompletionContext::Variable { prefix } = &context {
            let var_names: Vec<String> = shell.var_names().map(|s| s.to_string()).collect();
            return (start, complete_variable(prefix, &var_names));
        }

        // Path 2: command position.
        if let CompletionContext::Command { prefix } = &context {
            // -E: empty command line + an -E spec.
            if prefix.is_empty() && line[..pos].trim().is_empty() {
                if let Some(spec) = shell.completion_specs.empty_spec.clone() {
                    let cands = run_spec_with_empty_fallback(
                        &spec, line, pos, "", shell,
                    );
                    return (start, cands);
                }
            }
            let path = shell.get("PATH").unwrap_or("").to_string();
            return (start, complete_command(prefix, &path));
        }

        // Path 3: file/argument position.
        let CompletionContext::File { dir, prefix } = &context else {
            // analyze() returns one of three; unreachable.
            return (start, Vec::new());
        };

        let cmd_name = extract_command_name(&line[..pos]).unwrap_or_default();

        let spec_opt: Option<CompletionSpec> = shell
            .completion_specs
            .by_command
            .get(&cmd_name)
            .cloned()
            .or_else(|| shell.completion_specs.default_spec.clone());

        match spec_opt {
            Some(spec) => {
                let cands = run_spec_with_empty_fallback(
                    &spec, line, pos, &cmd_name, shell,
                );
                if cands.is_empty() {
                    // No spec hit AND no fallback flag set → empty result.
                    return (start, Vec::new());
                }
                (start, cands)
            }
            None => {
                // No spec at all → existing file completion.
                let home = shell.get("HOME").unwrap_or("").to_string();
                (start, complete_file(dir, prefix, &home))
            }
        }
    }

    /// Runs `resolve_spec` on the spec, applies `-o filenames` rendering
    /// and the empty-fallback (`-o default` / `-o bashdefault`).
    fn run_spec_with_empty_fallback(
        spec: &CompletionSpec,
        line: &str,
        pos: usize,
        cmd_name: &str,
        shell: &mut Shell,
    ) -> Vec<Candidate> {
        let wordbreaks = shell.get("COMP_WORDBREAKS")
            .unwrap_or(" \t\n")
            .to_string();
        let (comp_words, comp_cword) = tokenize_comp_words(&line[..pos], &wordbreaks);
        let cur_word = comp_words.get(comp_cword).cloned().unwrap_or_default();
        let prev_word = if comp_cword > 0 {
            comp_words.get(comp_cword - 1).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let ctx = CompletionCtx {
            cmd_name: cmd_name.to_string(),
            cur_word: cur_word.clone(),
            prev_word,
            comp_words,
            comp_cword,
            comp_line: line.to_string(),
            comp_point: pos,
        };

        let raw_results = resolve_spec(spec, &ctx, shell);

        // Take the (possibly mutated) options from current_completion_spec
        // back if Task 6's compopt has touched them.
        let effective_options = shell
            .current_completion_spec
            .take()
            .map(|s| s.options)
            .unwrap_or(spec.options);

        // Empty-fallback.
        let after_fallback: Vec<String> = if raw_results.is_empty() {
            if effective_options.default {
                file_completion_strings(&ctx.cur_word, shell)
            } else if effective_options.bashdefault {
                bashdefault_strings(line, pos, shell)
            } else {
                Vec::new()
            }
        } else {
            raw_results
        };

        // Filename rendering.
        let candidates: Vec<Candidate> = if effective_options.filenames {
            after_fallback
                .into_iter()
                .map(|name| {
                    let is_dir = std::fs::metadata(&name)
                        .map(|m| m.is_dir())
                        .unwrap_or(false);
                    let display = if is_dir { format!("{name}/") } else { name.clone() };
                    let mut replacement = escape_filename(&name);
                    if is_dir { replacement.push('/'); }
                    Candidate { display, replacement }
                })
                .collect()
        } else {
            after_fallback
                .into_iter()
                .map(|s| Candidate { display: s.clone(), replacement: s })
                .collect()
        };

        // Sort + dedupe by replacement (stable).
        let mut seen = std::collections::HashSet::new();
        let mut deduped: Vec<Candidate> = candidates
            .into_iter()
            .filter(|c| seen.insert(c.replacement.clone()))
            .collect();
        deduped.sort_by(|a, b| a.display.cmp(&b.display));
        deduped
    }

    fn file_completion_strings(prefix: &str, shell: &Shell) -> Vec<String> {
        let home = shell.get("HOME").unwrap_or("").to_string();
        complete_file("", prefix, &home)
            .into_iter()
            .map(|c| c.replacement)
            .collect()
    }

    fn bashdefault_strings(line: &str, pos: usize, shell: &Shell) -> Vec<String> {
        let (_, ctx) = analyze(line, pos);
        match ctx {
            CompletionContext::Variable { prefix } => {
                let names: Vec<String> = shell.var_names().map(|s| s.to_string()).collect();
                complete_variable(&prefix, &names)
                    .into_iter()
                    .map(|c| c.replacement)
                    .collect()
            }
            CompletionContext::Command { prefix } => {
                let path = shell.get("PATH").unwrap_or("").to_string();
                complete_command(&prefix, &path)
                    .into_iter()
                    .map(|c| c.replacement)
                    .collect()
            }
            CompletionContext::File { dir, prefix } => {
                let home = shell.get("HOME").unwrap_or("").to_string();
                complete_file(&dir, &prefix, &home)
                    .into_iter()
                    .map(|c| c.replacement)
                    .collect()
            }
        }
    }

    /// Extracts the command word (word 0) of the simple command that
    /// the cursor is in. Returns None if the cursor is before any
    /// command word (e.g., empty line).
    fn extract_command_name(head: &str) -> Option<String> {
        // Walk backward to find the start of the current simple command
        // (after the most recent `;`, `|`, `&`, `&&`, `||`, newline, or
        // a compound keyword), then take the first whitespace-delimited
        // word.
        let mut start = 0usize;
        let bytes = head.as_bytes();
        let mut in_single = false;
        let mut in_double = false;
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if !in_single && c == b'\\' {
                i += 2;
                continue;
            }
            if in_single {
                if c == b'\'' { in_single = false; }
                i += 1;
                continue;
            }
            if in_double {
                if c == b'"' { in_double = false; }
                i += 1;
                continue;
            }
            match c {
                b'\'' => in_single = true,
                b'"' => in_double = true,
                b';' | b'|' | b'&' | b'\n' | b'(' | b'{' => start = i + 1,
                _ => {}
            }
            i += 1;
        }
        // Skip leading whitespace and assignment prefixes.
        let region = &head[start..];
        let mut chars = region.char_indices().peekable();
        while let Some(&(_, c)) = chars.peek() {
            if c == ' ' || c == '\t' { chars.next(); } else { break; }
        }
        let word_start = chars.peek().map(|(i, _)| *i).unwrap_or(region.len());
        let rest = &region[word_start..];
        let word_end = rest
            .find(|c: char| c == ' ' || c == '\t')
            .unwrap_or(rest.len());
        let candidate = &rest[..word_end];

        // If it looks like an assignment prefix, the command is the NEXT word.
        if is_assignment(candidate) {
            let after = &rest[word_end..].trim_start();
            let next_end = after
                .find(|c: char| c == ' ' || c == '\t')
                .unwrap_or(after.len());
            if next_end == 0 { return None; }
            return Some(after[..next_end].to_string());
        }

        if candidate.is_empty() { None } else { Some(candidate.to_string()) }
    }

    /// Tokenizes a line into COMP_WORDS per the wordbreaks set.
    /// Returns (words, cword) where cword is the index of the word
    /// the cursor is in.
    pub(crate) fn tokenize_comp_words(line: &str, wordbreaks: &str) -> (Vec<String>, usize) {
        let ws: Vec<char> = wordbreaks.chars().filter(|c| c.is_ascii_whitespace()).collect();
        let non_ws: Vec<char> = wordbreaks.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let mut words: Vec<String> = Vec::new();
        let mut cur = String::new();
        for c in line.chars() {
            if ws.contains(&c) {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            } else if non_ws.contains(&c) {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                words.push(c.to_string());
            } else {
                cur.push(c);
            }
        }
        // If the line ends mid-word, that word IS the cursor word.
        // If the line ends with a separator, the cursor word is "" and
        // it occupies a new slot.
        let ends_with_sep = line
            .chars()
            .last()
            .map(|c| ws.contains(&c) || non_ws.contains(&c))
            .unwrap_or(true);
        if !cur.is_empty() {
            words.push(cur);
        } else if ends_with_sep || words.is_empty() {
            words.push(String::new());
        }
        let cword = words.len().saturating_sub(1);
        (words, cword)
    }
}
```

- [ ] **Step 2: Rewrite `HuckHelper::complete` to call `dispatch::resolve`**

Edit `src/completion.rs`. Replace the `complete` method body in the `impl Completer for HuckHelper` block (added in Task 1) with:

```rust
    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let mut shell = self.shell.borrow_mut();
        let (start, candidates) = dispatch::resolve(line, pos, &mut shell);
        let pairs = candidates
            .into_iter()
            .map(|c| rustyline::completion::Pair {
                display: c.display,
                replacement: c.replacement,
            })
            .collect();
        Ok((start, pairs))
    }
```

- [ ] **Step 3: Add unit tests for dispatch**

Inside `src/completion.rs`'s `#[cfg(test)] mod tests` block, add:

```rust
    #[test]
    fn dispatch_variable_context_bypasses_spec() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let shell = Rc::new(RefCell::new(Shell::new()));
        shell.borrow_mut().set("MY_VAR", "x".to_string());
        // Register a -F spec for some command — it should NOT fire on $var.
        shell.borrow_mut().completion_specs.by_command.insert(
            "echo".to_string(),
            crate::completion_spec::CompletionSpec {
                wordlist: Some("should_not_appear".to_string()),
                ..Default::default()
            },
        );
        let mut s = shell.borrow_mut();
        let (start, cands) = dispatch::resolve("echo $MY_V", 10, &mut s);
        assert_eq!(start, 6);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.contains(&"MY_VAR"), "{names:?}");
        assert!(!names.contains(&"should_not_appear"), "spec fired on var: {names:?}");
    }

    #[test]
    fn dispatch_command_position_uses_command_completion() {
        let mut shell = Shell::new();
        let (_, cands) = dispatch::resolve("ec", 2, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.contains(&"echo"), "{names:?}");
    }

    #[test]
    fn dispatch_arg_position_uses_spec() {
        let mut shell = Shell::new();
        shell.completion_specs.by_command.insert(
            "myc".to_string(),
            crate::completion_spec::CompletionSpec {
                wordlist: Some("alpha alpine beta".to_string()),
                ..Default::default()
            },
        );
        let (_, cands) = dispatch::resolve("myc al", 6, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert_eq!(names, vec!["alpha", "alpine"]);
    }

    #[test]
    fn dispatch_falls_back_to_file_when_no_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("targetfile"), b"").unwrap();
        let mut shell = Shell::new();
        let line = format!("ls {}/targ", dir.path().to_str().unwrap());
        let pos = line.len();
        let (_, cands) = dispatch::resolve(&line, pos, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.iter().any(|n| *n == "targetfile"), "{names:?}");
    }

    #[test]
    fn dispatch_o_default_falls_back_on_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alphafile"), b"").unwrap();
        let prior_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let mut shell = Shell::new();
        let spec = crate::completion_spec::CompletionSpec {
            wordlist: Some("nothing_matches".to_string()),
            options: crate::completion_spec::CompOptions {
                default: true,
                ..Default::default()
            },
            ..Default::default()
        };
        shell.completion_specs.by_command.insert("mycmd".to_string(), spec);

        let (_, cands) = dispatch::resolve("mycmd alpha", 11, &mut shell);
        std::env::set_current_dir(prior_cwd).unwrap();

        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.iter().any(|n| *n == "alphafile"), "fallback didn't fire: {names:?}");
    }

    #[test]
    fn dispatch_d_default_spec_applies_when_no_match() {
        let mut shell = Shell::new();
        shell.completion_specs.default_spec = Some(crate::completion_spec::CompletionSpec {
            wordlist: Some("dfault".to_string()),
            ..Default::default()
        });
        let (_, cands) = dispatch::resolve("randomcmd df", 12, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert_eq!(names, vec!["dfault"]);
    }

    #[test]
    fn tokenize_default_wordbreaks_is_whitespace() {
        let (words, cword) = dispatch::tokenize_comp_words("git checkout main", " \t\n");
        assert_eq!(words, vec!["git", "checkout", "main"]);
        assert_eq!(cword, 2);
    }

    #[test]
    fn tokenize_custom_wordbreaks_splits_on_colon() {
        let (words, _cword) = dispatch::tokenize_comp_words("user:pass", " \t\n:");
        assert_eq!(words, vec!["user", ":", "pass"]);
    }

    #[test]
    fn tokenize_trailing_separator_means_empty_cursor_word() {
        let (words, cword) = dispatch::tokenize_comp_words("cmd ", " \t\n");
        assert_eq!(words, vec!["cmd", ""]);
        assert_eq!(cword, 1);
    }
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --quiet completion::tests::dispatch 2>&1 | tail -10 && cargo test --quiet completion::tests::tokenize 2>&1 | tail -10`
Expected: 9 new tests pass.

- [ ] **Step 5: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 5:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```
Expected: 2189 tests pass (2180 + 9 new). Clippy clean.

- [ ] **Step 6: Smoke-test interactive tab completion**

This is the first user-visible behavior change. Run interactively:

```bash
cargo run --quiet
```

Then type (literal newline at the end of each line):
```
_myf() { COMPREPLY=(alpha beta gamma); }
complete -F _myf myc
myc <TAB>
```

Expected: tab key cycles through `alpha`, `beta`, `gamma`. Press Ctrl-D to exit.

If the tab key doesn't trigger (rustyline config), confirm `completion_type(CompletionType::List)` is still set in `src/shell.rs::run()`.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v76 task 5: tab-completion dispatch ladder

New dispatch submodule in src/completion.rs::dispatch with the 5-step
resolve(line, pos, &mut Shell) entry point:

  1. Variable context → existing complete_variable (always wins).
  2. Command position, empty line + -E spec → run -E spec.
  3. Command position, non-empty → existing complete_command.
  4. Arg position with registered spec → resolve_spec + empty-fallback
     (-o default → file completion; -o bashdefault → re-run the ladder
     via bashdefault_strings).
  5. Arg position with no spec → existing complete_file.

HuckHelper::complete is now a thin wrapper around dispatch::resolve.

extract_command_name walks back from cursor to find word 0 of the
current simple command (after `;` / `|` / `&` / `(` / `{` / newline),
skipping assignment prefixes.

tokenize_comp_words honors COMP_WORDBREAKS: whitespace bytes act as
separators only; non-whitespace bytes each produce their own
single-char word in COMP_WORDS (matches bash semantics when the var
is set to bash's default).

-o filenames decorates: directories get trailing `/`; metachars in
filenames are backslash-escaped via the existing escape_filename().

9 new unit tests cover the dispatch ladder (variable bypass,
command-pos bypass, arg-pos hit, file fallback, -o default fallback,
-D default-spec application) plus the tokenizer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `compopt` builtin

**Files:**
- Modify: `src/completion_builtins.rs` — add `builtin_compopt`
- Modify: `src/builtins.rs` — add `"compopt"` to `BUILTIN_NAMES`; one match arm
- Modify: `src/completion_spec.rs` — confirm `current_completion_spec` round-trip is wired

**Goal:** `compopt -o nospace` inside a `-F` function mutates the spec for the current completion; `compopt -o nospace cmd` outside a function mutates the spec for `cmd`; `compopt` outside a function with no name errors with status 1.

### Steps

- [ ] **Step 1: Add `builtin_compopt` to `src/completion_builtins.rs`**

Append below `builtin_compgen`:

```rust
/// `compopt` builtin.
pub fn builtin_compopt(args: &[String], _out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // Parse the same way as complete/compgen but only recognize -o/+o
    // and trailing names. -D and -E (mutate default/empty specs) are
    // deferred — recognized as flags but rejected with "not yet
    // supported" diagnostic, exit 1.
    let mut i = 0;
    let mut option_set: Vec<(String, bool)> = Vec::new();
    let mut is_default = false;
    let mut is_empty = false;
    let mut names: Vec<String> = Vec::new();

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') && !arg.starts_with('+') {
            break;
        }
        let leading = arg.chars().next().unwrap();
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut ci = 0;
        while ci < chars.len() {
            let c = chars[ci];
            match c {
                'o' => {
                    let arg_value: String = if ci + 1 < chars.len() {
                        let v: String = chars[ci + 1..].iter().collect();
                        ci = chars.len();
                        v
                    } else if i + 1 < args.len() {
                        i += 1;
                        ci = chars.len();
                        args[i].clone()
                    } else {
                        eprintln!("huck: compopt: -o: option requires an argument");
                        return ExecOutcome::Continue(2);
                    };
                    let off = leading == '+';
                    if !["default", "nospace", "filenames", "bashdefault", "dirnames"].contains(&arg_value.as_str()) {
                        eprintln!("huck: compopt: {arg_value}: invalid completion option");
                        return ExecOutcome::Continue(2);
                    }
                    option_set.push((arg_value, off));
                }
                'D' => {
                    is_default = true;
                }
                'E' => {
                    is_empty = true;
                }
                other => {
                    eprintln!("huck: compopt: -{other}: invalid option");
                    return ExecOutcome::Continue(2);
                }
            }
            ci += 1;
        }
        i += 1;
    }
    names.extend(args[i..].iter().cloned());

    if is_default || is_empty {
        eprintln!("huck: compopt: -D/-E not yet supported");
        return ExecOutcome::Continue(1);
    }

    // Helper to mutate one CompOptions in place per the parsed (name, off) list.
    let apply = |opts: &mut CompOptions, sets: &[(String, bool)]| {
        for (name, off) in sets {
            let v = !*off;
            match name.as_str() {
                "default" => opts.default = v,
                "nospace" => opts.nospace = v,
                "filenames" => opts.filenames = v,
                "bashdefault" => opts.bashdefault = v,
                "dirnames" => opts.dirnames = v,
                _ => unreachable!(),
            }
        }
    };

    if names.is_empty() {
        // In-function mutation. The dispatch path stashes the live spec
        // in shell.current_completion_spec before invoking -F.
        let Some(mut live) = shell.current_completion_spec.take() else {
            eprintln!("huck: compopt: not currently executing completion function");
            return ExecOutcome::Continue(1);
        };
        apply(&mut live.options, &option_set);
        shell.current_completion_spec = Some(live);
        return ExecOutcome::Continue(0);
    }

    // Named: mutate registry.
    let mut status = 0;
    for n in &names {
        match shell.completion_specs.by_command.get_mut(n) {
            Some(spec) => apply(&mut spec.options, &option_set),
            None => {
                eprintln!("huck: compopt: {n}: no completion specification");
                status = 1;
            }
        }
    }
    ExecOutcome::Continue(status)
}
```

- [ ] **Step 2: Wire `compopt` into `run_builtin`**

Edit `src/builtins.rs`. In `BUILTIN_NAMES`, add `"compopt"`:

```rust
    "complete", "compgen", "compopt",
```

In `run_builtin`, add a match arm:

```rust
        "compopt" => crate::completion_builtins::builtin_compopt(args, out, shell),
```

- [ ] **Step 3: Confirm Task 4's `call_completion_function` correctly stashes+restores current_completion_spec**

Re-read the function in `src/completion_spec.rs` to ensure it:
- Takes a clone of `spec`, stashes it in `shell.current_completion_spec` before the call.
- After the call, takes it BACK OUT (`shell.current_completion_spec.take()`) so the next completion isn't polluted.
- The mutated spec's options must flow back to the caller for Task 5's empty-fallback / filenames rendering.

Currently Task 4's implementation takes the mutated spec out but discards it. We need to RETURN the resolved spec to the caller. Change the function signature:

Before (Task 4):
```rust
pub fn call_completion_function(
    func_name: &str,
    spec: &CompletionSpec,
    ctx: &CompletionCtx,
    shell: &mut Shell,
) -> Vec<String> {
```

The Task 5 dispatch path already handles this — it calls `shell.current_completion_spec.take()` AFTER `resolve_spec` returns to read the mutated options. Re-read `run_spec_with_empty_fallback` in Task 5:

```rust
        let effective_options = shell
            .current_completion_spec
            .take()
            .map(|s| s.options)
            .unwrap_or(spec.options);
```

Good — the dispatch path already takes the mutated spec back. But Task 4's `call_completion_function` ALSO does a `.take()` at the end and then restores `prior_current_spec`. That clobbers what dispatch needs!

Fix: in `call_completion_function`, do NOT take the spec back out at the end. Leave it in `shell.current_completion_spec` for the caller (dispatch) to read. The "restore prior_current_spec" logic was wrong — completion functions cannot nest (rustyline is non-reentrant), so there's no prior_current_spec to worry about.

Edit `src/completion_spec.rs::call_completion_function` to remove these lines:

```rust
    // 6. Take spec back out (compopt may have mutated it).
    let _resolved_spec = shell.current_completion_spec.take();
    shell.current_completion_spec = prior_current_spec;
```

Also remove the earlier:

```rust
    let prior_current_spec = shell.current_completion_spec.take();
```

(the saved prior_current_spec is dead code — completion functions don't nest).

Just set the spec at the start of the function and leave it; dispatch's `run_spec_with_empty_fallback` does the `.take()` to read the (possibly mutated) options after the function returns.

Updated relevant section of `call_completion_function`:

```rust
    // 3. Stash the spec for compopt-in-function mutation. Dispatch
    //    will .take() this back out after we return.
    shell.current_completion_spec = Some(spec.clone());
```

And the corresponding cleanup (drop steps 6 above).

- [ ] **Step 4: Write tests for compopt**

Add to `src/completion_builtins.rs`'s `#[cfg(test)] mod tests` block:

```rust
    fn run_compopt(args: &[&str], shell: &mut Shell) -> (String, i32) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::<u8>::new();
        let outcome = builtin_compopt(&argv, &mut out, shell);
        let s = String::from_utf8(out).unwrap();
        let code = match outcome {
            ExecOutcome::Continue(n) => n,
            _ => panic!("compopt should not return non-Continue"),
        };
        (s, code)
    }

    #[test]
    fn compopt_outside_function_with_no_name_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-o", "nospace"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn compopt_named_mutates_registry() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "foo"], &mut sh);
        let (_, code) = run_compopt(&["-o", "nospace", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command["foo"].options.nospace);
    }

    #[test]
    fn compopt_named_plus_o_unsets() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "-o", "nospace", "--", "foo"], &mut sh);
        assert!(sh.completion_specs.by_command["foo"].options.nospace);
        let (_, code) = run_compopt(&["+o", "nospace", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert!(!sh.completion_specs.by_command["foo"].options.nospace);
    }

    #[test]
    fn compopt_named_missing_returns_1() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-o", "nospace", "ghost"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn compopt_invalid_option_errors() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "foo"], &mut sh);
        let (_, code) = run_compopt(&["-o", "nosort", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn compopt_in_function_mutates_live_spec() {
        let mut sh = Shell::new();
        // Function calls `compopt -o nospace` then sets COMPREPLY.
        let _ = crate::shell::process_line(
            "_myf() { compopt -o nospace; COMPREPLY=(alpha); }",
            &mut sh,
            false,
        );

        let spec = crate::completion_spec::CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let ctx = crate::completion_spec::CompletionCtx {
            cmd_name: "myc".to_string(),
            cur_word: String::new(),
            prev_word: String::new(),
            comp_words: vec!["myc".to_string(), String::new()],
            comp_cword: 1,
            comp_line: "myc ".to_string(),
            comp_point: 4,
        };
        let _ = crate::completion_spec::resolve_spec(&spec, &ctx, &mut sh);
        // After resolve_spec, dispatch reads current_completion_spec —
        // but for this unit test we read it directly to verify the
        // function's compopt call mutated it.
        let mutated = sh.current_completion_spec.as_ref().expect("spec still stashed");
        assert!(mutated.options.nospace, "compopt -o nospace inside -F did not take effect");
    }
```

- [ ] **Step 5: Run new tests**

Run: `cargo test --quiet completion_builtins::tests::compopt 2>&1 | tail -10`
Expected: 6 tests pass.

- [ ] **Step 6: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 6:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```
Expected: 2195 tests pass (2189 + 6 new). Clippy clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v76 task 6: compopt builtin

New builtin_compopt in src/completion_builtins.rs. Two modes:

* In-function (no names): mutates the live spec via
  shell.current_completion_spec, which the Task-5 dispatch path
  takes back out after the -F function returns. Errors with status 1
  when called outside a -F function with no names.

* Named (with names): mutates shell.completion_specs.by_command[name]
  directly. -o sets, +o clears. Status 1 if any name is missing.

Recognized options: default, nospace, filenames, bashdefault, dirnames
(same five as `complete -o`). Unknown option → exit 2. -D/-E recognized
but rejected as "not yet supported", exit 1.

Also fixes a Task-4 bug: call_completion_function was clobbering the
current_completion_spec slot on cleanup, which would have hidden any
compopt mutation from dispatch. Now leaves the spec stashed for
dispatch's run_spec_with_empty_fallback to .take() and read.

6 new unit tests cover both modes plus error paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Integration tests + bash-diff harness + docs

**Files:**
- Create: `tests/completion_integration.rs` — 15 binary-driven completion tests
- Create: `tests/scripts/completion_diff_check.sh` — 12 bash-diff fragments
- Modify: `docs/bash-divergences.md` — flip M-36; add change-log entry
- Modify: `README.md` — add v76 iteration row

### Steps

- [ ] **Step 1: Create the integration test file**

Create `tests/completion_integration.rs`:

```rust
//! Integration tests for v76 programmable completion. Drives the
//! `huck` binary via stdin and asserts on stdout/exit code. Tab
//! completion proper requires an interactive tty, so these tests use
//! `compgen` exclusively — which exercises the same `resolve_spec`
//! pipeline as Tab.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn compgen_wordlist_basic() {
    let (out, _, code) = run_huck(r#"compgen -W "alpha alpine beta" -- al"#);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha\nalpine\n");
}

#[test]
fn compgen_wordlist_no_match_exits_1() {
    let (out, _, code) = run_huck(r#"compgen -W "alpha beta" -- z"#);
    assert_eq!(code, 1);
    assert!(out.is_empty());
}

#[test]
fn compgen_action_builtin() {
    let (out, _, code) = run_huck(r#"compgen -A builtin -- ec"#);
    assert_eq!(code, 0);
    assert!(out.lines().any(|l| l == "echo"), "{out:?}");
}

#[test]
fn compgen_action_function() {
    let script = r#"
_alpha() { :; }
_alpine() { :; }
_beta() { :; }
compgen -A function -- _al
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    let names: Vec<&str> = out.lines().collect();
    assert_eq!(names, vec!["_alpha", "_alpine"]);
}

#[test]
fn complete_p_round_trip() {
    let script = r#"
complete -W "alpha apple banana" -- myc
complete -p myc
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("complete"));
    assert!(out.contains("-W"));
    assert!(out.contains("alpha apple banana"));
    assert!(out.contains("-- myc"));
}

#[test]
fn complete_r_removes() {
    let script = r#"
complete -W "x" -- foo
complete -p foo
complete -r foo
complete -p foo
"#;
    let (_out, _err, code) = run_huck(script);
    // The last `complete -p foo` fails (status 1) since the spec was
    // removed. We only check the OVERALL exit isn't 0 -- but in
    // non-interactive mode the shell exits with the LAST status.
    assert_eq!(code, 1);
}

#[test]
fn compgen_F_function_invocation() {
    let script = r#"
_myf() { COMPREPLY=(alpha beta gamma); }
compgen -F _myf -- ""
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha\nbeta\ngamma\n");
}

#[test]
fn compgen_F_function_reads_dollar_args() {
    let script = r#"
_myf() { COMPREPLY=("$1" "$2"); }
compgen -F _myf -- prefix
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    // $1 = compgen (cmd_name passed to -F context for compgen is the
    // builtin's own name), $2 = "prefix".
    assert_eq!(out, "compgen\nprefix\n");
}

#[test]
fn compgen_P_prefix_decorates() {
    let (out, _, code) = run_huck(r#"compgen -W "a b" -P "x:" -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "x:a\nx:b\n");
}

#[test]
fn compgen_S_suffix_decorates() {
    let (out, _, code) = run_huck(r#"compgen -W "a b" -S ":y" -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "a:y\nb:y\n");
}

#[test]
fn compgen_X_filter_removes() {
    let (out, _, code) = run_huck(r#"compgen -W "alpha apple banana cherry" -X "a*" -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "banana\ncherry\n");
}

#[test]
fn compgen_X_bang_keeps_only() {
    let (out, _, code) = run_huck(r#"compgen -W "alpha apple banana cherry" -X "!a*" -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha\napple\n");
}

#[test]
fn compopt_named_persists() {
    let script = r#"
complete -W "x" -- foo
compopt -o nospace foo
complete -p foo
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("-o nospace"), "{out:?}");
}

#[test]
fn complete_D_registers_default() {
    let script = r#"
complete -D -W "dflt"
complete -p
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("-D"), "{out:?}");
    assert!(out.contains("dflt"), "{out:?}");
}

#[test]
fn complete_invalid_action_exits_2() {
    let (_out, err, code) = run_huck(r#"complete -A hostname -- foo"#);
    assert_eq!(code, 2);
    assert!(err.contains("invalid action"), "{err:?}");
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test completion_integration --quiet 2>&1 | tail -5`
Expected: 15 tests pass.

- [ ] **Step 3: Create the bash-diff harness**

Create `tests/scripts/completion_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for programmable completion.
# Each fragment runs through `bash -c` and `huck -c` and the outputs
# must be byte-identical. Fragments that intentionally diverge are
# excluded with a comment.
#
# Usage: tests/scripts/completion_diff_check.sh
# Exits 0 on full match, 1 on any divergence.

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1"
    local fragment="$2"
    local bash_out huck_out

    bash_out=$(bash -c "$fragment" 2>&1; echo "EXIT:$?")
    huck_out=$("$HUCK_BIN" -c "$fragment" 2>&1; echo "EXIT:$?")

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(echo "$bash_out") <(echo "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. Static wordlist.
check "compgen -W basic" \
      'compgen -W "alpha alpine beta" -- al'

# 2. Wordlist with no match: bash and huck both exit 1 with empty stdout.
check "compgen -W no match" \
      'compgen -W "a b c" -- z'

# 3. -A builtin (subset that exists in both shells — at minimum echo/cd).
check "compgen -A builtin echo" \
      'compgen -A builtin -- echo'

# 4. -A function (both shells enumerate user-defined functions).
check "compgen -A function" \
      '_alpha() { :; }; _beta() { :; }; compgen -A function -- _'

# 5. -P prefix decoration.
check "compgen -P prefix" \
      'compgen -W "a b" -P "x:" -- ""'

# 6. -S suffix decoration.
check "compgen -S suffix" \
      'compgen -W "a b" -S ":y" -- ""'

# 7. -X filter removes.
check "compgen -X filter removes" \
      'compgen -W "alpha apple banana cherry" -X "a*" -- ""'

# 8. -X bang keeps only.
check "compgen -X bang keeps" \
      'compgen -W "alpha apple banana cherry" -X "!a*" -- ""'

# 9. -F function invocation, simple COMPREPLY assignment.
check "compgen -F basic" \
      '_f() { COMPREPLY=(alpha beta); }; compgen -F _f -- ""'

# 10. -F function reading $1 and $2 (cmd_name + cur_word).
check "compgen -F reads dollar args" \
      '_f() { COMPREPLY=("$1:$2"); }; compgen -F _f -- prefix'

# 11. -F function reading COMP_WORDS and COMP_CWORD.
check "compgen -F reads COMP_WORDS" \
      '_f() { COMPREPLY=("${COMP_WORDS[0]}-${COMP_CWORD}"); }; compgen -F _f -- ""'

# 12. -W with IFS-controlled splitting at use time.
#     Both bash and huck IFS-split -W at use time per POSIX.
check "compgen -W respects IFS" \
      'IFS=: compgen -W "a:b:c" -- ""'

# NOTE: complete -p re-input form is intentionally divergent (huck uses
# a deterministic flag ordering; bash's varies). Not exercised here.

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
```

Make it executable:

```bash
chmod +x tests/scripts/completion_diff_check.sh
```

- [ ] **Step 4: Build the debug binary and run the harness**

Run:
```bash
cargo build --quiet
tests/scripts/completion_diff_check.sh
```

Expected: all 12 fragments pass. If any fail, the diff is shown — investigate whether it's a real divergence to fix, an intentional divergence to document, or a test-setup issue.

Common failure modes to expect and adjust:
- "Pass: 11, Fail: 1" with the `IFS=:` fragment failing if huck's `-W` IFS-split timing differs from bash's — adjust the test or fix the underlying behavior.
- Function-stdout leakage if a fragment uses `echo` inside `-F`. Switch to `:` (the null command).

Iterate on the harness until clean. Exclude any genuine divergences with a `# DIVERGES: <why>` comment block and reduce the expected pass count accordingly.

- [ ] **Step 5: Update `docs/bash-divergences.md`**

Edit `docs/bash-divergences.md`. Find the line:

```
- **M-36: `complete` builtin / programmable completion** — `[deferred]` high.
```

Replace with:

```
- **M-36: `complete` builtin / programmable completion** — `[fixed v76 partial]` high. The `complete`, `compgen`, and `compopt` builtins ship with the Standard flag surface: `-F func`, `-W wordlist`, `-G glob`, `-A action` (8 of bash's 25 actions: `file`, `directory`, `command`, `function`, `variable`, `alias`, `builtin`, `keyword`), `-P prefix`, `-S suffix`, `-X filter`, `-o option` (5 of bash's 8: `default`, `nospace`, `filenames`, `bashdefault`, `dirnames`), `-D` (default-spec), `-E` (empty-line spec), `-p` (print), `-r` (remove), `--`. `COMP_WORDS`, `COMP_CWORD`, `COMP_LINE`, `COMP_POINT`, `COMPREPLY` populated/read. Tab-time dispatch in `src/completion.rs::dispatch::resolve` runs the registered `-F` function with `$1=cmd_name`, `$2=cur_word`, `$3=prev_word`, reads `COMPREPLY`, then applies `-X`/`-P`/`-S`/empty-fallback/`-o filenames`. `compopt` mutates the live spec inside `-F` via the ephemeral `Shell.current_completion_spec` slot. New `src/completion_spec.rs` + `src/completion_builtins.rs`; existing `src/completion.rs` and `src/shell.rs` refactor Shell behind `Rc<RefCell<Shell>>` at the readline boundary (internal `&mut Shell` signatures unchanged). **Deferred**: `complete -C "shell-cmd"` (run subshell, parse stdout); `-I` (initial-word, bash 5.2+); `-b` (load builtin completion shortcut); 16 of bash's 25 `-A` actions (`arrayvar`, `binding`, `disabled`, `enabled`, `export`, `group`, `helptopic`, `hostname`, `job`, `running`, `service`, `setopt`, `shopt`, `signal`, `stopped`, `user`); `-o` options `nosort`, `noquote`, `plusdirs`; `compopt -D`/`-E` (mutate default/empty specs from within a function); `COMP_TYPE`, `COMP_KEY` variables. **Behavioral divergences**: (1) `COMP_WORDBREAKS` defaults to `' \t\n'` (whitespace only), not bash's `' \t\n"\'><=;|&(:'` — settable. (2) `exit` inside a `-F` function does NOT terminate the shell; the function returns immediately with COMPREPLY-so-far (rustyline `complete()` cannot propagate `ExecOutcome::Exit`). (3) `complete -p` re-input form uses deterministic flag ordering; bash's varies — see new L-13 below.
```

Find the L-* section (search for `### L-`) and add a new entry at the end of its block:

```
- **L-13: `complete -p` re-input flag ordering** — `[fixed v76]`. huck's `complete -p` emits flags in a deterministic order (`-F` `-W` `-G` then each `-A` then `-P` `-S` `-X` then each `-o` then `-- name`); bash's emitter ordering varies between releases and configurations. The output is round-trip-parseable in both shells; only byte-equal diffs against bash are affected.
```

Then find the change-log section at the end of the file. Add a new entry chronologically:

```
- **2026-06-02**: v76 ships M-36 partial — programmable completion. New `src/completion_spec.rs` (~500 LOC) holding `CompletionSpec`/`CompletionSpecs`/`CompOptions`/`Action` plus the `resolve_spec()` generator that drives `-W`/`-G`/`-A`/`-X`/`-P`/`-S` and the `call_completion_function()` glue that sets `COMP_LINE`/`COMP_POINT`/`COMP_WORDS`/`COMP_CWORD`, invokes the user function via a new `pub(crate) crate::executor::call_function_body`, then reads `COMPREPLY` back. New `src/completion_builtins.rs` (~700 LOC) for `complete` / `compgen` / `compopt` — shared flag parser, three modes for `complete` (register / `-p` print / `-r` remove), `compgen` as a thin wrapper around `resolve_spec`, `compopt` with in-function (live-spec via `Shell.current_completion_spec`) and named (registry) modes. `src/completion.rs` grows from 757 → ~1100 with the `dispatch::resolve()` 5-step ladder (variable wins → command-pos commands → `-E` empty-line spec → spec via `by_command`/`default_spec` → file completion) plus `tokenize_comp_words` honoring `COMP_WORDBREAKS`. **Foundation**: `Shell` wrapped in `Rc<RefCell<Shell>>` in `src/shell.rs::run()`; `read_logical_command` refactored to take `&RefCell<Shell>` and scope all borrows around `editor.readline()` so the rustyline helper can `borrow_mut()` from inside `complete()`. Internal `&mut Shell` signatures across executor/expand/builtins/etc. unchanged. Fixes a latent aliasing issue in the prior code (mutable borrow held across `editor.readline()`). New `Shell.completion_specs: CompletionSpecs` and ephemeral `Shell.current_completion_spec: Option<CompletionSpec>` fields. 3 new builtins added to `BUILTIN_NAMES` (none POSIX-special). `COMP_WORDBREAKS` defaults to whitespace-only — see M-36 entry for the rationale. ~70 new unit tests + 15 integration tests (`tests/completion_integration.rs`) + 12 bash-diff fragments (`tests/scripts/completion_diff_check.sh` — huck's 4th harness). New L-13 entry for `complete -p` ordering divergence.
```

- [ ] **Step 6: Update `README.md`**

Edit `README.md`. Find the iteration table (search for `| v75`). Add a new row after v75:

```
| v76 | 2026-06-02 | Programmable completion (`complete`/`compgen`/`compopt`) | M-36 partial |
```

The exact column structure should match the existing table. If the current v75 row has different columns, adjust accordingly.

If there's a "test count" line elsewhere in the README, update it from 2149 to the new total — read it back from `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print sum}'`.

- [ ] **Step 7: Run the full test suite and the bash-diff harness one more time**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 7:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
cargo build --quiet && tests/scripts/completion_diff_check.sh
```

Expected:
- 2210 tests pass (2195 + 15 integration).
- Clippy clean.
- Bash-diff harness: 12/12 fragments byte-identical.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v76 task 7: integration tests + bash-diff harness + docs

* tests/completion_integration.rs (new): 15 binary-driven tests
  exercising compgen/complete/compopt end-to-end via huck -c.
* tests/scripts/completion_diff_check.sh (new): huck's 4th bash-diff
  harness; 12 fragments byte-identical against bash 5.2.
* docs/bash-divergences.md: M-36 flipped from [deferred] to
  [fixed v76 partial] with full surface description and the three
  documented behavioral divergences (COMP_WORDBREAKS default, exit
  inside -F, complete -p ordering). New L-13 entry for the -p
  ordering divergence. Change-log entry.
* README.md: v76 iteration row + test count bump.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist

Before merging the branch, the controller dispatches a final code-reviewer over the whole branch diff via `superpowers:requesting-code-review`. Specific things to verify:

- [ ] **All 2210 tests pass on the branch.**
- [ ] **Clippy clean (`cargo clippy --all-targets`).**
- [ ] **The bash-diff harness reports 12/12.**
- [ ] **No `&mut Shell` borrow is held across `editor.readline()` in `read_logical_command`.**
- [ ] **`shell.current_completion_spec` is `None` after every tab completion returns to rustyline.**
- [ ] **`$?` is unchanged across a completion-function invocation (huck's `last_status` is snapshotted+restored in `call_completion_function`).**
- [ ] **`COMP_*` variables remain set after a completion finishes (matches bash — they're meant to be readable until next completion).**
- [ ] **`complete -p` round-trips: parsing the printed output produces the same spec.**
- [ ] **Tab in command position (word 0) does NOT consult any registered `-F` spec.**
- [ ] **`complete -W '$opts' cmd` re-expands `$opts` at every Tab (wordlist stored raw, IFS-split at use).**
- [ ] **`docs/bash-divergences.md`'s M-36 entry mentions all three behavioral divergences (COMP_WORDBREAKS, `exit`-in-F, `-p` ordering).**

## Merge

After review fixes land, merge with `--no-ff`:

```bash
git checkout main
git merge --no-ff v76-programmable-completion -m "Merge v76: programmable completion (M-36)"
git push origin main
git branch -d v76-programmable-completion
```

Then update the long-running memory files (`huck_iterations.md`) per the iteration workflow.
