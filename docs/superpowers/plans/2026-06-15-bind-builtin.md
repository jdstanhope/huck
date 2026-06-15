# v161: `bind` builtin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A functional `bind` builtin over huck's rustyline editor — configurable readline variables (editing-mode vi/emacs, bell-style, …) that drive live rustyline config, real key rebinding to the rustyline-`Cmd`-mappable readline functions, and bash-compatible listing/error flags.

**Architecture:** `bind` is a pure `&mut Shell` builtin that records intent (variable sets, pending key (un)binds) onto a new `Shell.readline_settings` struct as plain strings. The `run()` loop in `src/shell.rs` — the only place the rustyline `Editor` lives — drains those between commands and applies them via the `Configurer` runtime setters and `Editor::bind_sequence`/`unbind_sequence`. A new pure module `src/readline_bind.rs` parses readline keyseq notation → rustyline `Event` and maps readline function names → rustyline `Cmd`, with `bool` validators so `builtins.rs` stays rustyline-free.

**Tech Stack:** Rust, rustyline 18 (default features include `custom-bindings`, so `bind_sequence` is available — no Cargo change). Spec: `docs/superpowers/specs/2026-06-15-bind-builtin-design.md` (read its Variable map + Function map tables — they are the behavior contract).

**Build/test (huck is a BIN crate — NO `--lib`):** `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | tail -5`; `cargo clippy --bins --quiet 2>&1 | tail -3`; harness loop `for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 || echo "FAIL $s"; done; echo done`.

