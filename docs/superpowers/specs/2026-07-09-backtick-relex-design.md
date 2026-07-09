# Backtick command substitution: capture → unescape → re-parse

**Status:** design (brainstormed 2026-07-09)
**Topic:** replace huck's streaming backtick lexer mode with bash's three-phase
capture-unescape-relex model. Retires **L-70** and flips the `iquote` bash-suite
category.

---

## 1. Problem

huck handles `` `…` `` with a single streaming lexer mode (`Mode::Backtick`) that
does three jobs inline as it walks the body: unescape backslashes, track nesting
with a `2^D−1` depth formula, and delegate body content to the quote-aware command
scanner. Flattening those three concerns into one pass produces two divergence
classes from bash 5.2:

**(a) L-70 — backslash-run collapse.** A contiguous run of ≥ 3 backslashes is
decoded incrementally (1–2 chars per scan step) instead of collapsed as a whole,
so surviving backslashes land in the wrong place. Verified with file-mode inputs
(exact byte counts, no `-c` wrapper layer):

| body (inside one backtick) | bash | huck |
|---|---|---|
| `echo \X`     | `X`  | `X`  |
| `echo \\X`    | `X`  | `X`  |
| `echo \\\X`   | `\X` | `X\`  ✗ |
| `echo \\\\X`  | `\X` | `X\`  ✗ |
| `echo \\\\\\X`| `\X` | `X\\` ✗ |
| `echo \\\\`   | `\`  | *syntax error: expected a command* ✗ |

This is the sole blocker for the `iquote` bash-suite category, whose residual is
`` eval tmp=`printf "$'\\\\\x%x'\n" 127` `` (a 5-backslash run inside a backtick).

**(b) Quote-blindness (found during design).** bash finds the closing backtick in
a raw, escape-only scan that **ignores quotes** — a `` ` `` inside `'…'` still
closes the substitution. huck delegates body content to the quote-aware scanner,
which opens quote spans and hides interior backticks:

```
echo `echo '`' hi`
# bash: error — "unexpected EOF while looking for matching '"
#       (closed at the ` after ', leaving the body `echo '` unterminated)
# huck: ` hi   (treated the quoted backtick as literal content)
```

Root cause is shared: the lexer attempts delimiter-matching, unescaping, and
nested recursion in one flattened pass, entangling three concerns and faking
recursion with arithmetic.

## 2. Design — three phases

Model bash's actual algorithm: **find the close (quote-blind), unescape once,
re-lex the result.** Backtick substitution becomes three sequential phases with a
clean lexer/parser handoff, replacing the flattened `Mode::Backtick`.

### Phase 1 — capture (lexer: dumb raw passthrough; parser: accumulate)

A new lexer mode `Mode::BacktickRaw` streams the body **verbatim**. Per scan step:

- on `\` → emit the `\` **and** the following char as raw text (both bytes pass
  through, so a `` \` `` cannot close and the backslashes survive for phase 2);
- on a bare, unescaped `` ` `` → emit the close atom (`EndBacktick`);
- otherwise → emit the char (or a run of ordinary chars) as raw text.

The mode is **quote-blind** and **`$()`-blind**: `'`, `"`, `(`, `#` are ordinary
raw bytes. Close-detection is purely char-level backslash escaping — an odd number
of backslashes before a `` ` `` escapes it, an even number closes. No depth
counter, no `2^D−1` formula, no delegation to the command scanner.

The **parser** (`parse_backtick_sub`) pushes `Mode::BacktickRaw`, pulls the raw
atoms, concatenates them into one raw body `String`, and stops at `EndBacktick`.

*Entry is unchanged:* command/dquote scanning still emits the `BeginBacktick`
signal when it steps onto a `` ` `` (cursor parked on the backtick), and the
outer dispatch (parser.rs:88, 274, 502, 595, 690) still calls `parse_backtick_sub`
with the correct `quoted` flag. Only what happens *after* the push changes.

### Phase 2 — unescape (pure parser-side function)

Apply bash's one-level rule to the captured string, left to right:

- `\\` → `\`
- `\$` → `$`
- `` \` `` → `` ` ``
- any other `\c` — including `\<newline>`, `\"`, `\'`, `\n`, `\ ` — kept **verbatim**
  (both bytes)
- a trailing lone `\` at end-of-body → kept

Only those three pairs remove the backslash; everything else is copied unchanged.
(Verified against bash: `` `echo \"hi\"` ``→`"hi"`, `` `echo a\<NL>b` ``→`ab`,
`` `echo a\ b` ``→`a b` — the survivors are handled by the phase-3 re-lex, not
phase 2.)

### Phase 3 — re-parse (parser recursion)

Feed the unescaped string to a **fresh** parse: build a new `Lexer` over the
string and run the command-sequence parser, yielding a `Sequence` that becomes
`WordPart::CommandSub { sequence, quoted }`.

- A nested backtick in the unescaped body is an ordinary backtick to the fresh
  parse, which recurses through phases 1–3. **Multi-level unescape falls out of
  the recursion** (exactly one level per nesting) — the `2^D−1` formula is deleted,
  not extended.
