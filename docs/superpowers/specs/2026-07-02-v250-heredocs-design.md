# v250 — Heredocs on the atom-command path (atom-native body emission) — Design

**Status: APPROVED (2026-07-02).** Third Phase-C **Stage 2** "port a deferred
construct onto the atom-command path" iteration (after v248 funcdefs, v249
here-strings) — and the deliberately-hard one: the roadmap flags heredoc bodies
as unresolved in the pull model. Direction:
`2026-06-30-phase-c-parser-driven-frontend-roadmap.md` (Stage 2, §6) + memory
`huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`.

## 1. Goal & context

huck's front-end is being inverted so the lexer emits small atoms and the PARSER
assembles words + structure — a DORMANT path (gated by a `command_atoms` lexer
flag defaulting to `false`; production still uses the batch Word-lexer +
`command.rs` oracle) that must produce ASTs byte-identical to the oracle, gated
by the differential harness `new_seq` (atoms) vs `old_seq` (oracle), `diff_cmd`
asserting equality. Each Stage-2 iteration removes one construct family from the
atom path's deferred set. v250 ports **heredocs** (`<<DELIM … DELIM`, `<<-`).

**Chosen architecture (Approach B — fully atom-native).** The heredoc body is
emitted by the lexer as ATOMS (like a `"…"` body) and assembled into a `Word` by
the parser — NOT collected as a whole `Word` by the lexer and stuffed into the
token (that pragmatic "Approach A" was rejected in favor of consistency with the
core rule). The production `collect_heredoc_bodies` back-patch stays for the
Word-lexer; the atom path gets its own emission + assembly.

**Binding architectural constraint (user, 2026-07-02):** the lexer keeps its own
heredoc state and detects the close-delimiter line itself; **the lexer never
depends on the parser** (no callback, no waiting for a parser-pushed mode to
begin body emission). The parser depends on the lexer's atom stream. This is why
heredoc body-emission is **lexer-internal state**, self-started at the newline,
rather than a parser-pushed `Mode` like the word-level modes.

## 2. Why heredocs are hard, and what already exists

Heredoc bodies are line-oriented and appear textually AFTER the newline that ends
the redirect line (and after any other commands on that line). In the pull model,
the parser consumes the `<<DELIM` redirect BEFORE the newline, so the body is not
yet available at redirect-parse time. Existing machinery:

- `TokenKind::Heredoc { body: Word, expand, strip_tabs }` (production) — carries a
  fully-collected body, filled by `collect_heredoc_bodies` (a back-patch pass
  triggered at the newline, `lexer.rs:2108`), which reuses
  `collect_one_heredoc_body` (`lexer.rs:4071`: delimiter matching, `<<-`
  tab-strip, expansion gating on quoted delimiter, `\`-line-continuation) and
  `scan_expanding_body_line` (`lexer.rs:4153`: `Literal` + `$`-expansions +
  backtick, heredoc backslash rules — `\` special only before `$`/`` ` ``/`\`;
  `"`/`'` literal).
- The pull model's readiness machinery (`fill_to`/`backfill_pending_at`,
  `lexer.rs:3396/3466`) exists for the WORD-lexer heredoc: `fill_to(idx)` keeps
  scanning until no `PendingHeredoc` has `token_idx == idx`, forcing the
  back-patch collection before exposing a `Heredoc` token. Approach B must AVOID
  this machinery (it would force premature body scanning) — so the atom path uses
  a SEPARATE pending queue that `backfill_pending_at` does not consult (§3.1),
  leaving the production `pending_heredocs`/`backfill` untouched.
- `Mode::HeredocBody` is declared but unused (`lexer.rs:625`) — reserved; NOT
  used as a parser-pushed mode here (see §1 constraint), but its slot documents
  the intent.
- v247's atom scanner emits an EMPTY `Heredoc` placeholder with NO `PendingHeredoc`
  recorded (`lexer.rs:2790–2796`) and the atom parser defers all heredocs.
- Engine-facing AST: `RedirOp::Heredoc { body: Word, expand: bool, strip_tabs:
  bool }` (`command.rs:321`), slotted to stdin (fd 0). UNCHANGED by v250.

## 3. Design

### 3.1 Lexer — atom vocabulary + self-contained body emission

**Opener token — REUSE `TokenKind::Heredoc`.** The `<<`/`<<-` opener stays the
existing `TokenKind::Heredoc { body, expand, strip_tabs }` variant (NOT a new
`HeredocOpen`) so `crate::command::next_is_redirect` recognizes it unchanged and
`command.rs` is not touched. On the atom path its `body` field is always the
empty `Word` (the real body arrives as atoms and the parser fills the AST from
them); `expand`/`strip_tabs` are carried as usual. v247 already emits this token
(with a hardcoded `expand: true`); v250 makes the atom scanner parse the
delimiter so `expand` is correct.