**Verified rustyline 18 facts (use these — don't re-derive):**
- `Editor::bind_sequence<E: Into<Event>, R: Into<EventHandler>>(&mut self, key_seq, handler)` and `unbind_sequence` — available (custom-bindings default).
- `impl From<KeyEvent> for Event` and `impl From<Cmd> for EventHandler` exist, so `editor.bind_sequence(key_event, cmd)` works directly (KeyEvent→Event, Cmd→EventHandler).
- Runtime config: `use rustyline::config::Configurer;` gives `editor.set_edit_mode(EditMode)`, `set_bell_style(BellStyle)`, `set_completion_show_all_if_ambiguous(bool)`, `set_completion_prompt_limit(usize)`, `set_keyseq_timeout(Option<u16>)`.
- Types: `rustyline::{KeyEvent, KeyCode (as Modifiers via rustyline::keys)}`; `KeyEvent::new(char, Modifiers)`, `KeyEvent::ctrl(char)`, `KeyEvent(KeyCode, Modifiers)`. `Modifiers` is a bitflags (`Modifiers::CTRL`, `ALT`, `NONE`). `KeyCode::{Char, Up, Down, Left, Right, Home, End, …}`. `Cmd`, `Movement`, `RepeatCount = u16`, `Word`, `At`, `Anchor`, `Event` are in `rustyline::keymap` / re-exported. The implementer MUST confirm exact import paths and enum-variant constructor args by reading `~/.cargo/registry/src/*/rustyline-18.0.0/src/{keys,keymap,binding}.rs` — they're stable but verify before relying on a specific variant.

**File structure:**
- `src/shell_state.rs` — `ReadlineSettings` + `Shell.readline_settings` + accessors (pure data, no rustyline).
- `src/readline_bind.rs` (NEW) — keyseq parser + function→Cmd map + validators (rustyline-coupled, pure functions).
- `src/builtins.rs` — `builtin_bind` + registration (uses accessors + bool validators only).
- `src/shell.rs` — run-loop drain/apply.
- `tests/scripts/bind_diff_check.sh`, `tests/bind_pty.rs`.

---

## Task 1: `ReadlineSettings` struct + `Shell` field + accessors

**Files:** `src/shell_state.rs`.

- [ ] **Step 1: Add the struct + field**

In `src/shell_state.rs`, add near the other Shell-owned config types:

```rust
/// readline-style settings driven by the `bind` builtin and applied to the
/// rustyline editor by the run loop. Pure data — no rustyline types here.
#[derive(Debug, Clone)]
pub struct ReadlineSettings {
    /// Every `set VAR value` (seeded with the 5 editor-mapped vars at their
    /// rustyline defaults so `bind -v` lists bash-matching defaults).
    pub vars: std::collections::BTreeMap<String, String>,
    /// Pending key bindings (keyseq, function) for the loop to apply.
    pub pending_binds: Vec<(String, String)>,
    /// Pending unbinds (keyseq) from `bind -r`.
    pub pending_unbinds: Vec<String>,
    /// Bindings the loop has applied — for `bind -p`/`-P` (keyseq -> function).
    pub active_binds: std::collections::BTreeMap<String, String>,
    /// Set when the loop must re-sync vars/binds to the editor.
    pub dirty: bool,
}

impl Default for ReadlineSettings {
    fn default() -> Self {
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("editing-mode".to_string(), "emacs".to_string());
        vars.insert("bell-style".to_string(), "audible".to_string());
        vars.insert("show-all-if-ambiguous".to_string(), "off".to_string());
        vars.insert("completion-query-items".to_string(), "100".to_string());
        vars.insert("keyseq-timeout".to_string(), "500".to_string());
        ReadlineSettings {
            vars,
            pending_binds: Vec::new(),
            pending_unbinds: Vec::new(),
            active_binds: std::collections::BTreeMap::new(),
            dirty: false,
        }
    }
}
```

Add a field to `Shell`: `pub readline_settings: ReadlineSettings,` and initialize it `ReadlineSettings::default()` in EVERY `Shell` constructor (grep for where `Shell {` is built — likely `Shell::new` and any test constructor; the compiler will flag missing fields).

- [ ] **Step 2: Write failing tests for the accessors**

In the `#[cfg(test)] mod tests` of `src/shell_state.rs`:

```rust
    #[test]
    fn readline_settings_set_and_list() {
        let mut shell = Shell::new();
        // default seeded vars present
        assert_eq!(shell.readline_settings.vars.get("editing-mode").map(String::as_str), Some("emacs"));
        // set a mapped var
        shell.set_readline_var("editing-mode", "vi");
        assert_eq!(shell.readline_settings.vars.get("editing-mode").map(String::as_str), Some("vi"));
        assert!(shell.readline_settings.dirty);
        // -v listing form
        let lines = shell.readline_var_lines();
        assert!(lines.iter().any(|l| l == "set editing-mode vi"));
        assert!(lines.iter().any(|l| l == "set bell-style audible"));
        // record a binding + list it
        shell.add_bind("\"\\C-x\"", "kill-line");
        assert_eq!(shell.readline_settings.pending_binds, vec![("\"\\C-x\"".to_string(), "kill-line".to_string())]);
    }
```

- [ ] **Step 3: Implement the accessors**

In `impl Shell`:

```rust
    /// Records a `set VAR value` (sets `dirty`). The run loop applies the
    /// editor-mapped ones; others are recorded for `bind -v` round-trip.
    pub fn set_readline_var(&mut self, name: &str, value: &str) {
        self.readline_settings.vars.insert(name.to_string(), value.to_string());
        self.readline_settings.dirty = true;
    }

    /// Queues a key binding (keyseq -> function) for the loop to apply.
    pub fn add_bind(&mut self, keyseq: &str, function: &str) {
        self.readline_settings.pending_binds.push((keyseq.to_string(), function.to_string()));
        self.readline_settings.dirty = true;
    }

    /// Queues an unbind (keyseq) for the loop to apply.
    pub fn add_unbind(&mut self, keyseq: &str) {
        self.readline_settings.pending_unbinds.push(keyseq.to_string());
        self.readline_settings.dirty = true;
    }

    /// `bind -v` lines: `set NAME VALUE`, sorted by name (BTreeMap iterates sorted).
    pub fn readline_var_lines(&self) -> Vec<String> {
        self.readline_settings.vars.iter().map(|(k, v)| format!("set {k} {v}")).collect()
    }

    /// `bind -V` lines: `` NAME is set to `VALUE' ``.
    pub fn readline_var_lines_verbose(&self) -> Vec<String> {
        self.readline_settings.vars.iter().map(|(k, v)| format!("{k} is set to `{v}'")).collect()
    }

    /// `bind -p` lines: `"KEYSEQ": FUNCTION`.
    pub fn active_bind_lines(&self) -> Vec<String> {
        self.readline_settings.active_binds.iter().map(|(k, f)| format!("{k}: {f}")).collect()
    }

    /// `bind -P` lines: `FUNCTION can be found on "KEYSEQ".`
    pub fn active_bind_lines_verbose(&self) -> Vec<String> {
        self.readline_settings.active_binds.iter().map(|(k, f)| format!("{f} can be found on {k}.")).collect()
    }
```

NOTE: for `-p` the keyseq is stored already-quoted (the builtin records the canonical `"\C-x"` form). Match bash's `"\C-x": kill-line` exactly — verify against `bash -c 'bind -p' | grep` for an example line and adjust the format string (bash uses `"keyseq": function`).

- [ ] **Step 4: Run + commit**

`cargo test readline_settings 2>&1 | tail -4` (PASS), full `cargo test` green, clippy clean.

```bash
git add src/shell_state.rs
git commit -m "v161 task 1: ReadlineSettings struct + Shell accessors

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `src/readline_bind.rs` — keyseq parser + function map + validators

**Files:** Create `src/readline_bind.rs`; add `mod readline_bind;` to `src/main.rs` (or wherever modules are declared — grep `mod shell_state;` to find the module-declaration file).

- [ ] **Step 1: Write the module skeleton + imports**

Create `src/readline_bind.rs`. First READ `~/.cargo/registry/src/*/rustyline-18.0.0/src/keys.rs`, `keymap.rs`, `binding.rs` to confirm the exact import paths and variant constructors. Likely imports:

```rust
//! Parsing of readline key sequences into rustyline `Event`s and mapping of
//! readline function names to rustyline `Cmd`s, for the `bind` builtin.
use rustyline::{Cmd, Event, KeyCode, KeyEvent, Modifiers, Movement};
// (confirm: some of these may be under rustyline::keys / rustyline::keymap;
//  rustyline re-exports KeyEvent/Cmd/Event at the crate root — verify.)
```

- [ ] **Step 2: `parse_keyseq` — failing test first**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_basic_keyseqs() {
        assert!(parse_keyseq("\\C-a").is_some());        // Ctrl-A
        assert!(parse_keyseq("\"\\C-a\"").is_some());    // quoted
        assert!(parse_keyseq("\\M-f").is_some());        // Meta-F (Alt)
        assert!(parse_keyseq("\\e[A").is_some());        // Up arrow escape seq
        assert!(parse_keyseq("a").is_some());            // literal char
        assert!(parse_keyseq("\\C-").is_none());         // incomplete
        assert!(parse_keyseq("").is_none());             // empty
    }
}
```

- [ ] **Step 3: Implement `parse_keyseq`**

```rust
/// Parses a readline key-sequence string into a rustyline `Event`.
/// Handles: optional surrounding double-quotes; `\C-x` (control),
/// `\M-x`/`\e` (meta/escape), well-known escape sequences (`\e[A`/`\e[B`/
/// `\e[C`/`\e[D` arrows, `\e[H`/`\e[F` home/end), `\t`/`\r`/`\n`/`\b`,
/// octal `\nnn`, hex `\xHH`, and literal characters. A single resulting key
/// becomes `Event::from(KeyEvent)`; a multi-key sequence (e.g. `\e` + `[A`
/// that doesn't match a known named key) becomes `Event::from(&[KeyEvent])`.
pub fn parse_keyseq(seq: &str) -> Option<Event> {
    // 1. strip surrounding quotes if present.
    let s = seq.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(seq);
    if s.is_empty() { return None; }

    // 2. First, recognize whole well-known escape sequences as named keys.
    match s {
        "\\e[A" | "\\eOA" => return Some(KeyEvent(KeyCode::Up, Modifiers::NONE).into()),
        "\\e[B" | "\\eOB" => return Some(KeyEvent(KeyCode::Down, Modifiers::NONE).into()),
        "\\e[C" | "\\eOC" => return Some(KeyEvent(KeyCode::Right, Modifiers::NONE).into()),
        "\\e[D" | "\\eOD" => return Some(KeyEvent(KeyCode::Left, Modifiers::NONE).into()),
        "\\e[H" | "\\eOH" => return Some(KeyEvent(KeyCode::Home, Modifiers::NONE).into()),
        "\\e[F" | "\\eOF" => return Some(KeyEvent(KeyCode::End, Modifiers::NONE).into()),
        _ => {}
    }

    // 3. Single key with optional \C-/\M- prefixes.
    let mut mods = Modifiers::NONE;
    let mut rest = s;
    loop {
        if let Some(r) = rest.strip_prefix("\\C-") { mods |= Modifiers::CTRL; rest = r; }
        else if let Some(r) = rest.strip_prefix("\\M-") { mods |= Modifiers::ALT; rest = r; }
        else { break; }
    }
    // The remaining `rest` must resolve to a single character (after one
    // backslash-escape) or be empty (invalid).
    let ch = match rest {
        "" => return None,
        "\\e" => '\x1b',
        "\\t" => '\t',
        "\\r" => '\r',
        "\\n" => '\n',
        "\\b" => '\x08',
        _ if rest.starts_with("\\x") => {
            let h = &rest[2..];
            char::from_u32(u32::from_str_radix(h, 16).ok()?)?
        }
        _ if rest.starts_with('\\') && rest.len() > 1 => {
            // octal \nnn or a single escaped char.
            let body = &rest[1..];
            if body.chars().all(|c| c.is_digit(8)) {
                char::from_u32(u32::from_str_radix(body, 8).ok()?)?
            } else {
                let mut cs = body.chars();
                let c = cs.next()?;
                if cs.next().is_some() { return None; }
                c
            }
        }
        _ => {
            let mut cs = rest.chars();
            let c = cs.next()?;
            if cs.next().is_some() { return None; } // more than one char and not an escape → unsupported
            c
        }
    };
    Some(KeyEvent::new(ch, mods).into())
}
```

NOTE: `KeyEvent::new` may normalize Ctrl chars; confirm `KeyEvent::new('a', Modifiers::CTRL)` yields what rustyline expects (it has special handling — read `keys.rs`'s `KeyEvent::new`). If `new` doesn't apply CTRL the way you need, use `KeyEvent::ctrl('a')` for the pure-Ctrl case. Adjust to match rustyline's representation so `bind_sequence` actually matches the typed key. The escape-sequence and literal cases are the important ones; keep the parser conservative (return `None` rather than mis-binding).

- [ ] **Step 4: `function_to_cmd` + validators + names — failing test**

```rust
    #[test]
    fn function_map_and_names() {
        assert!(function_to_cmd("beginning-of-line").is_some());
        assert!(function_to_cmd("kill-line").is_some());
        assert!(function_to_cmd("accept-line").is_some());
        assert!(function_to_cmd("no-such-function").is_none());
        assert!(is_known_function("clear-screen"));
        assert!(!is_known_function("totally-bogus"));
        assert!(readline_function_names().contains(&"accept-line"));
    }
