# huck v34 — Fatal Parameter-Expansion Errors (M-58) + Length-of-Positional (M-60) Design

**Goal:** Close two Tier 2 `[open]` divergences in one iteration:

- **M-58**: `${var:?msg}` and `${var:off:-N}`-with-negative-computed-length
  errors currently print to stderr and set `$? = 1` but DO NOT abort the
  surrounding simple command. Bash aborts the simple command AND exits the
  script in non-interactive mode.
- **M-60**: `${#1}`, `${#@}`, `${#*}` currently fail at lex time with
  `InvalidVarName` because `read_braced_name` rejects digit-only / special
  names. Bash returns the char-count of the positional (`${#1}`) or the
  count of positional args (`${#@}` / `${#*}`).

**Why bundle:** M-58 affects v32 (`${var/pat/repl}` is non-fatal — but no
fatal modifier exists there), v33 (substring `< 0`), and the pre-existing
`${var:?}` form. The fix is a single propagation mechanism shared by all
three. M-60 is a one-arm lexer extension + a one-line evaluator switch
from `shell.get` → `shell.lookup_var`; trivially small but reuses the
same evaluator-fix pattern v33 introduced. Bundling keeps the
`Length`-arm evaluator change in one commit and closes two `[open]`
items in one merge.

## Forms

### M-58 — fatal PE errors

| Source | Behavior in v34 |
|---|---|
| `${var:?msg}` when `var` unset/null | print `huck: var: msg`; set `$?=1`; **abort current simple command**; in non-interactive mode, **exit shell with status 1** |
| `${var?msg}` when `var` unset only | same, but treats `""` as set |
| `${var:offset:length}` with computed length `< 0` | print `huck: var: substring expression < 0`; abort + non-interactive exit |
| `${var:offset:length}` with bad arith in offset/length | print `huck: arithmetic: <msg>`; set `$?=1`; **stay non-fatal** (unchanged from v33) |
| All other PE errors (glob compile fail, unset var without `:?`, etc.) | unchanged — non-fatal |

### M-60 — length of special names

| Form | Result |
|---|---|
| `${#1}` | char count of `$1` (or `0` if unset) |
| `${#10}`, `${#42}` | same, for any digit-only name |
| `${#0}` | char count of `$0` (shell argv[0] or current function name) |
| `${#@}` | count of positional args (same as `${#}`) |
| `${#*}` | count of positional args (same as `${#}`) |

## Architecture: fatal-error propagation

Three-layer split:

1. **`expand_modifier` layer**: new `ExpansionResult::Fatal { status }` variant.
   Carried back to the immediate caller (`expand` / `expand_assignment` /
   `expand_pattern`). The message is `eprintln`'d before returning Fatal;
   only the status is in the variant.

2. **`Shell` side-channel**: new `pub pending_fatal_pe_error: Option<i32>`
   field. When the three `expand_*` functions see `Fatal { status }` from
   `expand_modifier`, they store the status on `Shell` and bail their
   field-construction loop.

3. **Executor + REPL drain**: `resolve()` (the executor's entry point for
   expanding command + args before fork/exec) peeks the flag after each
   `expand()` call and returns `Err(status)` if set. `execute_sequence_body`
   peeks after each pipeline. `Shell::run()`'s REPL loop drains the flag
   via `take_pending_fatal_pe_error()` after each `process_line`; if
   drained value is `Some(n)` AND `!shell.is_interactive`, exits with `n`.

**Entry guard in `expand_modifier`**: once the flag is set, every
subsequent invocation of `expand_modifier` returns `Empty` immediately
without doing work — saves having to check the flag at every `expand()`
callsite outside of `resolve()`/sequence-body.

## Interactive vs non-interactive

New `pub is_interactive: bool` field on `Shell`, set in `Shell::new()`
via:

```rust
use std::io::IsTerminal;
let is_interactive = std::io::stdin().is_terminal();
```

Stable in stdlib since Rust 1.70. No new dependency.

- `is_interactive = true` (huck started in a TTY): fatal PE error aborts
  the current simple command and bails the rest of the logical command;
  REPL loop drains the flag, sets `$? = status`, returns to prompt.
- `is_interactive = false` (piped stdin, script execution, cargo test
  runs): same per-command behavior, but the REPL loop also exits the
  shell with `status` after the logical command's pipelines stop.