- A nested `$(…)` is parsed normally by the fresh parse (phase 1 captured it raw).
- A malformed body (e.g. `echo '` from a quote-blind early close) yields a parse
  error that propagates — matching bash's rejection of `` `echo '`'` `` (message
  text per §5).

## 3. Lexer/parser interaction

| Concern | Today | After |
|---|---|---|
| Find the close `` ` `` | lexer: `2^D−1` formula + depth counter | lexer: local `\`-escape rule → close atom |
| Unescape backslashes | lexer: inline, 1–2 chars/step | parser: phase-2 pure fn over whole string |
| Nesting | lexer fakes it (formula + re-emitted `BeginBacktick`) | parser recursion (phase-3 re-parse) |
| Quotes in body | lexer delegates to quote-aware scanner (hides backticks) | phase 1 quote-blind; phase 3 re-lex is quote-aware |
| Body → AST | parser stream-parses live | parser re-parses the cooked string |

The lexer gets strictly dumber; the parser owns delimiter-matching (the capture
loop) and recursion (the re-parse). This honors the binding rule — *"the lexer
emits small atoms and NEVER forward-scans for a matching delimiter; the parser
owns delimiter-matching/recursion"* — more cleanly than today.

This introduces one pattern the front end doesn't currently have: **re-parsing a
derived string with a fresh `Lexer`.** `$(…)` streams in place; backticks will
capture-then-reparse. That asymmetry *is* the semantic difference bash draws
between `$()` and backticks (the extra unescape level). It is **not** the deleted
oracle (a whole parallel fat lexer) — it is the same lexer+parser invoked
recursively on a cooked substring, standard recursive descent.

## 4. What gets deleted

- `scan_step_backtick`'s escape/nesting branches and the `2^D−1` / `2^(D−1)−1`
  formulas.
- `emit_backtick_delim`.
- The `depth` field on the backtick mode (→ the simpler `Mode::BacktickRaw`).
- The nested-recursion-without-push logic in `parse_backtick_sub` (nesting now via
  phase-3 re-parse, not a shared depth frame).
- `parse_backtick_body_sequence` (→ phase-3 `parse_sequence` on the cooked string).
- The `bt_malformed_divergence_deferred` lenient-accept test — the malformed
  inputs now reject like bash.

## 5. Non-goals / accepted divergences

- **Exact error-message text** for malformed bodies. huck emits its own
  error-prologue format; the requirement is that it *rejects* where bash rejects,
  not byte-identical stderr. (Pre-existing error-prologue naming divergence.)
- **Deeply undefined POSIX corners** — a backtick inside an embedded here-document
  inside a backtick, etc. Match bash where feasible; document any residual in
  `bash-divergences.md`.
- **Performance:** phase 3 allocates the cooked body and re-lexes it — O(body) per
  nesting level, O(total source) overall. Acceptable; bash does the same.

## 6. Success criteria

1. The three documented divergences in `backtick_escape_diff_check.sh` (the
   commented `\\\$X`, `\\\\`, `` \\\`lit\\\` `` cases) become **passing** and are
   promoted from comments to live `checkf` cases.
2. The `iquote` bash-suite category **flips to PASS**.
3. A new parity matrix passes byte-identically to bash: backslash runs 0–8 before
   {plain char, `$`, `` ` ``, closing backtick, EOF} × nesting depth 1–3 ×
   {unquoted, in `"…"`, in `'…'`}; the quote-in-backtick close cases; `$()`-in-backtick.
4. No regressions: `cargo test -p huck-syntax` and `-p huck-engine`, all existing
   `*backtick*` harnesses, and the full official bash-suite runner (no PASS→not-PASS).
5. **L-70 deleted** from `docs/bash-divergences.md`; the lenient-accept note retired.

## 7. Risks

- **Hot path.** Command substitution is common; a correctness/perf regression here
  is high-impact. Gate: the parity matrix + full official runner.
- **Span/line numbers.** The re-parsed body's spans are relative to the cooked
  substring. The current code zeroes line fields in backtick bodies
  (`zero_lines_in_sequence`); phase 3 must preserve that so any line-numbered
  output matches.
- **Entry from quoted / operand contexts.** Backticks are reachable unquoted,
  inside `"…"`, and inside `${…}` operand dquote spans (the existing
  BeginBacktick-signal wiring). Phase 1 must be entered correctly from all three,
  and the `quoted` flag must thread to `WordPart::CommandSub`.
- **New recursion pattern.** Reviewers should confirm it does not reintroduce
  oracle-style parallel machinery.

## 8. Appendix — reference facts (verified against bash 5.2, file mode)

- Close-detection: quote-blind, `$()`-blind; only `\` matters (odd run before
  `` ` `` escapes, even closes).
- Phase-2 unescape removes the backslash for exactly `\\`, `\$`, `` \` ``; all
  other `\c` kept.
- `` `echo '`' hi` `` → bash **errors** (quote-blind early close); huck currently
  prints `` ` hi `` — this design makes huck error too.
- `` `echo $(echo hi)` `` → `hi` (phase 1 captures `$(…)` raw, phase 3 parses it).
- Nested `` `echo \`echo inner\`` `` → `inner` (must remain correct).
