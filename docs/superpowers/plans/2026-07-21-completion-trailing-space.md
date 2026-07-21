# Completion Trailing-Space + Meaningful `-o nospace` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Append bash's trailing space after a unique completion of a non-directory word, and make `complete -o nospace` suppress it — closing #42.

**Architecture:** The space is a **tab-dispatch decoration**, not a property of the shared candidate builders. `dispatch::resolve` applies it to the command/variable/plain-file branches; `run_spec_with_empty_fallback` applies it (gated on `nospace`) for the programmable path. It rides rustyline's insertion logic — the full `replacement` is inserted only on a unique match, so the space surfaces exactly when bash's does. `compgen` is untouched because it uses a different path (`run_spec` → raw strings).

**Tech Stack:** Rust (2024 edition), `crates/huck-engine` completion subsystem, rustyline 18 for the interactive editor.

**Spec:** `docs/superpowers/specs/2026-07-21-completion-trailing-space-design.md`
**Issue:** [#42](https://github.com/jdstanhope/huck/issues/42)

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run `cargo fmt --all` before every commit — CI enforces `cargo fmt --all --check`.
- **Never** run `cargo test --workspace` — this box (1 core / 1.9GB) OOM-kills the session. Always `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`.
- Build the binary with `cargo build -p huck --bin huck`. Guard harness/interactive runs with `ulimit -v 1500000` and a `timeout`.
- All completion candidates: `display` is the Tab-Tab label (NEVER gets a space); `replacement` is the inserted text (gets the trailing space for word-final non-directory candidates).
- Target fidelity is **bash 5.2.21**. Probe the real shell to confirm behavior; the spec's ground-truth table was measured, not assumed.

## Codebase Orientation

Read this before Task 1 — it corrects one thing the spec got slightly wrong.

**The spec says "modify the four candidate builders."** That is the wrong insertion point, because of a reuse hazard found during planning: `complete_command` / `complete_variable` / `complete_file` (in `completion.rs`) are reused by `file_completion_strings` (`completion.rs:617`) and `bashdefault_strings` (`completion.rs:632`), which take each candidate's `.replacement` as a **raw intermediate string** for the spec path's `-o default` / `-o bashdefault` empty-fallback. If a builder appended a space, that string (`"file.txt "`) would flow back into `run_spec_with_empty_fallback`'s renderer, fail its `is_dir` check (no file named `"file.txt "`), and get a **second** space — a double-decoration bug.

So the space is applied at the **tab-dispatch layer**, keeping the builders pure:

- `dispatch::resolve` (`completion.rs`) has the tab entry branches. The three **built-in** returns get the space via a helper: variable (`completion.rs:446`), command (`completion.rs:462`), plain-file/no-spec (`completion.rs:492`).
- The **programmable** path renders its own final candidates in `run_spec_with_empty_fallback` (`completion.rs`, the `filenames` / non-`filenames` branches around `:550-601`), where `nospace` and `is_dir` are both known — the space goes there, gated on `effective_options.nospace`.

**Why the built-in branches can't share the spec path's logic:** the built-in candidates carry a real `CandidateKind` (`Directory` vs `File`/`Command`/`Variable`), so a helper can skip directories by kind. Spec-path candidates are all `CandidateKind::Custom` regardless of dir-ness, so that path must decide dir-ness by `is_dir` at render time — which it already computes.

**`compgen` is unaffected** and must stay so: `builtin_compgen` (`completion_builtins.rs:487`) calls `completion_spec::run_spec` (returns `Vec<String>`, no `Candidate`, no space) and prints the strings. `compgen -A command` routes through `complete_action` → `complete_command(...).map(|c| c.display)` (uses `display`, not `replacement`). Neither touches the tab-dispatch decoration.

**`CandidateKind`** (`completion.rs:9`): `Command`, `Variable`, `File`, `Directory`, `Custom`.

**`CompOptions.nospace`** (`completion_spec.rs:40`): already parsed from `complete -o nospace`, currently unread. `effective_options` (the post-`compopt` options) is computed in `run_spec_with_empty_fallback` at `completion.rs:529`.

**Existing builder unit tests stay unchanged** — they test the builders directly (`complete_command_matches_builtin_prefix` at `completion.rs:1279` asserts `replacement == "echo"`, `complete_file` dir test at `:1370` asserts `"mysub/"`, etc.). Because the space is NOT added in the builders, these keep passing. New tests target `dispatch::resolve` and `run_spec_with_empty_fallback`.

---

## File Structure

- **Modify** `crates/huck-engine/src/completion.rs` — add a private `append_trailing_space_non_dir` helper; call it in the three built-in `dispatch::resolve` branches; add the space (nospace-gated) in `run_spec_with_empty_fallback`'s two rendering branches. New unit tests in the existing `#[cfg(test)] mod tests`.
- No other source files change. `completion_spec.rs`, `completion_builtins.rs`, and the `compgen` path are read-only here.

---

## Task 1: Trailing space on the built-in tab-dispatch branches

**Files:**
- Modify: `crates/huck-engine/src/completion.rs` (add helper; `dispatch::resolve` branches at `:446`, `:462`, `:492`)
- Test: `crates/huck-engine/src/completion.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Candidate`, `CandidateKind` (existing).
- Produces: `fn append_trailing_space_non_dir(cands: Vec<Candidate>) -> Vec<Candidate>` (module-private, in `completion.rs`) — appends a single `' '` to the `replacement` of every candidate whose `kind` is not `CandidateKind::Directory`; leaves `display` untouched.

- [ ] **Step 1: Write the failing tests**

Add to `crates/huck-engine/src/completion.rs`'s `mod tests`:

```rust
#[test]
fn dispatch_command_appends_trailing_space() {
    let mut sh = Shell::new();
    // `ech` uniquely prefixes the `echo` builtin among commands here.
    let (_start, cands) = dispatch::resolve("ech", 3, &mut sh);
    let echo = cands
        .iter()
        .find(|c| c.display == "echo")
        .expect("echo candidate present");
    assert_eq!(echo.replacement, "echo ", "command replacement gets a trailing space");
    assert_eq!(echo.display, "echo", "display stays clean (no space)");
}

#[test]
fn dispatch_variable_appends_trailing_space() {
    let mut sh = Shell::new();
    sh.set("MYUNIQUEVAR", "x".to_string());
    let (_start, cands) = dispatch::resolve("echo $MYUNIQ", 11, &mut sh);
    let v = cands
        .iter()
        .find(|c| c.display == "MYUNIQUEVAR")
        .expect("variable candidate present");
    assert_eq!(v.replacement, "MYUNIQUEVAR ");
    assert_eq!(v.display, "MYUNIQUEVAR");
}

#[test]
fn dispatch_plain_file_space_but_dir_slash() {
    // No completion spec registered -> the None (plain-file) branch.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("solofile.txt"), b"x").unwrap();
    std::fs::create_dir(dir.path().join("solodir")).unwrap();
    let mut sh = Shell::new();
    let _guard = crate::test_support::CWD_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let (_s1, file_cands) = dispatch::resolve("cat solof", 9, &mut sh);
    let (_s2, dir_cands) = dispatch::resolve("cat solod", 9, &mut sh);
    std::env::set_current_dir(prev).unwrap();

    let f = file_cands.iter().find(|c| c.display == "solofile.txt").unwrap();
    assert_eq!(f.replacement, "solofile.txt ", "regular file gets a space");
    let d = dir_cands.iter().find(|c| c.display == "solodir/").unwrap();
    assert_eq!(d.replacement, "solodir/", "directory keeps `/`, no space");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p huck-engine --jobs 1 --lib dispatch_command_appends -- --test-threads 1
```

Expected: FAIL — `assert_eq!(echo.replacement, "echo ")` gets `"echo"` (no space).

- [ ] **Step 3: Add the helper and wire the three branches**

Add the helper near the other free functions in `completion.rs` (e.g. just above `pub fn complete_command`):

```rust
/// Appends bash's post-completion trailing space to the `replacement` of
/// every non-directory candidate; directories keep their `/` and get no
/// space. `display` is never touched. This is the tab-dispatch decoration
/// for the BUILT-IN completion kinds (command/variable/plain-file), whose
/// `CandidateKind` reliably distinguishes directories. The space surfaces
/// only on a unique match — rustyline inserts the full `replacement` for a
/// single candidate and only the common prefix (space excluded) otherwise.
fn append_trailing_space_non_dir(mut cands: Vec<Candidate>) -> Vec<Candidate> {
    for c in &mut cands {
        if c.kind != CandidateKind::Directory {
            c.replacement.push(' ');
        }
    }
    cands
}
```

In `dispatch::resolve`, wrap the three built-in returns:

At `completion.rs:446` (variable):
```rust
            return (start, append_trailing_space_non_dir(complete_variable(prefix, &var_names)));
```

At `completion.rs:462` (command):
```rust
            return (
                start,
                append_trailing_space_non_dir(complete_command(prefix, &path, &funcs, &aliases)),
            );
```

At `completion.rs:492` (plain-file / `None` branch):
```rust
                let home = shell.get("HOME").unwrap_or("").to_string();
                (start, append_trailing_space_non_dir(complete_file(dir, prefix, &home)))
```

Do NOT touch the two spec-path returns (`:456` and the `Some(spec)` arm) — Task 2 handles those. Do NOT touch the builders themselves.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib dispatch_ -- --test-threads 1
```

Expected: the 3 new tests PASS. The existing builder tests (`complete_command_matches_builtin_prefix`, the `complete_file` tests) also still pass — confirm with:

```bash
cargo test -p huck-engine --jobs 1 --lib "completion::tests" -- --test-threads 1
```

Expected: all green (builders unchanged, so their `replacement == "echo"` / `"mysub/"` assertions hold).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/completion.rs
git commit -m "$(cat <<'EOF'
v-nospace task 1: trailing space on built-in tab completions (#42)

dispatch::resolve now appends bash's post-completion space to the
command/variable/plain-file branches via append_trailing_space_non_dir
(directories keep `/`, no space). Applied at the dispatch layer, not in
the builders, so file_completion_strings/bashdefault_strings keep reusing
the raw builder replacements without double-decoration.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Trailing space on the programmable path, gated by `nospace`

**Files:**
- Modify: `crates/huck-engine/src/completion.rs` (`run_spec_with_empty_fallback`, the `filenames` and non-`filenames` rendering branches, ~`:550-601`)
- Test: `crates/huck-engine/src/completion.rs` (`mod tests`)

**Interfaces:**
- Consumes: `effective_options` (already computed at `completion.rs:529`, has `.nospace`, `.filenames`, `.nosort`).
- Produces: no new symbols — behavior change only.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests`:

```rust
#[test]
fn spec_default_appends_space_nospace_suppresses() {
    // A `-W` wordlist spec for `tcmd`. Default -> trailing space;
    // -o nospace -> no space. Directories are not involved here.
    let mut sh = Shell::new();
    let _ = crate::shell::process_line("_noop() { :; }", &mut sh, false);
    let spec_default = crate::completion_spec::CompletionSpec {
        wordlist: Some("foobar".to_string()),
        ..Default::default()
    };
    std::rc::Rc::make_mut(&mut sh.completion_specs)
        .by_command
        .insert("tcmd".to_string(), spec_default);
    let (_s, cands) = dispatch::resolve("tcmd foo", 8, &mut sh);
    let c = cands.iter().find(|c| c.display == "foobar").unwrap();
    assert_eq!(c.replacement, "foobar ", "default spec completion gets a space");

    let spec_nospace = crate::completion_spec::CompletionSpec {
        wordlist: Some("foobar".to_string()),
        options: crate::completion_spec::CompOptions {
            nospace: true,
            ..Default::default()
        },
        ..Default::default()
    };
    std::rc::Rc::make_mut(&mut sh.completion_specs)
        .by_command
        .insert("tcmd".to_string(), spec_nospace);
    let (_s2, cands2) = dispatch::resolve("tcmd foo", 8, &mut sh);
    let c2 = cands2.iter().find(|c| c.display == "foobar").unwrap();
    assert_eq!(c2.replacement, "foobar", "-o nospace suppresses the trailing space");
}

#[test]
fn spec_filenames_dir_keeps_slash_even_under_nospace() {
    // A -o filenames -o nospace spec whose candidate is a real directory:
    // the `/` survives nospace; a real file under nospace gets no space.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("adir")).unwrap();
    std::fs::write(root.path().join("afile"), b"x").unwrap();
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "_t() { COMPREPLY=('adir' 'afile'); }",
        &mut sh,
        false,
    );
    std::rc::Rc::make_mut(&mut sh.completion_specs).by_command.insert(
        "dcmd".to_string(),
        crate::completion_spec::CompletionSpec {
            function: Some("_t".to_string()),
            options: crate::completion_spec::CompOptions {
                filenames: true,
                nospace: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let _guard = crate::test_support::CWD_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(root.path()).unwrap();
    let (_s, cands) = dispatch::resolve("dcmd a", 6, &mut sh);
    std::env::set_current_dir(prev).unwrap();

    let d = cands.iter().find(|c| c.display == "adir/").unwrap();
    assert_eq!(d.replacement, "adir/", "directory keeps `/` under nospace");
    let f = cands.iter().find(|c| c.display == "afile").unwrap();
    assert_eq!(f.replacement, "afile", "file gets no space under nospace");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p huck-engine --jobs 1 --lib spec_default_appends_space -- --test-threads 1
```

Expected: FAIL — `assert_eq!(c.replacement, "foobar ")` gets `"foobar"`.

- [ ] **Step 3: Add the space in both rendering branches**

In `run_spec_with_empty_fallback` (`completion.rs`), the two branches build `candidates`. Add the space to each, gated on `nospace`, skipping directories.

**`filenames` branch** (around `:550-582`): the `is_dir` value is already computed and the `replacement` already gets a `/` appended for directories. Append the space to the FINAL `replacement` when the candidate is not a directory and `nospace` is off. Change the tail of the map closure so, after the existing `if is_dir { replacement.push('/'); }`:

```rust
                    if is_dir {
                        replacement.push('/');
                    } else if !effective_options.nospace {
                        replacement.push(' ');
                    }
                    Candidate {
                        display,
                        replacement,
                        kind: CandidateKind::Custom,
                    }
```

**Non-`filenames` branch** (around `:583-591`): every candidate is a plain word (no directory concept). Append the space unless `nospace`:

```rust
            after_fallback
                .into_iter()
                .map(|s| {
                    let mut replacement = s.clone();
                    if !effective_options.nospace {
                        replacement.push(' ');
                    }
                    Candidate {
                        display: s,
                        replacement,
                        kind: CandidateKind::Custom,
                    }
                })
                .collect()
```

Note: the dedupe step below (`:604`, `seen.insert(c.replacement.clone())`) still dedupes correctly — all replacements in a branch gain the same suffix, so uniqueness is preserved.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib "completion::tests" -- --test-threads 1
```

Expected: the 2 new tests pass, and the existing spec-path tests (`spec_filenames_display_is_basename_not_full_path`, `spec_filenames_appends_slash_to_tilde_dir`, `filter_*`, `dispatch_d_default_spec_applies_when_no_match`, etc.) — check whether any asserted a bare `replacement` that now gains a space. If one fails on a now-spaced replacement, that is the intended behavior change: update its expected string to include the trailing space (files/words) — but NOT for directory or `-o nospace` cases. Directory-replacement assertions (ending `/`) must stay unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/completion.rs
git commit -m "$(cat <<'EOF'
v-nospace task 2: trailing space on programmable completion + nospace (#42)

run_spec_with_empty_fallback now appends the trailing space to non-dir
candidates unless `-o nospace` is set; directories keep `/` even under
nospace. This makes CompOptions.nospace load-bearing (was parsed but
unread), closing the core of #42.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `compgen` regression guard, existing-test sweep, docs, verify

**Files:**
- Test: `crates/huck-engine/src/completion.rs` or `crates/huck-engine/src/completion_builtins.rs` (`mod tests`) — a compgen-unchanged guard
- Modify: `crates/huck-engine/src/completion_spec.rs:40` area (the `nospace` doc comment), `docs/architecture.md` if it describes completion decoration

**Interfaces:**
- Consumes: everything from Tasks 1-2.
- Produces: nothing new.

- [ ] **Step 1: Add the compgen-unchanged regression guard**

The subtlest risk is the space leaking into `compgen` output (bash's `compgen` has NO trailing space). Add a guard test in `completion_builtins.rs`'s `mod tests` (follow the existing `compgen_*` test style there):

```rust
#[test]
fn compgen_output_has_no_trailing_space() {
    // The tab-dispatch trailing space must NOT reach compgen (bash's
    // compgen lists matches without a trailing space). #42.
    let mut sh = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    builtin_compgen(
        &["compgen".into(), "-W".into(), "foobar".into(), "foo".into()],
        &mut out,
        &mut err,
        &mut sh,
    );
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "foobar\n",
        "compgen output must not carry the tab-completion trailing space"
    );
}
```

Adjust the `builtin_compgen` call to match its real signature (check the existing `compgen_*` tests in that file for the exact argument shape — some take a `&[String]`, some a sink; mirror a neighbor).

- [ ] **Step 2: Run the guard + full completion suites**

```bash
cargo test -p huck-engine --jobs 1 --lib compgen -- --test-threads 1
cargo test -p huck-engine --jobs 1 --lib "completion" -- --test-threads 1
```

Expected: green. If `compgen_output_has_no_trailing_space` fails, a Task 1/2 change leaked into the `run_spec`/compgen path — stop and fix the leak, do not adjust the test.

- [ ] **Step 3: Run the `-p huck` completion integration binaries**

These drive the real binary and encode completion behavior; a change to candidate output can surface here (and only here) exactly as it does on CI.

```bash
for t in completion_integration complete_actions_integration arith_completion_integration; do
  ulimit -v 1500000
  echo "=== $t ==="
  timeout 200 cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | tail -4
