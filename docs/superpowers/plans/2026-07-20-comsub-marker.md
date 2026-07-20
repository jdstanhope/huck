# v316 — `command substitution:` marker for backtick bodies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a syntax error re-parsing a backtick `` `…` `` command-substitution body reports bash's `command substitution:` marker + the body-local line (offset by the outer backtick line) + a source-echo of the backtick body.

**Architecture:** The error rides a new `ParseError::InCommandSub { inner, body, err_pos }` (backtick fails at parse time — no Shell). `render_syntax_diag` is refactored into a thin entry + a `render_diag_inner(shell, err, source, local_line, marker, line_base)` worker; the `InCommandSub` arm recurses on `inner` against the body with the `CommandSub` marker and the composed base. The eval/top-level base is unified into the `line_base` param (byte-neutral).

**Tech Stack:** Rust (huck-syntax + huck-engine), bash-diff harnesses.

## Global Constraints

- **Issue:** [#213](https://github.com/jdstanhope/huck/issues/213). Spec: `docs/superpowers/specs/2026-07-20-comsub-marker-design.md`.
- Backtick command-substitution bodies ONLY; `$()`/`$(())`/`${}` already match bash and must stay unchanged.
- bash 5.2.21 byte-exact for single-line backtick bodies; multi-line best-effort (documented approximation).
- **Byte-neutral for eval + top-level:** `Marker::Default` (base 0, no marker) and `Marker::Eval` (eval base, `eval:`) output must be identical to v315 — pinned by `syntax_error_diag_diff_check.sh` (27) + `eval_line_diag_diff_check.sh` (10).
- **Box/build:** `cargo build -p huck --bin huck`; `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` / `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. NEVER `cargo test --workspace`. Touched `-p huck` integration binaries at `--test-threads 2`. `cargo fmt --all` before commit. `/usr/bin/grep` only.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File structure

- `crates/huck-engine/src/error_emit.rs` — `Marker` enum; split `render_syntax_diag` → entry + `render_diag_inner`; `emit_syntax_error_ex`/`emit_matching` gain `marker`/`line_base` params; the `InCommandSub` render arm.
- `crates/huck-engine/src/shell_state.rs` — `Diag::SyntaxNested { line, marker }` variant + its `error_prefix` arm.
- `crates/huck-syntax/src/command.rs` — `ParseError::InCommandSub` variant.
- `crates/huck-syntax/src/errors.rs` — its `Display` message arm.
- `crates/huck-syntax/src/parser.rs` — `parse_backtick_sub` phase-3 wrap (~line 2000).
- `tests/scripts/comsub_marker_diag_diff_check.sh` — NEW harness.

---

### Task 1: Refactor the renderer to thread `(marker, line_base)` — byte-neutral

**Files:**
- Modify: `crates/huck-engine/src/error_emit.rs` (`render_syntax_diag`, `emit_matching`, `emit_syntax_error_ex`)
- Modify: `crates/huck-engine/src/shell_state.rs` (`Diag` enum + `error_prefix`)

**Interfaces:**
- Produces: `enum Marker { Default, Eval, CommandSub }` (in error_emit.rs, `pub(crate)`); `render_diag_inner(shell, err, source, local_line: u32, marker: Marker, line_base: u32)`; `emit_syntax_error_ex(shell, line, body, echo, marker: Marker)`; `Diag::SyntaxNested { line: u32, marker: &'static str }`.

- [ ] **Step 1: Add the `Marker` enum** (error_emit.rs, near the top):
```rust
#[derive(Clone, Copy)]
pub(crate) enum Marker { Default, Eval, CommandSub }
```

- [ ] **Step 2: Add `Diag::SyntaxNested`** (shell_state.rs, in `enum Diag`):
```rust
    /// A syntax error carrying an explicit nested-context marker
    /// (`command substitution:`), which REPLACES `-c:` and ignores `eval_frame`.
    /// Prologue: `<name>: <marker>: line N: `. v316 (#213).
    SyntaxNested { line: u32, marker: &'static str },
```
And its `error_prefix` arm (mirror the `Diag::Syntax` arm's name/`!is_interactive` logic, but always print `marker`):
```rust
            Diag::SyntaxNested { line, marker } => {
                if !self.is_interactive {
                    out.push_str(marker);
                    out.push_str(": ");
                    out.push_str(&format!("line {line}: "));
                }
            }
```

- [ ] **Step 3: Thread `marker` through `emit_syntax_error_ex`.** Change its signature to take `marker: Marker` and pick the prologue:
```rust
pub(crate) fn emit_syntax_error_ex(
    shell: &Shell, line: u32, body: std::fmt::Arguments, echo: Option<&str>, marker: Marker,
) {
    with_err(|err| {
        let prefix = match marker {
            Marker::CommandSub => shell.error_prefix(Diag::SyntaxNested { line, marker: "command substitution" }),
            _ => shell.error_prefix(Diag::Syntax { line }),   // Default/Eval: eval_frame logic unchanged
        };
        let _ = write!(err, "{prefix}");
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
        if let Some(src) = echo {
            let _ = write!(err, "{prefix}`{src}'\n");
        }
    });
}
```
(Keep the thin `emit_syntax_error` wrapper delegating with `Marker::Default`.) `Diag::Syntax`'s `eval_frame`-based `eval:` marker stays as v315 shipped it — so `Marker::Eval` renders via `Diag::Syntax` and is byte-identical.

- [ ] **Step 4: Split `render_syntax_diag`** into the entry + worker. The entry derives marker + base from `eval_frame` (byte-identical to today), then calls the worker with the RAW local line:
```rust
pub fn render_syntax_diag(shell: &Shell, err: &ParseError, source: &str, token_line: u32) {
    let base = shell.line_base();
    let (marker, line_base) = match shell.eval_frame {
        Some(_) => (Marker::Eval, base),
        None => (Marker::Default, base),
    };
    render_diag_inner(shell, err, source, token_line.saturating_sub(base), marker, line_base);
}

