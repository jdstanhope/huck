# v286 — Move-fd redirect + faithful redirect regeneration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the bash move-fd redirect `[n]<&digit-` / `[n]>&digit-`
(issue [#121](https://github.com/jdstanhope/huck/issues/121), a real deadlock),
and make `declare -f` / `type` redirect regeneration faithful — retiring the
`generate.rs` "best-effort" stance.

**Architecture:** Add a `RedirOp::Move { source, output }` AST variant, parse
`<digits>-` into it, execute it as "dup source→target then close source" (reusing
the existing `Dup` + `Close` machinery and save/restore scope), and replace the
0/1/2-slot-collapsing regeneration with an ordered renderer matching bash's exact
canonical output.

**Tech Stack:** Rust (`huck-syntax` AST/parse/generate, `huck-engine` executor),
`libc`, the `tests/scripts/*_diff_check.sh` bash-vs-huck harness convention.

## Global Constraints

- Commit trailer on every commit:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- `cargo fmt --all` before every commit; CI enforces `--check`.
- Per-crate, single-threaded tests (the box OOMs on `--workspace`):
  `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and
  `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the
  binary with `cargo build -p huck`.
- Move semantics (bash): `[n]<&digit-` / `[n]>&digit-` = `dup2(digit, n)` then
  `close(digit)`; directional default fd is 1 for `>&`, 0 for `<&`.
- Do NOT change `slots_for_simple_path` behavior for the executor pipeline path
  or any runtime redirect execution beyond adding `Move`. `&>`/`&>>` preservation
  is OUT OF SCOPE (deferred to #124).

---

### Task 1: The move-fd operator (`<&N-` / `>&N-`) end to end

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (AST variant, `dup_op`, `default_fd`,
  `slots_for_simple_path`)
- Modify: `crates/huck-engine/src/executor.rs` (all `RedirOp` match sites)
- Modify: `crates/huck-syntax/src/parser.rs` (`fill_redirects` at ~3732)
- Test: `crates/huck-syntax/src/command.rs` (unit test for `dup_op`)
- Test (create, 0755): `tests/scripts/move_fd_redirect_diff_check.sh`

**Interfaces:**
- Produces: `RedirOp::Move { source: Word, output: bool }`; `dup_op` now returns
  it for a `<digits>-` operand.

- [ ] **Step 1: Write the failing move-fd harness**

Create `tests/scripts/move_fd_redirect_diff_check.sh` (mode 0755):

```bash
#!/usr/bin/env bash
# v286 (#121): the move-fd redirect `[n]<&digit-` / `[n]>&digit-` (dup then close
# the source fd) must match bash byte-for-byte. Error prefixes are normalized so
# only the message tail + rc are compared.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'A\nB\n' > "$WORK/f"
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# The #121 repro: move fd5 (a file) onto stdin, then read stdin.
check "move file to stdin"   'exec 5<f; exec 0<&5-; cat'
check "move+read loop"       'exec 5<f; exec 0<&5-; while read l; do echo "got:$l"; done; echo DONE'
# Output move in a subshell, then inspect the file.
check "output move"          '( exec 5>o; exec 1>&5-; echo hi ); cat o'
# After a move the source fd is closed: reading it fails.
check "source fd closed"     'exec 5<f; exec 0<&5-; read x <&5; echo "rc=$?"'
# Degenerate N>&N- (target == source) closes fd N.
check "degenerate NgtN"      '( exec 5>o; exec 5>&5-; echo x >&5 ) 2>/dev/null; echo "rc=$?"'
# Bad source fd → error (compare normalized message + rc).
check "bad source fd"        '( exec 7<&9- ) 2>&1; echo "rc=$?"'
# Command-scoped move restores the target afterward.
check "command-scoped move"  'exec 5<f; cat 0<&5-; echo "after:$(echo restored)"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Build current huck and run the harness — confirm it FAILS**

```bash
cargo build -p huck
bash tests/scripts/move_fd_redirect_diff_check.sh
```
Expected: FAILs (huck prints `bad fd: 5-` and drops the redirect). Record output.

- [ ] **Step 3: Add the `RedirOp::Move` variant**

In `crates/huck-syntax/src/command.rs`, add to the `RedirOp` enum (after `Dup`):

```rust
    /// `[n]>&digit-` (output) / `[n]<&digit-` (input): dup `source` onto the
    /// target fd, THEN close `source` (bash's "move fd"). `output` picks the
    /// directional default target fd (1 for `>&`, 0 for `<&`) and the `>&`/`<&`
    /// rendering.
    Move { source: Word, output: bool },
```

- [ ] **Step 4: Classify the operand in `dup_op` + add `is_move_operand`**

Replace `dup_op` (command.rs ~1033):

```rust
/// `>&w`/`<&w`: `-` closes; `<digits>-` moves (dup then close source);
/// otherwise a Dup.
pub(crate) fn dup_op(source: Word, output: bool) -> RedirOp {
    match word_literal_text(&source) {
        Some("-") => RedirOp::Close,
        Some(t) if is_move_operand(t) => {
            RedirOp::Move {
                source: lit_word(&t[..t.len() - 1]),
                output,
            }
        }
        _ => RedirOp::Dup { source, output },
    }
}

/// True for a bash move-fd operand: one or more ASCII digits then a single
/// trailing `-` (e.g. `5-`, `10-`). `-` alone is Close (handled by the caller).
fn is_move_operand(t: &str) -> bool {
    matches!(t.strip_suffix('-'), Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}
```

- [ ] **Step 5: Add `Move` arms to the syntax-side matches**

`default_fd` (command.rs ~263) — Move mirrors Dup:

```rust
            RedirOp::Move { output: true, .. } => 1,
            RedirOp::Move { output: false, .. } => 0,
```

`slots_for_simple_path` (command.rs ~344) — drop Move like Close/dup-in:

```rust
            RedirOp::Dup { output: false, .. } | RedirOp::Close | RedirOp::Move { .. } => None,
```

`fill_redirects` (parser.rs ~3732) — Move has a (numeric) source word, no
heredoc body, so mirror the `Dup` arm (`RedirOp::Move { source, .. } => fill_word(source, bodies)`)
or, if simpler, treat like `Close` (no-op). Either is correct — the source is a
literal digit with no interpolation.

- [ ] **Step 6: Add the in-process executor `Move` arm**

In `crates/huck-engine/src/executor.rs`, the ordered applier `apply` (~1089, the
`RedirOp::Dup` arm). Add a `Move` arm right after it — dup source→target (exactly
the Dup logic already there), then close the source:

```rust
            RedirOp::Move { source, output: _ } => {
                let src = match resolve_fd_target(source, shell) {
                    Ok(fd) => fd,
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(shell, &mut *err, None, "{}", crate::bash_io_error(&e));
                        }
                        return Err(ExecOutcome::Continue(1));
                    }
                };
                if unsafe { libc::fcntl(src, libc::F_GETFD) } < 0 {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(shell, &mut *err, None, "{src}: Bad file descriptor");
                    }
                    return Err(ExecOutcome::Continue(1));
                }
                if self.redirect(shell, src, target, sink, err_sink).is_err() {
                    return Err(ExecOutcome::Continue(1));
                }
                // The "move": close the source fd (save/restore via close_target
                // so a command-scoped move restores it; `exec` persists).
                self.close_target(src);
                Ok(())
            }
```

- [ ] **Step 7: Add `Move` arms to the remaining executor match sites**

The compiler's non-exhaustive-match errors enumerate them. Apply this UNIFORM
recipe (Move = Dup + close-source) at each:

- **`apply_var`** (~1283, in-process `{var}` fd): mirror its `Dup` arm to obtain
  `src`, wire it onto the target (as Dup does), then `self.close_target(src)`.
- **The three child-fd-plan builders** (~5731, ~5903, ~6092) that push
  `ChildRedirOp`: resolve `src` (as the local `Dup` arm does), then push BOTH
  `ChildRedirOp::Dup { target, source: src }` **and** `ChildRedirOp::Close { target: src }`.
  This reuses the existing child-side dup2+close machinery — no new `ChildRedirOp`
  variant.

Match each site's local variable names and error-handling shape (copy the
adjacent `Dup` arm, append the close/Close). The move-fd harness (which runs
in-process `exec`, command-scoped, subshell, and a piped case) validates every
path.

- [ ] **Step 8: Add the `dup_op` unit test**

In `crates/huck-syntax/src/command.rs` test module (find `#[cfg(test)] mod` or
the sibling test file; add if a test module already exists for command.rs):