```

- [ ] **Step 5: Implement `function_to_cmd`, `is_known_function`, `keyseq_is_valid`, `readline_function_names`**

```rust
/// Maps a readline function name to a rustyline `Cmd`, or `None` if rustyline
/// has no equivalent. Variant args mirror rustyline's own default keymap.
pub fn function_to_cmd(name: &str) -> Option<Cmd> {
    use rustyline::{At, Word, Anchor};
    Some(match name {
        "beginning-of-line" => Cmd::Move(Movement::BeginningOfLine),
        "end-of-line" => Cmd::Move(Movement::EndOfLine),
        "forward-char" => Cmd::Move(Movement::ForwardChar(1)),
        "backward-char" => Cmd::Move(Movement::BackwardChar(1)),
        "forward-word" => Cmd::Move(Movement::ForwardWord(1, At::AfterEnd, Word::Emacs)),
        "backward-word" => Cmd::Move(Movement::BackwardWord(1, Word::Emacs)),
        "kill-line" => Cmd::Kill(Movement::EndOfLine),
        "backward-kill-line" => Cmd::Kill(Movement::BeginningOfLine),
        "unix-line-discard" => Cmd::Kill(Movement::BeginningOfLine),
        "kill-word" => Cmd::Kill(Movement::ForwardWord(1, At::AfterEnd, Word::Emacs)),
        "backward-kill-word" => Cmd::Kill(Movement::BackwardWord(1, Word::Emacs)),
        "unix-word-rubout" => Cmd::Kill(Movement::BackwardWord(1, Word::Big)),
        "clear-screen" => Cmd::ClearScreen,
        "accept-line" => Cmd::AcceptLine,
        "previous-history" => Cmd::PreviousHistory,
        "next-history" => Cmd::NextHistory,
        "beginning-of-history" => Cmd::BeginningOfHistory,
        "end-of-history" => Cmd::EndOfHistory,
        "history-search-backward" => Cmd::HistorySearchBackward,
        "history-search-forward" => Cmd::HistorySearchForward,
        "reverse-search-history" => Cmd::ReverseSearchHistory,
        "forward-search-history" => Cmd::ForwardSearchHistory,
        "complete" => Cmd::Complete,
        "upcase-word" => Cmd::UpcaseWord,
        "downcase-word" => Cmd::DowncaseWord,
        "capitalize-word" => Cmd::CapitalizeWord,
        "transpose-chars" => Cmd::TransposeChars,
        "transpose-words" => Cmd::TransposeWords,
        "undo" => Cmd::Undo(1),
        "yank" => Cmd::Yank(1, Anchor::Before),
        "delete-char" => Cmd::Kill(Movement::ForwardChar(1)),
        "backward-delete-char" => Cmd::Kill(Movement::BackwardChar(1)),
        "abort" => Cmd::Abort,
        _ => return None,
    })
}

