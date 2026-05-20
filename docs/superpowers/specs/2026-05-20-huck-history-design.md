# huck v13: Command History and History Expansion

**Date:** 2026-05-20
**Status:** Design

## Goal

Add persistent command history and `!`-style history expansion to
huck. History survives across sessions via a histfile; the `history`
builtin lists and clears it; and the interactive loop expands event
designators (`!!`, `!n`, `!string`, `!$`, etc.) before executing a
line.

## Scope

**In scope:**
- A `History` store: in-memory entry list with absolute numbering,
  capped at 1000 entries
- Persistence: load at startup, save at every exit, file path from
  `$HISTFILE` or `$HOME/.huck_history`
- `history` builtin: list (numbered) and `history -c` (clear)
- History expansion event designators:
  - `!!` — the previous command
  - `!n` — the entry with absolute number `n`
  - `!-n` — the entry `n` back from the end
  - `!string` — most recent entry starting with `string`
  - `!$` — last word of the previous command
  - `!^` — first argument (second word) of the previous command
  - `!*` — all words after the first, space-joined, of the previous
    command
  - `^old^new^` — quick substitution on the previous command
- Bash-exact quoting: `!` expands everywhere except inside `'...'`;
  `\!` escapes it; `!` before whitespace / `=` / `(` / end-of-line
  is literal
- Expanded lines are echoed before running and stored (in expanded
  form) in history

**Out of scope (deferred):**
- Word designators (`!!:2`, `!!:^`, `!!:$`, `!!:2-4`, `!!:*`)
- Modifiers (`:h`, `:t`, `:r`, `:e`, `:p`, `:s/old/new/`, `:&`, `:g`)
- `!?string?` (containment search), `!#` (current line)
- `HISTSIZE` / `HISTFILESIZE` environment variables (fixed cap of 1000)
- `histappend`, `HISTCONTROL`, `HISTIGNORE`, timestamps
- `fc` builtin
- Re-expansion of substituted text (expansion is a single pass)

## Architecture

A new `src/history.rs` module owns the `History` struct, the
`HistError` type, and the free `expand` function. `History` is a field
on `Shell` (so the `history` builtin can reach it via `&mut Shell`).
The interactive loop in `src/shell.rs` calls `history::expand` on each
raw input line before parsing, echoes the expanded form, records it,
and runs it.

### `History` struct

```rust
pub struct History {
    entries: Vec<String>,    // oldest first
    base_number: usize,      // absolute number of entries[0]
    max: usize,              // cap; fixed at 1000
    file: Option<PathBuf>,   // resolved histfile path; None disables persistence
}
```

**Absolute numbering.** The entry at index `i` has display number
`base_number + i`. A fresh `History` has `base_number == 1`. When
`add` pushes past `max`, the oldest entry is removed and
`base_number += 1`, so live entries keep stable numbers. `clear`
empties `entries` and resets `base_number` to 1.

**Constructor.** `History::new()` resolves `file`:
1. If `$HISTFILE` is set and non-empty → that path.
2. Else if `$HOME` is set and non-empty → `$HOME/.huck_history`.
3. Else → `None` (persistence silently disabled).

`entries` starts empty; `base_number = 1`; `max = 1000`.
`History` derives `Clone` (all fields are `Clone`).

**Methods:**

```rust
impl History {
    pub fn new() -> Self;
    pub fn add(&mut self, line: String);          // push; evict oldest past max
    pub fn get(&self, number: usize) -> Option<&str>;   // absolute-number lookup
    pub fn last(&self) -> Option<&str>;           // most recent entry
    pub fn search_prefix(&self, prefix: &str) -> Option<&str>; // most recent starting with
    pub fn entries(&self) -> impl Iterator<Item = (usize, &str)>; // (number, line)
    pub fn clear(&mut self);
    pub fn load(&mut self);                       // read file into entries
    pub fn save(&self);                           // write entries to file
}
```

- `add` does not deduplicate and does not skip blank lines — the
  caller is responsible for not adding blank lines (it already checks
  `trim().is_empty()`).
- `get(number)`: if `number < base_number` or
  `number >= base_number + entries.len()`, returns `None`; else
  `entries[number - base_number]`.
- `search_prefix`: iterates `entries` from newest to oldest, returns
  the first whose text `starts_with(prefix)`.
- `load`: if `file` is `None`, no-op. Otherwise read the file; each
  line is one entry. If the file is missing, treat as empty (no
  error). Keep only the most recent `max` lines. `base_number` stays
  `1`. Errors other than not-found (e.g. permission) print a warning
  to stderr and leave history empty.
- `save`: if `file` is `None`, no-op. Otherwise write `entries`, one
  per line, each terminated by `\n`, overwriting the file. A write
  error prints a warning to stderr; it never aborts the shell.

### `history` builtin

Added to the `builtins.rs` dispatch table.

- `history` (no args) — print each entry as `{number:>5}\t{command}`
  (number right-aligned in a 5-wide field, then a tab, then the
  command). Exit status 0.
- `history -c` — call `History::clear()`. Exit status 0.
- Any other argument — print `huck: history: invalid option`
  to stderr; exit status 1.

