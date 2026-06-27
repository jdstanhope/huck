# v232 — Command-Position-Aware Alias Expander

**Date:** 2026-06-27
**Status:** approved
**Topic:** Make alias expansion respect command position and reserved
words, fixing the v231 regression where `case` patterns get wrongly
alias-expanded.

## Problem

v231 wired alias expansion into the `source`/file path (interactive
sourcing now expands aliases, matching bash). This exposed a latent bug
in the token-level expander (`crates/huck-engine/src/alias_expand.rs`):
it has no grammar awareness, so it treats *any* word following `|` as a
command-position word eligible for alias substitution.

Inside a `case` statement, `|` separates **patterns**, not pipeline
stages. So when a sourced file defines an alias whose name also appears
as a `case` pattern word, the pattern list gets rewritten and the
statement no longer parses. Concretely, with `alias ls=...` active,
`~/.nvm/bash_completion` contains:

```sh
case "${previous_word}" in
  use | run | exec | ls | list | uninstall) __nvm_installed_nodes ;;
  alias | unalias) __nvm_alias ;;
  *) __nvm_commands ;;
esac
```

The `ls` pattern (after `|`) is rewritten to the alias body, breaking the
case → `huck: …/bash_completion: line N: syntax error`. Minimal repro:

```sh
shopt -s expand_aliases
alias ls='ls --color'
f() { case "$x" in use | ls | list) echo hi ;; *) echo no ;; esac; }
# bash: parses fine; huck: syntax error
```

### Second, opposite bug in the same code

The current expander only grants command-position eligibility after a
fixed set of operator separators (`| && || ; & (` and newline). It treats
reserved words like `then`, `else`, `do`, `{` as ordinary words, which
return "not eligible". As a result the expander **misses** legitimate
expansions that bash performs:

```sh
alias mycmd='echo hi'
if true; then mycmd; fi    # bash expands `mycmd`; huck does not
while true; do mycmd; done # same
```

Both bugs stem from the same root cause: the expander does not model
command position. Fixing command position correctly fixes both
directions.

## Goal

Make the alias expander expand an alias name **iff** the word is the
first word of a simple command — bash's actual rule — with reserved-word
recognition and compound-command (`case`, `for`/`select`, `[[ ]]`)
context tracking. One model, applied on both call sites:

- the non-interactive/source path (`builtins.rs:6380`,
  `expand_aliases_in_tokens_mapped`), and
- the REPL path (`shell.rs:415`, `expand_aliases_in_tokens`, which
  delegates to the mapped variant).

Non-goals (stay deferred, tracked under L-69): redirect-prefix command
position, `-c`/`run_cmdstring` alias wiring, same-unit def-then-use
edge, alias2/alias4 subshell infra.

## Design

### Command-position model

Replace the single threaded `next_eligible: bool` with an `Expander`
struct that owns the full expansion state:

```rust
struct Expander<'a> {
    out: Vec<Token>,
    map: Vec<usize>,
    active: HashSet<String>,   // cycle protection (unchanged semantics)
    eligible: bool,            // is the next word in command position?
    ctx: Vec<Ctx>,             // compound-command context stack
    aliases: &'a HashMap<String, String>,
}
```

`expand_aliases_in_tokens_mapped` constructs an `Expander`, feeds tokens
one at a time, and returns `(out, map)` exactly as today. The byte-offset
/ line remap contract is unchanged: alias-body tokens inherit the source
index of the name token they replaced; untouched tokens map to
themselves. `expand_aliases_in_tokens` still calls the mapped variant and
drops the map.

A word is alias-expanded **iff** it is in command position AND not
suppressed by a context AND not a reserved word AND not in the `active`
cycle set AND `simple_word_text` returns an alias name. The trailing-blank
rule (an alias body ending in whitespace makes the following token
eligible) is preserved.

### Reserved words

A reserved word in a position where it is recognized is **never**
alias-expanded, and it sets the eligibility of the following word per the
table below. Recognition happens when the word is in command position
(`eligible` true and not context-suppressed), plus the specific context
slots noted in the context section (`in`, terminators).

| Reserved word(s)                                   | Next word command position? |
|----------------------------------------------------|-----------------------------|
| `if` `then` `elif` `else` `do` `while` `until` `{` `!` `time` | yes |
| `fi` `done` `esac` `}`                             | no (separator required next) |
| `for` `select` `function`                          | no (a NAME follows)         |
| `case`                                             | no (subject follows)        |
| `in`                                               | context-dependent (below)   |
| `[[` `]]`                                           | no                          |

Operator separators that grant command position are unchanged:
`Pipe`, `And`, `Or`, `Semi`, `Background`, `LParen`, and `Newline`.
`RParen` does not (it closes a subshell) — except as the case
pattern→body transition handled by context.

The reserved-word set is matched against `simple_word_text` (unquoted
plain literals only), so quoted or expanded words are never treated as
reserved.

### Context stack

`Ctx` is a small enum; the stack handles nesting:

