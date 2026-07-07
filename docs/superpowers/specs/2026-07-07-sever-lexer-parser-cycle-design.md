# Sever the lexer→parser cycle (command-word subscript lvalue) — design

**Date:** 2026-07-07
**Status:** design (brainstormed, approved section-by-section; revised after
planning uncovered two pre-existing divergences in the code being removed — see
"Behavior changes").
**Iteration:** the follow-up committed after v267 — make the huck front-end
dependency strictly one-way (parser → lexer) and rule-clean.

**Approach chosen (of three surfaced during planning):** full rule-clean removal
that lets the two incidental divergences resolve *toward bash* (rather than adding
machinery to preserve the current buggy behavior). The clean architecture fixes
them for free; see "Behavior changes (intended fixes)".

## Goal

Remove the last production edge where `crates/huck-syntax/src/lexer.rs` calls
into `crate::parser` (`lexer.rs:3769` → `parser::parse_fragment_word`), and the
speculative `[…]` forward-scan that feeds it (`lexer.rs:3733-3765`). After this
iteration the lexer depends on nothing in the parser, and the command-word
indexed-assignment lvalue (`a[i]=x`, `a[i]+=x`) is recognized the same rule-clean
way the array-element lvalue already is.

Two things are severed:
1. **The cycle** — the lexer no longer calls the parser to assemble a subscript
   `Word`.
2. **The forward-scan** — the lexer no longer clones the cursor and scans ahead
   for the matching `]` (then peeks past it for `=`) to decide assignment-vs-word.

This directly serves the project's binding front-end rule: *the lexer emits small
atoms and NEVER forward-scans for a matching delimiter; the parser owns
delimiter-matching / recursion and assembles words AND structure.*

## Background: the cycle, and the precedent that already solved it

The command-word indexed lvalue is the **last straggler** on the old bridge. The
sibling case — the **array-element** lvalue `a=([i]=x)` — was already ported to
the rule-clean shape in v252 T3 (`parser::parse_array_literal`,
`crates/huck-syntax/src/parser.rs:1259-1287`):

- the lexer emits a zero-width `LBracket` at element start (no forward-scan);
- the parser pushes `Mode::ParamSubscriptOperand`, calls `parse_word` to assemble
  the subscript `Word` (identical to the `${a[i]}` reader), consumes `RBracket`,
  pops the mode;
- the element value is parsed next.

No `parse_fragment_word`, no speculative scan. This iteration makes the
command-word case follow the same pattern. (The `${a[i]}` parameter-expansion
subscript, `parser.rs:735-758`, is the other existing instance of the pattern.)

### What the old command-word path does today (to be replaced)

`Lexer::try_scan_assign_prefix` (`lexer.rs:3666`), `Some('[')` branch
(`lexer.rs:3733-3800`):

1. Clones the cursor (`bracket`) and **forward-scans** the balanced `[…]`,
   accumulating the raw subscript text and tracking bracket depth
   (`lexer.rs:3736-3747`).
2. Peeks *past* `]` for `=` / `+=`; only then is it an assignment
   (`lexer.rs:3751-3764`).
3. On a confirmed assignment, calls
   `crate::parser::parse_fragment_word(&raw, self.opts)` (`lexer.rs:3769`) — **the
   cycle** — to turn the raw subscript text into a `Word` (the subscript can hold
   `$i` / `${j}` / `$((n))` / `$(…)` / quotes, which only the parser assembles).
4. Emits a single `TokenKind::AssignPrefix { target: Indexed { name, subscript },
   append }` word-part token, syncs the real cursor to `bracket`, sets value-mode
   state (`in_assignment_value = true`, `assign_val_tilde_ok = false`,
   `cmd_at_word_start = false`), and runs the compound-array `(` probe (emit a
   zero-width `ArrayOpen` for `a[sub]=(…)`), leaving the cursor on `(`.

The parser later turns the `AssignPrefix` word-part into an `Assignment` via
`try_split_assignment`. Note the scalar `name=` and bare `name+=` (Bare) branches
(`lexer.rs:3679-3730`) do **not** call the parser and are **out of scope** —
only the `Some('[')` (Indexed) branch is.