New `TokenKind` atoms (additive; only emitted on the atom path):

- `HeredocBodyBegin` / `HeredocBodyEnd` — bracket one heredoc body's part atoms
  (mirrors `BeginDquote`/`EndDquote`). Between them the lexer emits the body-part
  atoms.

Lexer state:

- `atom_pending_heredocs: VecDeque<PendingHeredoc>` — a NEW queue used ONLY by the
  atom path (delim/expand/strip_tabs; NO `token_idx`). Deliberately SEPARATE from
  the production `pending_heredocs` so `backfill_pending_at`/`fill_to` never gate
  the atom opener (which would force premature body scanning) and the production
  heredoc path is entirely untouched.
- `emitting_heredoc` — new lexer-internal state (an index/flag over
  `atom_pending_heredocs`) marking "currently emitting body atoms." Set at the
  newline; cleared when the queue drains.

Flow (in the atom scanner `scan_step_command_atoms` / `scan_command_operator_atom`):

1. **`<<`/`<<-`:** parse the delimiter via `parse_heredoc_delim` (delim + expand),
   detect `<<-` → strip_tabs, emit `TokenKind::Heredoc { body: Word(vec![]),
   expand, strip_tabs }`, push `PendingHeredoc { delim, expand, strip_tabs }` onto
   `atom_pending_heredocs`. (No token_idx — no back-patch, no `fill_to` gating.)
