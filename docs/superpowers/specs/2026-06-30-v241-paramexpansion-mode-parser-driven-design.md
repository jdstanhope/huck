# v241 — ParamExpansion mode + parser-driven `${…}` assembly (design)

**Status: DESIGN (approved direction).** Date: 2026-06-30.

First genuinely-divergent lexer mode of the Phase C parser-driven front-end
(roadmap: `2026-06-30-phase-c-parser-driven-frontend-roadmap.md`; direction:
memory `huck-frontend-parser-driven-direction`). v241 builds, **comprehensively
and dormant**, the lexical + parser machinery for `${…}` parameter expansion done
the parser-driven way: the lexer emits small **atoms** under parser-pushed modes,
and a **new `crates/huck-syntax/src/parser.rs`** assembles `WordPart::ParamExpansion`
from those atoms. It is the **template** every later mode follows.

## Goal

Parse the FULL `${…}` grammar (every form and operator) from atom tokens, in a new
parser that drives the v240 mode stack — producing the **same** `WordPart::ParamExpansion`
AST the current lexer pre-builds, verified by **differential tests** against the
existing lexer. Production is untouched (the old `scan_braced_param_expansion` path
still runs); the new path is exercised only by tests.

## Hard rules (binding — see memory `huck-frontend-parser-driven-direction`)

- The lexer emits **only small atoms**; it **never scans ahead for a matching
  delimiter**. A `}`/`/`/`:` is emitted as a close/separator atom the moment it is
  seen unquoted; the **parser** owns all matching, nesting, and close-detection.
- Complex-structure handling lives in the **parser** (`parser.rs`), not the lexer.
- **No fat-lexer scanners** (`scan_braced_operand`, `scan_substitution_operand`,
  `scan_braced_name`, … are NOT used by the new path; they remain only for the
  untouched production path).

## Global constraints

- **Byte-identical / dormant:** `cargo test --workspace` green + the full
  `*_diff_check.sh` release sweep byte-identical; 0 warnings. No production path
  pushes a non-`Command` mode or calls `parser.rs`; `command.rs` is untouched.
- **Existing AST types only:** `parser.rs` builds `WordPart::ParamExpansion`,
  `ParamModifier`, `SubscriptKind`, `Word`, etc. unchanged — so the engine
  (`expand.rs`/`param_expansion.rs`) needs no change.
- **Additive `TokenKind`:** new atom variants are added to `TokenKind`
  (`#[non_exhaustive]`); the existing `Word(Word)` token is unchanged and still the
  only token the production lexer emits.
- All lexer changes in `crates/huck-syntax/src/lexer.rs`; the new parser in
  `crates/huck-syntax/src/parser.rs` (+ `pub mod parser;` in `lib.rs`).

## Non-goals (deferred to later iterations)

- Operands containing `$(…)` / `$((…))` / backtick — those need `CommandSub`/`Arith`
  modes (not built yet). The differential corpus excludes them; `parser.rs` returns
  a clear `Unsupported` parse error on encountering one so tests can assert the
  boundary. Nested `${…}`, `$name`/specials, quotes, and literals ARE supported.
- Wiring `parser.rs` into production (the Stage-2 switch).
- The extquote `$'…'`-as-name form (`${$'x'}`) — documented as an edge handled in a
  follow-up; the corpus excludes it.

## 1. New lexer modes