This matches bash: `unset X; ${X:?missing}` in interactive mode prints
the error and returns to prompt; in non-interactive (piped) mode it
exits the script.

## AST

No AST changes. The fatal/non-fatal distinction lives entirely in the
evaluator layer; the existing `ParamModifier::ErrorIfUnset` and
`ParamModifier::Substring` variants stay shape-identical.

## Lexer

One arm changes: `Some('#')` in `parse_braced_param` at `src/lexer.rs:1027`.

Current behavior: after consuming `#`, if next is `}` → emit
`Var { name: "#" }`; otherwise `read_braced_name(chars)` reads a regular
identifier name, then expect `}`.

New behavior: after consuming `#`, dispatch on the next char:

```rust
let next = chars.peek().copied();
if next == Some('}') {
    // ${#} — bare hash, count of positionals.
    chars.next();
    parts.push(WordPart::Var { name: "#".to_string(), quoted });
    return Ok(());
}
let name = match next {
    Some(c) if c.is_ascii_digit() => {
        let mut s = String::new();
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() { s.push(d); chars.next(); } else { break; }
        }
        s
    }
    Some('@') => { chars.next(); "@".to_string() }
    Some('*') => { chars.next(); "*".to_string() }
    _ => read_braced_name(chars)?,
};
if name.is_empty() { return Err(LexError::EmptyParamName); }
if chars.next() != Some('}') { return Err(LexError::UnterminatedBrace); }
parts.push(WordPart::ParamExpansion { name, modifier: ParamModifier::Length, quoted });
return Ok(());
```

## Evaluator