fn render_diag_inner(shell: &Shell, err: &ParseError, source: &str,
                     local_line: u32, marker: Marker, line_base: u32) {
    let display_line = line_base + local_line;
    let eof_line = line_base + 1 + source.lines().count() as u32;
    let echo_line = source_logical_line(source, local_line);
    match err {
        ParseError::Unexpected(f) if matches!(f.found, Found::Token(_)) => {
            let Found::Token(k) = &f.found else { unreachable!() };
            let tok = spell_token(k);
            emit_syntax_error_ex(shell, display_line,
                format_args!("syntax error near unexpected token `{tok}'"), Some(&echo_line), marker);
        }
        ParseError::Unexpected(ExpectFailure { found: Found::Eof, matching: Some(d), .. }) if is_matching_delim(*d) => {
            emit_matching(shell, *d, source, local_line, marker, line_base);
        }
        ParseError::Lex(le) => match lex_is_shape3(le) {
            Some(d) => emit_matching(shell, d, source, local_line, marker, line_base),
            None => emit_syntax_error_ex(shell, display_line, format_args!("syntax error: {err}"), None, marker),
        },
        ParseError::UnterminatedIf | ParseError::UnterminatedLoop | ParseError::UnterminatedCase
        | ParseError::UnterminatedSubshell | ParseError::UnterminatedBrace | ParseError::UnterminatedFunction
        | ParseError::Unexpected(ExpectFailure { found: Found::Eof, .. }) => {
            emit_syntax_error_ex(shell, eof_line, format_args!("syntax error: unexpected end of file"), None, marker);
        }
        // ParseError::InCommandSub arm is added in Task 2.
        other => emit_syntax_error_ex(shell, display_line, format_args!("syntax error: {other}"), None, marker),
    }
}
```

- [ ] **Step 5: Update `emit_matching`** to take `(local_line, marker, line_base)` instead of reading `shell.line_base()`:
```rust
fn emit_matching(shell: &Shell, d: Delim, source: &str, local_line: u32, marker: Marker, line_base: u32) {
    let eof_line = line_base + 1 + source.lines().count() as u32;
    let line = match d {
        Delim::DollarParen | Delim::Paren => eof_line,
        _ => line_base + local_line,
    };
    let matchtxt = if matches!(d, Delim::DBracket) { "]]".to_string() } else { spell_delim(d).to_string() };
    emit_syntax_error_ex(shell, line, format_args!("unexpected EOF while looking for matching `{matchtxt}'"), None, marker);
}
```

- [ ] **Step 6: Build + prove byte-neutral.** `cargo build -p huck --bin huck && cargo build -p huck-engine`. Run the two guard harnesses — they MUST stay fully green (the whole point of this task): `bash tests/scripts/syntax_error_diag_diff_check.sh` (27/27) and `bash tests/scripts/eval_line_diag_diff_check.sh` (10/10). Also `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. If any eval/top-level output changed, the refactor is wrong — fix before proceeding.
- [ ] **Step 7: fmt + commit** (`v316 task 1: thread (marker, line_base) through the syntax renderer — byte-neutral`).

---

