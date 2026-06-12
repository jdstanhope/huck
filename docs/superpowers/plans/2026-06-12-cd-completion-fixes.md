# huck v143 — `cd` completion after bashrc (fixes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two bugs in huck's programmable-completion path so `cd <TAB>` works after `source ~/.bashrc` (bash-completion's `complete -o nospace -F _cd cd`): the `cd projects/projects` mis-join and `cd ~/<TAB>` returning nothing.

**Architecture:** (1) The programmable/spec path anchors the text replacement at the start of the whole `cur` word (`word_start`) and uses full-path candidates — bash's model — while the default file-completion path is untouched. (2) Tilde-expand a literal `~/` dir prefix at the two spec-path sites (the compgen enumerator + the `-o filenames` is-dir probe). `-o nospace` is out of scope (a no-op in huck) and logged as a deferred divergence.

**Tech Stack:** Rust; `src/completion.rs` (`analyze`/`dispatch::resolve`/`run_spec_with_empty_fallback`), `src/completion_spec.rs` (`enumerate_action`/`list_dir_with_path_prefix`). Tests use `tempfile` (already a dev-dependency).

**Reference:** spec at `docs/superpowers/specs/2026-06-12-cd-completion-fixes-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits in a prior iteration). Stay on `v143-cd-completion`. Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** BINARY crate — `cargo test --bin huck <filter>` (the completion unit tests live in `src/`, so use `--bin huck`), `cargo clippy --all-targets`. Builds take minutes. The `analyze`/`dispatch`/`enumerate_action` tests are in-crate `#[test]`s (dispatch is `pub(crate)`), run via `cargo test --bin huck`.

---

### Task 1: Bug 1 — anchor the programmable path at the word start

**Files:**
- Modify: `src/completion.rs` (`analyze` ~24; `dispatch::resolve` ~382/419-432; `file_completion_strings` ~527; `bashdefault_strings` File arm ~552; add tests in `mod tests` ~702)

- [ ] **Step 1: Write the failing tests** — add to `src/completion.rs` `mod tests` (after the existing `analyze_*` tests):

```rust
#[test]
fn analyze_full_reports_word_start_for_slash_word() {
    // `cd projects/sub`: whole word starts at 3; basename anchor after the slash.
    let (word_start, start, ctx) = analyze_full("cd projects/sub", 15);
    assert_eq!(word_start, 3);
    assert_eq!(start, 12); // just past "projects/"
    assert_eq!(
        ctx,
        CompletionContext::File { dir: "projects/".to_string(), prefix: "sub".to_string() }
    );
}

#[test]
fn analyze_full_word_start_equals_start_without_slash() {
    let (word_start, start, _) = analyze_full("cd pr", 5);
    assert_eq!(word_start, 3);
    assert_eq!(start, 3);
}

#[test]
fn spec_completion_anchors_at_word_start() {
    // A `complete -F _fake cd` returning FULL-PATH candidates must replace the
    // whole `projects/` word (anchor at word_start=3), not the after-slash
    // suffix — otherwise rustyline double-pastes -> `cd projects/projects`.
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "_fake() { COMPREPLY=(projects/alpha projects/beta); }",
        &mut sh,
        false,
    );
    sh.completion_specs.by_command.insert(
        "cd".to_string(),
        crate::completion_spec::CompletionSpec {
            function: Some("_fake".to_string()),
            ..Default::default()
        },
    );
    let (start, cands) = dispatch::resolve("cd projects/", 12, &mut sh);
    assert_eq!(start, 3, "must anchor at the start of `projects/`");
    let reps: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
    assert!(reps.contains(&"projects/alpha"), "{reps:?}");
    assert!(reps.contains(&"projects/beta"), "{reps:?}");
}

#[test]
fn spec_default_fallback_yields_full_relative_paths() {
    // `complete -o default -F _empty cd`: when the function yields nothing, the
    // empty-fallback must return FULL cur-relative paths (not basenames), so the
    // word_start anchor replaces the whole `<dir>/<base>` correctly.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("alpha")).unwrap();
    let base = dir.path().to_str().unwrap(); // absolute => no chdir needed
    let mut sh = Shell::new();
    let _ = crate::shell::process_line("_empty() { COMPREPLY=(); }", &mut sh, false);
    sh.completion_specs.by_command.insert(
        "cd".to_string(),
        crate::completion_spec::CompletionSpec {
            function: Some("_empty".to_string()),
            options: crate::completion_spec::CompletionOptions {
                default: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let line = format!("cd {base}/al");
    let pos = line.len();
    let (start, cands) = dispatch::resolve(&line, pos, &mut sh);
    assert_eq!(start, 3, "anchor at the start of the path word");
    let reps: Vec<String> = cands.iter().map(|c| c.replacement.clone()).collect();
    assert!(
        reps.iter().any(|r| r == &format!("{base}/alpha/")),
        "expected full path {base}/alpha/, got {reps:?}"
    );
}
```