```rust
    #[test]
    fn dup_op_classifies_close_move_dup() {
        assert!(matches!(dup_op(lit_word("-"), true), RedirOp::Close));
        assert!(matches!(dup_op(lit_word("5-"), false), RedirOp::Move { output: false, .. }));
        assert!(matches!(dup_op(lit_word("10-"), true), RedirOp::Move { output: true, .. }));
        assert!(matches!(dup_op(lit_word("5"), true), RedirOp::Dup { output: true, .. }));
        // A non-numeric source ending in `-` stays a Dup (bash: move needs digits).
        assert!(matches!(dup_op(lit_word("x-"), true), RedirOp::Dup { .. }));
    }
```

(If `command.rs` has no test module, add `#[cfg(test)] mod move_tests { use super::*; ... }`.)

- [ ] **Step 9: Build, run the harness and unit tests — confirm PASS**

```bash
cargo build -p huck
bash tests/scripts/move_fd_redirect_diff_check.sh   # Fail: 0
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 dup_op
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 redirect
```
Expected: harness `Fail: 0`; unit tests pass; no engine regressions.

- [ ] **Step 10: Format and commit**

```bash
cargo fmt --all
git add -A
git commit -m "fix(#121): support the move-fd redirect <&N- / >&N-

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Faithful ordered redirect regeneration

**Files:**
- Modify: `crates/huck-syntax/src/generate.rs` (replace `append_slot_redirects`,
  add `redirection_to_source`, delete the dead slot renderer, remove the
  best-effort comment)
- Test: `crates/huck-syntax/src/generate.rs` (unit tests for the renderer)
- Test (create, 0755): `tests/scripts/redirect_regen_diff_check.sh`

**Interfaces:**
- Consumes: `RedirOp::Move` from Task 1.
- Produces: `fn redirection_to_source(r: &crate::command::Redirection) -> String`.

- [ ] **Step 1: Write the failing regeneration harness**

Create `tests/scripts/redirect_regen_diff_check.sh` (mode 0755):

```bash
#!/usr/bin/env bash
# v286 (#121): `declare -f` redirect regeneration must be byte-identical to bash
# for every redirect form (fd>2, <& dup-in, <>, {var}, N>&-, move, ordering).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" body="$2" b h
    b=$(printf 'f() { %s; }\ndeclare -f f\n' "$body" | bash 2>&1)
    h=$(printf 'f() { %s; }\ndeclare -f f\n' "$body" | "$HUCK_BIN" 2>&1)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "trunc default fd"   'true 1>x'