Added to the v240 `Mode` enum (`ParamExpansion` already exists as a placeholder;
the operand modes are new variants). `scan_step` gains real arms for each
(replacing v240's `unreachable!` for `ParamExpansion`).

| Mode | Entered by | Lexes until | Emits |
|---|---|---|---|
| `ParamExpansion` (head) | parser, on `ParamOpen` | the operator / `}` | name, length/indirect prefixes, `ParamOp`, `[`/`]`, `ParamClose` |
| `ParamWordOperand` | parser, after a value/removal/case operator, and for substitution *replacement* and substring *length* | first unquoted `}` | `Lit`, `DollarName`, `ParamOpen` (nested), `ParamClose` |
| `ParamSubstPatternOperand` | parser, after a `/`-family operator | first unquoted `/` or `}` | `Lit`, `DollarName`, `ParamOpen`, `ParamSep` (`/`), `ParamClose` |
| `ParamSubstringOffsetOperand` | parser, after the substring `:` | first unquoted `:` or `}` | `Lit`, `DollarName`, `ParamOpen`, `ParamSep` (`:`), `ParamClose` |

`ParamWordOperand` covers value operands (`:- := :? :+ - = ? +`), removal patterns
(`# ## % %%`), case patterns (`^ ^^ , ,,`), the substitution **replacement**, and the
substring **length** — all of which simply run to `}` (their `/` and `:` are literal).
Glob-literalness is encoded by the `quoted` flag on each `Lit` atom (false outside
`"…"`), exactly as today's operand Words encode it; no separate "pattern" lexing is
needed at the atom level.

### Rationale: why value and pattern operands share one mode

Value and pattern operands **tokenize identically** — the value-vs-pattern difference
is *semantic* (what the engine does at expand time), not *lexical* (how bytes split
into tokens). Both are a `Word` of literal-runs + `$`-expansions that ends at the first
unquoted `}`. Concretely, the operand text `*.txt` produces the **same** operand `Word`
`[ Literal { text: "*.txt", quoted: false } ]` whether it follows `:-` (`${x:-*.txt}`,
value) or `#` (`${x#*.txt}`, pattern); quoting it (`"*.txt"`) flips `quoted:true` in
both, identically. "This `*` is a glob metacharacter" is therefore NOT a lexing-mode
property — it is carried by the `quoted` flag (false ⇒ glob-active), the same flag in
both cases. The **parser** supplies the value-vs-pattern meaning by the
`ParamModifier` it builds (`UseDefault{word}` vs `RemovePrefix{pattern}`) from the
operator it read; the **engine** treats `RemovePrefix.pattern` as a glob and
`UseDefault.word` as a value. The divergence lives in the parser + engine, never in the
lexer. This mirrors huck today: `modifier_with_operand` (lexer.rs:4178) is the single
shared helper for both value and pattern operands. The lone lexical nuance the current
code adds for pattern operands — threading `in_dquote` so a `$'…'`-as-name inside a
*nested* `${…}` in the pattern validates (M-156) — only affects the `$'…'`-name form,
which v241 excludes; when that form is added later, this mode gains the `in_dquote`
thread (or a sibling mode), and the differential corpus will catch it. Modes that
**split on a separator** (`ParamSubstPatternOperand` → `/`, `ParamSubstringOffsetOperand`
→ `:`) ARE distinct because their *termination* differs; value and pattern differ in
neither termination nor tokenization, so they share `ParamWordOperand`.

The `ParamOpen` atom itself is emitted by **`Command` mode** (and recursively by the
operand modes) when it sees `$` immediately followed by `{` — this is a 1-char peek,
not a look-ahead-for-`}`. The parser then `push_mode(ParamExpansion)`.

## 2. New `TokenKind` atoms

Additive variants (names indicative; exact spelling fixed in the plan):

```rust
// structural
ParamOpen,                       // ${
ParamClose,                      // }
LBracket,                        // [   (subscript open; reusable by ArrayLiteral later)
RBracket,                        // ]
ParamSep,                        // the operand-internal separator: / (subst) or : (substring)
// head-position
ParamName(String),               // identifier | digit-run | special-param char (@ * # ? ! $ -)
ParamLengthPrefix,               // leading # in ${#name}
ParamIndirect,                   // leading ! in ${!name}
ParamOp(ParamOpKind),            // the post-name modifier operator (enum below)
// word-part atoms (seed of the general set; reused when Command mode is converted)
Lit { text: String, quoted: bool },   // a literal run  -> WordPart::Literal
DollarName(String),                   // $name / special-param -> WordPart::Var / AllArgs / LastStatus
```

`ParamOpKind` encodes the operator the head mode recognized (a fixed-set match on the
char(s) right after the name — no delimiter scan):

```rust
enum ParamOpKind {
    // value: bool = colon-prefixed (:- vs -)
    UseDefault(bool), AssignDefault(bool), ErrorIfUnset(bool), UseAlternate(bool),
    RemovePrefix(bool /*longest ##*/), RemoveSuffix(bool /*longest %%*/),
    Substitute(SubstKind),          // / // /# /%   (SubstKind = {All, First, Prefix, Suffix})
    Case(CaseDirection, bool /*all ^^,,*/),
    Transform(TransformOp),         // @Q @P @U @L @u @E @A @K @k @a
    Substring,                      // the : that starts ${name:off[:len]} (':' not followed by -=?+)
}
```

The parser maps `ParamOpKind` + the operand `Word`(s) it assembles to the existing
`ParamModifier` variants (§4). The head mode never emits an operand — it stops at the
operator and the parser drives operand lexing by pushing the operand mode.

## 3. Per-mode scan rules (atom emission)

**`ParamExpansion` (head)** — entered after the parser consumed `ParamOpen`:
1. Optional leading marker: peek the first char.
   - `#` then a name char → emit `ParamLengthPrefix` (length form). `#` then `}` → it
     is the special-param name `#`: emit `ParamName("#")`.
   - `!` then a name char/special → emit `ParamIndirect`. `!` then `}` → special-param
     name `!`: emit `ParamName("!")`.
2. Name: a run of identifier chars, OR a digit-run, OR a single special-param char
   (`@ * # ? $ - !`) → emit `ParamName(text)`.
3. Optional subscript: `[` → emit `LBracket`; the parser then pushes a subscript
   context (lex the subscript body as operand atoms — see below) until `]` → emit
   `RBracket`.
4. Operator OR close: peek.
   - `}` → emit `ParamClose` (bare `${name}` / `${name[sub]}` / `${!name}`).
   - an operator sequence → recognize it (fixed-set: `:-`/`:=`/`:?`/`:+`/`-`/`=`/`?`/`+`/
     `#`/`##`/`%`/`%%`/`/`/`//`/`/#`/`/%`/`^`/`^^`/`,`/`,,`/`@<letter>`/`:`<not -=?+>=Substring)
     and emit `ParamOp(kind)`. The parser then pushes the matching operand mode.
   - anything else → the whole `${…}` is a bad substitution: the parser produces
     `ParamModifier::BadSubst { raw }` (it reconstructs `raw` from the consumed span).

Subscript body uses `ParamWordOperand`-style lexing but terminates on unquoted `]`
(a `RBracket` atom) rather than `}` — the parser pushes a `ParamSubscript` operand
context for it (a 5th operand mode, or `ParamWordOperand` parameterized to stop at
`]`; the plan picks one — recommended: a dedicated `ParamSubscriptOperand` mode for
symmetry).

**Operand modes** — each loop emits, per pull, ONE of:
- `Lit { text, quoted }` — a maximal run of literal chars, stopping at any of:
  the active terminator(s) for this mode (`}`, and `/` or `:` if this mode splits on
  it), `$`, `` ` ``, `"`, `'`, `\`. `quoted` reflects whether the run is inside `"…"`.
- on `"` / `'` — lex the quoted span, emitting `Lit { …, quoted: true/single }` for its
  content (single-quoted content is fully literal; double-quoted content still scans
  `$`/`` ` `` as expansion triggers).
- on `\` — escape: fold the escaped char into a `Lit`.
- on `$` followed by `{` → emit `ParamOpen` (the parser recurses: `push_mode(ParamExpansion)`).
- on `$` followed by a name char / special → emit `DollarName(name)`.
- on `$` followed by `(` → **Unsupported in v241** (deferred): the lexer emits a
  distinguished atom (`Lit`? no) — concretely the operand mode returns a
  `LexError`-free signal the parser turns into `ParseError::Unsupported`. The plan
  pins the exact mechanism; the corpus avoids it.
- on `` ` `` → likewise Unsupported in v241.
- on the active separator (`/` in `ParamSubstPatternOperand`, `:` in
  `ParamSubstringOffsetOperand`) **unquoted** → emit `ParamSep`.
- on `}` **unquoted** → emit `ParamClose`.

No operand mode counts depth or looks for a matching brace — it emits `ParamOpen`
(for nested `${`) and the parser handles the recursion + matching `}` via the mode
stack and `pop_mode`.

## 4. `parser.rs` — the assembler

New module `crates/huck-syntax/src/parser.rs`, `pub mod parser;` in `lib.rs`. It
drives `&mut Lexer` via the v240 pull API (`peek_kind`/`next_kind`/…) and mode stack
(`push_mode`/`pop_mode`), and the v240 `mark`/`rewind` where speculation is needed.

Public entry (dormant; tests + future Stage-2 callers):

```rust
pub(crate) fn parse_word(iter: &mut Lexer) -> Result<Word, ParseError>;       // assemble a Word from atoms
pub(crate) fn parse_param_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError>;
```

`parse_word` reads atoms in the current mode, building `Vec<WordPart>`:
`Lit→WordPart::Literal`, `DollarName→WordPart::Var`/`AllArgs`/`LastStatus`,
`ParamOpen→push_mode(ParamExpansion)+parse_param_expansion(...)+pop_mode`, until a
boundary atom for the caller's context (e.g. `}`, `ParamSep`, `RBracket`, or a
Command-level terminator — for v241 the relevant boundaries are the operand ones).

`parse_param_expansion` (called with the `ParamExpansion` mode already pushed):
1. Read optional `ParamLengthPrefix` / `ParamIndirect`.
2. Read `ParamName` → `name`.
3. Optional `LBracket` → push subscript mode → `parse_subscript` (→ `SubscriptKind`) → `RBracket`.
4. Read `ParamClose` (→ `ParamModifier::None` / `Length` / indirect/keys/prefix per the
   markers + subscript), OR `ParamOp(kind)`:
   - push the operand mode for `kind`, `parse_word` the operand(s) (two for
     `Substitute`/`Substring` — pattern/replacement, offset/length, split on `ParamSep`),
     `pop_mode`, read `ParamClose`.
   - map `(markers, kind, operands, subscript)` → the exact `ParamModifier` variant
     (`UseDefault{word,colon}`, `RemovePrefix{pattern,longest}`,
     `Substitute{pattern,replacement,anchor,all}`, `Substring{offset,length}`,
     `Case{direction,all,pattern}`, `Transform{op}`, `PrefixNames{at}`,
     `IndirectKeys`, `Length`, `None`).
5. Build `WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect }`.

The mapping table from `(ParamLengthPrefix?, ParamIndirect?, ParamOpKind, subscript)`
to `ParamModifier` + the `indirect`/`name` fields is reproduced verbatim from the
current `scan_braced_param_expansion` / `dispatch_braced_modifier` behavior (the
exploration map is the reference); the plan enumerates every row. Bad/unknown
operators → `ParamModifier::BadSubst { raw }` with `raw` rebuilt from the lexer span.

`ParseError` gains an `Unsupported` (or reuse a new variant) for the deferred
`$(…)`/arith/backtick-in-operand cases, so tests assert the boundary cleanly.

## 5. Nesting (what v241 supports)

Recurses via the mode stack for: nested `${…}` (`${x:-${y}}`, `${x/${a}/${b}}`,
`${arr[${i}]}`), `$name`/special-params in operands and subscripts, single/double/
ANSI-C quotes, and backslash escapes. **Deferred** (Unsupported): `$(…)`, `$((…))`,
`` `…` `` inside any operand/subscript — these arrive with `CommandSub`/`Arith` modes.

## 6. Differential testing (the proof)

The new path must produce the SAME AST as the current lexer for every in-scope
`${…}`. `WordPart`/`ParamModifier`/`SubscriptKind`/`Word` all derive `PartialEq`.

```rust
// for each input S in the corpus:
let old: Word = single_word_of(tokenize_with_opts(S, opts));     // production lexer's pre-built Word
let new: Word = { let mut lx = Lexer::new(S, opts, true); parser::parse_word(&mut lx)? };
assert_eq!(new, old, "param-expansion AST mismatch for {S:?}");
```

**Corpus (comprehensive, in-scope):** every form from the grammar map — `${x}`,
`${x:-d}`/`${x-d}`, `${x:=d}`, `${x:?m}`, `${x:+a}`, `${x#p}`/`${x##p}`,
`${x%p}`/`${x%%p}`, `${x/p/r}`/`${x//p/r}`/`${x/#p/r}`/`${x/%p/r}`, `${x^p}`/`${x^^}`/
`${x,}`/`${x,,}`, `${x@Q}`…`${x@a}`, `${#x}`, `${!x}`, `${!x[@]}`, `${!pre*}`/`${!pre@}`,
`${x:off}`/`${x:off:len}`, `${arr[i]}`/`${arr[@]}`/`${arr[*]}`, `${@}`/`${*}`/`${#}`/
`${?}`/`${$}`/`${!}`, quoted (`"${x:-a b}"`), nested `${x:-${y}}`, subscript `${a[$i]}`,
bad-subst (`${x@}`, `${}`). Both quoted and unquoted variants where it matters.

Plus `parser.rs` unit tests for the assembler internals and a `mark`/`rewind` test
if speculation is used (e.g. distinguishing `:-` from substring `:`).

## 7. Edges / open questions (resolve in the plan)

- **Substring vs `:-`:** after the name, `:` then `-`/`=`/`?`/`+` is a value operator;
  `:` then anything else is `Substring`. The head mode peeks one char to decide — no
  look-ahead. (If a cleaner split needs `mark`/`rewind`, use it.)
- **`//` anchor suppression:** when `all` (`//`), `/#`/`/%` anchors are NOT parsed
  (`anchor = None`) — reproduce this (current code lexer.rs ~4086).
- **`${!x}` family:** the many `!` sub-cases (indirect scalar, `IndirectKeys`,
  `PrefixNames`, bare `${!}` = `Var("!")`) — the plan enumerates them from the map.
- **`quoted` threading:** `parse_word`/`parse_param_expansion` take the enclosing
  `quoted`; nested expansions inherit it; `Lit.quoted` comes from quote spans. Must
  match the current `quoted` field values exactly (the differential test enforces it).
- **`Unsupported` mechanism** for deferred `$(…)`/arith/backtick — exact atom/error
  shape pinned in the plan.
- **Extquote `${$'x'}` names** — excluded from v241 corpus; follow-up.
