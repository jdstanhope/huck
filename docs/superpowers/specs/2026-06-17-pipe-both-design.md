# v179: `|&` pipe stdout+stderr (M-51) — Design

**Status:** approved 2026-06-17
**Iteration:** v179
**Origin:** Tracked divergence **M-51** (`[deferred]`, low): `cmd1 |& cmd2` is a
bash syntax error in huck. bash defines `|&` as shorthand for `2>&1 |` — the
left command's stderr is merged into the pipe. Surfaced by the parse sweep
(`test_cpuset_prs.sh` uses `(echo $$ > cgroup.procs) |& cat`).

## Goal

Support `cmd1 |& cmd2` ≡ `cmd1 2>&1 | cmd2` for all stage kinds, resolving M-51,
with no new AST/parser/executor machinery.

## Problem

The lexer's `'|'` arm (`src/lexer.rs:633`) handles only `||` (→ `Operator::Or`)
and `|` (→ `Operator::Pipe`); a following `&` is not recognized, so `|&` lexes as
`Pipe` then `Background`, which the parser rejects ("expected a command"). bash
treats `|&` as `2>&1 |`.

huck already parses and executes the desugar target correctly (verified):
`sh -c 'echo O; echo E 1>&2' 2>&1 | cat` and `{ echo o; echo e 1>&2; } 2>&1 | cat`
both match bash, and v176 lets a compound stage carry the `2>&1`. And `2>&1`
lexes to exactly three tokens — `Token::RedirFd(RedirFd::Number(2))`,
`Token::Op(Operator::DupOut)`, `Token::Word("1")`.

## Design

**Lexer-level desugar.** In the `'|'` arm, after the existing left-word flush,
add a branch for `|&` that emits the token sequence for `2>&1` followed by a
`Pipe`:

```rust
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::Or));
                    push_pos!(c_off, c_line);
                } else if chars.peek() == Some(&'&') {
                    // `|&` is bash shorthand for `2>&1 |`: merge the left command's
                    // stderr into the pipe, then pipe. Desugar at the token level so
                    // the existing pipeline/redirect machinery (incl. v176
                    // compound-stage redirects) handles it unchanged.
                    chars.next(); // consume the '&' of `|&`
                    tokens.push(Token::RedirFd(crate::command::RedirFd::Number(2)));
                    push_pos!(c_off, c_line);
                    tokens.push(Token::Op(Operator::DupOut));
                    push_pos!(c_off, c_line);
                    tokens.push(Token::Word(Word(vec![WordPart::Literal {
                        text: "1".to_string(),
                        quoted: false,
                    }])));
                    push_pos!(c_off, c_line);
                    tokens.push(Token::Op(Operator::Pipe));
                    push_pos!(c_off, c_line);
                } else {
                    tokens.push(Token::Op(Operator::Pipe));
                    push_pos!(c_off, c_line);
                }
```

(The original arm did a single `push_pos!(c_off, c_line)` after the if/else; this
moves a `push_pos!` into each branch so every pushed token gets a position at the
`|&` site. `RedirFd`, `Operator::DupOut`, `Word`, and `WordPart::Literal` are
already in scope in `lexer.rs`.)

### Why this shape

`|&` is *defined* as `2>&1 |`; implementing it at the token level means the
parser, the v176 compound-stage-redirect support, and the executor all handle it
with zero changes. A Simple producer stays a `Command::Simple` (with a `2>&1`
redirect appended by the parser), so `ls |& grep` runs `ls` via the normal
external-spawn path exactly like `ls 2>&1 | grep` — no forced forked wrapper, no
new `Operator` variant, no new parser arm, no executor change.

### Behavior

- `cmd1 |& cmd2` ≡ `cmd1 2>&1 | cmd2` — the producer's stdout AND stderr go to the
  consumer. Works for Simple and compound producers (`{ … } |& c`, `( … ) |& c`).
- Chains: `a |& b |& c` ≡ `a 2>&1 | b 2>&1 | c`.
- `cmd |&` with no following stage → the existing trailing-`|` error (bash also
  errors).
- Unaffected: `||` (logical or), `|` (plain pipe), `&` (background) — the `&`
  branch is reachable only immediately after a `|`.

## Verification

- **New bash-diff harness** `tests/scripts/pipe_both_diff_check.sh`: executing,
  byte-identical bash↔huck (stdout+exit). Cases: a producer writing to both
  streams `|& cat` (both lines appear), `|& grep <stderr-text>` (matches the
  stderr line), a compound producer `{ echo o; echo e 1>&2; } |& cat`, a subshell
  producer `( echo o; echo e 1>&2 ) |& cat`, a chain `… |& … |& cat`, and
  controls confirming plain `|` and `||` are unaffected.
- **Parse-sweep:** re-run `tools/parse_sweep.sh tools/scripts.tsv`; confirm
  `test_cpuset_prs.sh` (the `|&` user) now parses (it also needed v177, landed),
  and report `HUCK_GAP` movement / any other `|&` users.
- Full `cargo test` (0 failures) — including existing pipeline/lexer tests; add a
  lexer unit test that `tokenize("a |& b")` yields `[Word(a), RedirFd(2), DupOut,
  Word(1), Pipe, Word(b)]`. clippy clean; all `tests/scripts/*_diff_check.sh`
  green.

## Docs / close-out

Resolves M-51: **delete** the M-51 entry from `docs/bash-divergences.md` and
decrement the Tier-4 / missing-features count by 1. Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`.

## Scope boundary

In scope: the lexer `|&` desugar, the new harness + a lexer unit test, the M-51
doc removal. **Not** in scope: any AST/parser/executor change (none needed); a
dedicated `PipeBoth` operator (rejected in favor of the token-level desugar);
other clusters. No behavior change to `|`, `||`, or `&`.