### `ExpansionResult` extension

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum ExpansionResult {
    Value(String),
    Empty,
    Fatal { status: i32 },
}
```

### `Shell` fields and accessor

In `src/shell_state.rs::Shell`:

```rust
pub pending_fatal_pe_error: Option<i32>,
pub is_interactive: bool,
```

`Shell::new()` initialises both:

```rust
pending_fatal_pe_error: None,
is_interactive: std::io::stdin().is_terminal(),
```

(With `use std::io::IsTerminal;` at module top.)

Accessor on `impl Shell`:

```rust
pub fn take_pending_fatal_pe_error(&mut self) -> Option<i32> {
    self.pending_fatal_pe_error.take()
}
```

### `expand_modifier` changes

1. **Entry guard**:
   ```rust
   if shell.pending_fatal_pe_error.is_some() {
       return ExpansionResult::Empty;
   }
   ```

2. **`Length` arm** — switch to `lookup_var` and handle `@`/`*`:
   ```rust
   ParamModifier::Length => {
       let n = match name.as_str() {
           "@" | "*" => shell.positional_args.len(),
           _ => shell.lookup_var(name).unwrap_or_default().chars().count(),
       };
       ExpansionResult::Value(n.to_string())
   }
   ```

3. **`ErrorIfUnset` arm** — last lines change from
   `shell.set_last_status(1); ExpansionResult::Empty` to
   `ExpansionResult::Fatal { status: 1 }`. The eprintln stays.

4. **`Substring` arm** — the substring negative-length branch changes
   from `eprintln; set_last_status; ExpansionResult::Empty` to
   `eprintln; ExpansionResult::Fatal { status: 1 }`. The bad-arith
   branch (via `eval_arith_word`) stays Empty + `$?=1` (non-fatal).

### `expand_*` propagation

In each of `expand`, `expand_assignment`, `expand_pattern` in
`src/expand.rs`, the `WordPart::ParamExpansion` arm gains a `Fatal` case:

```rust
ExpansionResult::Fatal { status } => {
    shell.pending_fatal_pe_error = Some(status);
    return result;  // bail with whatever partial result we have
}
```

The `result` value is irrelevant because every caller will bail on the
flag.

## Executor wiring

### `resolve()` (`src/executor.rs:1118`)

After each `expand()` call, peek-check:

```rust
let prog_fields = glob_expand_fields(expand(&cmd.program, shell));
if let Some(status) = shell.pending_fatal_pe_error {
    return Err(status);
}
// ... command-not-found check ...
for word in &cmd.args {
    args.extend(glob_expand_fields(expand(word, shell)));
    if let Some(status) = shell.pending_fatal_pe_error {
        return Err(status);
    }
}
```

The flag stays set; the REPL drains it.

### `execute_sequence_body`

After each pipeline's `Continue(c)` outcome:

```rust
shell.set_last_status(c);
if shell.pending_fatal_pe_error.is_some() {
    return ExecOutcome::Continue(c);
}
```

This bails the rest of `;`/`&&`/`||` chains in the logical command, even
in interactive mode. Matches bash.

### REPL drain (`src/shell.rs::run()`)

After `process_line` returns and the existing `set_last_status` call:

```rust
ExecOutcome::Continue(status) => {
    shell.set_last_status(status);
    if let Some(fatal_status) = shell.take_pending_fatal_pe_error() {
        if !shell.is_interactive {
            shell.history.save();
            return fatal_status;
        }
        // Interactive: $? already set; fall through to next prompt.
    }
}
```

## Error handling matrix

| Trigger | Fatal? | Stderr | `$?` after | Aborts current simple command? | Aborts rest of `;`/`&&`/`||` sequence? | Exits shell (non-interactive)? |
|---|---|---|---|---|---|---|
| `${X:?msg}` (X unset) | yes | `huck: X: msg` | 1 | yes | yes | yes |
| `${X?msg}` (X unset) | yes | same | 1 | yes | yes | yes |
| `${X:?msg}` (X=set) | no | — | unchanged | no | no | no |
| `${s:0:-N}` neg-len | yes | `huck: s: substring expression < 0` | 1 | yes | yes | yes |
| `${s:@@@}` bad arith | no | `huck: arithmetic: ...` | 1 | no (still runs `echo` with empty field) | no | no |
| `${MISSING/foo/bar}` | no | — | unchanged | no | no | no |
| `${X#pattern}` bad pattern | no | — | unchanged | no | no | no |

## Scope (in)

- `${var:?}` / `${var?}` fatal-on-unset.
- Substring negative-computed-length fatal.
- `is_interactive` flag on Shell, set via stdin `IsTerminal::is_terminal`.
- `${#1}` / `${#10}` / `${#0}` length-of-positional.
- `${#@}` / `${#*}` count-of-positionals.
- Sequence-level abort on `pending_fatal_pe_error`.
- REPL non-interactive exit on `pending_fatal_pe_error`.

## Scope (out)

- **`set -e` / `errexit`**: separate concept (exit-on-any-nonzero). Not
  in v34.
- **Pipeline-stage fatal semantics**: `${X:?} | cat` aborts the first
  stage's `resolve()`; cat still runs and exits 0; pipeline `$?` reflects
  cat. The `pending_fatal_pe_error` still propagates to the REPL drain
  and exits the script in non-interactive mode. This matches bash.
- **Fatal errors in subshells**: `${X:?}` inside `(...)` aborts the
  subshell. The parent shell continues. (Subshell exit status is 1; the
  pending flag is local to the subshell's cloned Shell, so the parent's
  flag stays unset. This works as a side effect of v25/v28 subshell
  cloning — no special handling needed.)
- **Trap interaction**: `trap` is M-22-deferred; no `EXIT` trap fires on
  fatal PE exit.

## Testing

### Lexer unit tests (~6, in `src/lexer.rs` tests module)

- `brace_length_positional` — `${#1}` → `Length`, name `"1"`.
- `brace_length_multi_digit_positional` — `${#10}` → `Length`, name `"10"`.
- `brace_length_at` — `${#@}` → `Length`, name `"@"`.
- `brace_length_star` — `${#*}` → `Length`, name `"*"`.
- `brace_length_unchanged_for_named` — `${#foo}` regression guard.
- `brace_length_bare_hash_unchanged` — `${#}` regression guard.

### Evaluator unit tests (~9, in `src/param_expansion.rs` tests module)

M-60:
- `expand_modifier_length_at_returns_count`
- `expand_modifier_length_star_returns_count`
- `expand_modifier_length_positional_returns_char_count`
- `expand_modifier_length_unset_positional_returns_zero`

M-58:
- `expand_modifier_error_if_unset_returns_fatal`
- `expand_modifier_error_if_unset_with_message_returns_fatal_and_prints`
- `expand_modifier_substring_negative_computed_length_returns_fatal`
- `expand_modifier_substring_bad_arith_stays_empty_not_fatal` (regression
  guard)
- `expand_modifier_short_circuits_when_pending_fatal_is_set` — entry-guard
  test: pre-set `shell.pending_fatal_pe_error = Some(1)`, call any
  modifier, assert returns Empty without doing work.

### Integration tests (~10, new file `tests/pe_error_abort_integration.rs`)

- `error_if_unset_aborts_command_in_pipeline` — `${X:?missing} | cat`
  produces no echo output.
- `error_if_unset_aborts_rest_of_sequence` —
  `${X:?missing}; echo continued` doesn't print "continued".
- `error_if_unset_non_interactive_exits_shell` — script with
  `${X:?missing}\necho continued\n` exits before echo; exit status 1.
- `substring_negative_computed_length_aborts_command` —
  `${s:0:-4}` now aborts the command (the v33 test that documented the
  divergence is updated; see "Existing tests to update").
- `error_if_unset_colon_treats_empty_as_unset` — `X=""; ${X:?missing}`
  aborts.
- `error_if_unset_without_colon_only_aborts_when_unset` —
  `X=""; ${X?missing}` does NOT abort.
- `bad_arith_in_substring_stays_non_fatal` — `${s:@@@}` continues
  (regression guard).
- `length_positional_in_function` — `f() { echo ${#1}; }; f hello` → `5`.
- `length_at_star_match_hash_in_function` —
  `f() { echo ${#@},${#*},${#}; }; f x y z` → `3,3,3`.
- `error_if_unset_inside_subshell_does_not_kill_parent` —
  `(${X:?missing}); echo after` — bash: subshell exits 1, parent's
  `echo after` runs. Verify huck matches.

### Existing tests to update

- `tests/param_substring_integration.rs::substring_negative_computed_length_errors`:
  the v33 comment block explains the M-58 divergence. In v34, the test
  now needs to verify that `echo "[${s:0:-4}]"` does NOT print `[]`
  (because the simple command aborts before echo runs). Replace the
  comment and update the assertions.
- `tests/param_substring_integration.rs::substring_bad_arith_returns_empty_sets_status`:
  the v33 cross-reference is now stale (`see substring_negative_computed_length_errors
  for why $? is not asserted`). Bad-arith stays non-fatal, so update the
  comment but keep the assertions as-is — they still verify the empty
  output + stderr.

**Total new tests**: ~25. Baseline goes from 1276 → ~1301.

## Documentation

- `docs/bash-divergences.md`:
  - M-58: status flips to `[fixed v34]` with a one-line summary of the
    fatal-error propagation.
  - M-60: status flips to `[fixed v34]` with a one-line summary.
  - M-16 (substring): amend the v33 note that said "Inherits M-58
    divergence" — strike that clause.
  - Changelog row at the bottom.
- `README.md`: new v34 row in the status table.

## Implementation tasks (handoff to writing-plans)

| # | Task | Notes |
|---|---|---|
| 1 | M-60: lexer `#` arm extension + `Length` evaluator switch to `lookup_var` + `@`/`*` handling + lexer/evaluator unit tests | Self-contained; no dependency on M-58. |
| 2 | `ExpansionResult::Fatal` variant + `Shell::pending_fatal_pe_error` + `Shell::is_interactive` + `take_pending_fatal_pe_error()` accessor + `Shell::new()` init | Scaffold only; no observable behavior change yet (build-passes only). |
| 3 | `expand_modifier` entry-guard + ErrorIfUnset Fatal + Substring negative-length Fatal + evaluator unit tests | Modifier-layer behavior change. |
| 4 | Three `expand_*` propagation arms + `resolve()` peek-check + `execute_sequence_body` peek-check | Layer wiring. |
| 5 | `Shell::run()` REPL drain + non-interactive exit + update v33 substring integration tests + integration test file | Top-of-stack wiring + tests. |
| 6 | Docs: M-58 + M-60 → fixed, M-16 amend, changelog, README + full-suite verify | Mechanical close-out. |

Process: subagent-driven per the [[huck-iteration-workflow]] on the
`v34-pe-abort-and-length-positional` branch. Final code-reviewer over
the whole branch diff before `merge --no-ff` into `main`.
