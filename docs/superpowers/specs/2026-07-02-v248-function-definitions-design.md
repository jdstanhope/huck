# v248 — Function definitions on the atom-command path (dormant, differential) — Design

**Status: APPROVED (2026-07-02).** First of the Phase-C **Stage 2** "port a
deferred construct onto the atom-command path" iterations. Direction:
`2026-06-30-phase-c-parser-driven-frontend-roadmap.md` (Stage 2) + memory
`huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`.

## 1. Goal & context

v247 gave `Mode::Command` a dormant atom-emitting scanner (`command_atoms`
flag, default `false`) and made `parser.rs` assemble command-position ASTs from
atoms, byte-identical to the production `command.rs` oracle, gated by an
old-vs-new differential harness (`new_seq` atoms vs `old_seq` oracle). The atom
path covers simple commands, pipelines, and-or lists, redirects, and the basic
compounds (if/while/until/for/select/case/subshell/brace), but **defers ~10
construct families** to `UnsupportedCommand`/`UnsupportedExpansion`.

The end-goal (a future finale iteration) is to **flip `command_atoms` live** in
production and **delete the forward-scanning production scanners**. That flip is
gated on the atom path handling the *entire* grammar with a green differential
gate — so between here and the flip, each deferred family is ported onto the
atom path one iteration at a time, dormant + differential (exactly as v240–v247
were).

**v248 ports function definitions** — the highest-impact deferred family that
needs **no new lexer mode** (the atoms are already emitted; the work is purely
parser-side recognition + AST assembly). It is the model iteration for the
deferred-construct porting pattern.

## 2. Scope

**In scope.** Both funcdef forms, producing the same
`Command::FunctionDef { name: String, body: Box<Command> }` as the oracle:

- POSIX `name() compound-command` (with or without whitespace: `f(){…}`,
  `f() {…}`, `f ()  {…}`).
- bash `function NAME [()] compound-command` (`function f {…}`,
  `function f() {…}`).

Body coverage is **whatever the atom-path `parse_command` already handles**:
brace group, subshell, `if`/`while`/`until`/`for`/`select`/`case`, and a
redirected wrapping of any of those (`f() { :; } >log`). Trailing redirects on
the definition attach via the body's own `maybe_wrap_redirects`.

**Out of scope / pinned-deferred.**

- Funcdefs whose body is *itself* a still-deferred construct — `f() [[ x ]]`,
  `f() (( 1 ))`, `f() for ((;;)); do :; done`. Their body parse returns
  `UnsupportedCommand`, so the whole funcdef defers cleanly. These are **pinned**
  as known-deferred (asserted to return `UnsupportedCommand`) and lift
  automatically when `[[ ]]` / arith-command land in later iterations. v248 does
  NOT force them green.
- **No live flip.** `command_atoms` stays `false` in production; v248 changes
  only the dormant atom path + tests. Production (`command.rs`,
  `scan_step_command`, `process_line`) is untouched.
- No new lexer mode, no lexer changes at all (the funcdef atoms — `Lit(name)`,
  `Op(LParen)`, `Op(RParen)`, `Blank`, and the compound-body atoms — are already
  emitted by the v247 scanner).

## 3. Oracle reference (the spec is "match this")

`command.rs` is the source of truth (`old_seq`). The relevant oracle functions:

- **Dispatch** (`parse_command_inner`, command.rs ~1054–1141):
  - `Some(Keyword::Function) => parse_function_keyword_def(iter)`.
  - In the non-keyword branch: peek a `Word`, **consume it**, and if the next
    token is `Op(LParen)` → `parse_function_def(w, iter)`. (The Word-lexer
    consumes inter-token whitespace, so `f()` and `f ()` both reach this check
    with `Op(LParen)` next — the spaced form IS a funcdef.)
- **`parse_function_def(name_word, iter)`** (command.rs:1190): validate the name
  via `valid_function_name_text` (→ `FunctionName`); consume `(`; expect `)`
  (else `FunctionBody`); `finish_function_body`.
