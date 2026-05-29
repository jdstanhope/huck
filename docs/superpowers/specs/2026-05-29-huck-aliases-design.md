# huck v48 — Aliases (M-63)

## Goal

Add bash-style aliases to huck:

- `alias name=value` defines.
- Bare `alias` lists; `alias name` shows one.
- `unalias name [...]` removes; `unalias -a` clears all.
- Aliases expand at command position before parsing.
- Recursive expansion with cycle protection.
- Bash trailing-space rule: if alias body ends in whitespace, the
  next word is also alias-eligible.
- Aliases expand only for **interactive REPL input**, not inside
  function bodies, trap actions, or non-interactive script
  execution (bash defaults).

New tracked divergence: **M-63: Aliases**, added to
`docs/bash-divergences.md`.

## Scope decisions (locked)

1. **Feature scope**: full bash — define/list/remove + recursive
   expansion with cycle protection + trailing-space rule.
2. **Expansion site**: only when `shell.is_interactive == true` AND
   the caller passes `expand_aliases: true`. The REPL passes
   `shell.is_interactive`; trap firings pass `false`.

## Out of scope (deferred)

- `shopt -s expand_aliases` to override the interactive-only
  default. Would require a `shell.alias_expand_override: bool`
  field and updates at every `process_line` callsite. Future
  iteration.
- Aliases defined inside function bodies. Bash 4+ allows this;
  huck's `expand_aliases: bool` gate excludes function bodies.
- `alias -p` POSIX print form (equivalent to bare `alias`).

## Architecture

Four pieces:

1. **`src/shell_state.rs`** — new `pub aliases: HashMap<String,
   String>` field on `Shell`. Initialize in `Shell::new` as an
   empty map.

2. **`src/alias_expand.rs`** — new module with:
   - `pub fn expand_aliases_in_tokens(tokens: Vec<Token>,
     aliases: &HashMap<String, String>) -> Result<Vec<Token>,
     LexError>`.
   - Internal helpers for command-position tracking, cycle-
     protected recursive expansion, trailing-space detection,
     and "is this Word a plain identifier?" reconstruction.

3. **`src/builtins.rs`** — add `alias` and `unalias` builtins +
   dispatch arms in `run_builtin`.

4. **`src/shell.rs::process_line`** — add an `expand_aliases:
   bool` parameter. Update the two call sites in `src/shell.rs`
   (REPL → `shell.is_interactive`) and three call sites in
   `src/traps.rs` (all `false`).

### Field declaration

```rust
// In src/shell_state.rs, alongside vars/functions/aliases:
pub aliases: std::collections::HashMap<String, String>,
```

`Shell::new` initializes it to `HashMap::new()`.

### `expand_aliases_in_tokens` algorithm

```rust
pub fn expand_aliases_in_tokens(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<Vec<Token>, LexError> {
    let mut out = Vec::new();
    let mut next_eligible = true; // first position
    let mut active: HashSet<String> = HashSet::new();
    for token in tokens {
        next_eligible = process_one_token(
            token, &mut out, next_eligible, aliases, &mut active,
        )?;
    }
    Ok(out)
}
```

`process_one_token` returns the new value of `next_eligible`:

```rust
fn process_one_token(
    token: Token,
    out: &mut Vec<Token>,
    eligible: bool,
    aliases: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> Result<bool, LexError> {
    match &token {
        Token::Word(w) => {
            if eligible {
                if let Some(name) = simple_word_text(w) {
                    if !active.contains(&name)
                        && let Some(body) = aliases.get(&name).cloned()
                    {
                        active.insert(name);
                        let inner_tokens = crate::lexer::tokenize(&body)?;
                        let mut inner_next = true;
                        for inner in inner_tokens {
                            inner_next = process_one_token(
                                inner, out, inner_next, aliases, active,
                            )?;
                        }
                        let trailing = body.chars().last()
                            .is_some_and(|c| c.is_whitespace());
                        active.remove(active.iter().next_back().unwrap().as_str());
                        // Note: HashSet doesn't have indexed removal — the
                        // implementer must thread the name string through
                        // explicitly. See impl note below.
                        return Ok(trailing);
                    }
                }
            }
            out.push(token);
            Ok(false)
        }
        Token::Op(op) => {
            let separator = matches!(op,
                crate::lexer::Operator::Pipe
                | crate::lexer::Operator::And
                | crate::lexer::Operator::Or
                | crate::lexer::Operator::Semi
                | crate::lexer::Operator::Background
                | crate::lexer::Operator::LParen
            );
            out.push(token);
            Ok(separator)
        }
        Token::Newline => {
            out.push(token);
            Ok(true)
        }
        _ => {
            out.push(token);
            Ok(eligible)
        }
    }
}
```