### Task 2: `ParseError::InCommandSub` + parser wrap + the render arm

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (variant)
- Modify: `crates/huck-syntax/src/errors.rs` (Display arm)
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_backtick_sub` phase-3 wrap ~line 2000)
- Modify: `crates/huck-engine/src/error_emit.rs` (the `InCommandSub` arm)

**Interfaces:**
- Consumes: Task 1's `render_diag_inner`, `Marker`.
- Produces: `ParseError::InCommandSub { inner: Box<ParseError>, body: String, err_pos: usize }`.

- [ ] **Step 1: Add the variant** (command.rs, in `enum ParseError`):
```rust
    /// v316 (#213): a syntax error re-parsing a backtick command-substitution
    /// body. `inner` = the body error (already body-relative), `body` = the
    /// cooked backtick body (for the echo + body-local line), `err_pos` = the
    /// body-relative error offset. Rendered with `command substitution:`.
    InCommandSub { inner: Box<ParseError>, body: String, err_pos: usize },
```
Derives already on `ParseError` (`Clone, Debug, PartialEq, Eq`) are satisfied by `Box<ParseError>` + `String` + `usize`.

- [ ] **Step 2: Display arm** (errors.rs `parse_error_message_impl`) — delegate to the inner (real render is the engine's):
```rust
        ParseError::InCommandSub { inner, .. } => parse_error_message_impl(inner),
```

- [ ] **Step 3: Wrap in `parse_backtick_sub`** (parser.rs, phase 3 — the `match parse_sequence(&mut sub)?` around line 2000). Read the exact current lines first; replace the `?`-propagation with a wrap:
```rust
    let sequence = match parse_sequence(&mut sub) {
        Ok(Some(mut seq)) => { zero_lines_in_sequence(&mut seq); seq }
        Ok(None) => empty_sequence(),
        Err(inner) => {
            return Err(ParseError::InCommandSub {
                inner: Box::new(inner),
                body: cooked,
                err_pos: sub.cursor_pos(),
            });
        }
    };
```
(`cooked` is moved into the error; ensure it's not used afterward — it isn't, the only later use is building the `WordPart::CommandSub`, which is the `Ok` path.)

- [ ] **Step 4: Add the `InCommandSub` render arm** in `render_diag_inner` (error_emit.rs), before the fallback:
```rust
        ParseError::InCommandSub { inner, body, err_pos } => {
            // The backtick sits at display line `line_base + local_line`; the
            // body numbers from 1, so offset it by that line minus one.
            let comsub_base = line_base + local_line.saturating_sub(1);
            let body_local = 1 + body.as_bytes()[..(*err_pos).min(body.len())]
                .iter().filter(|&&b| b == b'\n').count() as u32;
            render_diag_inner(shell, inner, body, body_local, Marker::CommandSub, comsub_base);
        }
```

- [ ] **Step 5: Build + manual verify.** `cargo build -p huck --bin huck`. Check byte-for-byte vs bash:
```
for f in 'echo `case x in`' 'echo `case esac in esac)`' 'echo `echo "hi`' 'echo `echo )`'; do
  diff <(bash -c "$f" 2>&1 | sed -E 's#^bash: #SH: #') \
       <(./target/debug/huck -c "$f" 2>&1 | sed -E 's#.*/huck: #SH: #') && echo "MATCH: $f"
done
```
All four must MATCH (marker `command substitution:`, and the near-token case's echo must be the backtick BODY, not the outer line). Also confirm `$()` is unchanged: `echo $(case x in)` still `-c:`.
- [ ] **Step 6: Regression.** `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` green. The two guard harnesses still green.
- [ ] **Step 7: fmt + commit** (`v316 task 2: ParseError::InCommandSub + backtick wrap + command substitution: render arm`).

---

### Task 3: Harness + line-base + sweep + close #213

**Files:**
- Create: `tests/scripts/comsub_marker_diag_diff_check.sh`
- Modify: `docs/bash-test-suite-baseline.md` (provenance note, if warranted)

- [ ] **Step 1: Write the harness** `tests/scripts/comsub_marker_diag_diff_check.sh`. **Compare STDERR ONLY** (`2>&1 >/dev/null` — capture stderr, discard stdout) and **do NOT compare rc**. Rationale (verified against bash 5.2.21): a backtick-body syntax error is *recoverable* in bash — it prints the `command substitution:` diagnostic to stderr, then continues the outer command with an empty substitution (stdout differs, rc 0), while huck aborts the `-c` string (rc 2). That stdout/rc divergence is the pre-existing parse-time-vs-expansion-time behavior, **out of scope for #213** (filed as a follow-on in Step 4) — #213 is the marker text, which is on stderr and matches byte-for-byte.
```bash
#!/usr/bin/env bash
# v316 (#213): syntax error in a backtick command-sub body → `command substitution:` marker.
# STDERR-only (the marker); stdout/rc diverge by the pre-existing recover-vs-abort gap (follow-on).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck): #SH: #"; }
snorm() { sed -E "s#^.*/[^:]+: #SH: #"; }
check() { local l=$1 f=$2 b h
  b=$(bash -c "$f" 2>&1 >/dev/null | norm)
  h=$("$HUCK" -c "$f" 2>&1 >/dev/null | norm)
  if [ "$b" != "$h" ]; then echo "FAIL [$l]"; echo "  bash: [$b]"; echo "  huck: [$h]"; FAIL=1; else echo "PASS [$l]"; fi; }
