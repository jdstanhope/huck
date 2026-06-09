# v123 — `noclobber` + `>|` Redirect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `noclobber` shell option (`set -C` / `set -o noclobber`) so `>` refuses to truncate an existing regular file, plus the `>|`/`1>|`/`2>|` force-clobber operators that override it — byte-identical to bash 5.x. (M-21)

**Architecture:** Three units. (1) Lexer/parser learn `>|` → new `Redirect::Clobber(Word)`. (2) `noclobber` joins `ShellOptions` mirroring v120's `noglob`. (3) A single `open_writable(path, guard)` helper funnels every truncating open; `guard` (set when `noclobber` is on and the redirect is not a force-clobber) uses `O_EXCL` with bash's special-file exemption. When `noclobber` is off — the default and only prior state — `guard` is always false and behavior is byte-identical to today.

**Tech Stack:** Rust; `std::fs::OpenOptions` (`create_new` = `O_CREAT|O_EXCL`), `std::fs::metadata`.

Spec: `docs/superpowers/specs/2026-06-09-noclobber-design.md`.

**Conventions:**
- Build with `cargo build` (debug); harness uses `target/debug/huck`.
- Commit trailer (canonical, do not alter): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.
- Bash-diff harness fragments run as FILE-ARG scripts (L-27).
- Branch: `v123-noclobber` (create from `main` before Task 1 if not already on it).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/lexer.rs` | Lex `>|`/`1>|`/`2>|`; `Operator::RedirClobber`/`RedirErrClobber` | 1 |
| `src/command.rs` | `Redirect::Clobber(Word)`; parser arms; `is_redirect_op` | 1 |
| `src/shell_state.rs` | `ShellOptions.noclobber`; `dollar_dash_value` `C` | 2 |
| `src/builtins.rs` | `option_get`/`option_set` noclobber; `set -C`/`+C` | 2 |
| `src/executor.rs` | `open_writable` helper; `ResolvedRedirect::NoclobberTruncate`; route open sites + classification | 3 |
| `tests/noclobber_integration.rs` | NEW — probed cases vs bash | 4 |
| `tests/scripts/noclobber_diff_check.sh` | NEW — 46th bash-diff harness | 4 |
| `docs/bash-divergences.md`, `README.md` | drop M-21; move `>|`/`set -C`/noclobber to supported | 5 |

---

### Task 1: `>|` surface syntax (lexer + AST + parser)

**Files:**
- Modify: `src/lexer.rs` (Operator enum ~`:81`; `>` arm ~`:677`; `1>` arm ~`:691`; `2>` arm ~`:705`)
- Modify: `src/command.rs` (`Redirect` enum ~`:252`; `is_redirect_op` ~`:1586`; parser match ~`:1634`)

- [ ] **Step 1: Write failing lexer tests**

Add to the `#[cfg(test)] mod tests` block in `src/lexer.rs` (near the existing redirect-token tests around `:5840`):

```rust
#[test]
fn lex_clobber_stdout() {
    assert_eq!(tokenize(">|").unwrap(), vec![Token::Op(Operator::RedirClobber)]);
    assert_eq!(tokenize("1>|").unwrap(), vec![Token::Op(Operator::RedirClobber)]);
}

#[test]
fn lex_clobber_stderr() {
    assert_eq!(tokenize("2>|").unwrap(), vec![Token::Op(Operator::RedirErrClobber)]);
}

#[test]
fn lex_clobber_with_target() {
    // `>|f` → clobber op then word "f"
    assert_eq!(
        tokenize("cmd >|f").unwrap(),
        vec![w("cmd"), Token::Op(Operator::RedirClobber), w("f")]
    );
}

#[test]
fn lex_redir_then_pipe_unaffected() {
    // `> |` with a space is still redirect-out followed by a pipe.
    assert_eq!(
        tokenize("> |").unwrap(),
        vec![Token::Op(Operator::RedirOut), Token::Op(Operator::Pipe)]
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib lex_clobber 2>&1 | tail -20`
Expected: FAIL — `Operator::RedirClobber` / `RedirErrClobber` do not exist (compile error).

- [ ] **Step 3: Add the operators**

In `src/lexer.rs`, add to `pub enum Operator` (after `AndRedirAppend`):

```rust
    RedirClobber,    // >|
    RedirErrClobber, // 2>|
```