/// True if `function_to_cmd` would succeed (no rustyline types in signature).
pub fn is_known_function(name: &str) -> bool { function_to_cmd(name).is_some() }

/// True if `parse_keyseq` would succeed.
pub fn keyseq_is_valid(seq: &str) -> bool { parse_keyseq(seq).is_some() }

/// The static list of readline function NAMES for `bind -l` (informational —
/// includes names huck can't bind). Subset is acceptable; include at least the
/// names in `function_to_cmd` plus the common readline functions so scripts
/// that grep `bind -l` for a standard name find it.
pub fn readline_function_names() -> &'static [&'static str] {
    &[
        "abort", "accept-line", "backward-char", "backward-delete-char",
        "backward-kill-line", "backward-kill-word", "backward-word",
        "beginning-of-history", "beginning-of-line", "capitalize-word",
        "clear-screen", "complete", "delete-char", "downcase-word",
        "end-of-history", "end-of-line", "forward-char",
        "forward-search-history", "forward-word", "history-search-backward",
        "history-search-forward", "kill-line", "kill-word", "next-history",
        "previous-history", "reverse-search-history", "transpose-chars",
        "transpose-words", "undo", "unix-line-discard", "unix-word-rubout",
        "upcase-word", "yank",
    ]
}
```

CRITICAL: each `Cmd`/`Movement` variant + its args MUST match rustyline 18's actual enums — READ `keymap.rs` and fix any variant name/arg that differs (e.g. `Cmd::Undo` may take a `RepeatCount`, `Yank` may take `(RepeatCount, Anchor)`, `Word`/`At` variant names). The list above is from rustyline source but VERIFY each compiles. Drop any function whose `Cmd` you can't construct (move it to `None`) rather than guess.

- [ ] **Step 6: Run + commit**

`cargo test --bin huck readline_bind 2>&1 | tail` — wait, run `cargo test parse_basic_keyseqs function_map_and_names 2>&1 | tail -6` (both PASS). Build + clippy clean.

```bash
git add src/readline_bind.rs src/main.rs
git commit -m "v161 task 2: readline_bind module (keyseq parser + function map)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `builtin_bind` + registration