The HashSet removal above is awkward. The implementer should thread
the `name` `String` through as a local variable before the
recursive call:

```rust
if let Some(body) = aliases.get(&name).cloned() {
    active.insert(name.clone());
    // ... recursive expansion ...
    let trailing = body.chars().last().is_some_and(|c| c.is_whitespace());
    active.remove(&name);
    return Ok(trailing);
}
```

### `simple_word_text` helper

```rust
fn simple_word_text(w: &Word) -> Option<String> {
    // Returns the concatenated literal text iff EVERY part is an
    // unquoted Literal. Otherwise None (aliases never expand
    // quoted, parameter-expanded, command-substituted, etc).
    let mut text = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text: t, quoted: false } => text.push_str(t),
            _ => return None,
        }
    }
    if text.is_empty() { None } else { Some(text) }
}
```

### `alias` builtin

```rust
fn builtin_alias(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut names: Vec<&String> = shell.aliases.keys().collect();
        names.sort();
        for name in names {
            let _ = writeln!(out, "alias {}='{}'", name, escape_alias_value(&shell.aliases[name]));
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_err = false;
    for arg in args {
        if let Some(eq) = arg.find('=') {
            let name = &arg[..eq];
            let value = &arg[eq + 1..];
            if !is_valid_alias_name(name) {
                eprintln!("huck: alias: `{name}': invalid alias name");
                any_err = true;
                continue;
            }
            shell.aliases.insert(name.to_string(), value.to_string());
        } else {
            match shell.aliases.get(arg) {
                Some(v) => {
                    let _ = writeln!(out, "alias {}='{}'", arg, escape_alias_value(v));
                }
                None => {
                    eprintln!("huck: alias: {arg}: not found");
                    any_err = true;
                }
            }
        }
    }
    ExecOutcome::Continue(if any_err { 1 } else { 0 })
}

