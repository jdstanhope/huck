# Completion from `CursorContext` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete huck's hand-rolled `analyze_full` completion-context scanner and derive completion context from `huck_syntax::parse_recover`'s `CursorContext`, fixing the `if whi` / `echo "$(whi` / `echo $(( HO` / `for x in whi` divergences — closing #248.

**Architecture:** Extend the (merged, iteration-1) recovery capture to emit `WordPosition::RedirectTarget`, then replace the context-branching in `dispatch::resolve` (and the `-o bashdefault` fallback) with a `cursor_to_completion` mapper that reads `CursorContext.position` + `word` + `word_start`. The candidate builders, programmable-spec path, basename-display, and trailing-space decoration are unchanged — only the context source changes.

**Tech Stack:** Rust (2024 edition). `crates/huck-syntax` (capture extension) + `crates/huck-engine` (mapper, deletion, rewire).

**Spec:** `docs/superpowers/specs/2026-07-21-completion-from-cursorcontext-design.md`
**Issue:** [#248](https://github.com/jdstanhope/huck/issues/248)

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run `cargo fmt --all` before every commit — CI enforces `cargo fmt --all --check`.
- **Never** run `cargo test --workspace` — this box (1 core / 1.9GB) OOM-kills the session. Use `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck --bin huck`.
- The `-p huck` completion **integration binaries** run locally too (CI isn't special): run `completion_integration`, `complete_actions_integration`, `arith_completion_integration` single-threaded (`cargo test -p huck --test <name> --jobs 1 -- --test-threads 1`, `ulimit -v 1500000` guard) before pushing — `--lib` skips them.
- The strict `parse()` path stays byte-for-byte unchanged (the huck-syntax capture extension is under `recover_at_eof`); the full existing huck-syntax suite is the gate for that.
- `display` never carries a trailing space; only `replacement` (existing convention — do not disturb it).
- Target fidelity is **bash 5.2.21**. Probe the real shell to confirm behavior.
- Gate: existing correct completion behavior preserved, the four divergences fixed, no regressions to redirect-target / path / nested completion.

## Codebase Orientation

Read before Task 1.

- **`huck_syntax::parse_recover(src) -> RecoveredParse { tree, cursor: CursorContext }`** (merged in #246). `CursorContext { enclosing: Vec<Frame>, position: WordPosition, word: String, word_start: usize }`. `WordPosition` (all `#[non_exhaustive]`): `Command`, `Argument`, `VariableName`, `RedirectTarget`, `AssignRhs`, `Unknown`. `RedirectTarget` is DEFINED but the capture never produces it yet (Task 1 fixes that).
- **Position priority (iteration-1 capture, in `crates/huck-syntax/src/`):** inner `ParamExpansion`/`Arith`/`$name` → `VariableName`; inner `CommandSub`/backtick/subshell → `Command`; else the grammar slot. `RedirectTarget` must slot in at the BOTTOM (lowest priority) so `cat foo > ${HOM` stays `VariableName`. Confirmed today: `cat foo > $HOM`→`VariableName`, `cat foo > $(whi`→`Command`, `cat foo > $(( HO`→`VariableName`; only bare `cat foo > whi` (today `Argument`) becomes `RedirectTarget`.
- **The consumers to rewire** in `crates/huck-engine/src/completion.rs`:
  - `dispatch::resolve(line, pos, shell) -> (usize, Vec<Candidate>)` (`:450`) — the interactive Tab path. Its context branch is at `:457-494`: variable (`:460`), command incl. `-E` empty-spec (`:469`), file with spec `Some`/`None` (`:488`). This is where the mapper plugs in.
  - `bashdefault_strings(line, pos, shell) -> Vec<String>` (`:673`) — the `-o bashdefault` empty-fallback; uses `analyze` (`:674`) to pick command/variable/file builder and returns raw strings.
- **To delete** (`completion.rs`): `CompletionContext` enum (`:34`), `analyze` (`:42`), `analyze_full` (`:50`), and any helper that becomes unused after them. `is_assignment` (`:182`) is ALSO used at `:757` — KEEP it. `is_compound_keyword` (`:198`), `last_unescaped_dollar` (`:206`), `unescape` (`:224`) are likely analyze-only — grep each for remaining uses before deleting. The `analyze_*` unit tests (`:847` onward) go with the functions.
- **The candidate builders that STAY**: `complete_command`, `complete_variable`, `complete_file` (produce `Candidate`s; `complete_file` does the basename-display + `/`-for-dirs). `run_spec_with_empty_fallback` (programmable spec path). `append_trailing_space_non_dir` (dispatch-layer trailing space). `extract_command_name` (command name for spec lookup). `file_completion_strings`.
- `completion.rs` does NOT yet import `huck_syntax` — add the use.

---

## File Structure

- **Modify** `crates/huck-syntax/src/` (parser + recover) — emit `RedirectTarget` in the capture. Tests in `recover.rs`'s `mod tests`.
- **Modify** `crates/huck-engine/src/completion.rs` — add `cursor_to_completion` mapper; rewire `dispatch::resolve` + `bashdefault_strings`; delete `analyze`/`analyze_full`/`CompletionContext` + dead helpers + their tests; add mapper/dispatch tests.
- **Modify** `docs/architecture.md` — completion section note.

---

## Task 1: huck-syntax — emit `RedirectTarget`

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` and/or `crates/huck-syntax/src/recover.rs` (wherever the recovery capture sets `WordPosition`)
- Test: `crates/huck-syntax/src/recover.rs` (`mod tests`)

**Interfaces:**
- Consumes: the existing recovery capture (`CursorContext`, `WordPosition`).
- Produces: `parse_recover` sets `position == WordPosition::RedirectTarget` for a bare redirect operand at the cursor; inner-expansion cases are unchanged.

This is a judgment task — read how the capture currently sets `position` (the priority logic from iteration 1) and where the parser handles redirect operands, before changing anything.

- [ ] **Step 1: Write the failing tests**

Add to `recover.rs`'s `mod tests`:

```rust
#[test]
fn cursor_bare_redirect_target_is_redirect_target() {
    for src in ["cat foo > whi", "echo >whi", "cat < whi", "echo 2> whi", "echo >> whi"] {
        assert_eq!(
            parse_recover(src).cursor.position,
            WordPosition::RedirectTarget,
            "{src:?}"
        );
    }
}

#[test]
fn cursor_redirect_target_with_inner_expansion_keeps_inner_position() {
    // The inner expansion wins — a redirect target is a WORD.
    assert_eq!(parse_recover("cat foo > $HOM").cursor.position, WordPosition::VariableName);
    assert_eq!(parse_recover("cat foo > ${HOM").cursor.position, WordPosition::VariableName);
    assert_eq!(parse_recover("cat foo > $(whi").cursor.position, WordPosition::Command);
    assert_eq!(parse_recover("cat foo > $(( HO").cursor.position, WordPosition::VariableName);
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p huck-syntax --jobs 1 --lib cursor_bare_redirect_target -- --test-threads 1
```

Expected: FAIL — bare `cat foo > whi` currently reports `Argument`, not `RedirectTarget`.

- [ ] **Step 3: Implement**

In the capture's `position` determination, add a lowest-priority rule: when there is NO enclosing expansion mode (the `VariableName`/`Command`-from-mode rules did not fire) AND the cursor word is a redirect operand, set `RedirectTarget`. The parser knows it is parsing a redirect operand — thread that into the same recovery-boundary capture that already records command-vs-argument (Task-4 of #246 added a command/arg flag; add a redirect-target flag the same way, set when the parser is assembling a redirect target word, read at the capture boundary with lower priority than the inner-mode positions and, per bash, in place of `Argument`).

Read the redirect-parse function and the existing capture-flag mechanism (`set_recovery_cmd_word` / the position derivation) first; mirror that mechanism for the redirect flag. Gate all of it under `recover_at_eof` so the strict path is untouched.

- [ ] **Step 4: Run to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-syntax --jobs 1 --lib cursor_ -- --test-threads 1
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
```

Expected: both new tests PASS; the whole huck-syntax suite stays green (strict path unaffected).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/recover.rs
git commit -m "$(cat <<'EOF'
completion(#248) task 1: emit WordPosition::RedirectTarget for bare redirect operands

parse_recover now reports RedirectTarget for a bare redirect target at the
cursor, at LOWEST priority so an inner expansion (`> ${HOM`, `> $(whi`)
keeps its inner-mode position. Recovery-gated; strict path unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: huck-engine — the `cursor_to_completion` mapper

**Files:**
- Modify: `crates/huck-engine/src/completion.rs` (add the mapper + helper; add `use huck_syntax`)
- Test: `crates/huck-engine/src/completion.rs` (`mod tests`)

**Interfaces:**
- Consumes: `huck_syntax::parse_recover`, `CursorContext`, `WordPosition` (with Task-1's `RedirectTarget`); the existing `complete_command`/`complete_variable`/`complete_file`/`run_spec_with_empty_fallback`/`extract_command_name`/`append_trailing_space_non_dir`.
- Produces: `fn cursor_to_completion(cursor: &huck_syntax::CursorContext, line: &str, pos: usize, shell: &mut Shell) -> (usize, Vec<Candidate>)` — returns the replacement anchor and the candidates, applying the same trailing-space decoration `dispatch::resolve` applies today. NOT yet wired into `dispatch::resolve` (Task 3 does the switch), so it is dead code this task — add `#[allow(dead_code)]` if the build warns, removed in Task 3.

The mapper is the new context→source logic. Build it standalone with its own tests, so the switch (Task 3) is a small, low-risk rewire.

- [ ] **Step 1: Write the failing tests**

Add to `completion.rs`'s `mod tests` (they call `cursor_to_completion` via `parse_recover`):

```rust
fn map(line: &str) -> Vec<Candidate> {
    let mut sh = Shell::new();
    let cur = huck_syntax::parse_recover(line).cursor;
    cursor_to_completion(&cur, line, line.len(), &mut sh).1
}

#[test]
fn mapper_command_position_yields_commands() {
    // `if whi` / bare word / inside $( → command candidates.
    for line in ["if whi", "whi", "echo $(whi", "echo \"$(whi"] {
        let c = map(line);
        assert!(c.iter().any(|x| x.display == "while"), "{line:?}: {:?}",
            c.iter().map(|x| &x.display).collect::<Vec<_>>());
    }
}

#[test]
fn mapper_variable_position_yields_variables() {
    let mut sh = Shell::new();
    sh.set("MYUNIQUEVAR", "x".to_string());
    for line in ["echo $MYUNIQ", "echo ${MYUNIQ", "echo $(( MYUNIQ"] {
        let cur = huck_syntax::parse_recover(line).cursor;
        let c = cursor_to_completion(&cur, line, line.len(), &mut sh).1;
        assert!(c.iter().any(|x| x.display == "MYUNIQUEVAR"), "{line:?}");
    }
}

#[test]
fn mapper_argument_and_redirect_and_path_yield_files() {
    // Run in a scratch dir so file completion has known entries.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("uniqfile.txt"), b"x").unwrap();
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let mut sh = Shell::new();
    for line in ["cat uniqf", "for x in uniqf", "echo > uniqf", "cat ./uniqf"] {
        let cur = huck_syntax::parse_recover(line).cursor;
        let c = cursor_to_completion(&cur, line, line.len(), &mut sh).1;
        assert!(
            c.iter().any(|x| x.display.starts_with("uniqfile")),
            "{line:?}: {:?}", c.iter().map(|x| &x.display).collect::<Vec<_>>()
        );
    }
    std::env::set_current_dir(prev).unwrap();
}

#[test]
fn mapper_dir_prefix_split_anchors_after_slash() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub").join("leaf.txt"), b"x").unwrap();
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let mut sh = Shell::new();
    let line = "cat sub/le";
    let cur = huck_syntax::parse_recover(line).cursor;
    let (anchor, cands) = cursor_to_completion(&cur, line, line.len(), &mut sh);
    std::env::set_current_dir(prev).unwrap();
    assert_eq!(anchor, 8, "anchor is right after `sub/`");
    // display is the basename (leaf.txt), replacement carries the full path.
    assert!(cands.iter().any(|c| c.display == "leaf.txt"));
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p huck-engine --jobs 1 --lib mapper_ -- --test-threads 1
```

Expected: FAIL to compile — `cursor_to_completion` undefined.

- [ ] **Step 3: Implement the mapper**

Add `use huck_syntax::{CursorContext, WordPosition};` near the top of `completion.rs`. Implement:

```rust
/// Split a completion word on its last `/` into (dir, basename); the anchor is
/// `word_start + byte offset past the last '/'` (or `word_start` if none).
fn split_word_anchor(word: &str, word_start: usize) -> (String, String, usize) {
    match word.rfind('/') {
        Some(i) => (word[..=i].to_string(), word[i + 1..].to_string(), word_start + i + 1),
        None => (String::new(), word.to_string(), word_start),
    }
}

/// Map the parser-derived cursor context to a completion source. Replaces the
/// hand-rolled `analyze_full` context determination. Returns the replacement
/// anchor and the trailing-space-decorated candidates.
fn cursor_to_completion(
    cursor: &CursorContext,
    line: &str,
    pos: usize,
    shell: &mut Shell,
) -> (usize, Vec<Candidate>) {
    let has_slash = cursor.word.contains('/');
    // Variable — inner ${…}/$((…))/$name context always won in the capture.
    if cursor.position == WordPosition::VariableName {
        let names: Vec<String> = shell.completion_var_names();
        return (
            cursor.word_start,
            append_trailing_space_non_dir(complete_variable(&cursor.word, &names)),
        );
    }
    // Command with no `/` — command completion.
    if cursor.position == WordPosition::Command && !has_slash {
        // Preserve the existing `-E` empty-command-line spec fallback.
        if cursor.word.is_empty()
            && line[..pos].trim().is_empty()
            && let Some(spec) = shell.completion_specs.empty_spec.clone()
        {
            return (cursor.word_start, run_spec_with_empty_fallback(&spec, line, pos, "", shell));
        }
        let path = shell.get("PATH").unwrap_or("").to_string();
        let funcs: Vec<String> = shell.functions.keys().cloned().collect();
        let aliases: Vec<String> = shell.aliases.keys().cloned().collect();
        return (
            cursor.word_start,
            append_trailing_space_non_dir(complete_command(&cursor.word, &path, &funcs, &aliases)),
        );
    }
    // Argument — file completion + programmable-spec lookup (matches the old
    // File-context spec path). Command-with-`/`, RedirectTarget, and AssignRhs
    // all resolve to file completion below; only Argument consults a spec.
    let (dir, prefix, anchor) = split_word_anchor(&cursor.word, cursor.word_start);
    if cursor.position == WordPosition::Argument {
        let cmd_name = extract_command_name(&line[..pos]).unwrap_or_default();
        let spec_opt = shell
            .completion_specs
            .by_command
            .get(&cmd_name)
            .cloned()
            .or_else(|| shell.completion_specs.default_spec.clone());
        if let Some(spec) = spec_opt {
            // Programmable completion replaces the WHOLE word (bash's model).
            return (cursor.word_start, run_spec_with_empty_fallback(&spec, line, pos, &cmd_name, shell));
        }
    }
    // Plain file completion (Argument-no-spec, Command-with-`/`, RedirectTarget,
    // AssignRhs). Anchored after the last `/`; dirs get `/`, files get a space.
    let home = shell.get("HOME").unwrap_or("").to_string();
    (anchor, append_trailing_space_non_dir(complete_file(&dir, &prefix, &home)))
}
```

Match the real signatures of `complete_command`/`complete_variable`/`complete_file`/`run_spec_with_empty_fallback`/`extract_command_name`/`append_trailing_space_non_dir`/`completion_var_names` as they exist in the file (read them; adjust argument order if these differ). If a `dead_code` warning fires because nothing calls `cursor_to_completion` yet, add `#[allow(dead_code)]` on it with a comment that Task 3 wires it in.

- [ ] **Step 4: Run to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib mapper_ -- --test-threads 1
```

Expected: the 4 mapper tests PASS. `analyze_full` still exists and is still wired — this task added the mapper alongside it.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/completion.rs
git commit -m "$(cat <<'EOF'
completion(#248) task 2: cursor_to_completion mapper (unwired)

Maps parse_recover's CursorContext to a completion source: VariableName →
variables; Command (no /) → commands (with -E empty-spec fallback);
Argument → files + spec lookup; Command-with-/ / RedirectTarget / AssignRhs
→ file completion. Dir/prefix split anchors after the last /. Not yet wired
into dispatch::resolve (task 3).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: The switch — rewire `dispatch::resolve` + `bashdefault_strings`, delete `analyze_full`

**Files:**
- Modify: `crates/huck-engine/src/completion.rs`
- Test: `crates/huck-engine/src/completion.rs` (delete `analyze_*` tests; add dispatch-level regression tests)

**Interfaces:**
- Consumes: `cursor_to_completion` (Task 2), `parse_recover`.
- Produces: `dispatch::resolve` and `bashdefault_strings` derive context from `parse_recover`; `analyze`/`analyze_full`/`CompletionContext` deleted.

This is the judgment task — it deletes a subsystem and rewires two consumers. Read `dispatch::resolve` (`:450-495`) and `bashdefault_strings` (`:673`) fully first.

- [ ] **Step 1: Add dispatch-level regression tests (the four fixes + must-not-regress)**

Add to `completion.rs`'s `mod tests` — these drive the REAL `dispatch::resolve` and must pass after the switch:

```rust
#[test]
fn dispatch_fixes_the_four_divergences() {
    let mut sh = Shell::new();
    sh.set("HOMESENTINEL", "x".to_string());
    // if COND → command position.
    let (_s, c) = dispatch::resolve("if whi", 6, &mut sh);
    assert!(c.iter().any(|x| x.display == "while"), "if whi → commands");
    // command sub inside double quotes → command position.
    let (_s, c) = dispatch::resolve("echo \"$(whi", 10, &mut sh);
    assert!(c.iter().any(|x| x.display == "while"), "quoted $( → commands");
    // arithmetic → variable position.
    let (_s, c) = dispatch::resolve("echo $(( HOMESENT", 17, &mut sh);
    assert!(c.iter().any(|x| x.display == "HOMESENTINEL"), "arith → variables");
}

#[test]
fn dispatch_does_not_regress_redirect_or_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("uniqf.txt"), b"x").unwrap();
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let mut sh = Shell::new();
    for (line, pos) in [("echo > uniqf", 12), ("cat ./uniqf", 11), ("cat uniqf", 9)] {
        let (_s, c) = dispatch::resolve(line, pos, &mut sh);
        assert!(c.iter().any(|x| x.display.starts_with("uniqf")), "{line:?}");
    }
    std::env::set_current_dir(prev).unwrap();
}
```

Run them — they should already PASS if the mapper is correct but dispatch is not yet rewired ONLY if the old path happens to agree; for `if whi` / quoted `$(` / arith they will FAIL now (old `analyze_full` returns nothing). That failure is the proof the switch is needed.

```bash
cargo test -p huck-engine --jobs 1 --lib dispatch_fixes_the_four -- --test-threads 1
```

Expected: FAIL (old scanner returns no candidates for the four divergences).

- [ ] **Step 2: Rewire `dispatch::resolve`**

Replace the context-analysis block (`:457-494`, everything from `let (word_start, start, context) = analyze_full(...)` through the `match spec_opt` file branch) with:

```rust
        let cursor = huck_syntax::parse_recover(&line[..pos]).cursor;
        cursor_to_completion(&cursor, line, pos, shell)
```

Keep the defensive `shell.current_completion_spec = None;` line that precedes it. The mapper already contains the `-E` empty-spec fallback and the programmable-spec lookup, so no other branch is needed.

- [ ] **Step 3: Rewire `bashdefault_strings`**

`bashdefault_strings` (`:673`) currently `match`es `analyze`'s `CompletionContext` to pick a builder and return raw strings. Replace its context source with `parse_recover`, mapping `WordPosition` → builder:

```rust
    fn bashdefault_strings(line: &str, pos: usize, shell: &Shell) -> Vec<String> {
        let cursor = huck_syntax::parse_recover(&line[..pos]).cursor;
        use huck_syntax::WordPosition;
        match cursor.position {
            WordPosition::VariableName => {
                let names = shell.completion_var_names();
                complete_variable(&cursor.word, &names).into_iter().map(|c| c.replacement).collect()
            }
            WordPosition::Command if !cursor.word.contains('/') => {
                let path = shell.get("PATH").unwrap_or("").to_string();
                let funcs: Vec<String> = shell.functions.keys().cloned().collect();
                let aliases: Vec<String> = shell.aliases.keys().cloned().collect();
                complete_command(&cursor.word, &path, &funcs, &aliases).into_iter().map(|c| c.replacement).collect()
            }
            _ => {
                // Argument / RedirectTarget / AssignRhs / Command-with-`/` → files.
                let home = shell.get("HOME").unwrap_or("").to_string();
                let (dir, prefix, _anchor) = split_word_anchor(&cursor.word, cursor.word_start);
                complete_file(&dir, &prefix, &home).into_iter().map(|c| format!("{dir}{}", c.replacement)).collect()
            }
        }
    }
```

Note `bashdefault_strings` takes `&Shell` (not `&mut`); `parse_recover` needs no shell, so this is fine.

- [ ] **Step 4: Delete `analyze_full` / `analyze` / `CompletionContext` and dead helpers**

- Delete `CompletionContext` (`:34`), `analyze` (`:42`), `analyze_full` (`:50`).
- Delete the `analyze_*` unit tests (the `mod tests` block from `:847` onward that calls `analyze`/`analyze_full` — `analyze_empty_line_is_command`, `analyze_first_word_is_command`, `analyze_after_command_is_file`, `analyze_after_semicolon_is_command`, etc., including the `analyze_inside_*` / `analyze_after_closed_*` / `analyze_array_literal_*` / `analyze_redirect_target_*` cases). Their behavioral intent is now covered by the dispatch-level tests from Step 1 and Task 2's mapper tests.
- For each helper `is_compound_keyword` (`:198`), `last_unescaped_dollar` (`:206`), `unescape` (`:224`): `grep -n` it across `crates/huck-engine/src/`; delete it only if `analyze_full` was its sole caller. **KEEP `is_assignment`** — it is used at `:757`.

Verify the deletions:

```bash
grep -rn "analyze_full\|analyze(\|CompletionContext" crates/huck-engine/src/ | grep -v "recover\|parse_recover"
```

Expected: no output (all gone). Any lingering `dead_code` warning from a now-orphaned helper → delete that helper too.

- [ ] **Step 5: Run the tests**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib "completion" -- --test-threads 1
```

Expected: the four-divergence test and the no-regress test PASS; the whole completion test module is green. The mapper's `#[allow(dead_code)]` from Task 2 can now be removed (it is wired).

- [ ] **Step 6: Run the `-p huck` completion integration binaries**

```bash
for t in completion_integration complete_actions_integration arith_completion_integration; do
  ulimit -v 1500000
  echo "=== $t ==="
  timeout 200 cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | tail -4
done
```

Expected: green. They mostly assert `compgen`/`complete` **stdout** (unaffected). If one drives an interactive Tab context that changed to the corrected behavior, update that expectation to match bash — re-confirm against the real shell first.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/completion.rs
git commit -m "$(cat <<'EOF'
completion(#248) task 3: derive context from parse_recover, delete analyze_full

dispatch::resolve and bashdefault_strings now derive completion context from
parse_recover's CursorContext via cursor_to_completion; analyze_full/analyze/
CompletionContext + their unit tests are deleted. Fixes if/quoted-$(/arith/
for-in (command/variable completion now fires there); redirect/path/nested
completion preserved.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: PTY spot-check vs bash + docs

**Files:**
- Modify: `docs/architecture.md`

**Interfaces:**
- Consumes: the finished, wired completion.
- Produces: bash-parity evidence + docs.

- [ ] **Step 1: PTY spot-check the four fixes + regressions against bash**

Build the binary (`cargo build -p huck --bin huck`) and drive it under a Python `pty` (model on the prior completion fixes). For each fragment, press Tab and compare the candidate list / insertion to `bash --norc -i` under the same fragment, in a disposable scratch dir with known files:

- `if whi<TAB>` → commands (both).
- `echo "$(whi<TAB>` → commands.
- `echo $(( HO<TAB>` → variables.
- `for x in <file-prefix><TAB>` → files.
- Regressions: `echo > <file-prefix><TAB>` → files; `./<prefix><TAB>` → files; `cat foo > ${HO<TAB>` → variables (NOT files).

Record the per-case huck-vs-bash result in the report. Clean up the scratch dir.

- [ ] **Step 2: Docs**

In `docs/architecture.md`'s completion section: note that completion context is now derived from `huck_syntax::parse_recover` (the recovery parser) rather than a hand-rolled scanner, that this fixed the command-substitution / arithmetic / compound-command / redirect-target divergences, and remove any lingering `analyze_full` reference. Grep the repo for stale mentions:

```bash
grep -rn "analyze_full" docs/ crates/ README.md
```

Expected: none (outside this plan/spec paper trail).

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
completion(#248) task 4: docs + PTY bash-parity note

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Full verification and PR

**Files:** none — the gate.

- [ ] **Step 1: Per-crate suites + fmt**

```bash
cargo fmt --all --check
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
cargo test -p huck-cli --jobs 1 --lib -- --test-threads 1
```

Expected: all green, fmt clean.

- [ ] **Step 2: The `-p huck` completion integration binaries (pre-push, per repo rule)**

```bash
for t in completion_integration complete_actions_integration arith_completion_integration; do
  ulimit -v 1500000
  timeout 200 cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | tail -3
done
```

Expected: green.

- [ ] **Step 3: Diff-check sweep (unaffected — completion is interactive, not `-c`)**

```bash
cargo build --locked --bin huck
cargo build --release --locked --bin huck
ulimit -v 1500000
timeout 900 tests/scripts/run_diff_checks.sh 2>&1 | tail -3
```

Expected: `Diff-check sweep: N passed, 0 failed`.

- [ ] **Step 4: Push and open the PR**

```bash
git push -u origin completion-from-cursorcontext
gh pr create --title "completion: derive context from parse_recover, delete analyze_full (#248)" --body "$(cat <<'EOF'
Closes #248

Iteration 2 of the parser-driven completion effort (iteration 1 = #246,
merged). Deletes the hand-rolled `analyze_full` completion-context scanner and
derives context from `huck_syntax::parse_recover`'s `CursorContext`.

## What changed

- **huck-syntax:** the recovery capture now emits `WordPosition::RedirectTarget`
  for a bare redirect operand — at lowest priority, so `cat foo > ${HOM` stays
  variable completion (the inner expansion wins).
- **huck-engine:** a `cursor_to_completion` mapper (position + word → source,
  with the `/`-in-word→file rule, dir/prefix split, and programmable-spec lookup
  for arguments) replaces the context-branching in `dispatch::resolve` and the
  `-o bashdefault` fallback. `analyze_full` / `analyze` / `CompletionContext` and
  their unit tests are gone.

## Fixes (all previously completed nothing where bash completes)

- `if whi` → commands
- `echo "$(whi` → commands (inside the quoted command substitution)
- `echo $(( HO` → variables (arithmetic)
- `for x in whi` → words / files

## Verified (bash 5.2.21, PTY-probed)

The four fixes plus no regressions to redirect-target (`echo > f`), path
(`./f`), and the redirect-with-inner-expansion priority (`cat foo > ${HO` →
variables). Mapper + dispatch unit tests, the completion integration binaries,
and the diff sweep.

## Deferred follow-ups (pre-existing gaps, not regressions)

- Scalar assignment-value completion (`x=my` → file).
- Nested-command programmable-spec lookup inside `$(…)`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: Wait for CI**

```bash
gh pr checks --watch
```

Poll until CI **finishes** and passes (local green ≠ CI green — this box is 1-core, CI 4-core; and the completion change crosses into huck-cli's lib, so `huck-cli --lib` must be green too). Then hand the PR to the user. **Do not merge it.**

---

## Self-Review Notes

**Spec coverage.** Every spec section maps to a task: the `RedirectTarget` capture extension → Task 1; the `cursor_to_completion` mapper (all six rows + dir/prefix split + anchoring) → Task 2; deletion of `analyze_full`/`analyze`/`CompletionContext` + rewire of both consumers (`dispatch::resolve` AND `bashdefault_strings`) → Task 3; the four fixes → Task 3's dispatch tests; the three test layers → Tasks 2 (mapper units), 3 (dispatch units + integration binaries), 4 (PTY); docs → Task 4; deferred follow-ups noted in Task 5's PR body.

**Two judgment tasks flagged.** Task 1 (huck-syntax capture — read the priority logic + redirect-parse site) and Task 3 (delete a subsystem, rewire two consumers). Both give the mechanism, exact sites, and concrete tests as the contract.

**Type consistency.** `cursor_to_completion(cursor, line, pos, shell) -> (usize, Vec<Candidate>)`, `split_word_anchor(word, word_start) -> (String, String, usize)`, `WordPosition::{Command,Argument,VariableName,RedirectTarget,AssignRhs}`, and `CursorContext.{position,word,word_start}` are used identically across Tasks 2 and 3. `bashdefault_strings` keeps its `&Shell` (not `&mut`) signature.

**One consumer the spec under-emphasized, now explicit:** `bashdefault_strings` (the `-o bashdefault` fallback) is a SECOND `analyze` consumer beyond `dispatch::resolve`; Task 3 Step 3 rewires it. Missing it would leave a dangling `analyze` reference and fail the Step 4 grep.