**Files:** `src/builtins.rs`.

- [ ] **Step 1: Register the builtin**

Add `"bind"` to the `BUILTIN_NAMES` array (~line 24) in alphabetical position. In `run_builtin` (~line 68), add a dispatch arm: `"bind" => builtin_bind(args, out, shell),` (match the signature of neighboring builtins — most are `fn(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome`; confirm and match).

- [ ] **Step 2: Implement `builtin_bind`**

```rust
fn builtin_bind(args: &[String], out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    use crate::readline_bind::{is_known_function, keyseq_is_valid, readline_function_names};
    const USAGE: &str = "bind: usage: bind [-lpsvPSVX] [-m keymap] [-f filename] [-q name] [-u name] [-r keyseq] [-x keyseq:shell-command] [keyseq:readline-function or readline-command]";

    let mut i = 0;
    let mut rc = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-v" => { for l in shell.readline_var_lines() { let _ = writeln!(out, "{l}"); } }
            "-V" => { for l in shell.readline_var_lines_verbose() { let _ = writeln!(out, "{l}"); } }
            "-l" => { for f in readline_function_names() { let _ = writeln!(out, "{f}"); } }
            "-p" => { for l in shell.active_bind_lines() { let _ = writeln!(out, "{l}"); } }
            "-P" => { for l in shell.active_bind_lines_verbose() { let _ = writeln!(out, "{l}"); } }
            // No-op / empty listings (huck has no equivalent).
            "-s" | "-S" | "-X" => { /* no macros / shell-command bindings: empty */ }
            "-m" | "-q" | "-u" | "-f" => { i += 1; /* takes an arg; accept + no-op */ }
            "-r" => {
                i += 1;
                if let Some(seq) = args.get(i) { shell.add_unbind(seq); }
                else { eprintln!("huck: bind: -r: option requires an argument"); rc = 2; }
            }
            "-x" => { i += 1; /* keyseq:shell-command — deferred no-op */ }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: bind: {s}: invalid option");
                eprintln!("huck: {USAGE}");
                return ExecOutcome::Continue(2);
            }
            // Non-flag argument: `set VAR value`, or `keyseq:function`.
            _ => {
                if let Some(rest) = a.strip_prefix("set ").or_else(|| if a == "set" { Some("") } else { None }) {
                    // `bind 'set VAR VALUE'` arrives as one arg "set VAR VALUE".
                    let mut it = rest.split_whitespace();
                    if let (Some(var), Some(val)) = (it.next(), it.next()) {
                        // Validate mapped vars; record all.
                        if !validate_readline_var(var, val) {
                            eprintln!("huck: bind: {val}: invalid value for {var}");
                            rc = 1;
                        } else {
                            shell.set_readline_var(var, val);
                        }
                    }
                } else if let Some((seq, func)) = a.split_once(':') {
                    if !keyseq_is_valid(seq) {
                        eprintln!("huck: bind: {seq}: cannot parse key sequence");
                        rc = 1;
                    } else if !is_known_function(func) {
                        eprintln!("huck: bind: {func}: unknown function name");
                        rc = 1;
                    } else {
                        shell.add_bind(seq, func);
                    }
                } else {
                    eprintln!("huck: bind: {a}: unknown command");
                    rc = 1;
                }
            }
        }
        i += 1;
    }
    ExecOutcome::Continue(rc)
}

/// Validates a readline variable value for the 5 editor-mapped variables.
/// Unmapped variables accept any value (recorded for `bind -v` round-trip).
fn validate_readline_var(var: &str, val: &str) -> bool {
    match var {
        "editing-mode" => matches!(val, "emacs" | "vi"),
        "bell-style" => matches!(val, "none" | "audible" | "visible"),
        "show-all-if-ambiguous" => matches!(val, "on" | "off"),
        "completion-query-items" | "keyseq-timeout" => val.parse::<i64>().is_ok(),
        _ => true,
    }
}
```

