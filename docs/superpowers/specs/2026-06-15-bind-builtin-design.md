# v161: the `bind` builtin (readline configuration + key rebinding)

**Status:** design approved 2026-06-15
**Scope:** a functional `bind` builtin over huck's rustyline line editor —
configurable readline *variables* that map to live rustyline config, plus real
*key rebinding* to the subset of readline functions that have a rustyline `Cmd`
equivalent. Informational listing flags. `bind -x` (key→shell-command) deferred.

## Motivation

bash's `bind` configures GNU readline: `bind 'set editing-mode vi'`,
`bind '"\C-w": backward-kill-word'`, `bind -v`/`-l`/`-p`. huck has no `bind`
(→ `command not found`), so `.bashrc`-style configs and scripts that call it
break. huck's line editor is **rustyline** (not readline), but rustyline exposes
a genuine runtime-configuration and key-binding API (`Configurer` trait,
`Editor::bind_sequence`/`unbind_sequence`, the `Cmd` enum). v161 implements a
`bind` builtin that drives that API: it really switches editing-mode, sets the
bell, and rebinds keys to the readline functions rustyline can express — and is
honest (warns/no-ops) about the functions and forms it cannot.

## Architecture

`bind` runs as a builtin in the executor (`&mut Shell`); the rustyline `Editor`
lives only in the `run()` loop in `src/shell.rs`. The seam: `bind` records its
intent on `Shell`; the run loop applies it to the live `Editor` between commands.

### `Shell.readline_settings`
A new field on `Shell`:

```rust
pub struct ReadlineSettings {
    /// Every `set VAR value` ever applied (so `bind -v` round-trips them).
    /// Seeded with the 5 mapped variables at their rustyline defaults.
    pub vars: BTreeMap<String, String>,
    /// Pending key-binding requests (keyseq, function) to apply to the editor.
    pub pending_binds: Vec<(String, String)>,
    /// Pending unbind requests (keyseq) — from `bind -r`.
    pub pending_unbinds: Vec<String>,
    /// Active bindings huck has applied, for `bind -p`/`-P` listing.
    pub active_binds: BTreeMap<String, String>, // keyseq -> function
    /// Set when `vars`/`pending_*` changed and the loop must re-sync.
    pub dirty: bool,
}
```

`bind` writes here (all strings — no rustyline types in `builtins.rs`). The run
loop drains `pending_binds`/`pending_unbinds` and applies the mapped `vars`.

### `src/readline_bind.rs` (new module)
The only new rustyline-coupled code besides the run loop. Pure, unit-testable:

- `parse_keyseq(seq: &str) -> Option<Event>` — parse readline keyseq notation
  into a rustyline `Event` (a `KeyEvent` or a multi-key sequence). Handles:
  `\C-x` (control), `\M-x` / `\e` (meta/escape), escape sequences `\e[A`/`\e[B`/
  `\e[C`/`\e[D`/`\eOH`… (arrows/home/end → rustyline `KeyCode::Up`/etc.),
  combined `\C-M-x`, octal `\nnn` / hex `\xHH`, the quoted `"…"` form, and
  literal characters. Returns `None` on an unparseable sequence.
- `function_to_cmd(name: &str) -> Option<Cmd>` — table mapping supported readline
  function names → rustyline `Cmd` (see "Function map" below). `None` for
  functions with no rustyline equivalent.
- `is_known_function(name: &str) -> bool` and `keyseq_is_valid(seq: &str) -> bool`
  — thin `bool` validators with NO rustyline types in their signatures, so
  `builtins.rs` can validate at `bind`-invocation time without importing
  rustyline.
