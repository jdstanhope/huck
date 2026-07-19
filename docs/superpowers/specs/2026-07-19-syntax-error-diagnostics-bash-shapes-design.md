# v314 — align top-level syntax-error diagnostics to bash's 3-shape model

**Issue:** [#211](https://github.com/jdstanhope/huck/issues/211) — huck's
parser/lexer syntax-error messages diverge from bash 5.2.21 across a wide range
of inputs. Motivated by the posix2 near-miss [#209](https://github.com/jdstanhope/huck/issues/209),
which this architecture will resolve in the *next* phase (v315).

**Goal (v314):** replace huck's single ad-hoc syntax-error message shape with
bash's three canonical shapes for the **top-level** paths (`-c`, script file,
REPL) — including naming the offending token, echoing the source line, and
matching bash's line-number semantics — via a reusable `expect_next_kind`
cursor helper and a central renderer. This is the architecture the nested-context
phase (v315) builds on.

---

## The measured bash model (bash 5.2.21)

A curated probe of ~30 syntax-error inputs shows bash collapses to exactly
**three** diagnostic shapes. huck today emits a single fourth shape of its own
(`<name>: line N: syntax error: <descriptive>`), with no token name, no
source-line echo, its own delimiter wording, and — for `eval`/comsub — the wrong
line and no context marker.

### Shape 1 — "near unexpected token" (a token is present but misplaced)

Two lines: the diagnostic, then an echo of the offending source line.

```
-c: line 1: syntax error near unexpected token `)'
-c: line 1: `echo )'
```