fn escape_alias_value(v: &str) -> String {
    // Escape single quotes for embedding inside single quotes:
    // ' becomes '\''.
    v.replace('\'', r#"'\''"#)
}

fn is_valid_alias_name(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('=')
        && s.chars().all(|c| !c.is_whitespace() && !"|&;<>()$`\\\"'*?[]#~{}".contains(c))
}
```

### `unalias` builtin

```rust
fn builtin_unalias(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        eprintln!("huck: unalias: usage: unalias [-a] name [name ...]");
        return ExecOutcome::Continue(2);
    }
    if args[0] == "-a" {
        shell.aliases.clear();
        return ExecOutcome::Continue(0);
    }
    let mut any_err = false;
    for name in args {
        if shell.aliases.remove(name).is_none() {
            eprintln!("huck: unalias: {name}: not found");
            any_err = true;
        }
    }
    ExecOutcome::Continue(if any_err { 1 } else { 0 })
}
```

### Dispatch arms in `run_builtin`

Add `"alias" => builtin_alias(args, out, shell)` and `"unalias" =>
builtin_unalias(args, shell)` to the `match name { ... }` block at
`src/builtins.rs:46-61`.

Also add `"alias"` and `"unalias"` to the `is_builtin` and
`BUILTIN_NAMES` table (look for the existing list of names at
`src/builtins.rs:21-34` and add to it).

### `process_line` signature change

Current:
```rust
pub fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome
```

New:
```rust
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome
```

Inside the function, between `tokenize` and `parse`:

```rust
let tokens = match lexer::tokenize(line) {
    Ok(tokens) => tokens,
    Err(e) => { /* unchanged */ }
};
let tokens = if expand_aliases {
    match crate::alias_expand::expand_aliases_in_tokens(tokens, &shell.aliases) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("huck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    }
} else {
    tokens
};
match command::parse(tokens) { /* unchanged */ }
```

### Call-site updates

- `src/shell.rs:71` (REPL): `process_line(&buffer, &mut shell, shell.is_interactive)`. Note borrowing: read `shell.is_interactive` into a local first if the borrow checker complains:
  ```rust
  let do_alias = shell.is_interactive;
  match process_line(&buffer, &mut shell, do_alias) {
  ```
- `src/traps.rs:70, 82, 116` (three trap firings): `process_line(&action, shell, false)`.

## Test plan

### Unit tests in `src/alias_expand.rs#[cfg(test)] mod tests`

8 tests:

1. `simple_expansion` — alias `ll=ls -l`; tokenize `"ll"`, expand,
   re-tokenize source `"ls -l"` and compare tokens.
2. `no_expansion_outside_command_position` — alias `ll=ls -l`;
   tokenize `"echo ll"`, expand → `echo ll` unchanged.
3. `recursive_expansion` — alias `ls=ls --color`, alias `ll=ls -l`;
   tokenize `"ll"`, expand → `ls --color -l`.
4. `cycle_protection` — alias `ls=ls --color`; tokenize `"ls"`,
   expand → `ls --color` (only one substitution; the inner `ls`
   does NOT re-expand).
5. `expansion_after_pipe` — alias `ll=ls -l`; tokenize `"cat | ll"`,
   expand → cat | ls -l.
6. `expansion_after_semi` — alias `ll=ls -l`; tokenize
   `"true; ll"`, expand → `true; ls -l`.
7. `trailing_space_chains_expansion` — alias `sudo=sudo `, alias
   `ll=ls -l`; tokenize `"sudo ll"`, expand → `sudo ls -l`.
8. `quoted_word_not_expanded` — alias `ll=ls -l`; tokenize
   `"'ll'"`, expand → unchanged (single-quoted Literal is not a
   plain identifier per `simple_word_text`).

### Unit tests in `src/builtins.rs#[cfg(test)] mod alias_tests`

8 tests:

9. `alias_no_args_lists_empty` — empty `Shell.aliases` → empty
   output, status 0.
10. `alias_no_args_lists_sorted` — pre-load 3 aliases out of order;
    output is sorted lines.
11. `alias_defines_simple` — `alias ll=ls -l` → `shell.aliases`
    contains the entry.
12. `alias_lookup_existing_prints` — pre-load `ll=ls -l`;
    `alias ll` → stdout `alias ll='ls -l'\n`, status 0.
13. `alias_lookup_missing_status_1` — `alias xyz` → status 1.
14. `unalias_removes_existing` — pre-load; `unalias ll` → gone,
    status 0.
15. `unalias_missing_errors_status_1`.
16. `unalias_dash_a_clears_all` — pre-load 3; `unalias -a` →
    empty.

### Integration tests at `tests/aliases_integration.rs`

3 binary-driven tests:

1. `alias_expansion_via_repl` — script: `alias ll='echo HELLO'\nll\nexit\n`. Stdout contains a line `HELLO`.
2. `unalias_removes_expansion` — script: `alias ll='echo HELLO'\nunalias ll\nll\necho $?\nexit\n`. The third command `ll` should fail (not found); stderr or status 127 visible.
3. `recursive_alias_chain` — script: `alias l='ll'\nalias ll='echo INNER'\nl\nexit\n`. Stdout contains `INNER`.

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Foundation + builtins + unit tests**:
   - Add `pub aliases: HashMap<String, String>` field to `Shell`
     in `src/shell_state.rs`; initialize in `new`.
   - Create `src/alias_expand.rs` with `expand_aliases_in_tokens`
     + helpers + 8 unit tests.
   - Wire `pub mod alias_expand;` into `src/main.rs` between
     `mod arith;` and `mod brace_expand;` (alphabetical).
   - Add `alias` and `unalias` builtins in `src/builtins.rs`
     including dispatch + is_builtin/BUILTIN_NAMES updates + 8
     unit tests.
   - Change `process_line` signature; update 1 REPL call site +
     3 trap call sites.

2. **Integration tests**: create `tests/aliases_integration.rs`
   with 3 tests.

3. **Docs**:
   - Add **M-63: Aliases** entry to `docs/bash-divergences.md`.
   - Change-log entry.
   - README v48 row.
   - Remove "aliases" from "Not yet implemented" stanza.

Three tasks. TDD per task.

## Acceptance criteria

- All 16 unit tests pass.
- All 3 integration tests pass.
- `cargo test --all-targets` passes (modulo PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` has the new M-63 entry as
  `[fixed v48]`.
- `alias ll='ls -l'; ll` works at the interactive REPL.
- `alias` lists all defined aliases sorted; bare `alias name`
  shows one or errors.
- `unalias ll`, `unalias -a` work.
- Trap actions do NOT undergo alias expansion (no regression in
  trap tests).
- The pre-v48 `process_line` callers remain compatible after the
  signature change.