- `readline_function_names() -> &'static [&'static str]` — the static list of
  readline function NAMES for `bind -l` (the standard readline set; informational
  even for names huck can't bind).

### Run loop (`src/shell.rs`)
After each command (where history is already synced to the editor), if
`shell.readline_settings.dirty`:
1. Apply mapped `vars` to the editor via `Configurer` (see "Variable map").
2. For each `(keyseq, function)` in `pending_binds`: `parse_keyseq` +
   `function_to_cmd`; on success `editor.bind_sequence(event, EventHandler::from(cmd))`
   and record into `active_binds`; drain the vec.
3. For each keyseq in `pending_unbinds`: `parse_keyseq` → `editor.unbind_sequence(event)`;
   remove from `active_binds`; drain.
4. Clear `dirty`.

Non-interactive runs (`huck -c …`, scripts) have no editor — `bind` still records
settings and the listing flags still work; the apply step is simply skipped.

## The `bind` builtin (`src/builtins.rs`)

Add `bind` to `BUILTIN_NAMES` and `run_builtin`. It is a regular builtin. Parse:

### Variable assignment
- `bind 'set VAR VALUE'` (the `set …` is ONE argument, bash-style) — record
  `vars[VAR] = VALUE`; if VAR is one of the 5 mapped variables, validate the
  value and set `dirty`. Unmapped variables are recorded (so `bind -v` echoes
  them) but have no editor effect.

### Key binding
- `bind 'keyseq:function'` or `bind keyseq:function` — split on the first `:`.
  Validate: `keyseq_is_valid(keyseq)` (else `huck: bind: KEYSEQ: cannot parse`,
  no record) and `is_known_function(function)` (else
  `huck: bind: FUNCTION: unknown function name`, rc nonzero like bash). On
  success push `(keyseq, function)` to `pending_binds`, set `dirty`.
- `bind -r keyseq` — push to `pending_unbinds`, set `dirty`.
- `bind -x 'keyseq:shell-command'` — **deferred**: accept the arg, no-op, rc 0
  (logged as a follow-on divergence). `bind -X` lists none.

### Listing / informational
- `bind -v` — print each variable huck models as `set NAME VALUE`
  (the 5 mapped vars at current values + any user-set entries), sorted.
- `bind -V` — verbose form: `` NAME is set to `VALUE' ``.
- `bind -l` — print `readline_function_names()`, one per line.
- `bind -p` — print active key bindings as `"KEYSEQ": FUNCTION` (from
  `active_binds`); `bind -P` — verbose (`FUNCTION can be found on "KEYSEQ".`).
  Only huck's actually-applied bindings (real, not the full readline default set).
- `bind -s` / `-S` (macros), `-q name` (query), `-u name` (unbind function),
  `-m keymap`, `-f file` (read inputrc) — accept, minimal/empty output, rc 0
  (no-op where huck has no equivalent; `-f` is deferred/no-op).

### Errors
- Unknown option (`bind -Z`) → `huck: bind: -Z: invalid option` + the usage line,
  **rc 2** (matches bash's rc; `huck:` prefix is the established stderr class).
- All stderr uses huck's `huck:` prefix; harnesses compare stdout + rc only.
- bash's non-interactive "line editing not enabled" warning is NOT replicated
  (stderr-only, not byte-compared).

## Variable map (the 5 that reconfigure the live editor)

| readline variable | values | rustyline `Configurer` setter | default |
| --- | --- | --- | --- |
| `editing-mode` | `emacs` \| `vi` | `set_edit_mode(EditMode::Emacs/Vi)` | `emacs` |
| `bell-style` | `none` \| `audible` \| `visible` | `set_bell_style(BellStyle::None/Audible/Visible)` | `audible` |
| `show-all-if-ambiguous` | `on` \| `off` | `set_completion_show_all_if_ambiguous(bool)` | `off` |
| `completion-query-items` | integer | `set_completion_prompt_limit(usize)` | `100` |
| `keyseq-timeout` | integer (ms) | `set_keyseq_timeout(Option<u16>)` | `500` |

`vars` is seeded with these 5 at their defaults so `bind -v` lists them with
bash-matching default values. An invalid value for a mapped variable
(`bind 'set editing-mode xyz'`) → `huck: bind: xyz: invalid value`-style error,
rc nonzero (match bash where reasonable), variable unchanged.

## Function map (`function_to_cmd`, the rebindable readline functions)

A table covering the common readline functions that have a rustyline `Cmd`.
Initial set (extendable; mirrors rustyline's own default keymap so the
correspondence is authoritative):

| readline function | rustyline `Cmd` |
| --- | --- |
| `beginning-of-line` | `Move(Movement::BeginningOfLine)` |
| `end-of-line` | `Move(Movement::EndOfLine)` |
| `forward-char` | `Move(Movement::ForwardChar(1))` |
| `backward-char` | `Move(Movement::BackwardChar(1))` |
| `forward-word` | `Move(Movement::ForwardWord(1, At::AfterEnd, Word::Emacs))` |
| `backward-word` | `Move(Movement::BackwardWord(1, Word::Emacs))` |
| `kill-line` | `Kill(Movement::EndOfLine)` |
| `backward-kill-line` | `Kill(Movement::BeginningOfLine)` |
| `unix-line-discard` | `Kill(Movement::BeginningOfLine)` |
| `kill-word` | `Kill(Movement::ForwardWord(1, At::AfterEnd, Word::Emacs))` |
| `backward-kill-word` | `Kill(Movement::BackwardWord(1, Word::Emacs))` |
| `unix-word-rubout` | `Kill(Movement::BackwardWord(1, Word::Big))` |
| `clear-screen` | `ClearScreen` |
| `accept-line` | `AcceptLine` |
| `previous-history` | `PreviousHistory` |
| `next-history` | `NextHistory` |
| `beginning-of-history` | `BeginningOfHistory` |
| `end-of-history` | `EndOfHistory` |
| `history-search-backward` | `HistorySearchBackward` |
| `history-search-forward` | `HistorySearchForward` |
| `reverse-search-history` | `ReverseSearchHistory` |
| `forward-search-history` | `ForwardSearchHistory` |
| `complete` | `Complete` |
| `upcase-word` | `UpcaseWord` |
| `downcase-word` | `DowncaseWord` |
| `capitalize-word` | `CapitalizeWord` |
| `transpose-chars` | `TransposeChars` |
| `transpose-words` | `TransposeWords` |
| `undo` | `Undo(1)` |
| `yank` | `Yank(1, Anchor::Before)` |
| `delete-char` | `Kill(Movement::ForwardChar(1))` |
| `backward-delete-char` | `Kill(Movement::BackwardChar(1))` |
| `self-insert` | `SelfInsert(1, …)` (special — accept) |
| `abort` | `Abort` |

(The implementer verifies each `Cmd`/`Movement` variant against rustyline 18's
actual enum — names above are from its source; the exact constructor args
(`RepeatCount`, `At`, `Word`, `Anchor`) are taken from rustyline's own default
keymap so behavior matches.) A function NOT in the table →
`huck: bind: NAME: cannot bind (no rustyline equivalent)`, the binding is not
recorded, rc reflects "couldn't fully apply" (match bash's rc where it accepts
the name; if bash rejects, match that).

## Out of scope (deferred — logged as low divergences after merge)

- **`bind -x` (key → shell command)** — needs a custom `ConditionalEventHandler`
  running a shell command mid-edit; feasible but involved. Accept-and-no-op now.
- **Readline functions with no rustyline `Cmd`** (e.g. `dump-functions`,
  `re-read-init-file`, `dabbrev-expand`, keyboard macros) — warn/no-op.
- **`.inputrc` reading (`bind -f file`)** — no-op.
- **Full `bind -v` parity** — huck lists the 5 modeled variables + user-set
  entries, not bash's ~30 readline variables.

## Testing strategy

1. `tests/scripts/bind_diff_check.sh` — stdout + rc vs bash, using TARGETED
   greps (huck lists 5 vars; bash lists ~30, so no whole-output compare):
   - `bind -v | grep '^set editing-mode'` (default emacs).
   - `bind 'set editing-mode vi'; bind -v | grep editing-mode` (round-trip → vi).
   - `bind 'set bell-style none'; bind -v | grep bell-style`.
   - `bind -l | grep -cx accept-line` (function present), `bind -l | grep -cx beginning-of-line`.
   - `bind '"\C-x":beginning-of-line'; bind -p | grep beginning-of-line` (real binding listed).
   - unknown option rc: `bind -Z 2>/dev/null; echo $?` → 2.
   - `bind 'set editing-mode emacs'; echo rc=$?` → rc 0.
   - bogus function: `bind '"\C-x":no-such-fn' 2>/dev/null; echo $?` (match bash's rc).
2. Rust unit tests on `readline_bind`: `parse_keyseq("\\C-a")`,
   `parse_keyseq("\"\\e[A\"")`, `parse_keyseq("\\M-f")`, invalid seq → None;
   `function_to_cmd("beginning-of-line").is_some()`, unknown → None;
   `is_known_function` / `keyseq_is_valid` booleans.
3. A PTY test (`tests/bind_pty.rs`, expectrl — the harness used for Ctrl-Z /
   coproc / procsub-stop tests) proving the EDITOR EFFECT: launch huck
   interactively, send `bind 'set editing-mode vi'\n`, then drive a vi-mode key
   sequence (e.g. `ESC` then `0`/`$` motion or `dd`) and assert the line buffer
   behaves vi-style — proving the run-loop seam actually reconfigures the live
   rustyline editor. Plus a `bind '"\C-x":kill-line'`-then-`Ctrl-x` PTY check that
   the rebind takes effect. (PTY tests are the only way to verify the live-editor
   behavior; the diff harness covers parsing/listing/rc.)

Full `cargo test` + all existing harnesses stay green (the run-loop change only
runs when `bind` set `dirty`; `bind` is a new builtin, additive).

## Components touched

- `src/shell_state.rs` — `ReadlineSettings` struct + `Shell.readline_settings`
  field + accessors (a `bind`-facing API: `set_readline_var`, `add_bind`,
  `add_unbind`, `readline_var_lines`, `active_bind_lines`).
- `src/readline_bind.rs` (new) — `parse_keyseq`, `function_to_cmd`,
  `is_known_function`, `keyseq_is_valid`, `readline_function_names`, unit tests.
- `src/builtins.rs` — `builtin_bind` + `BUILTIN_NAMES`/`run_builtin` wiring.
- `src/shell.rs` — the run loop drains `pending_binds`/`pending_unbinds` and
  applies mapped vars via `Configurer` between commands.
- `tests/scripts/bind_diff_check.sh`, `tests/bind_pty.rs`.
