# v345 — flip the `alias` bash-suite category

**Issue:** [#329 — alias: array-literal in alias body, alias after leading
redirection, alias expanding to a comment](https://github.com/jdstanhope/huck/issues/329).

**Goal:** flip the bash-suite `alias` category to PASS (byte-identical) by
fixing three alias-expansion roots. Target: full runner PASS 32 → 33.

## Background & feasibility spike

The `alias` category is FAIL with a 44-line residual, but almost all of it is a
**cascade**: `alias.tests` hits a fatal parse error at line 56 (root R1) and
aborts, so every downstream line — including its `${THIS_SH} ./aliasN.sub`
sub-invocations — never runs and shows as "missing" vs bash.

To measure the **true** residual, R1 was neutralized (the line-56 array-alias
replaced with a benign scalar alias) and the full file re-run against bash: the
diff then reduces to **exactly** the `alias1.sub` and `alias4.sub`
divergences — roots R2 and R3 — with no other roots surfacing. So the category
is a fully-characterized **3-root cluster**, all well-defined bash
alias-expansion behavior (no fundamental walls). `alias2.sub`, `alias3.sub`,
`alias5.sub`, `alias6.sub` already match.

All three roots require `shopt -s expand_aliases` (aliases only expand in
interactive shells or when that shopt is set — the tests set it).

## Root R1 — array literal as the LEADING command word of an alias body

`shopt -s expand_aliases; alias foo='a=(1 2 3); echo "${a[@]}"'; foo` → huck
`syntax error: unsupported command` (`ParseError::UnsupportedCommand`,
parser.rs:3143); bash runs it.

**Precisely scoped by the spike:**
- `eval "a=(1 2 3); …"` → **works** (eval's re-lex handles arrays).
- alias body `echo pre; a=(1 2 3)` (array NOT leading) → **works**.
- alias body `a=(9); echo ok` (array IS the leading word) → **fails**.

So the bug is only in the injected-body **leading-word** scan. When an alias
body is injected (`push_injection`) and `maybe_expand_command_alias`
re-drives to lex the body's first command word (to expand a leading alias in
the body), the scan of a leading `name=(` does **not** emit the zero-width
`ArrayOpen` signal that `try_scan_assign_prefix`'s `(`-probe normally emits
(lexer.rs:5455-5467). Without `ArrayOpen`, the `(` later surfaces as a bare
`Op(LParen)` after the assignment word `Lit "a="`, which the parser rejects at
parser.rs:3143 (`is_assignment_word` → `UnsupportedCommand`).

**Fix:** ensure the leading-word scan of an injected alias body runs the same
assignment-prefix + `ArrayOpen` probe path as a normal command-word scan, so a
leading `name=(…)` / `name+=(…)` emits `ArrayOpen` and parses as an array
literal. The implementer must instrument the re-drive path
(`maybe_expand_command_alias` → `fill_to`/first-atom scan) to find where the
`ArrayOpen` is dropped for the leading body word and route it through (or not
bypass) `try_scan_assign_prefix`'s array probe. Non-leading array assignments
and `eval` already work and must stay working.

## Root R2 — alias expansion when the command word follows a redirection

`alias foo=echo; < /dev/null foo bar` → bash prints `bar` (expands `foo`);
huck `foo: command not found`. Also: `> /dev/null a` (a is an alias),
`eval '</dev/null e ok 3'`, `a=true e ok 4`.

`parse_command` (parser.rs:3279) calls `iter.expand_command_alias()` **once**,
right after skipping leading newlines — i.e. only at the absolute command
start. When leading redirections precede the command word
(`< /dev/null foo`), the first token is the redirection operator (not a
command-name `Lit`), so `expand_command_alias` is a no-op there; the parser
then consumes the redirection and reaches `foo`, but never re-invokes alias
expansion for it. bash expands an alias in command-word position regardless of
preceding redirections.

**Fix:** in the simple-command parse path, drive `expand_command_alias()` for
the command word after any leading redirections are consumed — i.e. whenever
the parser is about to read the command **word** and none has been seen yet,
not only at the very first token. The exact site is the word/redirection
interleaving loop of the simple-command parser (`parse_word_command` and/or the
`parse_command` entry): before classifying the first *word* token as the
command name, attempt alias expansion on it. Must fire only for the command
word (the first word of the simple command), matching bash — a redirection
target or a later argument is not alias-expanded.

## Root R3 — alias whose expansion begins with `#` starts a comment

`alias comment='#'; comment` → bash treats the expanded `#` as a comment
(no-op); huck runs `#` as a command (`#: command not found`). Also
`alias long_comment='# for x in '; long_comment text after` → bash: whole
expanded line is a comment.

huck's comment recognition is gated at lexer.rs:4280
(`Some('#') if self.cmd_at_word_start => …`). When the alias body `#` is
injected and scanned, the `#` is not recognized as a comment introducer — the
injected `#` either arrives with `cmd_at_word_start == false` or is scanned as
a word before the comment gate applies.

**Fix:** the leading `#` of an injected alias body (at a word boundary /
command-word-start) must be recognized as a comment introducer, exactly as a
literal `#` at command start is. The implementer traces whether
`cmd_at_word_start` is set correctly for the injected body's first char and
routes the `#` to the existing comment-scan path. Comment semantics
(rest-of-line, including further injected text and any following real source on
the same logical line — bash consumes to end of line) must match bash;
`alias x='# for x in '; x text after` confirms the trailing real text after the
alias is also swallowed by the comment.

## Verification

- **Official `alias` runner** produces zero diff (the flip signal).
- **Diff-check harness** `alias_diff_check.sh` with one fragment per root plus
  regression guards (all require `shopt -s expand_aliases`):
  - R1: leading `a=(1 2 3)` in an alias body; leading `a+=(…)`; regression —
    NON-leading array-in-alias (`echo x; a=(1 2)`) and a plain scalar alias
    still work; `eval "a=(…)"` still works.
  - R2: `< /dev/null foo bar`; `> /dev/null a`; `a=true e ok`; regression — a
    redirection target that happens to match an alias name is NOT expanded.
  - R3: `alias c='#'; c`; `alias lc='# x '; lc text after`; regression — a
    literal mid-word `#` (`echo a#b`) is unaffected.
- **Unit tests** in the lexer/parser crates for each root where a focused test
  is natural (leading-body-word `ArrayOpen`; command-word alias after a
  redirection; injected `#` comment).
- **No-regression:** full bash-suite runner PASS **32 → 33**, per-category diff
  vs the v344 baseline (exactly the 32 + `alias`, nothing regressed — R2 touches
  the shared command-word parse path, so verify the alias/command/redirect
  categories explicitly); `tests/scripts/run_diff_checks.sh` green; per-crate
  lib tests + the alias/redirect/command `-p huck` integration bins.

## Scope / non-goals

- Only the three roots above. Other alias behaviors already pass
  (`alias2/3/5/6.sub`) and must stay passing.
- No rework of the alias-injection architecture beyond what each root needs.

## Summary of touched files

- `crates/huck-syntax/src/lexer.rs` — R1 (injected-body leading-word `ArrayOpen`
  probe) and R3 (injected `#` comment recognition).
- `crates/huck-syntax/src/parser.rs` — R2 (alias expansion for the command word
  after leading redirections).
- `tests/scripts/alias_diff_check.sh` (new).