NOTE on `set` arg arrival: `bind 'set editing-mode vi'` is ONE shell word → arrives as the single arg `"set editing-mode vi"`. Some callers write `bind set editing-mode vi` (3 args) — handle BOTH: if `a == "set"`, consume the next two args as var/value. Adjust the `set` branch to also handle the 3-arg form by peeking `args[i+1]`/`args[i+2]` when `a == "set"`. Test with `bash -c "bind 'set editing-mode vi'"` form (the common one). Verify bash's exact rc for an invalid value and match it.

- [ ] **Step 3: Verify vs bash + commit**

```bash
H=./target/debug/huck; cargo build 2>&1 | tail -1
$H -c 'bind -v' | grep -E 'editing-mode|bell-style'                 # set editing-mode emacs / set bell-style audible
$H -c "bind 'set editing-mode vi'; bind -v | grep editing-mode"     # set editing-mode vi
$H -c 'bind -l | grep -cx accept-line'                              # 1
$H -c "bind '\"\\C-x\":kill-line'; echo rc=$?"                      # rc=0 (recorded; applied by the loop interactively)
$H -c 'bind -Z 2>/dev/null; echo $?'                                # 2
$H -c "bind '\"\\C-x\":bogus' 2>/dev/null; echo $?"                 # 1
```
Compare rc/stdout to `bash --norc --noprofile -c '…'` (stderr prefix differs). Full `cargo test` green, all harnesses byte-identical, clippy clean.

```bash
git add src/builtins.rs
git commit -m "v161 task 3: builtin_bind (flags, set vars, key (un)bind records)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Run-loop application (Configurer + bind_sequence)

**Files:** `src/shell.rs`.

- [ ] **Step 1: Add a helper that applies pending settings to the editor**

In `src/shell.rs`, add (near the run loop; it needs the rustyline `Editor` + `Configurer`):

```rust
use rustyline::config::Configurer;

/// Applies any pending `bind` settings (mapped vars + key (un)binds) from the
/// shell to the live rustyline editor, then clears the dirty flag. No-op when
/// nothing is dirty.
fn apply_readline_settings(
    editor: &mut Editor<HuckHelper, FileHistory>,
    shell_cell: &Rc<RefCell<Shell>>,
) {
    let mut shell = shell_cell.borrow_mut();
    if !shell.readline_settings.dirty { return; }

    // 1. Mapped variables.
    if let Some(v) = shell.readline_settings.vars.get("editing-mode") {
        let mode = if v == "vi" { rustyline::EditMode::Vi } else { rustyline::EditMode::Emacs };
        editor.set_edit_mode(mode);
    }
    if let Some(v) = shell.readline_settings.vars.get("bell-style") {
        let style = match v.as_str() {
            "none" => rustyline::config::BellStyle::None,
            "visible" => rustyline::config::BellStyle::Visible,
            _ => rustyline::config::BellStyle::Audible,
        };
        editor.set_bell_style(style);
    }
    if let Some(v) = shell.readline_settings.vars.get("show-all-if-ambiguous") {
        editor.set_completion_show_all_if_ambiguous(v == "on");
    }
    if let Some(n) = shell.readline_settings.vars.get("completion-query-items").and_then(|s| s.parse::<usize>().ok()) {
        editor.set_completion_prompt_limit(n);
    }
    if let Some(n) = shell.readline_settings.vars.get("keyseq-timeout").and_then(|s| s.parse::<u16>().ok()) {
        editor.set_keyseq_timeout(Some(n));
    }

    // 2. Pending key binds.
    let binds = std::mem::take(&mut shell.readline_settings.pending_binds);
    for (seq, func) in binds {
        if let (Some(event), Some(cmd)) = (crate::readline_bind::parse_keyseq(&seq), crate::readline_bind::function_to_cmd(&func)) {
            editor.bind_sequence(event, cmd);
            shell.readline_settings.active_binds.insert(seq, func);
        }
    }
    // 3. Pending unbinds.
    let unbinds = std::mem::take(&mut shell.readline_settings.pending_unbinds);
    for seq in unbinds {
        if let Some(event) = crate::readline_bind::parse_keyseq(&seq) {
            editor.unbind_sequence(event);
            shell.readline_settings.active_binds.remove(&seq);
        }
    }
    shell.readline_settings.dirty = false;
}
```

Confirm the exact import paths for `EditMode`/`BellStyle` (rustyline re-exports `EditMode` at the crate root; `BellStyle` is under `rustyline::config` — verify and fix). `editor.set_*` come from the `Configurer` trait — the `use rustyline::config::Configurer;` brings them in scope.

- [ ] **Step 2: Call it in the run loop**

In the interactive REPL loop (the `loop { … match read_logical_command(…) }` around line 358), call `apply_readline_settings(&mut editor, &shell_cell);` once per iteration — place it at the TOP of the loop body (before `read_logical_command`), so a `bind` run in the previous command's `process_line` takes effect for the NEXT line read. (A `bind` issued interactively then affects the subsequent keypresses — matching bash.) Ensure it doesn't hold the shell borrow across `read_logical_command` (the helper borrows internally and drops before returning).

- [ ] **Step 3: Build + manual smoke + commit**

`cargo build 2>&1 | tail -3` clean; `cargo clippy --bins --quiet` clean. (The live effect is verified by the PTY test in Task 6; here just confirm it compiles and the non-interactive path — where `apply_readline_settings` is never called because there's no editor loop — is unaffected.) Full `cargo test` green, all harnesses byte-identical.

```bash
git add src/shell.rs
git commit -m "v161 task 4: apply bind settings to the live rustyline editor

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `bind_diff_check.sh` harness