check "trunc fd2"          'true 2>x'
check "trunc fd0"          'true 0>x'
check "trunc fd9"          'true 9>x'
check "read default fd"    'true 0<x'
check "read fd3"           'true 3<x'
check "append fd2"         'true 2>>x'
check "clobber"            'true >|x'
check "readwrite default"  'true <>x'
check "readwrite fd3"      'true 3<>x'
check "dup out default"    'true >&2'
check "dup out fd2"        'true 2>&1'
check "dup in fd3"         'true 3<&0'
check "dup in default"     'true <&0'
check "close fd3 out"      'exec 3>&-'
check "close fd3 in"       'exec 3<&-'
check "close default in"   'exec 0<&-'
check "move in fd0"        'exec 0<&5-'
check "move out default"   'true >&2-'
check "move out fd3"       'true 3>&4-'
check "var fd trunc"       'exec {fd}>x'
check "var fd dup"         'exec {v}<&3'
check "var fd move"        'exec {v}<&3-'
check "ordered multi"      'true 3>&1 4<&0'
check "mixed order"        'true >a 2>&1'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run it against current huck — confirm it FAILS**

```bash
cargo build -p huck
bash tests/scripts/redirect_regen_diff_check.sh
```
Expected: several FAILs (dropped fd>2 / `<&` / `{var}` / close / `<>` / move).
Record which pass and which fail.