| input (`-c`) | bash names token | huck today |
|---|---|---|
| `echo )` | `` `)' `` | `syntax error: unexpected token after command` |
| `echo a ;; echo b` | `` `;;' `` | `syntax error: expected a command` |
| `done` | `` `done' `` | `syntax error: unexpected 'done'` |
| `esac` | `` `esac' `` | `syntax error: unexpected 'esac'` |
| `fi` / `then echo x` | `` `fi' `` / `` `then' `` | `unexpected 'fi'` / `unexpected 'then'` |
| `case esac in esac) ;; esac` | `` `)' `` | `unexpected token after command` |
| `& echo x` / `\| echo x` | `` `&' `` / `` `\|' `` | `expected a command` |
| `echo <>` | `` `newline' `` | `expected a filename after redirection` |
| `for x in ; do :; done; in` | `` `in' `` | `unexpected 'in'` |
| `do echo x` | `` `do' `` | `unexpected 'do'` |

The named token is the **actual** token found where the grammar expected
something else. Where end-of-line/EOF is what was found in word position, bash
spells it `newline` (e.g. `echo <>`).

### Shape 2 — "unexpected end of file" (EOF while a keyword/paren construct is open)

One line, **no** source echo, reported at the **EOF line** (input line count + 1;
line 2 for a one-line `-c`).

```
-c: line 2: syntax error: unexpected end of file
```

Triggers: unterminated `(` (subshell), `{` (brace group), `if`, `then`, `case`,
`for`, `while`. huck today: its own `unterminated 'if' (expected 'then'/'fi')`
etc. at line 1.

### Shape 3 — "unexpected EOF while looking for matching `X'" (EOF inside a quote/delimiter)

One line, **no** `syntax error:` prefix, **no** echo.

```
-c: line 1: unexpected EOF while looking for matching `"'
```

| input | bash delimiter `X` | line | huck today |
|---|---|---|---|
| `echo "hi` | `` `"' `` | 1 | `unterminated quote` |
| `echo 'hi` | `` `'' `` | 1 | `unterminated quote` |
| `` echo `foo `` | `` `` ` `` | 1 | `unterminated '(' (expected matching ')')` |
| `echo $(foo` | `` `)' `` | 2 | `unterminated '(' …` |
| `echo $((1+` | `` `)' `` | 1 | `unterminated arithmetic expansion` |
| `echo ${x` | `` `}' `` | 1 | `unterminated '${...}'` |
| `[[ -n x` | `` `]]' `` then Shape 2 | 1 then 2 | `unterminated '[[ ]]' (missing ']]')` |

Note the line-number quirk: a quote/`$((`/`${` reports line 1 (the delimiter's
line); a `$(` command-substitution reports line 2 (the EOF line). `[[ -n x`
emits Shape 3 **and** Shape 2 on two lines.

### Context markers (deferred to v315)

For nested errors bash swaps the `-c:` source-name segment for `eval:` or
`command substitution:` and adjusts the line number (e.g. `eval: line 199:`).
These are **out of scope for v314** and are the missing piece for #209 (posix2).

---

## Scope & phasing

- **v314 (this spec) — top-level shapes.** Full bash-alignment of the three
  shapes for the `-c`/script/REPL paths, decision **(A)**: huck emits bash-exact
  syntax errors in **all** modes (interactive included); huck's descriptive
  wording is retired. Establishes `expect_next_kind`, the structured error, and
  the renderer.
- **v315 (next) — nested context markers.** `eval:` / `command substitution:`
  prefixes and their line numbers. Resolves #209 (posix2), which stays open,
  re-scoped to this phase.

**posix2 does NOT flip in v314** — it fires inside `eval`. v314's payoff is
measured by re-sweeping the categories whose errors are top-level; the sweep
reports the actual movement (no flip count is pre-promised).

### Non-goals (v314)

- Arithmetic-**expression** errors (`syntax error in expression`, `operand
  expected`, `error token is "…"`) — a separate subsystem already aligned in
  v215/v216. Untouched.
- `test`/`[` builtin syntax errors (`[: integer expression expected`).
- Nested context markers (`eval:`, `command substitution:`) — v315.
- The `[[ -n x` two-line (Shape 3 + Shape 2) combination — render the primary
  Shape 3 line; the exact double-line match may defer to v315 if it resists.

---

## Architecture

Two cooperating layers. The parser captures *what was expected vs. found*; the
renderer turns that into bash's shapes. THE RULE is preserved: the lexer remains
an atom source, the parser owns which tokens are expected.

### Front layer (huck-syntax) — structured capture

A cursor helper alongside `peek_kind`/`next_kind` on the lexer-as-token-cursor
(`lexer.rs:~5929`):

```rust
/// Peek the NEXT atom; if its kind is in `expected`, consume and return it,
/// else fail with the classifier tuple. Single-atom only — never scans ahead
/// (the name is the guardrail against drift into a forward scanner).
pub fn expect_next_kind(&mut self, expected: &[TokenKind]) -> Result<Token, ExpectFailure>;
```

```rust
pub struct ExpectFailure {
    pub found: Found,             // what was actually there
    pub matching: Option<Delim>,  // an open delimiter/construct, if any (drives Shape 2 vs 3)
    pub pos: usize,               // byte offset, for line-number computation
}
pub enum Found { Token(TokenKind), Eof }
pub enum Delim { Paren, Brace, DQuote, SQuote, Backtick, DollarParen, DollarDParen, DollarBrace, DBracket }
```

`ParseError` gains one structured variant carrying it:

```rust
ParseError::Unexpected(ExpectFailure)
```

`expect_next_kind` populates `found` (the peeked atom or `Eof`) and `pos`; it
leaves `matching: None`. The `matching` delimiter is context the **parser** holds,
so the ~dozen migrated call sites set it only where the expectation lives inside
an open construct (rare in v314 — most Shape-1 failures are `found: Token(..)`
with `matching: None`). Shapes 2 and 3 come predominantly from the existing
`Unterminated*` (parser) and `Lex(Unterminated*)` (lexer) variants, which already
know their construct/delimiter — so `matching` is a forward-looking hook for
v315, not load-bearing for v314's Shape-1 work.

**Migration scope (v314):** convert only the **Shape-1-producing** `return
Err(...)` sites — the misplaced-token cases (`UnexpectedToken`,
`UnexpectedKeyword`, `MissingCommand` at a `;;`/`&`/`|`, `MissingRedirectTarget`,
`UnexpectedBackground`, `ForVariable` where a token is present). There are ~43
`Err(ParseError::…)` sites of these kinds; only the subset that fires with a
concrete offending token migrates to `expect_next_kind` / `ParseError::Unexpected`.
The `Unterminated*` variants (Shape 2) and `Lex(Unterminated*)` variants
(Shape 3) already carry what their shapes need — they stay as-is and only render
differently. The other ~110 `peek_kind` sites are **not** touched.

### Back layer (huck-engine) — the renderer

A single classifier maps a parse/lex error to a shape and emits it. It replaces
the `format_args!("syntax error: {e}")` call at `shell.rs:~478` (and the
equivalent comsub/eval sites, though those keep the `-c` marker in v314).

Classification:

| huck error | shape | rendered |
|---|---|---|
| `Unexpected{found: Token(t), ..}` | 1 | `syntax error near unexpected token \`SPELL(t)'` + echo line |
| `Unexpected{found: Eof, matching: Some(d)}` where `d` is a quote/`$(`/`$((`/`${`/`` ` `` | 3 | `unexpected EOF while looking for matching \`SPELL_DELIM(d)'` |
| `Unexpected{found: Eof, matching: Some(Paren\|Brace)}` or `Unterminated{If,Loop,Case,Subshell,Brace}` | 2 | `syntax error: unexpected end of file` (EOF line) |
| `Lex(Unterminated{Quote,Substitution,Arith,Brace,LegacyArith,ArithBlock})` | 3 | `unexpected EOF while looking for matching \`X'` |
| `UnterminatedDoubleBracket` | 3 (primary) | `unexpected EOF while looking for \`]]'` (Shape 2 second line deferred) |

`SPELL(t)` maps `TokenKind` → bash spelling:
`RParen`→`)`, `LParen`→`(`, `Op(SemiSemi)`→`;;`, `Op(Amp)`→`&`, `Op(Pipe)`→`|`,
reserved words→their literal (`done`, `esac`, `fi`, `then`, `in`, `do`, …),
`Newline`/EOF-in-word-position→`newline`. `SPELL_DELIM(Delim)` maps
`DQuote`→`"`, `SQuote`→`'`, `Backtick`→`` ` ``, `DollarParen`→`)`,
`DollarDParen`→`)`, `DollarBrace`→`}`, `Paren`→`)`.

`emit_syntax_error` grows to optionally emit the **second echo line**
(`` <prefix> `<source line>' ``) for Shape 1, and a `shape`/`no-"syntax error:"-prefix`
flag for Shape 3. The source line for the echo is the input line containing the
error position (already available at the call site as the `line` argument);
the echo reproduces that logical line verbatim, wrapped in `` `…' ``.

### Line-number semantics (v314, top-level)

- **Shape 1:** the line of the offending token (huck already computes this by
  counting newlines to the cursor; reuse it via `ExpectFailure.pos`).
- **Shape 2:** the **EOF line** = (count of `\n` in the whole input) + 1.
- **Shape 3:** delimiter-dependent — quote/`$((`/`${` report the delimiter's
  line; `$(` reports the EOF line. Encode per-`Delim` in the renderer.

These are pinned against the diff harness; a stubborn per-shape quirk (or the
backtick delimiter-spelling recovery below) may split to a follow-on issue
rather than block v314.

### Known fiddly bits (flagged, not blockers)

1. **Backtick spelling.** huck normalizes `` `…` `` to command-substitution
   internally, so a Shape-3 render must recover that the *original* delimiter was
   a backtick to print `` ` `` rather than `)`. If the `Delim` isn't threaded
   through the backtick lexer path cleanly, record it and defer that one spelling
   to v315.
2. **`[[ -n x` double line.** Emit the primary Shape-3 line; the trailing Shape-2
   line is deferred (see non-goals).

---

## Testing

- **New `tests/scripts/syntax_error_diag_diff_check.sh`** — the curated probe set
  (all three shapes: misplaced tokens, EOF-in-keyword-construct, EOF-in-delimiter),
  byte-diffing huck vs bash on stderr **and** rc, normalizing only each shell's own
  `<name>:` prefix. This is the gold-standard gate.
- **Update the ~4 huck unit-test files** that assert the old wording
  (`unexpected token after command`, `unterminated 'if' …`, `unterminated quote`,
  `unterminated '${'`) to the new bash-exact strings. Sweep `crates/` for any
  other assertion of the retired messages.
- **Sweep `run_diff_checks.sh`** for harnesses that pin huck's descriptive
  syntax-error wording (e.g. `error_message_diff_check.sh`) and update expected
  output. Build BOTH debug and release binaries first.
- **Re-sweep the affected bash-suite categories** (`parser`, `errors`, `comsub*`,
  `array`, and any others whose diff is syntax-error-dominated) and record actual
  movement in `docs/bash-test-suite-baseline.md`.

## Rejected alternatives

- **(B) Dual-mode** (bash-exact non-interactive, descriptive interactive):
  rejected in brainstorming — two rendering paths to keep in sync, and huck's
  identity is bash-compatibility; a single code path is less error-prone.
- **Mass-migrate all 125 `peek_kind` sites to `expect_next_kind`:** unnecessary
  churn and risk; only the ~dozen Shape-1-producing `Err`-return sites need it.
- **Enrich each `ParseError` variant individually** (per-variant token fields)
  instead of one `Unexpected(ExpectFailure)`: more variants, more match arms, and
  it scatters the classifier logic; the single structured variant centralizes it.
- **Do posix2's `eval:` marker now (one big iteration):** larger blast radius and
  couples the architecture to the nested-context plumbing; phasing lands the
  reusable core first and de-risks v315.