## Scope

**In scope (only this):** the command-word indexed lvalue `a[i]=x` / `a[i]+=x`
recognized by `try_scan_assign_prefix`'s `Some('[')` branch, its forward-scan,
its `parse_fragment_word` call, and the resulting `AssignPrefix { Indexed … }`
emission — replaced by the parser-driven pattern below.

**Out of scope / unchanged:** scalar `name=` and bare `name+=`
(`AssignTarget::Bare`); the array-element lvalue `a=([i]=x)` (already correct —
it is the template); `${a[i]}` subscripts; `[[ … ]]` / `(( … ))`; every other
lexer behavior. No bash-observable behavior changes (byte-identical gate).

## Design

### Overview of the new flow (`a[i]=x` at command-word-start)

```
lexer:   Lit "a"   LBracket   [ …subscript body atoms… ]   RBracket   AssignEq{append:false}   [ …value atoms… ]
                    │          └─ parser pushes Mode::ParamSubscriptOperand,      │              └─ lexed under value-mode
                    │             parse_word assembles the subscript Word         │                 (begin_assignment_value already fired)
                    └─ emitted at word-start on `name[`; no forward-scan          └─ distinct token so the parser can call
                                                                                     begin_assignment_value BEFORE the value is pulled
parser:  assembles command word → sees Lit + LBracket → subscript Word → AssignEq → begin_assignment_value → value
         → AssignTarget::Indexed { name, subscript }, value
```

### §1 Lexer — emit `LBracket`, delete the scan + the parser call

In `try_scan_assign_prefix`, **delete** the entire `Some('[')` branch
(`lexer.rs:3733`-end of that arm): the forward-scan, the `parse_fragment_word`
call, the `AssignPrefix { Indexed … }` emission, the cursor sync, and its inline
value-mode/`ArrayOpen` handling.

Replace it with: at command-word-start, when a bare `name` (`[A-Za-z_][A-Za-z0-9_]*`)
is immediately followed by `[`, emit `Lit { text: name }` then a zero-width
`TokenKind::LBracket`, consume the `[`, and set a lexer flag
`pending_lvalue_subscript = true`. **Nothing is scanned past `[`.** (This reuses
the existing `LBracket` token — the same one the array-element path emits.)

Rationale for the position gate: the `LBracket`-lvalue signal is emitted **only**
when `cmd_at_word_start` holds and the identifier-then-`[` shape is present, so
ordinary mid-word `[` (globs elsewhere) is untouched. A command-word-start
`name[` that turns out NOT to be an assignment (a glob like `a[bc] file`) is
handled by the parser's glob fold-back (§3), not by the lexer.

### §2 Lexer — the post-subscript `=` / `+=` and value-mode

After the parser assembles the subscript and consumes `RBracket` (still §3), the
lexer is back in `Mode::Command` with `pending_lvalue_subscript` still set. On the
next command scan step, if that flag is set:

- if the next char is `=`, or `+` followed by `=`: emit a distinct
  `TokenKind::AssignEq { append }` token (`append = true` for `+=`), consume the
  operator char(s), clear `pending_lvalue_subscript`, and **do not lex the value
  yet** (it is pulled later, after the handshake).
- otherwise: clear `pending_lvalue_subscript` and resume ordinary word scanning
  (the brackets were a glob — the tail flows into the word normally).

`pending_lvalue_subscript` must survive the parser's `ParamSubscriptOperand`
excursion, including any `$(…)`/`${…}` inside the subscript (which push/pop their
own modes and run `boundary_reset`). The plan must ensure `boundary_reset` and
mode push/pop do **not** clear it, and that it is cleared on exactly the two
outcomes above (assignment or glob) — never left dangling into the next word.

`AssignEq` being a **distinct token emitted before the value is lexed** is what
makes the `begin_assignment_value` handshake sound: the parser consumes `AssignEq`,
sets value-mode, and only then pulls the value.

### §3 Parser — command-word assembly, subscript, `=` decision, glob fold-back

In the command-word assembly path (the `parse_word` / `parse_word_command`
machinery that collects word-parts — cf. `parser.rs:301-305` for the current
`AssignPrefix` handling), add handling for `LBracket` at command-word-start:

On `Lit name` immediately followed by `LBracket`:
1. Consume `LBracket`. Push `Mode::ParamSubscriptOperand`. `parse_word` the
   subscript body. Consume `RBracket`. Pop the mode. (Mirror
   `parse_array_literal:1259-1271` exactly, including its error/pop-on-error
   handling.)
2. Peek the next token:
   - **`AssignEq { append }`** → consume it; call
     `iter.begin_assignment_value(append)` (§4); parse the value with the same
     reader the old path's value used (so `a[i]=(…)` compound arrays,
     `a[i]=$(…)`, quotes, tildes all behave identically); build
     `AssignTarget::Indexed { name, subscript }` and the resulting `Assignment` /
     `WordPart::AssignPrefix` exactly as `try_split_assignment` produces today, so
     everything downstream (`fill_command`'s `AssignTarget::Indexed` heredoc walk
     at `parser.rs:3118/3128/3189`, the engine's assignment executor) is
     unchanged.
   - **anything else (glob fold-back)** → the brackets were an ordinary glob.
     Fold the pieces back into the word: append a literal `[`, the subscript
     `Word`'s parts, and a literal `]` to the word under assembly, then continue
     normal word scanning. The subscript body was lexed under
     `ParamSubscriptOperand`, so its expansions (`$x`, `$(…)`, `${…}`, quotes) are
     assembled parts — i.e. the fold-back word **expands** them. This matches
     huck's ordinary word-glob path (verified: `./a[$x]` → `a[2]` in both huck and
     bash) and is a behavior change vs today's literal-swallow — see D1 under
     "Behavior changes". Char-class / nested-bracket globs (`a[[:alpha:]]`)
     self-heal: `ParamSubscriptOperand` terminates on the inner `]`, but the
     leftover `]` re-coalesces into the following word run, reproducing the same
     literal text globbing sees in bash (the no-`=` branch is still taken, so it is
     correctly a glob, not an assignment).

The result is that the lexer emits the *same* `LBracket`+subscript atoms whether
the construct is an assignment or a glob; the **parser** decides, by the presence
of the trailing `AssignEq`. This is the rule-clean disambiguation replacing the
lexer's old forward-scan.

### §4 Value-mode handshake — `Lexer::begin_assignment_value`

Add `pub(crate) fn begin_assignment_value(&mut self, append: bool)` to the lexer.
It replicates what the old Indexed arm set immediately before the value
(`lexer.rs:3776-3794`), with ONE intended change (the tilde flag, D2):

- `self.cmd_at_word_start = false;`
- `self.in_assignment_value = true;`
- `self.assign_val_tilde_ok = true;` — **changed from the old Indexed arm's
  `false`**, so a leading `~` in the value expands, matching the scalar `name=`
  arm and bash. This fixes divergence D2 (see "Behavior changes"). The colon-tilde
  re-enable (`a[i]=a:~/y`, already correct today) is unaffected — it is handled by
  the value scanner's own `:`-reset, not this flag's initial value.
- the compound-array `(` probe: `skip_line_continuations(&mut self.cursor)`; if
  the cursor is on `(`, emit a zero-width `ArrayOpen` so the parser pushes
  `Mode::ArrayLiteral` for `a[i]=(…)` / `a[i]+=(…)`. Cursor left on `(`.

Called strictly parser → lexer, after the parser has consumed `AssignEq` and
before it pulls any value token. The `append` parameter is available for parity
with the old arm even if the current flag-setting does not branch on it (kept for
clarity / future-proofing only if the old arm distinguished — otherwise omit it;
the plan resolves this against the exact old code).

### §5 Deletions — the bridge and its dead surface

Once §1-§4 land, the following have no remaining callers and are removed:

- `parser::parse_fragment_word` (`parser.rs:3289-3316`) — the array path never
  used it; the command-word path is now off it. Delete the function and its 4
  unit tests (`parser.rs:7440-7473`, the `v266 T1` block) and the
  `crate::lexer::Word` fallback doc.
- `crate::parser::parse_fragment_word` reference in `lexer.rs` (the call at 3769
  and the doc mention at line 14) — gone with the branch. After this, **no
  `crate::parser::` / `crate::parser` reference remains in non-test lexer.rs
  code** (the `parse_sequence` references at `lexer.rs:7077/7220/7319/7332` are
  `#[cfg(test)]` and may stay — tests may depend on both crates' internals; the
  production one-way invariant is what matters).
- `LexError::SubscriptParseError` (`lexer.rs:18`) and its
  `lex_error_message_impl` arm — only produced by the deleted `.map_err` at
  `lexer.rs:3770`. Remove the variant and its message arm (confirm no other
  producer/consumer first).
- Repoint the two executor comments (`executor.rs:9225`, `9245`) that reference
  `parse_fragment_word` to describe the new parser-side assembly.

New surface added: `TokenKind::AssignEq { append: bool }`, the
`pending_lvalue_subscript` lexer flag, and `Lexer::begin_assignment_value`. If
`AssignEq` can reuse an existing operator token cleanly the plan may do so, but a
dedicated zero-width token is preferred for an unambiguous parser hand-off.

## Behaviors to preserve (byte-identical checklist)

Every case below must produce identical bytes (stdout+stderr+exit) under bash and
huck, and identical ASTs where covered by unit tests:

- `a[0]=v`, `a[i]=v`, `a[i]+=v` (scalar-valued indexed assignment + append).
- Subscript expansions: `a[$(echo 2)]=v`, `a[${i}]=v`, `a[$((n+1))]=v`,
  `a['x']=v` / `a["y"]=v` (quoted subscripts), `a[i+1]=v` (arithmetic text).
- Compound RHS: `a[i]=(x y)`, `a[i]+=(z)`, and with a `\<newline>` between `]=`
  and `(`.
- Colon-tilde value: `a[i]=a:~/y` (already correct; stays correct). *(Leading
  tilde `a[i]=~/x` is intentionally CHANGED — see D2.)*
- Assoc keys: `declare -A m; m[key]=v`, `m[$k]=v`.
- **Glob fold-back:** `a[bc]` / `[abc]` / `a[b-c]` / `a[!x]` / `a[[:alpha:]]` as a
  **command word** with no `=` (glob, matching **bash**), plus mixed shapes
  `a[b] c`, `a[b]c=` (name `a[b]c` — match bash), `a[b]=c d`. *(Literal-subscript
  globs like `a[bc]` are already byte-identical to both; expansion-bearing ones
  like `a[$x]` are intentionally CHANGED — see D1.)*
- Inline-assignment lists and command-prefixed assignments: `a[i]=v cmd`,
  `x=1 a[i]=2 cmd`.
- Error parity: `a[$(echo 1]=x` (unterminated `$(` in a subscript) and
  `a[i}=x` — the error class must match today's behavior (note: today this is
  `SubscriptParseError`; after removal it becomes whatever the general parser
  path yields — this may shift the error *variant*; the harness asserts the
  bash-observable message, and any internal-variant shift is documented).