- [ ] **Step 4: Lex `>|` in the three arms**

In `src/lexer.rs`, the `'>'` arm currently reads:

```rust
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::DupOut));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
```

Add a `'|'` branch before the `else` (do the same for the `'1' if … '>'` arm, which is byte-identical):

```rust
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::DupOut));
                } else if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirClobber));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
```

In the `'2' if … '>'` arm, the existing code is:

```rust
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrAppend));
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::DupErr));
                } else {
                    tokens.push(Token::Op(Operator::RedirErr));
                }
```

Add the `'|'` branch before its `else`:

```rust
                } else if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrClobber));
                } else {
```

- [ ] **Step 5: Run lexer tests — expect PASS for lexer, FAIL elsewhere**

Run: `cargo test --lib lex_clobber 2>&1 | tail -20`
Expected: the 4 lexer tests PASS. (Other crates may not compile yet if `command.rs` matches on `Operator` exhaustively — that's fixed next.)

- [ ] **Step 6: Write failing parser tests**

Add to the `#[cfg(test)] mod tests` in `src/command.rs` (near the `exec_stdout`/`exec_stderr` redirect tests around `:2427`):

```rust
#[test]
fn parse_clobber_stdout() {
    let seq = parse_one("cmd >| f").unwrap();
    assert_eq!(exec_stdout(&seq), &Some(Redirect::Clobber(ww("f"))));
}

#[test]
fn parse_clobber_stderr() {
    let seq = parse_one("cmd 2>| e").unwrap();
    assert_eq!(exec_stderr(&seq), &Some(Redirect::Clobber(ww("e"))));
}
```

(Use whatever the existing redirect tests use to obtain a parsed `Sequence` and pull `stdout`/`stderr` — `parse_one`, `exec_stdout`, `exec_stderr`, `ww` are the helpers used by the adjacent `Redirect::Truncate` tests; mirror them exactly.)

- [ ] **Step 7: Run to verify they fail**

Run: `cargo test --lib parse_clobber 2>&1 | tail -20`
Expected: FAIL — `Redirect::Clobber` does not exist.

- [ ] **Step 8: Add the AST variant**

In `src/command.rs`, add to `pub enum Redirect` (after `Append(Word)`):

```rust
    /// `>|file` — force truncate, overriding `noclobber` (`set -C`).
    Clobber(Word),
```

- [ ] **Step 9: Wire the parser + `is_redirect_op`**

In `src/command.rs`, add the two operators to `is_redirect_op` (inside the `matches!(...)` list):

```rust
            | Operator::RedirClobber
            | Operator::RedirErrClobber
```

In the redirect match (the `match op { … }` around `:1634`), add after the `RedirErrAppend` arm:

```rust
                    Operator::RedirClobber    => stdout = Some(Redirect::Clobber(target)),
                    Operator::RedirErrClobber => stderr = Some(Redirect::Clobber(target)),
```

- [ ] **Step 10: Fix any now-non-exhaustive matches on `Redirect` to compile**

`cargo build 2>&1 | grep -E "error|Clobber" | head -40`. Anywhere the compiler reports a non-exhaustive `match` on `Redirect` that does NOT involve file-opening (e.g. a `Debug`/clone-ish or fd-classification arm), add `Redirect::Clobber(_)` alongside the existing `Redirect::Truncate(_)` so it is treated identically for now. The execution/open routing is Task 3 — for Task 1, the goal is only that `Clobber` parses and the crate compiles. If a match arm clearly opens a file for `Truncate`, leave a `Redirect::Clobber(w) => Redirect::Truncate-equivalent` behavior (treat as a plain truncate) so behavior is correct pre-Task-3; Task 3 refines it. Do NOT add `todo!()`/`unimplemented!()`.

- [ ] **Step 11: Run the build + the new tests**

Run: `cargo build 2>&1 | tail -5 && cargo test --lib 'lex_clobber' 2>&1 | tail -5 && cargo test --test '*' parse_clobber 2>&1 | tail -5`
Run: `cargo test --lib parse_clobber 2>&1 | tail -8`
Expected: compiles; all 6 new tests PASS.

- [ ] **Step 12: Smoke — `>|` no longer a parse error**

Run: `printf 'echo hi >| /tmp/huck_clobber_smoke\ncat /tmp/huck_clobber_smoke\n' | ./target/debug/huck /dev/stdin`
Expected: prints `hi` (no "expected a filename after redirection").

- [ ] **Step 13: Commit**

```bash
git add src/lexer.rs src/command.rs
git commit -m "feat(v123): lex+parse >| /1>| /2>| into Redirect::Clobber

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `noclobber` shell option (`set -C` / `-o noclobber` / `$-`)

**Files:**
- Modify: `src/shell_state.rs` (`ShellOptions` ~`:107`; `dollar_dash_value` ~`:427`)
- Modify: `src/builtins.rs` (`option_get` ~`:4307`; `option_set` ~`:4321`; `set` minus loop ~`:4424`; `set` plus loop ~`:4468`)

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` in `src/shell_state.rs` (near the `dollar_dash_value` tests around `:1933`):

```rust
#[test]
fn noclobber_off_by_default() {
    let sh = Shell::new_for_test();
    assert!(!sh.shell_options.noclobber);
    assert!(!sh.dollar_dash_value().contains('C'));
}

#[test]
fn noclobber_shows_in_dollar_dash() {
    let mut sh = Shell::new_for_test();
    sh.shell_options.noclobber = true;
    assert!(sh.dollar_dash_value().contains('C'));
}
```

(Use the same `Shell` test constructor the adjacent `dollar_dash_value` tests use — they reference `sh`; mirror their setup exactly, e.g. `Shell::new()` or a test helper.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib noclobber_ 2>&1 | tail -15`
Expected: FAIL — no field `noclobber` on `ShellOptions`.

- [ ] **Step 3: Add the field**

In `src/shell_state.rs`, add to `pub struct ShellOptions` (after `pub noglob: bool,`):

```rust
    pub noclobber: bool,
```

(`ShellOptions` derives `Default` and has no literal constructors, so no other init sites need editing.)

- [ ] **Step 4: Emit `C` in `$-`**

In `dollar_dash_value` (`src/shell_state.rs:427`), add **after** the `xtrace`/`x` push (trailing, uppercase position):

```rust
        if self.shell_options.noclobber { out.push('C'); }
```

- [ ] **Step 5: Run the shell_state tests**

Run: `cargo test --lib noclobber_ 2>&1 | tail -8`
Expected: both PASS.

- [ ] **Step 6: Write failing builtin tests**

Add to the `#[cfg(test)] mod tests` in `src/builtins.rs` (near the existing `set -o`/option tests; search for an existing `run(&["-o", ...]` test such as the one at `:10649`):

```rust
#[test]
fn set_dash_c_enables_noclobber() {
    let mut shell = Shell::new_for_test();
    let _ = run(&["-C"], &mut shell);
    assert!(shell.shell_options.noclobber);
    assert_eq!(option_get(&shell, "noclobber"), Some(true));
}

#[test]
fn set_plus_c_disables_noclobber() {
    let mut shell = Shell::new_for_test();
    let _ = run(&["-C"], &mut shell);
    let _ = run(&["+C"], &mut shell);
    assert!(!shell.shell_options.noclobber);
}

#[test]
fn set_o_noclobber_enables() {
    let mut shell = Shell::new_for_test();
    let _ = run(&["-o", "noclobber"], &mut shell);
    assert_eq!(option_get(&shell, "noclobber"), Some(true));
}
```

(Match the exact `run(...)` helper + `Shell` constructor the neighboring `set` tests use — e.g. the test at `:10649` calls `run(&["-o", "noclobber"], &mut shell)`; reuse that pattern and its imports.)

- [ ] **Step 7: Run to verify they fail**

Run: `cargo test --lib set_dash_c set_plus_c set_o_noclobber 2>&1 | tail -15`
Expected: FAIL — `set -C` hits the `other =>` "not yet supported" arm / `option_get` returns the table default and `option_set` returns `Unimplemented`.

- [ ] **Step 8: Implement `option_get` / `option_set`**

In `src/builtins.rs`, add to `option_get` (`:4307`, before the `other =>` arm):

```rust
        "noclobber" => Some(shell.shell_options.noclobber),
```

And to `option_set` (`:4321`, before the `other =>` arm):

```rust
        "noclobber" => { shell.shell_options.noclobber = value; Ok(()) }
```

- [ ] **Step 9: Implement `set -C` / `+C` short flags**

In the `set` minus-flag loop (`src/builtins.rs:~4424`, the `match c { b'e' => …, b'f' => …, … }`), add:

```rust
                    b'C' => shell.shell_options.noclobber = true,
```

In the plus-flag loop (`:~4468`), add:

```rust
                    b'C' => shell.shell_options.noclobber = false,
```

- [ ] **Step 10: Run builtin tests + smoke**

Run: `cargo test --lib set_dash_c set_plus_c set_o_noclobber 2>&1 | tail -8`
Expected: all PASS.

Run: `printf 'set -C\necho "$-"\nset -o | grep noclobber\n' | ./target/debug/huck /dev/stdin`
Expected: a line containing `C`, then `noclobber       \ton`.

- [ ] **Step 11: Commit**

```bash
git add src/shell_state.rs src/builtins.rs
git commit -m "feat(v123): wire noclobber option (set -C/+C, -o noclobber, \$- C)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: guarded-open helper + route every truncating open

**Files:**
- Modify: `src/executor.rs` — add `open_writable` near `open_resolved` (`:2459`); `ResolvedRedirect` enum (`:2026`); resolve sites (`:556`, `:2154`, `:2179`); `open_resolved` (`:2461`); `resolved_path` (`:2473`); inline open sites (`:1655`, `:1718`, `:3388`, `:3447`); classification sites (`:493`, `:2145`)

- [ ] **Step 1: Write failing unit tests for `open_writable`**

Add to the `#[cfg(test)] mod tests` in `src/executor.rs`:

```rust
#[test]
fn open_writable_guard_creates_new_file() {
    let dir = std::env::temp_dir().join(format!("huck_nc_new_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("new.txt");
    let _ = std::fs::remove_file(&p);
    let f = open_writable(p.to_str().unwrap(), true);
    assert!(f.is_ok(), "guarded open should create a nonexistent file");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn open_writable_guard_blocks_existing_regular_file() {
    let dir = std::env::temp_dir().join(format!("huck_nc_block_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("exists.txt");
    std::fs::write(&p, b"orig").unwrap();
    let f = open_writable(p.to_str().unwrap(), true);
    assert!(f.is_err(), "guarded open must refuse an existing regular file");
    assert_eq!(f.err().unwrap().to_string(), "cannot overwrite existing file");
    // file untouched
    assert_eq!(std::fs::read(&p).unwrap(), b"orig");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn open_writable_guard_exempts_dev_null() {
    let f = open_writable("/dev/null", true);
    assert!(f.is_ok(), "guarded open must allow non-regular files like /dev/null");
}

#[test]
fn open_writable_unguarded_truncates_existing() {
    let dir = std::env::temp_dir().join(format!("huck_nc_trunc_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("trunc.txt");
    std::fs::write(&p, b"original-content").unwrap();
    {
        let _f = open_writable(p.to_str().unwrap(), false).unwrap();
    }
    assert_eq!(std::fs::read(&p).unwrap(), b"", "unguarded open should truncate");
    let _ = std::fs::remove_file(&p);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib open_writable_ 2>&1 | tail -15`
Expected: FAIL — `open_writable` not defined.

- [ ] **Step 3: Add the helper**

In `src/executor.rs`, immediately after `open_resolved` (`:2459`):

```rust
/// Opens `path` for writing, truncating. When `guard_noclobber` is true
/// (the `noclobber` option is on and this is a plain `>`/`&>`, not `>|`),
/// refuse to overwrite an existing **regular** file — but exempt
/// non-regular files (e.g. /dev/null, FIFOs), matching bash's `set -C`.
fn open_writable(path: &str, guard_noclobber: bool) -> io::Result<File> {
    if !guard_noclobber {
        return OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path);
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            match std::fs::metadata(path) {
                // existing non-regular file (device, fifo, …): open for write,
                // no O_EXCL / no truncate.
                Ok(md) if !md.is_file() => OpenOptions::new().write(true).open(path),
                // existing regular file (or stat failed): bash refuses.
                _ => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "cannot overwrite existing file",
                )),
            }
        }
        Err(e) => Err(e),
    }
}
```

- [ ] **Step 4: Run the helper tests**

Run: `cargo test --lib open_writable_ 2>&1 | tail -8`
Expected: all 4 PASS.

- [ ] **Step 5: Add the `ResolvedRedirect::NoclobberTruncate` variant**

In `src/executor.rs:2026`:

```rust
enum ResolvedRedirect {
    Truncate(String),
    NoclobberTruncate(String),
    Append(String),
}
```

- [ ] **Step 6: Route `open_resolved` + `resolved_path`**

Replace `open_resolved` (`:2459`) body's `Truncate` arm and add the new arm; rewrite to call the helper:

```rust
fn open_resolved(redirect: &ResolvedRedirect) -> io::Result<File> {
    match redirect {
        ResolvedRedirect::Truncate(path) => open_writable(path, false),
        ResolvedRedirect::NoclobberTruncate(path) => open_writable(path, true),
        ResolvedRedirect::Append(path) => OpenOptions::new()
            .create(true)
            .append(true)
            .open(path),
    }
}
```

In `resolved_path` (`:2473`):

```rust
fn resolved_path(redirect: &ResolvedRedirect) -> &str {
    match redirect {
        ResolvedRedirect::Truncate(p)
        | ResolvedRedirect::NoclobberTruncate(p)
        | ResolvedRedirect::Append(p) => p,
    }
}
```

- [ ] **Step 7: Build the right `ResolvedRedirect` at the resolve sites**

At each `Redirect::Truncate(w)` → `ResolvedRedirect::Truncate(path)` construction, choose the guarded variant when `noclobber` is on, and map `Redirect::Clobber(w)` to the unguarded `Truncate`. There are three resolve sites:

**(a) `src/executor.rs:556`** (compound-redirect scope) currently:

```rust
        Redirect::Truncate(word) | Redirect::Append(word) => {
            let path = match expand_single(word, shell) { … };
            let resolved = if matches!(r, Redirect::Append(_)) {
                ResolvedRedirect::Append(path)
            } else {
                ResolvedRedirect::Truncate(path)
            };
```

Change the pattern to also accept `Clobber`, and pick the variant:

```rust
        Redirect::Truncate(word) | Redirect::Clobber(word) | Redirect::Append(word) => {
            let path = match expand_single(word, shell) {
                Ok(p) => p,
                Err(()) => return Err(ExecOutcome::Continue(1)),
            };
            let resolved = if matches!(r, Redirect::Append(_)) {
                ResolvedRedirect::Append(path)
            } else if matches!(r, Redirect::Clobber(_)) {
                ResolvedRedirect::Truncate(path)            // force: never guarded
            } else if shell.shell_options.noclobber {
                ResolvedRedirect::NoclobberTruncate(path)   // plain `>` under -C
            } else {
                ResolvedRedirect::Truncate(path)
            };
```

**(b) stdout at `src/executor.rs:2154`** currently:

```rust
        Some(Redirect::Truncate(w)) => {
            let path = expand_single(w, shell).map_err(|()| 1)?;
            if let Some(status) = shell.pending_fatal_pe_error { return Err(status); }
            Some(ResolvedRedirect::Truncate(path))
        }
```

Replace with a combined arm that branches on the variant:

```rust
        Some(r @ (Redirect::Truncate(w) | Redirect::Clobber(w))) => {
            let path = expand_single(w, shell).map_err(|()| 1)?;
            if let Some(status) = shell.pending_fatal_pe_error { return Err(status); }
            let resolved = if matches!(r, Redirect::Clobber(_)) {
                ResolvedRedirect::Truncate(path)
            } else if shell.shell_options.noclobber {
                ResolvedRedirect::NoclobberTruncate(path)
            } else {
                ResolvedRedirect::Truncate(path)
            };
            Some(resolved)
        }
```

**(c) stderr at `src/executor.rs:2179`** — apply the identical transformation as (b) to the `cmd.stderr` `Some(Redirect::Truncate(w))` arm.

- [ ] **Step 8: Update the stdin-unreachable + classification arms**

`src/executor.rs:2145` (stdin) currently:

```rust
        Some(Redirect::Truncate(_) | Redirect::Append(_)) => {
            unreachable!("parser never produces Truncate/Append for stdin")
        }
```

→ add `Clobber`:

```rust
        Some(Redirect::Truncate(_) | Redirect::Clobber(_) | Redirect::Append(_)) => {
            unreachable!("parser never produces Truncate/Clobber/Append for stdin")
        }
```

`src/executor.rs:493` (compound stdin classification) currently:

```rust
            Redirect::Truncate(_) | Redirect::Append(_) | Redirect::Dup { .. } => {
                eprintln!("huck: unsupported stdin redirect on compound");
                return ExecOutcome::Continue(1);
            }
```

→ add `Clobber`:

```rust
            Redirect::Truncate(_) | Redirect::Clobber(_) | Redirect::Append(_) | Redirect::Dup { .. } => {
                eprintln!("huck: unsupported stdin redirect on compound");
                return ExecOutcome::Continue(1);
            }
```

- [ ] **Step 9: Route the 4 inline open sites**

At `src/executor.rs:1655`, `:1718`, `:3388`, `:3447` the stdout/stderr inline opens each match `Some(Redirect::Truncate(w))` and open with `OpenOptions::new().write(true).create(true).truncate(true).open(&path)`. For each, (a) widen the pattern to also bind `Clobber`, (b) compute the guard, (c) swap the open call. The surrounding error-cleanup block stays verbatim.

For each `Some(Redirect::Truncate(w)) => {` stdout/stderr arm, change the header to:

```rust
                    Some(r @ (Redirect::Truncate(w) | Redirect::Clobber(w))) => {
```

and, just before the `match OpenOptions::new()...` line in that arm, insert:

```rust
                        let guard = shell.shell_options.noclobber
                            && !matches!(r, Redirect::Clobber(_));
```

and replace:

```rust
                        match OpenOptions::new().write(true).create(true).truncate(true).open(&path) {
```

with:

```rust
                        match open_writable(&path, guard) {
```

(`shell` is in scope at all four sites — verify by reading the surrounding function. Keep the `use std::os::unix::io::IntoRawFd;` line and everything else in the arm unchanged.)

- [ ] **Step 10: Build + run the full executor/lib suite**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean (no non-exhaustive-match errors; if any remain, they are `Redirect`/`ResolvedRedirect` matches missing `Clobber`/`NoclobberTruncate` — add the arm mirroring `Truncate`).

Run: `cargo test --lib 2>&1 | tail -12`
Expected: all existing lib tests + the 4 `open_writable_` tests PASS.

- [ ] **Step 11: Manual end-to-end checks vs the spec table**

Run:
```bash
cargo build 2>&1 | tail -1
d=$(mktemp -d)
printf 'set -C\necho orig > %s/f\necho new > %s/f 2>/dev/null\necho "rc=$? c=$(cat %s/f)"\n' "$d" "$d" "$d" > "$d/blocked.sh"
./target/debug/huck "$d/blocked.sh"     # expect: rc=1 c=orig
printf 'set -C\necho orig > %s/g\necho new >| %s/g\necho "rc=$? c=$(cat %s/g)"\n' "$d" "$d" "$d" > "$d/force.sh"
./target/debug/huck "$d/force.sh"        # expect: rc=0 c=new
printf 'set -C\necho x > /dev/null\necho "rc=$?"\n' > "$d/devnull.sh"
./target/debug/huck "$d/devnull.sh"      # expect: rc=0
printf 'set -C\necho orig > %s/h\necho new &> %s/h 2>/dev/null\necho "c=$(cat %s/h)"\n' "$d" "$d" "$d" > "$d/amp.sh"
./target/debug/huck "$d/amp.sh"          # expect: c=orig (&> honors noclobber transitively)
```
Expected: the commented values. Investigate any mismatch before committing.

- [ ] **Step 12: Commit**

```bash
git add src/executor.rs
git commit -m "feat(v123): enforce noclobber via open_writable; >| overrides

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: integration tests + 46th bash-diff harness

**Files:**
- Create: `tests/noclobber_integration.rs`
- Create: `tests/scripts/noclobber_diff_check.sh`

- [ ] **Step 1: Write the bash-diff harness**

Create `tests/scripts/noclobber_diff_check.sh` (model on `tests/scripts/bash_rematch_diff_check.sh`). Each fragment self-contains its workdir via `mktemp -d` so the bash and huck runs are independent; the noclobber error is suppressed (`2>/dev/null`) because its prefix differs (L-01):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v123: noclobber (set -C) + >| redirect
# (M-21). File-arg execution (L-27). The noclobber *error message* prefix
# differs (huck: vs bash: line N:), so blocking cases suppress stderr and
# assert rc + file content; the error text is checked in the Rust integration
# test instead.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "blocked-overwrite" 'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo new > "$d/f" 2>/dev/null; echo "rc=$? c=$(cat "$d/f")"'
check "force-clobber"     'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo new >| "$d/f"; echo "rc=$? c=$(cat "$d/f")"'
check "append-allowed"    'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo more >> "$d/f"; echo "rc=$?"; cat "$d/f"'
check "new-file-allowed"  'd=$(mktemp -d); set -C; echo new > "$d/nf"; echo "rc=$? c=$(cat "$d/nf")"'
check "devnull-exempt"    'set -C; echo x > /dev/null; echo "rc=$?"'
check "stderr-force"      'd=$(mktemp -d); echo orig > "$d/f"; set -C; cat /nonexistent_huck_xyz 2>| "$d/f"; cat "$d/f"'
check "ampredir-blocked"  'd=$(mktemp -d); echo orig > "$d/f"; set -C; echo new &> "$d/f" 2>/dev/null; echo "c=$(cat "$d/f")"'
check "toggle-off"        'd=$(mktemp -d); echo orig > "$d/f"; set -C; set +C; echo new > "$d/f"; echo "c=$(cat "$d/f")"'
check "off-baseline"      'd=$(mktemp -d); echo orig > "$d/f"; echo new > "$d/f"; echo "c=$(cat "$d/f")"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable + run the harness**

Run: `chmod +x tests/scripts/noclobber_diff_check.sh && cargo build 2>&1 | tail -1 && ./tests/scripts/noclobber_diff_check.sh`
Expected: `Total: 9, Pass: 9, Fail: 0`. If `stderr-force` fails because `cat`'s error text differs, replace its fragment with one whose stderr comes from a builtin that is byte-identical across shells, or assert only the file's mtime/existence; investigate before changing the assertion.

- [ ] **Step 3: Write the integration test**

Create `tests/noclobber_integration.rs` (model on `tests/bash_rematch_integration.rs` for the binary-vs-bash harness — reuse its helper that runs a fragment as a file-arg through both the `huck` binary and `bash`). Cover the probed cases and additionally assert the huck error TEXT (after stripping the `huck: ` prefix) equals bash's (after stripping `bash: line N: `):

```rust
// Each test runs a fragment as a file-arg (L-27) through the huck binary and
// asserts behavior. Where the noclobber error is involved we compare the text
// after the shell-specific prefix.
mod common; // if the existing integration tests use a shared helper module; otherwise inline a `run_huck(frag) -> (stdout, stderr, code)` helper mirroring an existing integration test.

#[test]
fn blocked_overwrite_keeps_file_and_errors() {
    let (out, err, code) = run_huck_frag(
        r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; echo new > "$d/f"; echo "c=$(cat "$d/f")""#,
    );
    assert_eq!(code, 0, "the `echo c=...` command succeeds; only the blocked redirect is rc 1");
    assert!(out.contains("c=orig"), "file must be untouched: {out}");
    assert!(err.contains("cannot overwrite existing file"), "stderr: {err}");
}

#[test]
fn force_clobber_overwrites() {
    let (out, _err, _code) = run_huck_frag(
        r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; echo new >| "$d/f"; cat "$d/f""#,
    );
    assert!(out.contains("new"), "{out}");
}

#[test]
fn devnull_exempt_under_noclobber() {
    let (_out, _err, code) = run_huck_frag(r#"set -C; echo x > /dev/null; echo done"#);
    assert_eq!(code, 0);
}

#[test]
fn blocked_redirect_command_exit_status_is_1() {
    // The simple command whose ONLY redirect is blocked exits 1.
    let (_out, _err, code) = run_huck_frag(
        r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; echo new > "$d/f""#,
    );
    assert_eq!(code, 1, "a redirect-blocked command exits 1");
}

#[test]
fn stderr_force_clobber() {
    let (out, _e, _c) = run_huck_frag(
        r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; printf 'E\n' 2>| "$d/f" >&2 2>&1; true"#,
    );
    let _ = out; // behavior asserted by the diff harness; keep this test for the 2>| parse path
}
```

(Define `run_huck_frag(frag) -> (String /*stdout*/, String /*stderr*/, i32)` by copying the binary-invocation helper from `tests/bash_rematch_integration.rs`; it writes the fragment to a tempfile and runs `env!("CARGO_BIN_EXE_huck")` with that file as `arg`. If that integration file uses a different helper name/signature, match it.)

- [ ] **Step 4: Run the integration test**

Run: `cargo test --test noclobber_integration 2>&1 | tail -15`
Expected: all PASS. (If `blocked_redirect_command_exit_status_is_1` fails, confirm the inline open site returns `ExecOutcome::Continue(1)` on the `open_writable` error — it does in Task 3's preserved error block.)

- [ ] **Step 5: Payoff smoke (mise `>|` line)**

Run:
```bash
printf 'set -C\nprintf "spec\\n" >| /tmp/huck_mise_spec\ncat /tmp/huck_mise_spec\n' > /tmp/huck_mise_smoke.sh
./target/debug/huck /tmp/huck_mise_smoke.sh
```
Expected: prints `spec` (the `>|` that mise's `_mise` uses now parses and force-writes). Note in the commit/PR that this closes the `>|` parse gap; full `mise<TAB>` still needs bash-completion 2.12 (env).

- [ ] **Step 6: Commit**

```bash
git add tests/noclobber_integration.rs tests/scripts/noclobber_diff_check.sh
git commit -m "test(v123): noclobber integration + 46th bash-diff harness

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: docs — drop M-21, move to supported

**Files:**
- Modify: `docs/bash-divergences.md` (the M-21 entry + Summary counts)
- Modify: `README.md` (the "Not yet implemented" redirections/set lines)

- [ ] **Step 1: Remove M-21 from the divergences doc**

In `docs/bash-divergences.md`, DELETE the M-21 entry (the `>|` / noclobber line; search `M-21`). It is a current-divergences-only doc, so the resolved item is removed (not flipped). Decrement the Tier-2 (Missing features) count in the Summary by 1. If a follow-on gap is discovered during implementation (e.g. arbitrary `n>|`), add a new `[deferred]` entry for it instead.

- [ ] **Step 2: Update the README**

In `README.md`:
- In **"Command syntax & operators"** (the redirections sentence ~`:45`), add `>|` (force-clobber) to the listed output redirections.
- In **"Builtins & options"** `set` list (~`:97`), add `-C`/`-o noclobber`.
- In **"Known differences from bash" → "Not yet implemented"**: remove `>|` from the redirections bullet (~`:110`) and remove `-C` (noclobber) from the `set`/`declare` modes bullet (~`:114`).
- Bump the harness count: "**45 bash-diff harnesses**" → "**46 bash-diff harnesses**" (~`:19`).

- [ ] **Step 3: Verify no stale references**

Run: `grep -n "M-21\|>|" docs/bash-divergences.md; grep -n "noclobber\|>|\|45 bash-diff\|46 bash-diff" README.md`
Expected: no `M-21` in the divergences doc; README mentions `>|`/noclobber only in supported sections + `46 bash-diff harnesses`.

- [ ] **Step 4: Commit**

```bash
git add docs/bash-divergences.md README.md
git commit -m "docs(v123): drop M-21; move >| + noclobber to supported

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build 2>&1 | tail -3` — clean.
- [ ] `cargo test 2>&1 | tail -15` — full suite green (≥ prior count + the new tests).
- [ ] `cargo clippy --all-targets 2>&1 | tail -5` — no warnings.
- [ ] `for h in tests/scripts/*_diff_check.sh; do echo "== $h =="; bash "$h" | tail -1; done` — all 46 harnesses pass.
- [ ] Spot-check the spec's probed table one more time against `./target/debug/huck` and `bash`.

## Self-review notes (plan author)
- **Spec coverage:** Unit 1 → Task 1; Unit 2 → Task 2; Unit 3 (`open_writable`, `NoclobberTruncate`, all open sites, classification, `&>` transitive) → Task 3; tests+harness → Task 4; docs → Task 5. `$-` `C` (Task 2) + the no-bare-`echo $-`-in-harness decision (Task 4 harness comment) are both covered. Special-file exemption + symlink-followed regular-file check are in the `open_writable` body (Task 3 Step 3) and tested (Step 1 `/dev/null`).
- **Type consistency:** `Redirect::Clobber(Word)`, `Operator::RedirClobber`/`RedirErrClobber`, `ResolvedRedirect::NoclobberTruncate(String)`, `open_writable(path: &str, guard_noclobber: bool) -> io::Result<File>` are used identically across tasks.
- **Zero-regression hinge:** noclobber off ⇒ every `open_writable(_, false)` is exactly the prior `write+create+truncate` open; `>|` always passes `false`. Existing `Redirect::Truncate(ww(...))` tests untouched (new variant, not a changed signature).