done
```

Expected: green. These assert mostly on `compgen`/`complete` **stdout** (no space), so they should be unaffected. If one asserts on an interactive Tab insertion and now sees a space, update that expectation to include the space (it is the intended change) — but re-confirm it is a Tab-insertion assertion, not a compgen-output one.

- [ ] **Step 4: Update docs**

- `crates/huck-engine/src/completion_spec.rs:40`: update the `nospace` field doc comment to state it now suppresses the post-completion trailing space (drop any "parsed but unread" wording).
- `docs/architecture.md`: if it describes completion decoration (`/` for dirs), add that word-final non-directory completions also get a trailing space, suppressible via `-o nospace`.

- [ ] **Step 5: Manual PTY spot-check against bash**

Build and drive the real binary under a PTY (the "unique → insert full replacement with space" step is rustyline's, so it needs an end-to-end check). Use a scratch dir. Confirm, against bash 5.2.21:

```bash
cargo build -p huck --bin huck
```

Spot-checks (type the fragment, press Tab, observe the trailing char): a unique command completes with a trailing space; a unique file completes with a trailing space; a directory completes with `/` and no space; an ambiguous prefix extends with no space; `complete -o nospace -W foobar tc; tc foo<TAB>` inserts `foobar` with no space; `complete -W foobar tc; tc foo<TAB>` inserts `foobar ` with a space. Drive it with a Python `pty` script (as prior completion fixes did) and compare the trailing character to bash under the same fragment.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v-nospace task 3: compgen-unchanged guard + docs (#42)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Full verification and PR

**Files:** none — this is the gate.

- [ ] **Step 1: Per-crate suites + fmt**

```bash
cargo fmt --all --check
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```

Expected: green, fmt clean.

- [ ] **Step 2: Diff-check sweep (must stay green — it is non-interactive, so unaffected)**

```bash
cargo build --locked --bin huck
cargo build --release --locked --bin huck
ulimit -v 1500000
timeout 900 tests/scripts/run_diff_checks.sh 2>&1 | tail -3
```

Expected: `Diff-check sweep: N passed, 0 failed`.

- [ ] **Step 3: File the deferred follow-up**

Open an issue for the deferred corner: core readline appends `/` (not a space) when a completed variable's value is an existing directory (`echo $HOM<TAB>` → `$HOME/`). Label `divergence` + `bug` + `sev:low`. Note it in the PR body.

- [ ] **Step 4: Push and open the PR**

```bash
git push -u origin completion-trailing-space
gh pr create --title "completion: trailing space after unique completion + meaningful -o nospace (#42)" --body "$(cat <<'EOF'
Closes #42