**Files:** Create `tests/scripts/bind_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Model the `chk` helper on `tests/scripts/local_case_attrs_diff_check.sh`. Use TARGETED greps (huck lists 5 vars, bash ~30 — never whole-output compare):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `bind` builtin (stdout + exit).
# Targeted greps only: huck models a 5-variable subset, bash lists ~30, so
# whole-output `bind -v` comparison would (correctly) differ.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "default editing-mode" 'bind -v | grep -E "^set editing-mode "'
chk "default bell-style"   'bind -v | grep -E "^set bell-style "'
chk "set editing-mode vi"  "bind 'set editing-mode vi'; bind -v | grep -E '^set editing-mode '"
chk "set bell-style none"  "bind 'set bell-style none'; bind -v | grep -E '^set bell-style '"
chk "set show-all"         "bind 'set show-all-if-ambiguous on'; bind -v | grep show-all-if-ambiguous"
chk "l has accept-line"    'bind -l | grep -cx accept-line'
chk "l has beginning-of"   'bind -l | grep -cx beginning-of-line'
chk "keyseq:fn rc ok"      "bind '\"\\C-x\":kill-line'; echo rc=\$?"
chk "set var rc ok"        "bind 'set editing-mode emacs'; echo rc=\$?"
chk "unknown option rc"    'bind -Z >/dev/null 2>&1; echo rc=$?'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
```

IMPORTANT: run each fragment through `bash --norc --noprofile -c '…'` first to learn bash's actual output and ensure huck matches. Several cases depend on bash's exact behavior:
- `bind -l | grep -cx accept-line` → bash prints `1` (and a stderr warning, suppressed). huck must also print `1`.
- `bind '...'; echo rc=$?` → bash returns 0 for a successful set/bind (with the stderr warning). huck returns 0.
- `bind -Z` rc → bash 2, huck 2.
- For `set editing-mode vi`-then-`bind -v` round-trip: bash echoes `set editing-mode vi`; huck must match that exact line.
If a case can't match bash's stdout (e.g. bash emits extra lines), narrow the grep further or drop the case with a comment. Do NOT weaken to hide a real bug.

- [ ] **Step 2: Run + commit**

`chmod +x tests/scripts/bind_diff_check.sh && cargo build 2>&1 | tail -1 && bash tests/scripts/bind_diff_check.sh` → all PASS.