check_script() { local l=$1; shift; local f; f=$(mktemp); printf '%s\n' "$@" > "$f"; local b h
  b=$(bash "$f" 2>&1 >/dev/null | snorm)
  h=$("$HUCK" "$f" 2>&1 >/dev/null | snorm); rm -f "$f"
  if [ "$b" != "$h" ]; then echo "FAIL [$l]"; echo "  bash: [$b]"; echo "  huck: [$h]"; FAIL=1; else echo "PASS [$l]"; fi; }
# backtick bodies → command substitution: marker
check 'bt-unterm-case'  'echo `case x in`'
check 'bt-near-token'   'echo `case esac in esac)`'
check 'bt-unterm-quote' 'echo `echo "hi`'
check 'bt-bad-paren'    'echo `echo )`'
# line base: backtick on script line 3
check_script 'bt-script-line3' 'echo a' 'echo b' 'echo `case x in`'
check_script 'bt-script-near'  'echo a' 'echo b' 'echo `esac`'
# control: $() stays -c: (no marker)
check 'ds-control-case'  'echo $(case x in)'
check 'ds-control-esac'  'echo $(esac)'
if [ $FAIL -ne 0 ]; then echo "comsub_marker_diag_diff_check FAILED" >&2; exit 1; fi
echo "comsub_marker_diag_diff_check OK"
```
`chmod +x`. Run: `cargo build -p huck --bin huck && bash tests/scripts/comsub_marker_diag_diff_check.sh` — all PASS.

- [ ] **Step 2: Full sweep.** `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (green); build both debug+release binaries; `ulimit -v 1500000; timeout 900 bash tests/scripts/run_diff_checks.sh 2>&1 | tail -6` (green — the new harness auto-included; `coproc_diff_check.sh` is a KNOWN pre-existing flake, if it's the only failure re-run it in isolation to confirm and note it, NOT a regression). Confirm `backtick_escape`/`cmdsub_comment`/`syntax_error_diag`/`eval_line_diag` all green.
- [ ] **Step 3: Integration binaries.** Run any `-p huck` integration binary that asserts backtick or syntax-error output at `--test-threads 2` (grep `tests/*.rs` for `backtick|command substitution|syntax error`). Report each + result.
- [ ] **Step 4: File the follow-on issue** for the recover-vs-abort divergence: `gh issue create` (labels `divergence`, `bug`, `sev:low`), titled about huck aborting on a backtick command-substitution syntax error where bash recovers (empty substitution, continues the outer command, rc 0). Body: bash defers backtick body parsing to expansion time, so a backtick body syntax error is a *runtime, per-command-recoverable* error (stderr diagnostic + empty substitution + continue, rc 0); huck parses the backtick body up-front, so it is a *fatal parse error* aborting the `-c` string (rc 2). #213 (v316) aligned the stderr marker text; this tracks the stdout/rc behavior. Record the issue number.
- [ ] **Step 5: Baseline doc.** No category flips (posix2 already PASS). Add a one-line provenance note that v316 (#213) closed the backtick `command substitution:` marker gap; do NOT change any PASS count.
- [ ] **Step 6: fmt + commit** (`v316 task 3: comsub_marker harness + sweep + close #213`).

---

## Notes for the executor

- Task 1 is a pure refactor — its success criterion is BYTE-NEUTRAL output (the two guard harnesses green). Do not add any `InCommandSub` handling in Task 1.
- `$()` must stay `-c:` — the `InCommandSub` wrapper is ONLY in `parse_backtick_sub`, never `parse_command_sub`. If a `$()` case regresses to `command substitution:`, the wrap leaked into the wrong path.
- The near-token echo is the key visible payoff: bash echoes the backtick BODY; confirm huck does too (the recursion passes `source = body`).
