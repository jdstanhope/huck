# huck v97 — redirections on compound commands Design

**Status:** approved design, ready for implementation plan.
**Implements:** redirections attached to compound commands — `while … done <REDIR`,
`until/for … done <REDIR`, `if … fi <REDIR`, `case … esac <REDIR`,
`{ …; } <REDIR`, `( … ) <REDIR`, `select … done <REDIR`, C-style
`for ((;;)) … done <REDIR`, and `(( … )) <REDIR` — with every redirect type
(`<`, `<<` heredoc, `<<<` here-string, `>`, `>>`, `2>`, `2>>`, `&>`-style, fd-dup).
Today huck supports redirects only on **simple** commands; any redirect after a
compound terminator raises `syntax error: unexpected token after command`.
**Primary driver:** `~/.nvm/nvm.sh` line 567 — `done <<EOF\n$lines\nEOF` (a heredoc
on a `while` loop's `done`). Because the whole script is wrapped in one `{ … }`
group, this currently surfaces as the misleading `nvm.sh: line 11` error and
aborts all of nvm.
**Closes:** a new Tier-2 entry (compound-command redirections) `[fixed v97]`.
**Branch (impl):** `v97-compound-redirects`.

## Root cause (verified)

`while read x; do echo $x; done <<EOF\na\nEOF` → huck `unexpected token after
command`; bash → `a`. Confirmed for `if/fi`, `for/done`, `case/esac`, `{ }`,
`( )` too — huck supports redirects on **none** of them. The `Redirect` enum
(`Read`/`Truncate`/`Append`/`Heredoc`/`HereString`/`Dup`) and heredoc lexing
(`Token::Heredoc { body, expand, strip_tabs }`) are already complete and used by
simple commands; the gap is (1) the parser doesn't consume redirects after a
compound terminator and (2) there's no AST slot / executor arm to apply them.

## Section 1 — AST (`src/command.rs`)

Add one wrapper variant to `Command`:
```rust
Redirected {
    inner: Box<Command>,
    stdin: Option<Redirect>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
},
```
This mirrors `SimpleCommand`'s existing 3-slot, last-wins redirect model (which
already covers `<`/`<<`/`<<<` → stdin, `>`/`>>` → stdout, `2>`/`2>>` → stderr,
and `>&`/`2>&` → `Dup`). No new redirect types are introduced. The wrapper
composes: `inner` is the compound `Command` (`While`/`If`/`For`/`Case`/
`BraceGroup`/`Subshell`/`Select`/`ArithFor`/`Arith`).

## Section 2 — Parser (`src/command.rs`)

1. **Factor a reusable helper** from `parse_simple_stage`'s redirect loop
   (`src/command.rs:~1640-1665`): the arm handling `Token::Heredoc` and the
   `Token::Op(redir-op)` → target → `stdin/stdout/stderr` assignment. Extract
   `fn parse_trailing_redirects(iter) -> Result<RedirSlots, ParseError>` where
   `RedirSlots = (Option<Redirect>, Option<Redirect>, Option<Redirect>, bool)`
   (the three slots + a `saw_any` flag). It loops while the next token is a
   redirect operator or `Token::Heredoc`, applying the same last-wins logic, and
   stops at any non-redirect token. `parse_simple_stage` is refactored to use it
   (behavior byte-unchanged for simple commands).
2. **Wrap compounds**: in `parse_command_inner` (and the brace-group / subshell /
   arith dispatch sites), after a compound `Command` is parsed and BEFORE
   returning it, call `parse_trailing_redirects`. If `saw_any`, return
   `Command::Redirected { inner: Box::new(compound), stdin, stdout, stderr }`;
   otherwise return the compound unchanged. Apply this uniformly so every
   compound kind is covered by one code path.
   - The natural single chokepoint: wherever `parse_command_inner` returns a
     compound. If compounds return from several arms, wrap at the end (collect
     the parsed `Command`, then run the redirect-consume+wrap step once).
   - Pipelines: a redirected compound can be a pipeline stage
     (`{ …; } | cmd`) — confirm the wrap happens at the compound level so the
     existing pipeline parsing still sees a single `Command`. (Redirects bind to
     the compound, then the pipe; matches bash.)

## Section 3 — Executor (`src/executor.rs`)

Add a `Command::Redirected { inner, stdin, stdout, stderr }` arm to `run_command`.
It applies the redirects at the **real fd level** (0/1/2) around the inner
command's execution, reusing the existing dup2 save/restore pattern
(`BuiltinStdinGuard`, `src/executor.rs:1819`, and `ResolvedRedirect`/
`open_resolved`/heredoc pipe helpers):

