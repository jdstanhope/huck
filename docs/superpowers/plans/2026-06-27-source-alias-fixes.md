# source path/device-file + expand_aliases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix source-path resolution (CWD/`sourcepath` fallback), accept device-files/fifos in `source`, and honor `shopt expand_aliases` in non-interactive (file/`source`/`-c`) mode.

**Architecture:** A+B rewrite `resolve_source_path` (builtins.rs). C adds an `alias_generation` counter + a provenance-mapped alias expander (`expand_aliases_in_tokens_mapped`), then wires alias expansion into the `run_sourced_contents_in_sinks` chunk loop — remapping token byte-offsets/lines back to the original source via the provenance map, and re-tokenizing the remainder when the alias table (or the `expand_aliases`/`extglob` shopt) changes.

**Tech Stack:** Rust; the existing lexer/`TokenCursor`/`parse_one_unit` pipeline; `alias_expand.rs`.

## Global Constraints

- Commit trailer verbatim on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Run the FULL suite with `cargo test --workspace` (plain `cargo test` skips most crates).
- Diff-check harnesses run a fragment as a FILE (temp script) through BOTH bash and huck on the SAME temp path, asserting byte-identical merged stdout+stderr+exit — model on `tests/scripts/io_error_diff_check.sh`.
- NO bash category is claimed to flip for `builtins` (it keeps ~10 other blockers). C TARGETS the `alias` category flip — re-measure, do not assume.
- Out of scope (do NOT attempt): CDPATH, set +p, declare/kill prologue, the other builtins clusters, a push-back lexer for mid-unit alias expansion.

---

### Task 1: A+B — source path resolution + device-file acceptance

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`resolve_source_path` ~6245)
- Create: `tests/source_device_integration.rs`
- Create: `tests/scripts/source_device_diff_check.sh`

**Interfaces:**
- Consumes: `shell.shopt_options.get("sourcepath")` (exists, default true), `shell.lookup_var("PATH")`.
- Produces: a `resolve_source_path` that does PATH (gated on `sourcepath`) + CWD fallback and accepts any existing non-directory path.

- [ ] **Step 1: Write the failing integration test**

Create `tests/source_device_integration.rs`:

```rust
//! v231 A+B: source path resolution (CWD/sourcepath fallback) + device files.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn unique(tag: &str, ext: &str) -> std::path::PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("huck_v231_{tag}_{}_{}.{ext}", std::process::id(), n))
}

/// Run `script` as a file arg (non-interactive). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let path = unique("s", "sh");
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

/// Run `script` (file arg) with `feed` piped to huck's stdin.
fn run_file_stdin(script: &str, feed: &str) -> (String, i32) {
    let path = unique("p", "sh");
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let mut child = Command::new(huck_bin()).arg(&path)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(feed.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn source_cwd_fallback_sourcepath_off() {
    // shopt -u sourcepath; a bare filename present in CWD is sourced from CWD.
    let dir = unique("d", "dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("src4.sub"), "set -- m n o p\n").unwrap();
    let script = format!("shopt -u sourcepath\ncd {}\n. src4.sub\necho \"$@\"\n", dir.display());
    let (o, _, c) = run_file(&script);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(o, "m n o p\n"); assert_eq!(c, 0);
}

#[test]
fn source_cwd_fallback_sourcepath_on() {
    // default sourcepath on: not in PATH → CWD fallback still sources it.
    let dir = unique("d2", "dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.sub"), "echo SOURCED\n").unwrap();
    let script = format!("cd {}\n. s.sub\n", dir.display());
    let (o, _, _) = run_file(&script);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(o, "SOURCED\n");
}

#[test]
fn source_dev_null() {
    let (o, _, c) = run_file(". /dev/null\necho \"rc=$?\"\n");
    assert_eq!(o, "rc=0\n"); assert_eq!(c, 0);
}

#[test]
fn source_dev_stdin_runs_piped_content() {
    let (o, c) = run_file_stdin(". /dev/stdin\necho done\n", "echo PIPED-OK\n");
    assert_eq!(o, "PIPED-OK\ndone\n"); assert_eq!(c, 0);
}

#[test]
fn source_missing_still_errors() {
    let (_, e, c) = run_file(". /no/such_xyz_v231\n");
    assert!(e.contains("No such file or directory"), "stderr: {e}");
    assert!(!e.contains("os error"), "leaks rust io text: {e}");
    assert_ne!(c, 0);
}

#[test]
fn source_directory_still_is_a_directory() {
    let (_, e, _) = run_file(". /etc\n");
    assert!(e.contains(".: /etc: is a directory") || e.contains("/etc: is a directory"), "stderr: {e}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test source_device_integration 2>&1 | tail -15`
