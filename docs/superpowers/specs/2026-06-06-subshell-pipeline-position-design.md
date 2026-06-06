# huck v100 — subshell/compound-headed pipeline in any sequence position (M-11a) Design

**Status:** approved design, ready for implementation plan.
**Implements:** a pipeline whose FIRST stage is a subshell or compound command
(`( … ) | cmd`, `{ …; } | cmd`, `if…fi | cmd`, `for…done | cmd`, etc.) now parses
in ANY sequence position — after `;`, `&&`, `||`, `&`, or a newline inside a
compound/function body — not just as the first command of a line. Today it parses
ONLY in first position; elsewhere the `|` after the `)`/`}`/`fi` is left
unconsumed → `syntax error: unexpected token after command`.
**Closes:** **M-11a** (`[deferred]` → `[fixed v100]`).
**Primary driver:** `~/.nvm/nvm.sh`'s `nvm_list_aliases` — a function body where
`( for … done; wait ) | command sort` follows other statements (so it's a
non-first, newline-separated sequence element → routed through the rest-of-
sequence parser). This is the last identified parse blocker for that function.
**Branch (impl):** `v100-subshell-pipeline-position`.

## Root cause (verified)

`parse_sequence` (`src/command.rs:585`) parses the FIRST command then wraps it in a
pipeline when a `|` immediately follows:
```rust
let raw_first = parse_command(iter)?;
let first = if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
    // build Command::Pipeline { stages: [raw_first, …] }
} else { raw_first };
```
This is why `( echo a ) | sort` works in first position. But the REST elements —
the `Semi`/`And`/`Or`/`Amp` arms of the loop (`:691,707,711,715`) — call
`parse_command(iter)?` DIRECTLY, with no pipeline wrap. So a subshell/compound-
headed pipeline after a connector leaves the `|` unconsumed and the loop's
`other =>` arm errors `UnexpectedToken`. `parse_subshell_sequence`
(`src/command.rs:1435`) has the identical first-position wrap and the same
unwrapped rest connectors.

Verified minimal cases: `( echo a ) | sort` ✓ (first); `echo z; ( echo a ) | sort`
✗ (after `;`); `true && ( echo a ) | sort` ✗ (after `&&`). Inside a function body
the whole body is one sequence whose newline-separated statements are `Semi` rest
elements, so nvm's construct hits the gap.

## Section 1 — Factor a `parse_command_then_pipeline` helper (`src/command.rs`)

Extract the first-position wrap block into one helper used everywhere:
```rust
/// Parses a command and, if a `|` immediately follows, assembles the full
/// pipeline (the command becomes the first stage). For a simple command,
/// `parse_command`/`parse_pipeline_with_first` already consume the pipeline;
/// this helper adds the wrap for a COMPOUND/subshell first stage (which
/// `parse_command` returns without checking for a trailing `|`).
fn parse_command_then_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    let raw = parse_command(iter)?;
    if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
        let mut stages = vec![raw];
        iter.next(); // consume `|`
        skip_newlines(iter);
        let mut more = true;
        while more {
            let (cmd, next_pipe) = parse_next_stage(iter)?;
            stages.push(cmd);
            if next_pipe {
                // simple stage consumed its own `|`
            } else if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                iter.next();
                skip_newlines(iter);
            } else {
                more = false;
            }
        }
        Ok(Command::Pipeline(Pipeline { negate: false, commands: stages }))
    } else {
        Ok(raw)
    }
}
```
This is the EXACT logic currently inlined at `parse_sequence:585-628` and
`parse_subshell_sequence:1442-1462` — moved into a shared fn.

## Section 2 — Apply the helper to every sequence-element position

- **`parse_sequence`**: replace `let raw_first = …; let first = if … { … } else { … }`
  with `let first = parse_command_then_pipeline(iter)?;`. In the loop, replace each
  `parse_command(iter)?` in the connector arms — `Semi` (`:707`), `And` (`:711`),
  `Or` (`:715`), and `Amp` (`:691`, the v98 `&`-separator) — with
  `parse_command_then_pipeline(iter)?`.
- **`parse_subshell_sequence`**: replace its first-position block with
  `let first = parse_command_then_pipeline(iter)?;`, and replace the
  `parse_command(iter)?` in its rest connector arms (Semi/And/Or, and its `&`
  arm — which currently hand-rolls pipeline assembly for the next command; that
  hand-rolled block can be simplified to the helper) with the helper.