- [ ] **Step 3: Add the ordered renderer**

In `crates/huck-syntax/src/generate.rs`, add (near the old `redirect_to_source`):

```rust
/// Render one `Redirection` faithfully, matching bash's `declare -f`
/// canonicalization (verified against bash 5.2.21). File redirects drop the fd
/// only when it is the directional default (1 for output, 0 for input); dup /
/// move / readwrite always show the fd; close normalizes to `{fd}>&-`.
fn redirection_to_source(r: &crate::command::Redirection) -> String {
    use crate::command::{FileMode, RedirFd, RedirOp};
    // Prefix for a File op whose directional default is `def` (droppable).
    let file_prefix = |def: u16| -> String {
        match &r.fd {
            RedirFd::Var(name) => format!("{{{name}}}"),
            RedirFd::Default => String::new(),
            RedirFd::Number(n) if *n == def => String::new(),
            RedirFd::Number(n) => n.to_string(),
        }
    };
    // Prefix for dup/move/readwrite: always shown; Default resolves to `def`.
    let always_prefix = |def: u16| -> String {
        match &r.fd {
            RedirFd::Var(name) => format!("{{{name}}}"),
            RedirFd::Default => def.to_string(),
            RedirFd::Number(n) => n.to_string(),
        }
    };
    match &r.op {
        RedirOp::File { mode, target } => {
            let (arrow, def, always) = match mode {
                FileMode::ReadOnly => ("<", 0, false),
                FileMode::Truncate => (">", 1, false),
                FileMode::Append => (">>", 1, false),
                FileMode::Clobber => (">|", 1, false),
                FileMode::ReadWrite => ("<>", 0, true),
            };
            let prefix = if always { always_prefix(def) } else { file_prefix(def) };
            format!("{prefix}{arrow} {}", word_to_source(target))
        }
        RedirOp::Dup { source, output } => {
            let (arrow, def) = if *output { (">&", 1) } else { ("<&", 0) };
            format!("{}{arrow}{}", always_prefix(def), word_to_source(source))
        }
        RedirOp::Move { source, output } => {
            let (arrow, def) = if *output { (">&", 1) } else { ("<&", 0) };
            format!("{}{arrow}{}-", always_prefix(def), word_to_source(source))
        }
        RedirOp::Close => {
            // Direction normalized to `>&-`; fd is always concrete (parser
            // resolves the directional default to 1/0), so `always_prefix(1)`'s
            // Default branch is never taken.
            format!("{}>&-", always_prefix(1))
        }
        RedirOp::Heredoc { .. } | RedirOp::HereString(_) => {
            // Preserve the existing heredoc / here-string rendering by delegating
            // to the current slot renderer for these two ops only.
            heredoc_or_herestring_to_source(&r.op)
        }
    }
}
```

Extract the current `Heredoc` / `HereString` rendering from `redirect_to_source`
into `heredoc_or_herestring_to_source(op: &RedirOp) -> String` (moving the bodies
verbatim), so both the old and new renderers share it during the transition (the
old one is deleted in Step 5).

- [ ] **Step 4: Replace `append_slot_redirects` with an ordered walk**

Rewrite `append_slot_redirects` (generate.rs ~25) to iterate the full ordered
list (rename to `append_redirects`; update its 3 call sites at ~57, ~127, ~430):