## Testing

- **New harness** `tests/scripts/subscript_lvalue_diff_check.sh` — every checklist
  case above, byte-identical bash↔huck (following the `alias_expand_diff_check.sh`
  pattern; guarded with `ulimit -v 1500000` + `timeout`).
- **Existing harnesses** must stay green, especially `array_*`, any glob/bracket
  harness, and the assignment harnesses.
- **Unit tests** — parser AST tests for `a[i]=v` / `a[$(..)]=v` / `a[i]+=v` /
  `a[i]=(..)` producing `AssignTarget::Indexed` with the right subscript `Word`;
  a lexer token-stream test for the `Lit name, LBracket, …, RBracket, AssignEq`
  shape; a glob-fold-back AST test (`a[bc]` → a single glob word, no
  `AssignPrefix`). Replace the 4 deleted `parse_fragment_word` tests with these.
- **Full suites**, guarded per this box: `( ulimit -v 2500000; cargo test -p
  huck-syntax --jobs 1 --lib -- --test-threads 1 )` and the same for
  `huck-engine`; build the binary with `cargo build -p huck`.
- **Invariant assertion** (a test or a CI grep): no `crate::parser` reference in
  non-`#[cfg(test)]` `lexer.rs` code.

## Risks & invariants

- **Glob fold-back is the one behavior-sensitive path** (a common construct). The
  safety target is now huck's **ordinary word-glob path** (which already matches
  bash — verified: `./a[$x]` → `a[2]`, `./a[[:alpha:]]` → the char-class match).
  The fold-back must produce the same parts that path would; the new harness
  (asserting bash-parity) is the proof. A *degenerate* char-class SUBSCRIPT in an
  actual assignment (`a[[:alpha:]]=x`) may mis-classify as a glob (the inner `]`
  ends the subscript before the `=`); this is nonsensical input and is documented,
  not handled.