Expected: FAIL (CWD-fallback + device tests fail; missing/directory may already pass).

- [ ] **Step 3: Rewrite `resolve_source_path`**

Replace the function (builtins.rs ~6245) with:

```rust
fn resolve_source_path(
    filename: &str,
    shell: &crate::shell_state::Shell,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    // Accept any existing path that is NOT a directory: regular file, char/block
    // device, fifo, or a symlink to one (bash sources /dev/null, /dev/stdin,
    // fifos, and procsub /dev/fd/N). A directory is rejected here and reported as
    // "is a directory" by the caller's None branch.
    let usable = |p: &Path| -> bool {
        match std::fs::metadata(p) { // follows symlinks
            Ok(m) => !m.is_dir(),
            Err(_) => false,
        }
    };
    if filename.contains('/') {
        let p = PathBuf::from(filename);
        return usable(&p).then_some(p);
    }
    // No slash: PATH search is gated on `shopt sourcepath` (default on); when off,
    // or when the file is not found in PATH, fall back to the current directory.
    let sourcepath = shell.shopt_options.get("sourcepath").unwrap_or(true);
    if sourcepath {
        let path_var = shell.lookup_var("PATH").unwrap_or_default();
        for dir in path_var.split(':') {
            if dir.is_empty() { continue; }
            let candidate = PathBuf::from(dir).join(filename);
            if usable(&candidate) { return Some(candidate); }
        }
    }
    let cwd_candidate = PathBuf::from(filename); // ./filename
    usable(&cwd_candidate).then_some(cwd_candidate)
}
```

- [ ] **Step 4: Run the integration tests**

Run: `cargo test --test source_device_integration 2>&1 | tail -10`
Expected: all 6 PASS.

- [ ] **Step 5: Create the diff-check harness**