NOTE: match the EXACT field names in `crate::completion_spec::{CompletionSpec, CompletionOptions}` (grep them first: `CompletionOptions` has `default`, `bashdefault`, `filenames`, `nospace`, `dirnames`; `CompletionSpec` has `function`, `options`, …). Adjust the struct literals if a field differs.

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck completion:: 2>&1 | tail -25` (or filter the four new test names).
Expected: `analyze_full_*` FAIL to compile (`analyze_full` undefined) → after you add the fn they’d compile; the anchor tests FAIL because `dispatch::resolve` returns `start=12` not `3`, and the fallback test FAILs because it returns basenames. Record what you see (compile error first is fine).

- [ ] **Step 3a: Add `analyze_full` + make `analyze` a thin wrapper** — `src/completion.rs`. Rename the body of the existing `pub fn analyze` to `analyze_full`, change its return type to a 3-tuple `(word_start, start, context)`, and add a 2-line `analyze` wrapper. Concretely:

Change the signature line (was `pub fn analyze(line: &str, pos: usize) -> (usize, CompletionContext) {`) to:
```rust
/// Like `analyze`, but also returns the start of the WHOLE word
/// (`word_start`) — the anchor the programmable-completion path uses to
/// replace the entire `cur` word with full-path candidates.
pub(crate) fn analyze_full(line: &str, pos: usize) -> (usize, usize, CompletionContext) {
```
Update each of the four `return`/final expressions inside that body to prepend `word_start`:
- `return (word_start + name_off, CompletionContext::Variable { .. });`
  → `return (word_start, word_start + name_off, CompletionContext::Variable { .. });`
- `return (word_start + slash + 1, CompletionContext::File { dir, prefix });`
  → `return (word_start, word_start + slash + 1, CompletionContext::File { dir, prefix });`
- `return (word_start, CompletionContext::File { dir: String::new(), prefix: unescape(word) });`
  → `return (word_start, word_start, CompletionContext::File { dir: String::new(), prefix: unescape(word) });`
- final `(word_start, CompletionContext::Command { prefix: unescape(word) })`
  → `(word_start, word_start, CompletionContext::Command { prefix: unescape(word) })`

Then add the wrapper directly above `analyze_full` (keeps all existing callers/tests unchanged):
```rust
/// Classifies the completion context at byte offset `pos` in `line`.
/// Returns the basename replacement offset and the context.
pub fn analyze(line: &str, pos: usize) -> (usize, CompletionContext) {
    let (_, start, ctx) = analyze_full(line, pos);
    (start, ctx)
}
```

- [ ] **Step 3b: Anchor the spec branch at `word_start`** — `src/completion.rs` `dispatch::resolve`. Change the analyze call (line ~382) from `let (start, context) = analyze(line, pos);` to:
```rust
        let (word_start, start, context) = analyze_full(line, pos);
```
Then change the `match spec_opt { … }` (lines ~419-433) to:
```rust
        match spec_opt {
            Some(spec) => {
                let cands = run_spec_with_empty_fallback(&spec, line, pos, &cmd_name, shell);
                // Programmable completion replaces the WHOLE cur word with
                // full-path candidates (bash's model) -> anchor at word_start,
                // not the basename offset. Fixes `cd projects/projects`.
                (word_start, cands)
            }
            None => {
                // No spec at all -> existing default file completion
                // (basenames, anchored after the last '/').
                let home = shell.get("HOME").unwrap_or("").to_string();
                (start, complete_file(dir, prefix, &home))
            }
        }
```
(The old `if cands.is_empty() { return (start, Vec::new()); }` is removed — empty candidates produce no completion regardless of offset. `start` is still used by the `None` branch, so it is not dead; if clippy warns `unused` on `word_start` in some build, that means the spec branch edit didn’t land — re-check.)

- [ ] **Step 3c: Empty-fallback emits FULL cur-relative paths** — `src/completion.rs`. Replace `file_completion_strings` (~527) with:
```rust
    fn file_completion_strings(prefix: &str, shell: &Shell) -> Vec<String> {
        let home = shell.get("HOME").unwrap_or("").to_string();
        // Split the cur word into (dir, base) and re-prepend dir so the
        // empty-fallback yields FULL cur-relative paths, consistent with the
        // word_start anchor (matches compgen / bash's `-o default`).
        let (dir, base) = match prefix.rfind('/') {
            Some(idx) => (&prefix[..=idx], &prefix[idx + 1..]),
            None => ("", prefix),
        };
        complete_file(dir, base, &home)
            .into_iter()
            .map(|c| format!("{dir}{}", c.replacement))
            .collect()
    }
```
And the `CompletionContext::File` arm of `bashdefault_strings` (~552) to prepend `dir`:
```rust
            CompletionContext::File { dir, prefix } => {
                let home = shell.get("HOME").unwrap_or("").to_string();
                complete_file(&dir, &prefix, &home)
                    .into_iter()
                    .map(|c| format!("{dir}{}", c.replacement))
                    .collect()
            }
```

- [ ] **Step 4: Run tests + no-regression**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --bin huck completion 2>&1 | tail -20` → the 4 new tests pass; ALL existing completion tests still pass (the default-path tests, the `analyze_*` tests via the wrapper).
Run: `cargo clippy --all-targets 2>&1 | tail -8` → no new warnings.

- [ ] **Step 5: Commit**
```bash
git add src/completion.rs
git commit -m "$(printf 'fix: anchor programmable completion at the word start\n\nFull-path candidates from a complete -F/compgen spec now replace the whole\ncur word (bash model) instead of the after-slash suffix, fixing the\n`cd projects/projects` mis-join. Empty-fallback emits full cur-relative paths.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Bug 2 — tilde expansion in the spec/compgen path

**Files:**
- Modify: `src/completion.rs` (add `expand_tilde_prefix` ~near `resolve_dir` 273; `run_spec_with_empty_fallback` filenames probe ~495; add tests)
- Modify: `src/completion_spec.rs` (`enumerate_action` ~393; `list_dir_with_path_prefix` ~566; add a test)

- [ ] **Step 1: Write the failing tests**

In `src/completion.rs` `mod tests`:
```rust
#[test]
fn expand_tilde_prefix_handles_leading_tilde_slash() {
    assert_eq!(expand_tilde_prefix("~/projects", "/home/x"), "/home/x/projects");
    assert_eq!(expand_tilde_prefix("~/", "/home/x"), "/home/x/");
    assert_eq!(expand_tilde_prefix("projects/", "/home/x"), "projects/"); // no tilde
    assert_eq!(expand_tilde_prefix("~/p", ""), "~/p"); // empty home -> unchanged
}

#[test]
fn spec_filenames_appends_slash_to_tilde_dir() {
    // A `-o filenames` candidate `~/projects` (a real dir under HOME) must get a
    // trailing `/` so `cd ~/<TAB>` can descend. COMPREPLY is single-quoted so the
    // shell does NOT tilde-expand it — the candidate stays literal `~/projects`.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("projects")).unwrap();
    let mut sh = Shell::new();
    sh.set("HOME", home.path().to_str().unwrap().to_string());
    let _ = crate::shell::process_line("_t() { COMPREPLY=('~/projects'); }", &mut sh, false);
    sh.completion_specs.by_command.insert(
        "cd".to_string(),
        crate::completion_spec::CompletionSpec {
            function: Some("_t".to_string()),
            options: crate::completion_spec::CompletionOptions {
                filenames: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let (_start, cands) = dispatch::resolve("cd ~/", 5, &mut sh);
    let reps: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
    assert!(reps.contains(&"~/projects/"), "tilde dir should get trailing slash: {reps:?}");
}
```

In `src/completion_spec.rs` `mod tests` (it already has `use super::*;` and `use crate::shell_state::Shell;`):
```rust
#[test]
fn directory_action_tilde_expands_home() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("projects")).unwrap();
    std::fs::create_dir(home.path().join("pub")).unwrap();
    let mut sh = Shell::new();
    sh.set("HOME", home.path().to_str().unwrap().to_string());

    let res = enumerate_action(Action::Directory, "~/", &sh);
    assert!(res.contains(&"~/projects".to_string()), "{res:?}");
    assert!(res.contains(&"~/pub".to_string()), "{res:?}");

    let res2 = enumerate_action(Action::Directory, "~/pro", &sh);
    assert_eq!(res2, vec!["~/projects".to_string()], "{res2:?}");
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck tilde 2>&1 | tail -20`
Expected: `expand_tilde_prefix_*` fails to compile (fn undefined); the `enumerate_action` tilde test returns an empty vec (read_dir("~/") fails); the filenames test lacks the trailing `/`. Record.

- [ ] **Step 3a: Add the helper** — `src/completion.rs`, near `resolve_dir` (~273):
```rust
/// Replaces a leading `~/` with `home/` (the only tilde form `_filedir`
/// emits to `compgen`). Other inputs pass through unchanged.
pub(crate) fn expand_tilde_prefix(s: &str, home: &str) -> String {
    match s.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => format!("{home}/{rest}"),
        _ => s.to_string(),
    }
}
```

- [ ] **Step 3b: Tilde-expand the `-o filenames` is-dir probe** — `src/completion.rs` `run_spec_with_empty_fallback`. Just BEFORE the `let candidates: Vec<Candidate> = if effective_options.filenames {` block (~491), capture home; then use the expanded path only for the metadata probe (display/replacement keep the literal text):
```rust
        let home = shell.get("HOME").unwrap_or("").to_string();
        let candidates: Vec<Candidate> = if effective_options.filenames {
            after_fallback
                .into_iter()
                .map(|name| {
                    let is_dir = std::fs::metadata(expand_tilde_prefix(&name, &home))
                        .map(|m| m.is_dir())
                        .unwrap_or(false);
                    let display = if is_dir {
                        format!("{name}/")
                    } else {
                        name.clone()
                    };
                    // Preserve a leading `~/` UNescaped (it is tilde-expansion
                    // intent, like the `~` the user already typed); escaping it
                    // would yield `cd \~/projects` (a literal `~` dir). Escape
                    // only the remainder.
                    let mut replacement = match name.strip_prefix("~/") {
                        Some(rest) => format!("~/{}", escape_filename(rest)),
                        None => escape_filename(&name),
                    };
                    if is_dir {
                        replacement.push('/');
                    }
                    Candidate { display, replacement }
                })
                .collect()
        } else {
            after_fallback
                .into_iter()
                .map(|s| Candidate { display: s.clone(), replacement: s })
                .collect()
        };
```
(Changes vs the original: the `let home = …` line; `std::fs::metadata` arg → `expand_tilde_prefix(&name, &home)`; and the `replacement` now preserves a leading `~/` unescaped. Confirm `shell` is in scope and `let home` compiles before the `effective_options` borrow.)

- [ ] **Step 3c: Tilde-expand the compgen enumerator** — `src/completion_spec.rs`. Change `enumerate_action` (~393) to pass `home`:
```rust
fn enumerate_action(action: Action, prefix: &str, shell: &Shell) -> Vec<String> {
    let home = shell.get("HOME").unwrap_or("").to_string();
    match action {
        Action::File => list_dir_with_path_prefix(prefix, false, &home),
        Action::Directory => list_dir_with_path_prefix(prefix, true, &home),
        // … all other arms unchanged …
```
And `list_dir_with_path_prefix` (~566):
```rust
fn list_dir_with_path_prefix(prefix: &str, dirs_only: bool, home: &str) -> Vec<String> {
    let (dir, base) = match prefix.rfind('/') {
        Some(idx) => (&prefix[..=idx], &prefix[idx + 1..]),
        None => ("", prefix),
    };
    let scan_raw = if dir.is_empty() { "." } else { dir };
    // _filedir passes a literal `~/…`; expand it for the read_dir, but
    // re-prepend the ORIGINAL `dir` so candidates come back as `~/projects`.
    let scan_dir = crate::completion::expand_tilde_prefix(scan_raw, home);
    let bare_results = list_dir_filtered(&scan_dir, base, dirs_only);
    bare_results
        .into_iter()
        .map(|name| format!("{dir}{name}"))
        .collect()
}
```

- [ ] **Step 4: Run tests + no-regression**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --bin huck tilde 2>&1 | tail -10` → all new tests pass.
Run: `cargo test --bin huck completion 2>&1 | tail -15` → no regressions.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 5: Commit**
```bash
git add src/completion.rs src/completion_spec.rs
git commit -m "$(printf 'fix: tilde-expand a literal ~/ in the spec/compgen completion path\n\ncompgen -d/-f and the -o filenames is-dir probe now expand a leading ~/\n(what _filedir passes), so `cd ~/<TAB>` lists and descends $HOME.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: 63rd bash-diff harness

**Files:**
- Create: `tests/scripts/cd_completion_diff_check.sh`

- [ ] **Step 1: Write the harness** — `tests/scripts/cd_completion_diff_check.sh`. It passes `~/` QUOTED (literal, as `_filedir` does) so it actually exercises the fix, and uses a clean scratch `$HOME` with NO dotfiles (so the visible-vs-hidden compgen question doesn't confound byte-identity):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v143: compgen -d/-f tilde + slash
# prefixes (the building block under bash-completion's _cd / _filedir).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
TMP=$(mktemp -d) || exit 1
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/projects/alpha" "$TMP/projects/beta" "$TMP/pub"
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(HOME="$TMP" bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(HOME="$TMP" "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Quoted ~/ => literal, reaches compgen unexpanded (the _filedir case).
check "compgen -d quoted ~/"      'compgen -d -- "~/" | sort'
check "compgen -d var ~/"         'cur="~/"; compgen -d -- "$cur" | sort'
check "compgen -d var ~/pro"      'cur="~/pro"; compgen -d -- "$cur" | sort'
check "compgen -d var ~/projects/" 'cur="~/projects/"; compgen -d -- "$cur" | sort'
check "compgen -f var ~/projects/" 'cur="~/projects/"; compgen -f -- "$cur" | sort'
# Relative slash prefix (already worked; coverage).
check "compgen -d projects/"      'cd ~ && compgen -d -- projects/ | sort'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: chmod + build + run**

Run: `chmod +x tests/scripts/cd_completion_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/cd_completion_diff_check.sh`
Expected: `Total: 6, Pass: 6, Fail: 0`. If any FAIL, paste the diff and STOP (a real divergence — do not weaken the harness). If a FAIL is on the empty-base `compgen -d -- "~/"` case due to a pre-existing compgen hidden-files difference (huck hides dotfiles, bash may not), note it: the scratch `$HOME` has no dotfiles so this should not trigger — if it does, it’s a separate pre-existing divergence to report, not something to mask.

- [ ] **Step 3: Commit**
```bash
git add tests/scripts/cd_completion_diff_check.sh
git commit -m "$(printf 'test: 63rd bash-diff harness for cd-completion compgen tilde/slash\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Docs — deferred nospace divergence

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Add the low-impact entry**

Find the Tier-4 (Low-impact) section and the summary table (`grep -n "Low-impact (Tier 4)\|L-35" docs/bash-divergences.md`). Add a new entry with the next free L-number (L-35 is the latest from v142, so use **L-36**), in the house format of neighboring short L-entries:
```
- **L-36: `complete -o nospace` is a no-op (no default trailing space after completion)** — `[deferred]`, low (v143). huck never appends the trailing space bash adds after completing a final (non-directory) word — rustyline (`CompletionType::List`) inserts the replacement verbatim; the only append is `/` for directories. So `complete -o nospace` has nothing to suppress (it parses into `CompletionOptions.nospace` but is unread at tab-dispatch). Honoring nospace meaningfully would first require implementing bash's default trailing-space behavior. Low impact: the directory-descend flow is unaffected (`cd dir/<TAB>` already adds no space).
```
Increment the Tier-4 count in the summary table by 1 (30 → 31).

- [ ] **Step 2: Commit**
```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: log L-36 (complete -o nospace no-op / no default trailing space)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Full suite**

Run: `cargo test 2>&1 | grep -E "test result|[1-9][0-9]* failed" | tail -40`
Expected: every line `ok`, zero failures (baseline 3080+ tests after v142, plus the ~7 new completion tests). Paste any failure.

- [ ] **Step 2: Completion + PTY paths explicitly (what v143 touches)**

Run: `cargo test --bin huck completion 2>&1 | tail -8` (unit completion tests — the default path must be untouched).
Run: `cargo test --bin huck compgen 2>&1 | tail -8`.
Run: `cargo test --test pty_interactive 2>&1 | tail -10` (if present — completion PTY tests must still pass; may be slow).

- [ ] **Step 3: All bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" 2>/dev/null | tail -1; done`
Expected: every harness ends with `Fail: 0` (or its own success line); the new `cd_completion_diff_check.sh` → `Pass: 6`.

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 5: Payoff verification**

Build: `cargo build 2>&1 | tail -2`. The end-to-end `cd projects/<TAB>` descend + `cd ~/<TAB>` list are covered by the in-crate dispatch tests (Tasks 1-2) and the harness (Task 3) — the real `_cd` uses the same `dispatch::resolve` + `compgen` path. Confirm by re-stating the passing test names (`spec_completion_anchors_at_word_start`, `spec_filenames_appends_slash_to_tilde_dir`, `directory_action_tilde_expands_home`). No separate manual PTY step required; if you want extra assurance, note that sourcing bash-completion in a PTY and TAB-ing `cd ~/` is the manual smoke (do NOT source the user's `~/.bashrc` — credentials).

- [ ] **Step 6: Commit (only if a verification-driven fix was needed)**

If Steps 1-4 surfaced a real issue, make the SMALLEST fix, re-run, commit with the trailer. Otherwise no commit — verification only.

---

## Notes for the implementer
- **Default file-completion path is sacred** — the `None` branch in `dispatch::resolve` + `complete_file` must NOT change behavior. Only the `Some(spec)` (programmable) path moves to the `word_start` anchor + full-path fallback + tilde.
- **`analyze` stays a thin wrapper** over `analyze_full` so the ~12 existing `analyze` tests and the `bashdefault_strings` caller are untouched.
- **Match struct field names** in `completion_spec::{CompletionSpec, CompletionOptions}` exactly (grep before writing the test literals).
- **`expand_tilde_prefix` is `pub(crate)`** in `completion.rs` and reused from `completion_spec.rs` via `crate::completion::expand_tilde_prefix` (DRY — one definition).
- **Harness uses QUOTED `~/`** — an unquoted `~/` would be word-expanded before compgen and would not exercise the fix.