- **`parse_function_keyword_def(iter)`** (command.rs:1209): consume `function`;
  read the name Word and validate (→ `FunctionName`); optionally consume `()`
  (a `(` must be followed by `)`, else `FunctionBody`); `finish_function_body`.
- **`finish_function_body(name, iter)`** (command.rs:1176): `skip_newlines`; EOF
  → `UnterminatedFunction`; `body = parse_command(iter)`; require
  `is_function_body_shape(&body)` (→ `FunctionBody`); return
  `FunctionDef { name, body }`.
- **`is_function_body_shape`** (command.rs:1148): body must be
  `If/While/For/Select/Case/BraceGroup/Subshell/DoubleBracket/Arith/ArithFor`,
  or a `Redirected` recursively wrapping one.
- **`valid_function_name_text`** (command.rs:1354): the name Word must be a
  single valid identifier-shaped literal.

## 4. Design

Parser-only; all edits in `crates/huck-syntax/src/parser.rs` plus widening two
oracle validators to `pub(crate)` in `command.rs`.

### 4.1 Reuse the oracle's pure validators

Widen `valid_function_name_text` (command.rs:1354) and `is_function_body_shape`
(command.rs:1148) from private to `pub(crate)`, and call them from the atom
path (as v247 already reuses `command::next_is_redirect` /
`command::try_split_assignment`). They are pure (no lexer/parse-flow coupling),
so there is exactly one implementation of the name rule and the body-shape rule.
The parse *flow* helpers (`parse_function_def` / `parse_function_keyword_def` /
`finish_function_body`) are NOT reused — they call the oracle's `parse_command`;
the atom path needs versions that call the atom-path `parse_command` — so they
are reimplemented in `parser.rs` (small, mirroring the oracle control flow).

### 4.2 Detection in atom-path `parse_command` (parser.rs ~1529–1549)

- **`function` keyword.** In the `match peek_leading_keyword(iter)?` block, add
  `Some(Keyword::Function) => return parse_function_keyword_def(iter)` before the
  `Some(_) => Err(UnsupportedCommand)` catch-all. `peek_leading_keyword` already
  classifies a bare `function` (a lone `Lit("function")` followed by a
  word-boundary atom).

- **`name()` form, incl. the spaced variant.** Replace the current
  `name(`-deferral block. The atom stream has an explicit `Blank` between a
  spaced `f ()`'s name and `(`, and only `peek2` token-lookahead exists, so
  mirror the oracle's *consume-then-check* using the existing `mark`/`rewind`
  (parser.rs already uses them at ~1038/1055):

  1. `let m = iter.mark();`
  2. Peek: must be a `Lit { quoted: false, .. }` (or a legacy `Word`) — a funcdef
     name candidate. If not, skip funcdef detection.
  3. `let name_word = consume_command_word(iter)?;` then skip `Blank`s.
  4. If the next token is `Op(LParen)` → it IS a funcdef: hand `name_word` +
     `iter` to `parse_function_def` (which validates the name, consumes `()`,
     and parses the body).
  5. Else `iter.rewind(&m);` and fall through to `parse_simple` unchanged.

  Ordering: the `function` keyword arm and this `name()` detection sit exactly
  where the current deferral is — after the compound-keyword dispatch and after
  the bare-`(` / `((` / heredoc guards — so existing precedence is preserved.

### 4.3 The reimplemented flow helpers (parser.rs)

Mirror command.rs, calling the atom-path `parse_command` for the body and
skipping `Blank`s where the Word-lexer had none:

- `parse_function_def(name_word: Word, iter) -> Result<Command, ParseError>`:
  `valid_function_name_text(&name_word).ok_or(FunctionName)?`; skip `Blank`s;
  consume `(`; skip `Blank`s; expect `)` (else `FunctionBody`);
  `finish_function_body(name, iter)`.