- **`pending_lvalue_subscript` lifetime** — must survive the subscript-body mode
  excursion and be cleared on exactly the assignment / glob outcomes; a leaked
  flag would mis-tokenize the *next* word's `=`. Covered by the mixed-shape tests.
- **Value-mode timing** — `begin_assignment_value` fires after `AssignEq` is
  consumed and before the value is pulled; the distinct `AssignEq` token is what
  guarantees the value is not pre-lexed under the wrong mode.
- **THE RULE** — after this change the lexer emits atoms + a single-terminator
  `ParamSubscriptOperand` run (parser-pushed); the parser owns the `[`…`]`
  matching and the `=` decision. No speculative delimiter scan, no lexer→parser
  call.

## Behavior changes (intended fixes)

The clean architecture resolves two pre-existing huck→bash divergences that live
in the very code being deleted. Both move huck *toward* bash. They are the only
observable behavior changes; everything else is byte-identical to today. Both get
a bash-parity harness case and a note in the iteration log (and
`docs/bash-divergences.md` if either had an entry — neither currently does).

- **D1 — command-position `name[…]` with no `=` now expands its subscript.**
  Today huck literal-swallows the whole `name[…]` region (`$`, `` ` ``, quotes stay
  LITERAL). Example: `x=2; echo a[$x]` → today `a[$x]`, after `a[2]` (bash: `a[2]`).
  Root: the deleted forward-scan's `None =>` literal-swallow arm
  (`lexer.rs:3799-3820`). After the change the region flows through ordinary
  word-part assembly (the glob fold-back, §3), so it expands like every other
  word — matching bash. Literal-only subscripts (`a[bc]`) are unchanged.

- **D2 — leading `~` in an indexed-assignment value now expands.**
  Example: `a[0]=~/y` → today `~/y`, after `/home/…/y` (bash: expands; scalar
  `x=~/y` already expands). Root: the old Indexed arm set
  `assign_val_tilde_ok = false`; `begin_assignment_value` (§4) sets it `true`,
  matching the scalar arm. Colon-tilde (`a[i]=a:~/y`) was and stays correct.

- **I3 (found during implementation, accepted) — unquoted nested-bracket
  subscripts `a[b[i]]=v` no longer recognized.** The old forward-scan
  depth-counted `[`; `Mode::ParamSubscriptOperand` terminates on the first `]`, so
  `a[a[0]]=v` folds to a glob (→ command-not-found) instead of an assignment. This
  UNIFIES behavior with the two sibling subscript contexts (`${a[a[0]]}`,
  array-literal `([a[0]]=x)`), which already rejected unquoted nested brackets —
  the command-word lvalue was the lone exception. Quoted / command-sub subscripts
  are unaffected. Narrow indirect-index idiom; accepted and tracked as **L-73** in
  `docs/bash-divergences.md`. A real fix (balanced nested-bracket subscripts across
  all contexts) is a separate iteration.

## Non-goals

- No change to scalar/bare assignment, array literals, `${…}`, or any expansion
  semantics **beyond D1/D2 above and the accepted I3/L-73 consequence**.
- Not fixing any *other* latent divergence (e.g. the internal error-*variant* shift
  when a malformed subscript stops going through `SubscriptParseError` — the
  bash-observable message is what the harness asserts).
- Not touching the `#[cfg(test)]` lexer→parser test references (the one-way
  invariant is a production-code property).