- Any OTHER sequence loop that parses rest elements via `parse_command` (grep
  `parse_command(iter)?` and audit each call site): apply the helper where the
  call parses a sequence ELEMENT that may be a compound-headed pipeline. (Do NOT
  change call sites that parse a single non-sequence command, e.g. a function
  body shape, an if-condition single command, etc. — only the
  list/sequence-element positions.)

Because the helper returns `raw` unchanged when no `|` follows, every existing
non-pipeline sequence (`a; b`, `a && b`, `a & b`) and every simple-command
pipeline (`a | b`, already handled inside `parse_command`) is byte-identical —
the only behavioral change is that a COMPOUND/subshell first stage now forms a
pipeline in non-first positions too.

## Section 3 — No AST / executor change

A `Command::Pipeline` whose first stage is a `Command::Subshell`/compound already
executes correctly (proven by the working first-position case — `( echo a ) |
sort` runs fine today). v100 only changes PARSING, so no executor or AST change is
needed. Negation (`! ( a ) | b`) flows through `parse_command`'s existing
bang-handling unchanged.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/command.rs` | new `parse_command_then_pipeline` helper; apply at the first + all rest-connector positions in `parse_sequence` and `parse_subshell_sequence` (and any other audited sequence-element call site) |
| `tests/subshell_pipeline_position_integration.rs` | NEW — `( ) | cmd` after `;`/`&&`/`||`/`&`, inside function/loop/if/brace bodies; compound-headed (`{ } | cmd`, `if…fi | cmd`) after a connector; the nvm-shaped function |
| `tests/scripts/subshell_pipeline_position_diff_check.sh` | NEW — 25th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-11a `[fixed v100]`; changelog; README row |

## Testing

1. **Parser unit tests**: `echo z; ( echo a ) | cat` parses to a `Sequence` whose
   `rest[0]` is a `Command::Pipeline` with a `Subshell` first stage; `x && { echo
   a; } | cat` likewise; a non-pipeline rest element (`a; b`) is unchanged (not a
   pipeline).
2. **Integration** (`tests/subshell_pipeline_position_integration.rs`) — verify
   stdout vs bash:
   - `echo z; ( echo a ) | sort` → `z\na`.
   - `true && ( printf 'b\\na\\n' ) | sort` → `a\nb`.
   - `false || ( echo x ) | cat` → `x`.
   - `( echo a ) | cat & wait; ( echo b ) | cat` (Amp/sequence mix).
   - inside a function: `f() { echo z; ( echo a ) | sort; }; f` → `z\na`.
   - inside a for body: `for i in 1; do ( echo $i ) | cat; done`.
   - compound-headed: `echo z; { echo a; echo b; } | sort` → `z\na\nb`;
     `echo z; if true; then echo a; fi | cat` → `z\na`.
   - the nvm shape: a function with `local X; ( for n in a b; do echo $n & done;
     wait ) | sort` → parses and runs.
   - regression: `a; b`, `a && b`, `a | b`, `( a ) | b` (first) all unchanged.
3. **bash-diff harness** `tests/scripts/subshell_pipeline_position_diff_check.sh`
   (25th): the deterministic forms above, byte-identical to bash 5.2.
4. **Regression**: full suite — especially pipeline, subshell, sequence, `&`/v98,
   function, and the existing pipefail harness (M-50, where M-11a was discovered).
5. **End-to-end**: re-bisect `nvm.sh` — `nvm_list_aliases` (de-wrapped line 1192)
   now parses; report the next gap (if any).

## Edge cases & notes

- **Negated compound-headed pipeline** (`! ( a ) | b`): `parse_command` strips the
  `!` and returns the inner; the helper then wraps in a pipeline; the negation is
  applied to the pipeline — confirm it matches bash (`! ( false ) | cat; echo $?`).
- **Trailing redirects on a compound stage** (`( a ) >f | b` — unusual): out of
  scope; the helper only handles the `|` chaining (redirects on compound commands
  are v97's `maybe_wrap_redirects`, applied inside `parse_command`).
- **No regression surface**: the helper is a strict superset of the prior
  first-position block; non-`|` elements return unchanged.