- `parse_function_keyword_def(iter) -> Result<Command, ParseError>`: consume the
  `function` keyword word (`consume_command_word`); skip `Blank`s; read + validate
  the name word (non-Word/invalid → `FunctionName`); optionally consume `( )`
  (skipping `Blank`s; a `(` must be followed by `)`, else `FunctionBody`);
  `finish_function_body(name, iter)`.
- `finish_function_body(name, iter) -> Result<Command, ParseError>`:
  `skip_newlines`; EOF → `UnterminatedFunction`; `body = parse_command(iter)?`
  (atom path); `is_function_body_shape(&body)` else `FunctionBody`; return
  `Command::FunctionDef { name, body: Box::new(body) }`. A body that is itself a
  deferred construct makes `parse_command` return `UnsupportedCommand`, which
  propagates — the funcdef defers cleanly (pinned case).

## 5. Testing (differential gate)

All in `parser.rs` `mod tests`, using the existing `diff_cmd` (asserts
`new_seq == old_seq`) and the `new_seq`-only deferred assertion.

- **`atoms_function_defs`** (`diff_cmd`): `f(){ :; }`, `f() { :; }`,
  `f ()  { :; }` (spaced), `function f { :; }`, `function f() { :; }`; each
  supported body shape: `f() ( a; b )`, `f() if x; then y; fi`,
  `f() while x; do y; done`, `f() for i in a b; do echo $i; done`,
  `f() case $x in a) echo a;; esac`, `f() select x in a; do :; done`; redirected
  body `f() { :; } >log` and `f() { :; } 2>&1`; nested `f() { g() { :; }; }`;
  a funcdef inside a compound `if true; then f() { :; }; fi`.
- **`atoms_function_defs_errors`** (error parity via `new_seq`/`old_seq` compare):
  `f() echo` (non-compound body → `FunctionBody`), `function` (→ `FunctionName`),
  `function 1 { :; }` (invalid name → `FunctionName`), `f(` / `f()` (unterminated
  → `UnterminatedFunction`/`FunctionBody` as the oracle returns), `f ( a )`
  (a funcdef attempt whose `(` is not followed by `)` → `FunctionBody`, NOT a
  command `f` with a subshell arg — matching the oracle). Because the exact error
  variant is whatever the oracle returns, this test compares `new_seq` to
  `old_seq` rather than hard-coding variants.
- **`atoms_function_defs_deferred`** (`new_seq` → `UnsupportedCommand`):
  `f() [[ x ]]`, `f() (( 1 ))`, `f() for ((;;)); do :; done` — deferred body,
  pinned.
- **Non-funcdef parity** (guard against over-eager detection, via `diff_cmd`):
  `f` (bare word → plain command), `echo function` (`function` as an arg
  mid-command stays literal), `func --opt` (a name that is a prefix of
  `function` is a plain command — the `name()` mark/rewind must restore the
  stream when no `(` follows).
- Full `huck-syntax` lib green single-threaded
  (`cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`), doctests
  green, `cargo build -p huck-syntax` 0 warnings.

## 6. Non-goals / follow-ons

- The live flip + scanner deletion (the finale) — unchanged by v248.
- Other deferred families (heredoc bodies, `[[ ]]`, array literals, arith
  command, process substitution, here-strings, coproc, `$[ ]`) — future
  port iterations; funcdefs with those as bodies lift automatically as each lands.

## 7. Invariants

- Byte-identical: every in-scope funcdef input parses to the SAME AST on the atom
  path as the oracle (`diff_cmd`); a well-formed in-scope divergence is a v248
  bug to fix, not to pin.
- Production untouched: `command_atoms` defaults `false`; `command.rs` changes
  are ONLY the two `pub(crate)` visibility widenings (no logic change);
  `scan_step_command` / `process_line` unchanged.
- 0 warnings; every commit carries the `Co-Authored-By: Claude Opus 4.8 (1M
  context)` trailer; branch `v248-function-definitions`, not `main`.