```bash
git add tests/scripts/bind_diff_check.sh
git commit -m "v161 task 5: bind bash-diff harness (vars/listing/rc)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: PTY test (live-editor effect) + final verification

**Files:** Create `tests/bind_pty.rs`.

- [ ] **Step 1: Write a PTY test proving a rebind takes effect**

Model on the existing PTY tests (grep `tests/*_pty.rs` — e.g. `tests/procsub_stop_pty.rs` — for the `expectrl::spawn`/`OsSession` setup, prompt-wait, and send/expect pattern). The ROBUST, observable proof: bind an unused key to `accept-line`, then use it to submit a command and assert the command ran.

```rust
// tests/bind_pty.rs
// Verifies the run-loop seam actually reconfigures the live rustyline editor:
// `bind '"\C-o":accept-line'` then Ctrl-O submits a line (proving bind_sequence
// was applied). Ctrl-O is not a default huck/emacs binding, so a plain Ctrl-O
// without the bind would NOT accept the line.
use expectrl::{spawn, Eof}; // match the imports the other *_pty.rs tests use

#[test]
fn bind_rebinds_a_key_to_accept_line() {
    // spawn huck interactively (copy the spawn/env setup from procsub_stop_pty.rs)
    let mut p = /* spawn huck, wait for prompt */ unimplemented_setup();
    // record the binding (takes effect on the NEXT line read)
    p.send_line(r#"bind '"\C-o":accept-line'"#).unwrap();
    // wait for prompt again
    // type a command WITHOUT pressing Enter, then press Ctrl-O (\x0f)
    p.send("echo BOUND_OK").unwrap();
    p.send("\x0f").unwrap(); // Ctrl-O
    // assert the command ran
    p.expect("BOUND_OK").unwrap();
    p.send_line("exit").unwrap();
    p.expect(Eof).ok();
}
```

This is a SKETCH — replace `unimplemented_setup()` and the exact `expectrl` API calls with the real pattern from the sibling `*_pty.rs` test (same crate version, same spawn helper, same prompt string). The key assertions: (1) after the `bind`, Ctrl-O (`\x0f`) accepts the typed line; (2) `BOUND_OK` appears. If Ctrl-O conflicts with something, pick another unbound key (verify it's unbound by default first). Keep it to ONE robust assertion — PTY tests are flaky when over-specified (lesson from the coproc/Ctrl-Z work: prefer one clear observable over many brittle ones).

OPTIONAL second assertion (editing-mode vi), only if it proves robust: after `bind 'set editing-mode vi'`, type text, send ESC then a vi motion, and assert. If vi-mode PTY proves flaky, SKIP it — the rebind test already proves the seam (both `bind_sequence` and the `Configurer` setters run in the same `apply_readline_settings` step), and the diff harness + unit tests cover the rest. Do NOT block the iteration on a flaky vi-mode PTY assertion.

- [ ] **Step 2: Run the PTY test (multiple times — it's interactive)**

Run: `cargo test --test bind_pty 2>&1 | tail -10` — PASS. Run it 5× to confirm it's not flaky: `for i in $(seq 5); do cargo test --test bind_pty 2>&1 | grep -E 'test result'; done`. All 5 pass. If flaky, simplify the assertion / increase the expect timeout / pick a more reliable key, per the project's PTY-test lessons.

- [ ] **Step 3: Commit**

```bash
git add tests/bind_pty.rs
git commit -m "v161 task 6: PTY test — bind rebinds a key in the live editor

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` + `cargo clippy --bins --quiet` clean.
- [ ] `cargo test` FULLY green (incl. the new `readline_bind` unit tests, `ReadlineSettings` tests, and `bind_pty`).
- [ ] `bash tests/scripts/bind_diff_check.sh` → all PASS; ALL `tests/scripts/*_diff_check.sh` byte-identical (bind is additive — a new builtin; the run-loop change only fires when `dirty`).
- [ ] `bind_pty` passes 5× consecutively (not flaky).
- [ ] Manual sanity: `./target/debug/huck` interactively → `bind 'set editing-mode vi'` then editing behaves vi-style; `bind -p` after a rebind lists it.

## Notes for the implementer

- **Verify every rustyline variant against the installed source** (`~/.cargo/registry/src/*/rustyline-18.0.0/src/{keys,keymap,binding,config}.rs`). The `Cmd`/`Movement`/`KeyEvent`/`EditMode`/`BellStyle` names + arg shapes in this plan are from that source but CONFIRM each compiles; drop any function from `function_to_cmd` whose `Cmd` you can't construct (→ `None`, and remove from `readline_function_names` if you want exact `bind -l`/binding parity, or keep the name listable-but-unbindable).
- **`builtins.rs` must NOT import rustyline types** — it uses only the `bool` validators + the `Shell` accessors. All rustyline coupling lives in `readline_bind.rs` and `shell.rs`.
- **Additive invariant:** `bind` is a new builtin and `apply_readline_settings` only does work when `dirty`. No existing harness/test should change. If one does, something leaked.
- **Non-interactive:** `huck -c 'bind …'` never reaches the REPL loop, so `apply_readline_settings` is never called — `bind` just records + lists. That's correct (matches bash, which can't edit either) and is what the diff harness exercises.
- **PTY flakiness:** one robust observable assertion beats many brittle ones; bump timeouts; run repeatedly before declaring green (the v157 coproc lesson).