Create `tests/scripts/source_device_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v231 A+B: source CWD/sourcepath fallback
# + device-file/fifo acceptance. File mode on the SAME temp path for both shells.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-srcdev.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
checkf_pipe() {
    local label="$1" body="$2" feed="$3" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-srcdev.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(printf '%s\n' "$feed" | bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$feed" | "$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf       "dev null"        '. /dev/null; echo "rc=$?"'
checkf_pipe  "dev stdin"       '. /dev/stdin; echo end' 'echo PIPED'
checkf       "fifo source"     'f=$(mktemp -u "${TMPDIR:-/tmp}/huck-fifo.XXXXXX"); mkfifo "$f"; { echo "echo FIFO_OK" > "$f" & }; . "$f"; echo "rc=$?"; rm -f "$f"'
checkf       "missing"         '. /no/such_xyz_v231; echo "rc=$?"'
checkf       "directory"       '. /etc; echo "rc=$?"'
checkf       "sourcepath off"  'shopt -u sourcepath; d=$(mktemp -d "${TMPDIR:-/tmp}/huck-sd.XXXXXX"); echo "set -- m n o p" > "$d/x.sub"; cd "$d"; . x.sub; echo "$@"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 6: Build huck and run the harness**

Run: `cargo build --bin huck && HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/source_device_diff_check.sh`
Expected: every line `PASS:`, `Fail: 0`. (Procsub `. <(…)` is covered by the alias/source category re-measure in Task 3, not pinned here; if you add a procsub case and it diverges, drop it and note the procsub-fd lifecycle as a follow-on rather than weakening the harness.)

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/source_device_integration.rs tests/scripts/source_device_diff_check.sh
git commit -m "$(printf 'v231 task 1: source CWD/sourcepath fallback + device-file acceptance\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: C-infra — `alias_generation` counter + provenance-mapped expander

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`alias_generation` field + `Shell::new` init)
- Modify: `crates/huck-engine/src/builtins.rs` (bump in `builtin_alias`/`builtin_unalias`)
- Modify: `crates/huck-engine/src/alias_expand.rs` (`expand_aliases_in_tokens_mapped` + refactor)

**Interfaces:**
- Produces: `shell.alias_generation: u64` (bumped on every successful `shell.aliases` mutation); `pub fn expand_aliases_in_tokens_mapped(tokens, aliases) -> Result<(Vec<Token>, Vec<usize>), LexError>` where the second vec maps each output token → its source token index. Task 3 consumes both.
- This task is additive: nothing reads `alias_generation` and nothing calls the `_mapped` fn in production yet, so behavior is unchanged (workspace stays green).

- [ ] **Step 1: Write the failing unit tests**

Add to `alias_expand.rs` `#[cfg(test)]` mod:

```rust
#[test]
fn mapped_expansion_tracks_source_indices() {
    // alias ll='ls -l'; tokens: [ll, /usr] → [ls, -l, /usr] with map [0,0,1].
    let aliases = make_aliases(&[("ll", "ls -l")]);
    let toks = crate::lexer::tokenize("ll /usr").unwrap();
    let (out, map) = super::expand_aliases_in_tokens_mapped(toks, &aliases).unwrap();
    let words: Vec<String> = out.iter().filter_map(|t| match t {
        crate::lexer::Token::Word(w) => super::simple_word_text(w), _ => None }).collect();
    assert_eq!(words, vec!["ls", "-l", "/usr"]);
    assert_eq!(map, vec![0, 0, 1]);
}

#[test]
fn mapped_noop_is_identity() {
    let aliases = make_aliases(&[("ll", "ls -l")]);
    let toks = crate::lexer::tokenize("echo hi").unwrap(); // no alias at cmd pos
    let n = toks.len();
    let (out, map) = super::expand_aliases_in_tokens_mapped(toks, &aliases).unwrap();
    assert_eq!(out.len(), n);
    assert_eq!(map, (0..n).collect::<Vec<_>>());
}
```

Add to `shell_state.rs` tests (or a builtins test) — the generation bump:

```rust
#[test]
fn alias_generation_bumps_on_define_and_unalias() {
    let mut sh = crate::shell_state::Shell::new();
    let g0 = sh.alias_generation;
    let mut out: Vec<u8> = Vec::new(); let mut err: Vec<u8> = Vec::new();
    super::builtin_alias(&["foo=bar".into()], &mut out, &mut err, &mut sh);
    assert!(sh.alias_generation > g0, "define must bump");
    let g1 = sh.alias_generation;
    super::builtin_unalias(&["foo".into()], &mut err, &mut sh);
    assert!(sh.alias_generation > g1, "unalias must bump");
}
```
(Place the bump test in `builtins.rs` tests where `builtin_alias`/`builtin_unalias` are in scope via `super::`.)