2. **`\n` with a non-empty `atom_pending_heredocs`:** emit the `Newline` token,
   then set `emitting_heredoc`. On subsequent `scan_step` calls, for the front
   pending heredoc:
   - emit `HeredocBodyBegin`,
   - emit body-part atoms line by line until the close-delimiter line:
     - **literal** (quoted-delimiter) heredoc → one `Lit { text, quoted: true }`
       accumulating the raw body verbatim (with `<<-` tab-strip applied per
       line),
     - **expanding** heredoc → mirror `scan_expanding_body_line`: `Lit` chunks +
       `DollarName`/`ParamOpen`/`CmdSubOpen`/`BeginBacktick`/`ArithOpen` opener
       atoms (parser recurses, as for `"…"`), with heredoc backslash rules, `<<-`
       tab-strip, and `\`-line-continuation joining lines,
   - the lexer detects the close-delimiter line itself (whole-line exact match,
     tab-stripped for `<<-`) → emit `HeredocBodyEnd`, `pop_front` the pending
     heredoc, continue with the next,
   - when the queue empties, clear `emitting_heredoc` and resume normal Command
     scanning (text after the last delimiter is the next command).
   - EOF before a close delimiter → `LexError::UnterminatedHeredoc` (as the
     oracle).

The lexer consults only its own state; it never calls the parser. Body emission
begins at the newline from lexer state, not from a parser action.

### 3.2 Parser — collect body atoms, assemble, attach in source order

Heredoc bodies cannot be assembled at redirect-parse time (the atoms follow the
newline). Split collection from attachment:

1. **Redirect parse (`parse_one_redirect`, heredoc arm):** stop deferring.
   Consume the `TokenKind::Heredoc { expand, strip_tabs, .. }` opener (its `body`
   is the empty placeholder, ignored), build a provisional
   `RedirOp::Heredoc { body: Word(vec![]), expand, strip_tabs }` (empty body for
   now). Also handle heredoc at command position (leading `<<EOF` — an
   empty-words command reading stdin) by relaxing the command-position guard, as
   here-strings did.
2. **Body collection (at newline-consumption sites):** after consuming a
   `Newline`, while the next atom is `HeredocBodyBegin`, consume one body group
   (`Begin` → part atoms → `End`), assembling the body `Word` with the SAME
   part-handling the parser uses for `"…"` bodies (`parse_dquote`'s
   literal-coalescing + expansion recursion — extract a shared helper). Push each
   assembled `Word` onto an ordered accumulator `heredoc_bodies: Vec<Word>`. The
   newline sites are `skip_newlines` and the connector/separator handling in
   `parse_and_or`/`parse_sequence`.
3. **Attachment (once, after `parse_sequence`):** walk the completed `Sequence`
   in source order and fill each still-empty `RedirOp::Heredoc { body }` from
   `heredoc_bodies` front-to-back via a recursive
   `attach_heredoc_bodies(&mut Command, &mut impl Iterator<Item = Word>)`
   descending pipelines, and-or lists, compound bodies, and redirect lists.
   Source order of heredoc redirects == body emission order, so the positional
   zip is exactly correct (matches bash's stacked-heredoc pairing:
   `cat <<A <<B` → body 1 → A, body 2 → B).
4. **Threading:** `parse_sequence` (atom entry) owns `heredoc_bodies`, threads
   `&mut Vec<Word>` to the newline sites, and does the final attach walk before
   returning.

Rationale for collect-then-attach: at the newline the heredoc redirect lives
inside an already-built `Command` deeper in the returned AST; a single
source-order final walk avoids threading mutable references to those nodes up
through the recursion (painful in Rust) while staying deterministic.

### 3.3 mark/rewind interaction

`mark`/`rewind` must not span heredoc-body emission (the cursor reset would
desync `pending_heredocs`/`emitting_heredoc`). In the atom parser the only marks
are bounded to before any newline — funcdef leading-word detection (v248) and the
arith `((` disambiguation — so neither spans a newline, hence neither spans body
emission. To make any future violation loud rather than silently corrupting:
capture a cheap heredoc-state generation counter in `Mark` and `debug_assert!` in
`rewind` that it is unchanged. Test with heredoc-after-funcdef-shaped leading
words.

## 4. Scope

**In scope** (all byte-identical to the oracle via `diff_cmd`/error parity):

- `cmd <<DELIM … DELIM` and leading `<<DELIM … DELIM`.
- `<<-` tab-strip (body + delimiter lines).
- Literal delimiters (`<<'EOF'`, `<<"EOF"`, `<<\EOF`) → no expansion.
- Expanding bodies: `$var`, `${x:-d}`, `$(cmd)`, `` `cmd` ``, `$((expr))`;
  heredoc backslash rules (`\$`/`` \` ``/`\\` escaped, other `\` literal); `"`/`'`
  literal inside bodies; `\`-line-continuation in expanding bodies.
- Empty body; delimiter text as a non-matching substring of a body line.
- Multiple heredocs on one line (`cat <<A <<B`), heredoc across a pipeline
  (`a <<X | b <<Y`), heredoc on a compound (`{ cat; } <<EOF`, `if…fi <<EOF`),
  heredoc interleaved with other redirects/words (`cat <<EOF >out arg`).
- Unterminated (EOF before delimiter) → same `UnterminatedHeredoc` error.

**Out of scope / stays as-is.** The live flip; other deferred families (process
sub, array literals, `[[ ]]`, arith command, coproc, `$[ ]`). Production heredoc
path (`scan_step_command`, `collect_heredoc_bodies`, `fill_to`/`backfill`)
UNCHANGED.

## 5. Invariants

- Byte-identical: every in-scope heredoc parses to the SAME AST / same error on
  the atom path as the oracle. A well-formed in-scope divergence is a v250 BUG.
- Production untouched: `command_atoms` defaults `false`; the Word-lexer heredoc
  machinery (`collect_heredoc_bodies`, `scan_step_command`'s `<<` handling,
  `fill_to`/`backfill_pending_at`) is unchanged; `command.rs`/`process_line`
  unchanged. Engine-facing `RedirOp::Heredoc` AST unchanged.
- The lexer does not depend on the parser (no callback / no parser-driven start
  of body emission); body emission is lexer-internal state started at the newline.
- 0 warnings; every commit carries the `Co-Authored-By: Claude Opus 4.8 (1M
  context)` trailer; branch `v250-heredocs`, not `main`.

## 6. Implementation staging (≈6 tasks; literal before expanding)

1. `HeredocBodyBegin`/`End` atoms + `atom_pending_heredocs` queue +
   `emitting_heredoc` field; atom scanner parses the delimiter (reusing
   `TokenKind::Heredoc` as the opener, `expand` now correct) + records the pending
   record at `<<`/`<<-` (opener only — body still unemitted; parser still defers).
   Proves the opener + delimiter parse without changing observable behavior.
2. Lexer body emission for **literal** heredocs (quoted delimiter → one raw
   `Lit`) + `emitting_heredoc` state + delimiter detection + `HeredocBodyBegin/End`.
   Unit-tested at the atom-stream level.
3. Parser: `parse_one_redirect` heredoc arm (provisional body) + body collection
   at newlines + source-order `attach_heredoc_bodies` walk + `parse_sequence`
   threading — wired end-to-end for LITERAL heredocs (`diff_cmd` green for
   `cat <<'EOF'\n…\nEOF`, leading, `<<-`).
4. **Expanding** body emission (mirror `scan_expanding_body_line`: `$`
   classification, backtick, arith, backslash rules, `\`-line-continuation) +
   parser expansion recursion in the body assembler. `diff_cmd` for expanding
   bodies.
5. Positional coverage: multiple/stacked heredocs, across pipelines, on compounds,
   interleaved with other redirects/words.
6. mark/rewind generation-guard + `debug_assert`; error parity (unterminated) +
   full adversarial corpus; full `huck-syntax` lib green, doctests, 0 warnings.