```rust
enum Ctx {
    CaseSubject,   // after `case`, before `in`
    CasePattern,   // pattern list: after `in`, or after ;;/;&/;;&
    CaseBody,      // clause body (normal command position resumes)
    ForName,       // after `for`/`select`, before `in`
    ForList,       // after for/select `in`, until separator or `do`
    DoubleBracket, // inside [[ ... ]]
}
```

**`case`** (the regression fix):
- command-position `case` → push `CaseSubject`; subject word not expanded.
- `in` while top is `CaseSubject` → replace top with `CasePattern`.
- in `CasePattern`: words are **not** expanded; `Pipe` keeps
  `CasePattern` (pattern alternative, does **not** grant eligibility);
  optional leading `LParen` stays in `CasePattern`; `RParen` → replace top
  with `CaseBody` and set eligible (body command position).
- in `CaseBody`: normal command-position logic applies; `DoubleSemi` /
  `SemiAmp` / `DoubleSemiAmp` → replace top with `CasePattern`; a
  command-position nested `case` pushes a new `CaseSubject`.
- `esac` recognized in `CasePattern` or command-position `CaseBody` →
  pop. Eligible is false after `esac` (a separator follows).

**`for` / `select`**:
- command-position `for` or `select` → push `ForName`; the NAME is not
  expanded.
- `in` while top is `ForName` → replace with `ForList`.
- in `ForList`: words are **not** expanded (they are the iteration list);
  a `Semi`/`Newline`, or the reserved word `do`, ends the list (pop;
  `do` then grants eligibility for the body).
- `for x; do` / `for x in; do` (no list) handled because `Semi`/`Newline`
  / `do` pop `ForName`/`ForList` regardless.

**`[[ ]]`**:
- command-position `[[` → push `DoubleBracket`; interior words not
  expanded; `]]` pops. (`(( ))` arithmetic is lexed separately and needs
  no handling.)

### Worked example (the regression)

`case "$x" in use | ls | list) echo hi ;; *) echo no ;; esac`, with
`alias ls='ls --color'`:

| token        | ctx top before | action                                  |
|--------------|----------------|-----------------------------------------|
| `case`       | (empty)        | reserved, push `CaseSubject`, elig=false |
| `"$x"`       | CaseSubject    | not simple word; push, elig=false        |
| `in`         | CaseSubject    | → `CasePattern`, elig=false              |
| `use`        | CasePattern    | suppressed; push                         |
| `\|`          | CasePattern    | stay `CasePattern`, elig=false           |
| `ls`         | CasePattern    | **suppressed — NOT expanded** ✅          |
| `\|` `list`   | CasePattern    | suppressed                               |
| `)`          | CasePattern    | → `CaseBody`, elig=true                   |
| `echo` `hi`  | CaseBody       | normal; `echo` not aliased               |
| `;;`         | CaseBody       | → `CasePattern`                          |
| `*` `)`      | CasePattern    | suppressed; `)` → `CaseBody`             |
| `echo` `no`  | CaseBody       | normal                                   |
| `;;`         | CaseBody       | → `CasePattern`                          |
| `esac`       | CasePattern    | pop; stack empty                         |

Legitimate pipeline expansion is unaffected: `cat | ll` (empty stack
throughout) still expands `ll` after the real `Pipe`.

## Testing

- **Unit tests** in `alias_expand.rs` (all existing tests stay green):
  - case pattern words not expanded (the regression);
  - `|` inside case pattern not a command separator;
  - nested `case` push/pop; `;;`/`;&`/`;;&` → pattern; optional `(pat)`;
  - expand-after-`then`/`else`/`do`/`{` (the second bug);
  - reserved words themselves not expanded;
  - `for x in ls cat; do` — list words not expanded, body word expanded;
  - `[[ ls == x ]]` — interior not expanded;
  - mapped-index/offset contract preserved across a case (map still
    points at raw source indices).
- **Diff-check harness** `tests/scripts/alias_case_diff_check.sh`: run
  case/reserved-word/alias fragments through bash and huck in file mode,
  assert byte-identical stdout+stderr+exit.
- **Regression integration test**: reproduce the `.bashrc`-style
  `case "$x" in use | ls | list) …` fragment with an alias active;
  assert it parses and runs (no syntax error).
- **Full sweep**: `cargo test --workspace` (~3770 tests) green; bash-test
  suite re-run to confirm no category regresses and to measure whether
  `alias` shifts.

## Risks

- **Under-expansion regressions**: the new model expands in *more* places
  than before (after `then`/`do`/`{`). Any existing test asserting the
  old miss must be updated to the bash-correct behavior; the full
  workspace + diff-harness sweep is the safety net.
- **Reserved-word over-matching**: only unquoted plain literals
  (`simple_word_text`) are matched, and only in recognized positions, so
  `echo case`, `grep for file`, `"do"` are unaffected.
- **Context leak across malformed input** (e.g. an alias body that opens
  but never closes a `case`): acceptable — malformed input already has
  undefined expansion; the stack resets per top-level expander call.