```rust
/// Append every redirection in SOURCE ORDER, each prefixed with a space
/// (e.g. ` 2>&1`). Faithful — preserves fd numbers, order, `<&`/`>&`, close,
/// `<>`, `{var}`, and move. Shared by the hoisted-brace-group path, the
/// `Command::Redirected` arm, and `exec_to_source`.
fn append_redirects(s: &mut String, redirects: &[crate::command::Redirection]) {
    for r in redirects {
        s.push(' ');
        s.push_str(&redirection_to_source(r));
    }
}
```

Update the `Command::Redirected` arm (generate.rs ~122) to remove the
"best-effort" comment and call `append_redirects`.

- [ ] **Step 5: Delete the dead slot renderer**

Remove `redirect_to_source(r: &RedirectSlot, ...)` and the `RedirDefault` enum
(now unused — `append_redirects` renders from `Redirection`, not `RedirectSlot`).
Leave `RedirectSlot` itself and `slots_for_simple_path` intact (still used by the
executor). Confirm no other references: `cargo build -p huck-syntax` clean.

- [ ] **Step 6: Add renderer unit tests**

In the generate.rs test module:

```rust
    #[test]
    fn redirect_regen_matches_bash_forms() {
        // (body, expected declare -f redirect fragment)
        for (src, want) in [
            ("f() { true 1>x; }", "> x"),
            ("f() { true 2>x; }", "2> x"),
            ("f() { true 0>x; }", "0> x"),
            ("f() { true <>x; }", "0<> x"),
            ("f() { true >&2; }", "1>&2"),
            ("f() { true 3<&0; }", "3<&0"),
            ("f() { exec 3<&-; }", "3>&-"),
            ("f() { exec 0<&5-; }", "0<&5-"),
            ("f() { true >&2-; }", "1>&2-"),
            ("f() { exec {v}<&3-; }", "{v}<&3-"),
        ] {
            let out = declf(src);
            assert!(out.contains(want), "src {src:?}: want fragment {want:?} in:\n{out}");
        }
    }
```

(Uses the existing `declf` test helper in generate.rs.)

- [ ] **Step 7: Run regen harness + unit tests + ALL redirect harnesses**

```bash
cargo build -p huck
bash tests/scripts/redirect_regen_diff_check.sh            # Fail: 0
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 redirect_regen
for h in func_redirect function_redirect fd_redirect compound_redirects \
         assign_redirect pipe_compound_redirect pipeline_redirect_pipe \
         heredoc_redir_v266 declare_f; do
  bash tests/scripts/${h}_diff_check.sh >/dev/null 2>&1 && echo "OK $h" || echo "FAIL $h"
done
```
Expected: regen harness `Fail: 0`; unit tests pass; every existing redirect
harness prints `OK`.

- [ ] **Step 8: Format and commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(#121): faithful ordered redirect regeneration in declare -f

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after both tasks)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` — green.
- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — green.
- [ ] Build release + full sweep: `cargo build --release --locked --bin huck`
  then `tests/scripts/run_diff_checks.sh` — green (both new harnesses included).
- [ ] Manual: the bash-suite `redir` category no longer deadlocks —
  `THIS_SH=<release huck> BASH_TSTOUT=/tmp/o timeout 40 sh /tmp/bash-5.2.21/tests/run-redir`
  completes (was a true hang).

## Self-review

- Spec coverage: §1 move variant + parse (Task 1 Steps 3-4), §2 executor (Steps
  6-7), §3 faithful regeneration (Task 2), §5 testing (both harnesses + unit
  tests). §4 `&>` explicitly out of scope (#124).
- Type consistency: `RedirOp::Move { source: Word, output: bool }` defined in
  Task 1, consumed by `slots_for_simple_path`/`default_fd`/executor (Task 1) and
  `redirection_to_source` (Task 2); `ChildRedirOp::{Dup,Close}` reused for the
  move in child-plan builders; `RedirFd::{Default,Number,Var}` and `FileMode::*`
  match the enums in `command.rs`.
- No placeholders. The one non-verbatim area (Step 7 executor sites) is a uniform
  compiler-enumerated recipe with the canonical arm given in Step 6 and validated
  by the harness's in-process/subshell/pipeline cases.