1. Build a `CompoundRedirectScope` guard that, for each present slot, resolves
   and applies the redirect by `dup2`-ing onto the target fd and saving the
   original:
   - `stdin` `<file` → open read, dup2 → fd 0; `<<heredoc`/`<<<` → write the
     expanded body to a pipe, dup2 read end → fd 0 (reuse the existing heredoc
     stdin helper at `src/executor.rs:~1838`).
   - `stdout` `>`/`>>` → open (trunc/append), dup2 → fd 1; `>&N` → dup2 N → fd 1.
   - `stderr` `2>`/`2>>` → open, dup2 → fd 2; `2>&N` → dup2 N → fd 2.
   - Save each replaced fd; `impl Drop` restores via `dup2(saved, fd)` + close.
2. **Flush `io::stdout()` before applying and before restoring** so Rust's
   buffered stdout (used by `StdoutSink::Terminal` and builtins) lands in the
   right place across the fd swap.
3. Execute `inner` with the existing `sink`. In `StdoutSink::Terminal` mode all
   output goes to fd 1 (and external children inherit 0/1/2), so the fd-level
   redirect transparently applies to every nested command — matching bash.
4. The guard drops after the inner command returns (or on early return /
   `?`-propagation), restoring fds. The inner command's exit status is the
   `Redirected` command's status.

**Edge — `StdoutSink::Capture`** (a redirected compound inside `$(…)`): the
capture pipe is fd-based too; a `>file` inside the captured compound correctly
diverts that output to the file (so it is NOT captured), matching bash. The
implementer verifies capture + redirect interaction with a test
(`x=$({ echo a; echo b >/tmp/f; }); echo "[$x]"` → `[a]`, file has `b`).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/command.rs` | `Command::Redirected` variant; `parse_trailing_redirects` helper (factored from `parse_simple_stage`); wrap each compound when trailing redirects follow |
| `src/executor.rs` | `Redirected` arm; `CompoundRedirectScope` guard (dup2 save/restore for 0/1/2, heredoc/file/dup), `io::stdout()` flush; reuse `ResolvedRedirect`/heredoc helpers |
| `tests/compound_redirects_integration.rs` | NEW — redirects on every compound kind + heredoc-on-done |
| `tests/scripts/compound_redirects_diff_check.sh` | NEW — 22nd bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | new Tier-2 entry `[fixed v97]`; changelog; README row |

## Testing

1. **Integration** (`tests/compound_redirects_integration.rs`):
   - `while read x; do echo "g:$x"; done <<EOF\na\nb\nEOF` → `g:a\ng:b`.
   - `for i in 1 2; do echo $i; done >FILE` then read FILE → `1\n2`.
   - `if true; then echo hi; fi >FILE` → FILE has `hi`.
   - `{ echo a; echo b; } >FILE` → `a\nb`; `( echo x ) >FILE` → `x`.
   - `case z in z) echo m;; esac >FILE` → `m`.
   - append `>>`, here-string `<<<`, `2>FILE` on a compound, fd-dup `2>&1`.
   - capture + redirect edge (above).
   - **no-redirect regression**: a bare `while/if/for/{}/case` is unchanged
     (still the plain compound `Command`, not wrapped).
2. **bash-diff harness** `tests/scripts/compound_redirects_diff_check.sh` (22nd):
   the forms above producing deterministic stdout (use `cat FILE` after, or
   read-from-heredoc loops) byte-identical to bash 5.2.
3. **Regression**: all existing simple-command redirect tests pass (the factored
   `parse_trailing_redirects` must be behavior-identical); pipelines, `set -e`,
   subshell, function tests unaffected.
4. **End-to-end**: a temp script `while read x; do echo $x; done <<EOF…EOF`
   sourced via `huck SCRIPT` works; and (manual, noted in changelog) re-bisecting
   `nvm.sh` shows it now parses past line 567 — surfacing the NEXT gap if any.

## Edge cases & notes

- **Multiple redirects / order**: last-wins per fd (same as simple commands).
  `{ …; } >a >b` → `b` wins (bash opens both but the last dup2 wins for the fd);
  acceptable and matches the simple-command model.
- **Heredoc body already lexed**: `Token::Heredoc` carries the expanded-or-literal
  body; the parser just attaches it — no new lexer work, and `continuation`
  already knows to collect a heredoc body after `done`/`fi`/etc. (the lexer is
  position-agnostic).
- **`set -e` / exit status**: the `Redirected` command's status is the inner
  command's status; redirect-open failure (e.g. `>/nonexistent/dir/f`) prints
  `huck: <file>: <err>` and yields status 1 without running the inner command
  (matching bash, mirroring the simple-command failure path).
- **No regression surface**: a compound with no trailing redirect is parsed and
  executed exactly as before; `Redirected` is only constructed when `saw_any`.
- **Out of scope**: arbitrary `N>file` for N∉{0,1,2} (huck's 3-slot model already
  doesn't support general fds on simple commands either — same limitation,
  unchanged); process substitution `<(…)` (separate gap).