Run: `cargo test -p huck-engine mapped_expansion 2>&1 | tail` and `cargo test -p huck-engine alias_generation 2>&1 | tail` → FAIL (items don't exist).

- [ ] **Step 2: Add the `alias_generation` field + init**

In `shell_state.rs`, add a field near `aliases`:

```rust
    /// Bumped on every successful mutation of `aliases` (the `alias`/`unalias`
    /// builtins). The non-interactive source loop re-tokenizes the remainder when
    /// this changes, so a newly-defined alias affects subsequently-parsed commands.
    pub alias_generation: u64,
```

In the `Shell::new` struct literal (the single constructor), add next to `aliases: …,`:

```rust
            alias_generation: 0,
```

- [ ] **Step 3: Bump the counter in alias/unalias**

In `builtin_alias`, after a successful `shell.aliases.insert(…)` (the define branch), add `shell.alias_generation += 1;`. In `builtin_unalias`, after `shell.aliases.clear()` (the `-a` branch) and after a successful `shell.aliases.remove(name)` (when it returned `Some`), add `shell.alias_generation += 1;`. (Bump only on real mutations, not on the list/`not found` paths.)

- [ ] **Step 4: Add `expand_aliases_in_tokens_mapped` + refactor**

In `alias_expand.rs`, add the mapped variant and a `process_token_mapped` mirroring `process_token` but pushing the source index alongside each output token; refactor `expand_aliases_in_tokens` to delegate:

```rust
/// Like `expand_aliases_in_tokens` but also returns, per output token, the index
/// of the SOURCE token it originated from. Alias-body tokens inherit the index of
/// the alias-name token they replaced; untouched tokens map to themselves. Used by
/// the non-interactive source loop to remap byte-offsets/lines back to the raw
/// source after expansion rewrites the token stream.
pub fn expand_aliases_in_tokens_mapped(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<(Vec<Token>, Vec<usize>), LexError> {
    let mut out: Vec<Token> = Vec::new();
    let mut map: Vec<usize> = Vec::new();
    let mut next_eligible = true;
    let mut active: HashSet<String> = HashSet::new();
    for (src_idx, token) in tokens.into_iter().enumerate() {
        next_eligible = process_token_mapped(
            token, src_idx, &mut out, &mut map, next_eligible, aliases, &mut active)?;
    }
    Ok((out, map))
}

fn process_token_mapped(
    token: Token,
    src_idx: usize,
    out: &mut Vec<Token>,
    map: &mut Vec<usize>,
    eligible: bool,
    aliases: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> Result<bool, LexError> {
    match &token {
        Token::Word(w) => {
            if eligible
                && let Some(name) = simple_word_text(w)
                && !active.contains(&name)
                && let Some(body) = aliases.get(&name).cloned()
            {
                active.insert(name.clone());
                let inner_tokens = crate::lexer::tokenize(&body)?;
                let mut inner_eligible = true;
                for inner in inner_tokens {
                    // Body tokens inherit the alias-name token's source index.
                    inner_eligible = process_token_mapped(
                        inner, src_idx, out, map, inner_eligible, aliases, active)?;
                }
                active.remove(&name);
                let trailing = body.chars().last().is_some_and(|c| c.is_whitespace());
                return Ok(trailing);
            }
            out.push(token); map.push(src_idx);
            Ok(false)
        }
        Token::Op(op) => {
            let separator = matches!(op,
                Operator::Pipe | Operator::And | Operator::Or
                | Operator::Semi | Operator::Background | Operator::LParen);
            out.push(token); map.push(src_idx);
            Ok(separator)
        }
        Token::Newline => { out.push(token); map.push(src_idx); Ok(true) }
        _ => { out.push(token); map.push(src_idx); Ok(eligible) }
    }
}
```

Refactor the existing `expand_aliases_in_tokens` to delegate (drops the map):

```rust
pub fn expand_aliases_in_tokens(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<Vec<Token>, LexError> {
    expand_aliases_in_tokens_mapped(tokens, aliases).map(|(t, _)| t)
}
```

(If `simple_word_text` is module-private, the unit test referencing it via `super::simple_word_text` needs it `pub(crate)` — widen to `pub(crate)` only if the test cannot see it.)

- [ ] **Step 5: Run tests + full workspace (no behavior change)**

Run: `cargo test -p huck-engine mapped 2>&1 | tail` , `cargo test -p huck-engine alias_generation 2>&1 | tail` → PASS.
Run: `cargo test --workspace 2>&1 | tail -3` → `0 failed` (additive; `expand_aliases_in_tokens` still behaves identically via delegation).

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins.rs crates/huck-engine/src/alias_expand.rs
git commit -m "$(printf 'v231 task 2: alias_generation counter + provenance-mapped alias expander\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: C-wire — alias expansion in the source loop + re-measure

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`run_sourced_contents_in_sinks` ~6285)
- Create: `tests/alias_expand_integration.rs`
- Create: `tests/scripts/alias_expand_diff_check.sh`