bash appends a trailing space after a unique completion of a non-directory
word; huck never did, so `complete -o nospace` had nothing to suppress. This
adds the default space and makes `nospace` suppress it.

## Approach

The space is a tab-dispatch decoration, applied where the interactive editor
gets its candidates — not in the shared builders (which are reused as raw
strings by the `-o default`/`-o bashdefault` empty-fallback, where a baked-in
space would double-decorate). `dispatch::resolve` decorates the
command/variable/plain-file branches; `run_spec_with_empty_fallback` decorates
the programmable path, gated on `-o nospace`. It rides rustyline's insertion
logic, so the space appears only on a unique match — exactly like bash.

`compgen` is untouched (it uses `run_spec` → raw strings), guarded by a test.

## Verified (bash 5.2.21, PTY-probed)

Unique command/file/variable → trailing space; directory → `/`, no space;
ambiguous → common prefix, no space; `-o nospace` → no space but directories
still get `/`.

## Deferred

Core readline's `$HOME/` behavior (variable whose value is a directory) — filed
as a follow-up.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: Wait for CI**

```bash
gh pr checks --watch
```

Poll until CI **finishes** and passes (local green ≠ CI green — this box is 1-core, CI is 4-core). Then hand the PR to the user. **Do not merge it.**

---

## Self-Review Notes

**Spec coverage.** Every spec requirement maps to a task: the mechanism and the built-in kinds → Task 1; the programmable path + `nospace` (and directory-keeps-`/`-under-nospace) → Task 2; the `compgen`-unchanged guarantee, existing-test sweep, and docs → Task 3; verification + the deferred follow-up → Task 4.

**One deliberate deviation from the spec, documented in Codebase Orientation.** The spec named "the four candidate builders" as the insertion sites. Planning found that three of them (`complete_command`/`complete_variable`/`complete_file`) are reused as raw strings by the empty-fallback helpers, so decorating them there would double-decorate. The plan applies the space one layer out — at `dispatch::resolve` for the built-in kinds, and inside `run_spec_with_empty_fallback` for the spec path — which achieves the same behavior without the hazard and keeps the builders' existing unit tests green.

**Deferred item preserved.** The variable-value-is-a-directory `/` corner is out of scope in both spec and plan, and Task 4 files it.