### History expansion

```rust
pub fn expand(line: &str, history: &History)
    -> Result<Option<String>, HistError>;

pub enum HistError {
    EventNotFound(String),  // the offending token, e.g. "!foo", "!99"
    Substitution(String),   // the `old` text from ^old^new^ not found
}
```

- `Ok(None)` — nothing expanded; caller runs the original line.
- `Ok(Some(s))` — at least one expansion fired; caller echoes `s`
  and runs it.
- `Err(_)` — a referenced event/substitution failed; caller prints
  the error and does NOT run the line.

`HistError`'s `Display`:
- `EventNotFound(t)` → `{t}: event not found`
- `Substitution(o)` → `{o}: substitution failed`

**Fast path.** If `line` contains no `!` and, after skipping leading
ASCII blanks, does not begin with `^`, return `Ok(None)` without
further work.

**Quick substitution `^old^new^`.** Recognized only when the first
non-blank character of the line is `^`. Parse three `^`-delimited
fields: `^old^new^` or `^old^new` (trailing `^` optional). Take the
previous command (`history.last()`); if there is none, return
`Err(Substitution(old))`. Replace the first occurrence of `old` with
`new`; if `old` does not occur, return `Err(Substitution(old))`.
Return `Ok(Some(result))`. Any text after the closing `^` (bash
allows trailing modifiers there) is appended verbatim — but since
modifiers are out of scope, treat trailing text as a literal suffix.

**The `!`-scanner.** Walk the line character by character with two
mutually-exclusive quote flags:

- `in_single`: toggled by `'` only when `!in_double`.
- `in_double`: toggled by `"` only when `!in_single`.