**Interfaces:**
- Consumes: `expand_aliases_in_tokens_mapped` + `shell.alias_generation` (Task 2); the chunk loop's `offsets`/`token_lines`/`total`/`TokenCursor`.

- [ ] **Step 1: Write the failing integration test**

Create `tests/alias_expand_integration.rs` (copy the `run_file` helper from Task 1's file, temp prefix `huck_v231al_`):

```rust
//! v231 C: shopt expand_aliases honored in non-interactive (file) mode.
// (copy run_file/COUNTER/huck_bin helper, temp prefix "huck_v231al_", returning (stdout, stderr, code))

#[test]
fn expand_aliases_def_then_use_across_lines() {
    let (o, _, c) = run_file("shopt -s expand_aliases\nalias foo='echo HELLO'\nfoo\n");
    assert_eq!(o, "HELLO\n"); assert_eq!(c, 0);
}

#[test]
fn alias_with_arg_keeps_following_words() {
    let (o, _, _) = run_file("shopt -s expand_aliases\nalias ll='echo LL'\nll /usr\n");
    assert_eq!(o, "LL /usr\n");
}

#[test]
fn not_expanded_without_shopt() {
    // default (no expand_aliases) in non-interactive mode: alias NOT expanded.
    let (_, e, _) = run_file("alias foo='echo HELLO'\nfoo\n");
    assert!(e.contains("foo: command not found"), "stderr: {e}");
}

#[test]
fn unalias_then_use_is_command_not_found() {
    let (o, e, _) = run_file("shopt -s expand_aliases\nalias foo='echo HI'\nfoo\nunalias foo\nfoo\n");
    assert_eq!(o, "HI\n");
    assert!(e.contains("foo: command not found"), "stderr: {e}");
}

#[test]
fn trailing_space_continues_expansion() {
    // alias ending in space → the next word is also alias-expanded.
    let (o, _, _) = run_file("shopt -s expand_aliases\nalias a='b '\nalias b='echo'\na hi\n");
    assert_eq!(o, "hi\n");
}

#[test]
fn redefine_alias_affects_later_use() {
    let (o, _, _) = run_file("shopt -s expand_aliases\nalias g='echo one'\ng\nalias g='echo two'\ng\n");
    assert_eq!(o, "one\ntwo\n");
}
```

Run: `cargo test --test alias_expand_integration 2>&1 | tail -15` → FAIL (aliases not expanded in file mode).

- [ ] **Step 2: Inject alias expansion after `tokenize_partial`**

In `run_sourced_contents_in_sinks`, locate (builtins.rs ~6353):

```rust
        let base_line = contents.as_bytes()[..start].iter().filter(|&&b| b == b'\n').count() as u32;
        let token_lines: Vec<u32> = lex_lines[..total].iter().map(|&l| l + base_line).collect();
        let mut iter = crate::command::TokenCursor::new(tokens, token_lines);
```

Replace with (note `tokens`/`offsets`/`token_lines`/`total` are re-bound on the expand path; `offsets` from `tokenize_partial` has a sentinel at `[total]` — the existing loop already reads `offsets[total]`, and the remap appends it as `offsets2[E]`):

```rust
        let base_line = contents.as_bytes()[..start].iter().filter(|&&b| b == b'\n').count() as u32;
        let token_lines: Vec<u32> = lex_lines[..total].iter().map(|&l| l + base_line).collect();
        // v231: honor `shopt expand_aliases` (and interactive) in the file/source/-c
        // path. Expand aliases on the chunk tokens, remapping offsets/lines back to
        // the ORIGINAL source tokens via a provenance map so byte-offset bookkeeping
        // (`unit_end_abs`, `set -v`) stays anchored to the raw source bytes.
        let expand = shell.is_interactive
            || shell.shopt_options.get("expand_aliases").unwrap_or(false);
        let alias_gen = shell.alias_generation;
        let (tokens, token_lines, offsets, total) = if expand && !shell.aliases.is_empty() {
            match crate::alias_expand::expand_aliases_in_tokens_mapped(tokens, &shell.aliases) {
                Ok((exp, map)) => {
                    let e = exp.len();
                    let offsets2: Vec<usize> = (0..e)
                        .map(|j| offsets[map[j]])
                        .chain(std::iter::once(offsets[total]))
                        .collect();
                    let lines2: Vec<u32> = (0..e).map(|j| token_lines[map[j]]).collect();
                    (exp, lines2, offsets2, e)
                }
                Err(le) => {
                    {
                        let mut err = crate::executor::err_writer(err_sink, sink);
                        e!(&mut *err, "huck: {}: line {}: syntax error{}",
                            path.display(), line_of(start), crate::lex_error_message(&le));
                    }
                    last_status = 2;
                    break 'outer;
                }
            }
        } else {
            (tokens, token_lines, offsets, total)
        };
        let mut iter = crate::command::TokenCursor::new(tokens, token_lines);
```

- [ ] **Step 3: Extend the post-unit re-lex check**

Locate the existing extglob re-lex block (builtins.rs ~6434), which reads:

```rust
                    let new_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                    if new_extglob != extglob {
                        start = match &terr {
                            Some((_, foff)) if unit_end_idx == total => {
                                line_start_of(start + *foff)
                            }
                            _ => unit_end_abs,
                        };
                        prev_end = start;
                        continue 'outer;
                    }
```

Replace the condition so the remainder is also re-tokenized when alias expansion turns on/off mid-chunk OR the alias table changed (re-lexing from raw bytes means each source token is expanded at most once — no double-expansion):

```rust
                    let new_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                    let new_expand = shell.is_interactive
                        || shell.shopt_options.get("expand_aliases").unwrap_or(false);
                    if new_extglob != extglob
                        || new_expand != expand
                        || (new_expand && shell.alias_generation != alias_gen)
                    {
                        start = match &terr {
                            Some((_, foff)) if unit_end_idx == total => {
                                line_start_of(start + *foff)
                            }
                            _ => unit_end_abs,
                        };
                        prev_end = start;
                        continue 'outer;
                    }
```

- [ ] **Step 4: Build + run the integration tests**

Run: `cargo build --bin huck && cargo test --test alias_expand_integration 2>&1 | tail -12`
Expected: all 6 PASS. (If `trailing_space_continues_expansion` fails, the body-token eligibility/continuation is already in `process_token_mapped` — re-check the remap didn't drop a token.)

- [ ] **Step 5: Create the diff-check harness**

Create `tests/scripts/alias_expand_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v231 C: shopt expand_aliases in file mode.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aliasx.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "def then use"     'shopt -s expand_aliases; alias foo="echo HELLO"; foo'
checkf "alias with arg"   'shopt -s expand_aliases; alias ll="echo LL"; ll /usr'
checkf "no shopt = literal" 'alias foo="echo HELLO"; foo'
checkf "unalias then use" 'shopt -s expand_aliases; alias foo="echo HI"; foo; unalias foo; foo'
checkf "trailing space"   'shopt -s expand_aliases; alias a="b "; alias b="echo"; a hi'
checkf "redefine"         'shopt -s expand_aliases; alias g="echo one"; g; alias g="echo two"; g'
checkf "set -v echo raw"  'set -v; shopt -s expand_aliases; alias ll="echo LL"; ll /usr'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 6: Run the harness**

Run: `HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/alias_expand_diff_check.sh`
Expected: `Fail: 0`. (The `set -v echo raw` case pins that `set -v` echoes the RAW `ll /usr`, not the expansion — the provenance map's purpose. If it diverges, the offset remap is wrong.)

- [ ] **Step 7: Full regression sweep**

```bash
cargo test --workspace 2>&1 | tail -3
cargo build --release --bin huck
for f in tests/scripts/*_diff_check.sh; do
  if [ "$(basename "$f")" = "funcnest_diff_check.sh" ]; then
    HUCK_BIN="$(pwd)/target/release/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  else
    HUCK_BIN="$(pwd)/target/debug/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  fi
done | grep -v '^ok ' || echo "all harnesses pass"
```
Expected: `cargo test --workspace` → `0 failed`; `all harnesses pass`. (This guards the hot script-execution path the C seam changed — a regression here is the main risk.)

- [ ] **Step 8: Category re-measure (the alias FLIP target)**

```bash
for c in alias builtins procsub; do
  echo "== $c =="
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "\| $c \||Scratch dir"
done
```
Report each category's status. If `alias` flips to PASS, record it (PASS count 10→11). If it does not, read the printed `<scratch>/alias.diff` and record the EXACT residual (e.g. the `alias2.sub` `$TMPDIR`/path lines, unicode-alias edges) so the follow-on is documented — do NOT chase those here. For `builtins`/`procsub`, confirm the source4/6/7 lines shrank; no `builtins` flip is expected. (The runner auto-builds release and is slow ~minutes — be patient; if the runner env is unavailable, record that and proceed.)

- [ ] **Step 9: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/alias_expand_integration.rs tests/scripts/alias_expand_diff_check.sh
git commit -m "$(printf 'v231 task 3: expand aliases in the file/source loop (provenance-mapped)\n\nWires the existing alias expander into run_sourced_contents_in_sinks gated on\n(is_interactive || shopt expand_aliases); remaps offsets/lines to the original\nsource via the provenance map so set -v and byte-advance stay raw-anchored; the\nalias-generation counter re-tokenizes the remainder on define/unalias so a new\nalias affects later commands. Targets the alias-category flip.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Self-Review

**Spec coverage:** A (sourcepath + CWD fallback) + B (device-file acceptance) in Task 1 ✓; C-infra (alias_generation + mapped expander) in Task 2 ✓; C-wire (seam injection + provenance remap + re-lex-on-change) in Task 3 ✓. The provenance map keeping offsets raw-anchored, the `set -v` raw-echo pin, the def-then-use timing via the generation counter, and the negative (no-shopt) case are all tested. The `alias` re-measure is the deliverable to watch (flip not assumed).

**Placeholders:** none — every code step shows complete code. The one acknowledged "verify" item (procsub byte-identity in Task 1 Step 6) has a concrete fallback (cover via integration, note as follow-on), not a vague gap.

**Type consistency:** `expand_aliases_in_tokens_mapped(tokens, aliases) -> Result<(Vec<Token>, Vec<usize>), LexError>` and `alias_generation: u64` are used identically across Tasks 2/3; the remap (`offsets2`/`lines2`/`e`) matches the loop's `offsets`/`token_lines`/`total` shapes; the re-lex condition reuses the existing `start = match &terr {…}` computation verbatim.

**Note for implementer/reviewer:** the offsets sentinel — `tokenize_partial`'s `offsets` is indexed at `[total]` by the existing loop (so it has `total+1` entries); the remap relies on this to set `offsets2[E]`. If a build error shows `offsets[total]` out of bounds, that assumption is wrong — stop and re-derive rather than trimming the sentinel.