Maintain an `escaped` flag set by a preceding unescaped `\`.

A `!` triggers expansion **unless** any of:
- `in_single` is true (inside single quotes), OR
- `escaped` is true (preceded by `\`), OR
- the next character is whitespace, `=`, `(`, or there is no next
  character (end of line).

`!` *does* expand inside double quotes — this matches bash.

When a `!` triggers, classify by the characters that follow:

| Lookahead | Event |
|---|---|
| `!` | previous command (`history.last()`) |
| `$` | last whitespace-word of the previous command |
| `^` | second word of the previous command |
| `*` | words `[1..]` of the previous command, space-joined |
| `-` then digits | `!-n`: entry `n` back from the end |
| digits | `!n`: entry with absolute number `n` |
| other | `!string`: read the token, prefix-search history |

For `!string`, the token is the maximal run of characters after `!`
that are not whitespace, not a quote (`'` or `"`), and not `!`.

Resolution:
- `!!` → `history.last()`; `None` → `Err(EventNotFound("!!"))`.
- `!n` → `history.get(n)`; `None` → `Err(EventNotFound("!{n}"))`.
- `!-n` → entry at `base_number + entries.len() - n`; equivalently
  `history.get(last_number + 1 - n)`. Out of range →
  `Err(EventNotFound("!-{n}"))`. `!-1` equals `!!`.
- `!string` → `history.search_prefix(string)`; `None` →
  `Err(EventNotFound("!{string}"))`.
- `!$` → split `history.last()` on ASCII whitespace; last word. No
  previous command → `Err(EventNotFound("!$"))`. A one-word previous
  command yields that word.
- `!^` → second word of the split. No previous command →
  `Err(EventNotFound("!^"))`. Fewer than two words → empty string.
- `!*` → words `[1..]` space-joined. No previous command →
  `Err(EventNotFound("!*"))`. Fewer than two words → empty string.

The scanner copies non-triggering characters through verbatim and
substitutes resolved text for each trigger. Substituted text is **not**
re-scanned. If at least one trigger fired, return `Ok(Some(result))`;
otherwise `Ok(None)`.

**Escaping.** When the scanner sees `\` followed by `!`, it emits both
characters verbatim (`\!`) and does not treat the `!` as a trigger.
The shell lexer's normal backslash handling removes the `\` later.

## REPL integration

`src/shell.rs::run` changes. Per loop iteration:

1. `editor.readline(PROMPT)` → raw `line`.
2. `match history::expand(&line, &shell.history)`:
   - `Err(e)` → `eprintln!("huck: {e}")`; `shell.set_last_status(1)`;
     `continue` (line not run, not recorded).
   - `Ok(None)` → `to_run = line.clone()`.
   - `Ok(Some(expanded))` → `println!("{expanded}")`;
     `to_run = expanded`.
3. If `to_run.trim()` is non-empty:
   `shell.history.add(to_run.clone())` and
   `editor.add_history_entry(to_run.as_str())`.
4. `process_line(&to_run, &mut shell)` as today.

**Startup:** after `Shell::new()` and before the loop,
`shell.history.load()`. Then iterate the loaded entries and push each
into `editor` via `add_history_entry` so rustyline arrow-up recall
spans sessions.

**Exit:** call `shell.history.save()` on both exit paths — the
`ExecOutcome::Exit(code)` branch and the `Err(ReadlineError::Eof)`
branch. Factor the save into a small local closure or helper so the
call is not duplicated divergently.

## Data flow examples

Session: user runs `ls -l /tmp`, then `echo hello`, then types `!!`.

1. `expand("!!", history)` — `!` triggers, lookahead `!` → previous
   command = `"echo hello"`. Returns `Ok(Some("echo hello"))`.
2. REPL prints `echo hello`, sets `to_run = "echo hello"`.
3. `"echo hello"` is added to history (now the newest entry) and run.

User types `echo !$` after `ls -l /tmp`:

1. `expand("echo !$", history)` — copies `echo `, hits `!`, lookahead
   `$` → last word of `"ls -l /tmp"` = `"/tmp"`. Result
   `"echo /tmp"`. Returns `Ok(Some("echo /tmp"))`.
2. REPL echoes `echo /tmp`, runs it.

User types `^hello^world^` after `echo hello`:

1. First non-blank char is `^` → quick substitution. Previous command
   `"echo hello"`, replace first `hello` with `world` →
   `"echo world"`. Returns `Ok(Some("echo world"))`.

User types `!nope` with no matching history:

1. `!` triggers, token `nope`, `search_prefix("nope")` → `None`.
2. Returns `Err(EventNotFound("!nope"))`.
3. REPL prints `huck: !nope: event not found`, sets `$?` to 1, does
   not run anything.

User types `echo '!!'`:

1. `'` sets `in_single`. The `!!` inside is literal (not triggered).
   `'` clears `in_single`. No trigger fired → `Ok(None)`.
2. Line runs as-is; `echo` prints `!!`.

User types `echo "!!"`:

1. `"` sets `in_double`. `!` still triggers inside double quotes.
   `!!` → previous command. Expansion fires.
2. This matches bash (history expansion is not suppressed by double
   quotes).

## Error handling summary

| Condition | Behavior |
|---|---|
| `!n` / `!-n` / `!string` / `!!` with no match | `Err(EventNotFound)`; line not run; `$?` = 1 |
| `^old^new^` with `old` absent or no previous command | `Err(Substitution)`; line not run; `$?` = 1 |
| `!` before whitespace / `=` / `(` / end-of-line | literal `!`, no expansion |
| `!` inside `'...'` | literal |
| `!` inside `"..."` | expands (bash-compatible) |
| `\!` | literal `!` (backslash passes through to the lexer) |
| Histfile missing at load | empty history, no error |
| Histfile unreadable / unwritable | warning to stderr; shell continues |
| `$HISTFILE` and `$HOME` both unset | persistence disabled silently |
| `history` with an unknown option | stderr error; `$?` = 1 |

## Testing

**`src/history.rs` unit tests:**
- `add` appends; cap eviction drops oldest and bumps `base_number`
- `get` by absolute number: in range, below range, above range
- `search_prefix` returns the most recent match
- `clear` empties entries and resets `base_number` to 1
- `load` / `save` round-trip through a `tempfile` path; missing file
  loads as empty; load truncates to the most recent `max`
- `expand`: fast-path no-op (no `!`, no leading `^`); `!!`; `!n`
  (valid and out-of-range); `!-n` (including `!-1` == `!!`);
  `!string` (hit and miss); `!$`, `!^`, `!*` (with and without
  arguments); `^old^new^` and `^old^new`; `^` substitution failure;
  `!` literal before whitespace/`=`/EOL; `!` inside `'...'` literal;
  `!` inside `"..."` expands; `\!` escape; `'` inside `"..."` does not
  open a single-quote region; multiple triggers in one line

**`src/builtins.rs` tests:**
- `history` lists entries with numbers in the expected format
- `history -c` empties the list
- `history --bad` returns exit status 1 and writes to stderr

**Integration tests (`tests/history_integration.rs`):**
- `!!` reruns the previous command
- `echo !$` substitutes the last argument
- `^a^b^` performs quick substitution
- `!missing` prints `event not found` and does not run the line
- `history` lists prior commands; `history -c` clears them
- Persistence: launch one huck process with `$HISTFILE` set to a temp
  file and run commands; launch a second process with the same
  `$HISTFILE`; confirm `history` in the second session shows the
  first session's commands

## File layout impact

- **New:** `src/history.rs` — `History`, `HistError`, `expand`
  (~350 lines including tests)
- **New:** `tests/history_integration.rs`
- **Modify:** `src/shell_state.rs` — add `history: History` field to
  `Shell`; initialize in `Shell::new()`
- **Modify:** `src/shell.rs` — `mod` use; load at startup; expand,
  echo, and record in the loop; save on exit
- **Modify:** `src/builtins.rs` — `history` builtin + dispatch entry
- **Modify:** `src/main.rs` — register `mod history`
- **Modify:** `README.md` — v13 row, features section, builtins list,
  test count
- **Cargo.toml:** no new dependencies (`tempfile` is already a
  dev-dependency)

## Open questions

None at design time.

## References

- bash(1) — HISTORY EXPANSION section (event designators)
- bash(1) — `history` builtin
- POSIX does not specify history expansion; behavior follows bash
