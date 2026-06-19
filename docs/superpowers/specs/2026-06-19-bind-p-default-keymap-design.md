# v191: honest default keymap for `bind -p` / `-P` — Design

**Status:** approved 2026-06-19
**Iteration:** v191
**Origin:** Last of the three v188 coverage-sweep divergences. huck's `bind -p`
shows only USER-added bindings (`active_binds`), so in `-c` mode it prints
nothing; bash prints its entire ~494-line default emacs keymap.

## Constraint (decided in brainstorming)

huck's line editor is **rustyline**, not GNU readline, so bash's 494-line keymap
describes functions/keys huck doesn't use — emitting it verbatim would be
fiction. Instead, `bind -p`/`-P` emit huck's **real** default emacs bindings (the
standard emacs keys rustyline honors for huck's supported functions), layered
with the user's `active_binds`. Truthful and useful, not byte-identical to bash's
larger set. The honesty bar is enforced by the harness: **every binding line huck
emits must also appear in bash's `bind -p`** (huck's keymap ⊆ bash's).

## bash contract (verified)

- `bind -p`: `"keyseq": function`, one line per (keyseq, function) pair, sorted by
  function name; `# function (not bound)` for known-but-unbound functions.
- `bind -P`: one line per function — `function can be found on "k1", "k2".`
  (all keyseqs, comma-separated) or `function is not bound to any keys`.
- keyseqs are double-quoted, backslash-escaped (`"\C-a"`, `"\ef"`, `"\C-?"`).

## huck's honored function set

`readline_bind::function_to_cmd` maps exactly 33 function names to a
`rustyline::Cmd` (the functions huck can actually bind/honor): beginning-of-line,
end-of-line, forward/backward-char, forward/backward-word, kill-line,
backward-kill-line, unix-line-discard, kill-word, backward-kill-word,
unix-word-rubout, clear-screen, accept-line, previous/next-history,
beginning/end-of-history, history-search-backward/forward,
reverse/forward-search-history, complete, upcase/downcase/capitalize-word,
transpose-chars, transpose-words, undo, yank, delete-char, backward-delete-char,
abort. The default keymap covers these only.

## Design

### 1. `DEFAULT_EMACS_BINDS` (`src/readline_bind.rs`)

A static `&[(&str, &str)]` of (keyseq, function) — the standard emacs bindings
rustyline honors, each verified to appear in bash's default `bind -p` (the
harness enforces this ⊆ relation):

```rust
pub const DEFAULT_EMACS_BINDS: &[(&str, &str)] = &[
    ("\\C-a", "beginning-of-line"),  ("\\C-e", "end-of-line"),
    ("\\C-f", "forward-char"),       ("\\C-b", "backward-char"),
    ("\\ef",  "forward-word"),       ("\\eb",  "backward-word"),
    ("\\C-k", "kill-line"),          ("\\C-u", "unix-line-discard"),
    ("\\C-w", "unix-word-rubout"),   ("\\ed",  "kill-word"),
    ("\\e\\C-?", "backward-kill-word"),
    ("\\C-l", "clear-screen"),       ("\\C-g", "abort"),
    ("\\C-j", "accept-line"),        ("\\C-m", "accept-line"),
    ("\\C-p", "previous-history"),   ("\\C-n", "next-history"),
    ("\\e<",  "beginning-of-history"), ("\\e>", "end-of-history"),
    ("\\C-r", "reverse-search-history"), ("\\C-s", "forward-search-history"),
    ("\\C-i", "complete"),
    ("\\eu",  "upcase-word"),        ("\\el",  "downcase-word"),
    ("\\ec",  "capitalize-word"),    ("\\C-t", "transpose-chars"),
    ("\\et",  "transpose-words"),    ("\\C-_", "undo"),
    ("\\C-y", "yank"),               ("\\C-d", "delete-char"),
    ("\\C-?", "backward-delete-char"),
];
```

Functions in the honored set with NO default binding here
(backward-kill-line, history-search-backward, history-search-forward) render as
`# function (not bound)`. (These three are genuinely unbound by default in this
context — keeping them out of the bind table preserves the ⊆-bash honesty check
since bash's default for them in `bind -p` is also a `# … (not bound)` comment or
absent from the bound lines.)

### 2. Track unbinds (`src/shell_state.rs`)

Add `unbound: std::collections::BTreeSet<String>` to `ReadlineSettings`;
`add_unbind` records the keyseq there (in addition to queuing `pending_unbinds`
for the loop). This lets the effective keymap drop a DEFAULT keyseq the user
unbound (`bind -r '\C-a'`), not just user-added ones.

### 3. Effective keymap + rendering (`src/shell_state.rs`)

A helper builds the effective keymap: start from `DEFAULT_EMACS_BINDS`, overlay
`active_binds` (a user bind for a keyseq overrides/adds), then remove any keyseq
in `unbound`. Result: `BTreeMap<keyseq, function>`.

- **`active_bind_lines` (`bind -p`)** — rewritten: for each known function (sorted
  by name): emit `"keyseq": func` (via `quote_keyseq`) for each of its keyseqs
  (sorted); if the function has no keyseq, emit `# func (not bound)`.
- **`active_bind_lines_verbose` (`bind -P`)** — rewritten: for each known function
  (sorted): `func can be found on "k1", "k2".` (keyseqs sorted) or
  `func is not bound to any keys`.

`builtin_bind`'s `-p`/`-P` dispatch is unchanged (it already calls these two
methods). `quote_keyseq` is reused.

## Verification

- **New bash-diff harness** `tests/scripts/bind_keymap_diff_check.sh`:
  - **⊆-honesty**: every BINDING line huck's `bind -p` emits (lines starting `"`)
    must also be present, byte-identical, in `bash -c 'bind -p'` — proves huck
    fabricates no bindings. (Iterate huck's `"…": …` lines; assert
    `bash bind -p` contains each via `grep -Fxq`.)
  - **core bindings present + exact format**: huck `bind -p | grep -F '"\C-a": beginning-of-line'`
    equals bash's same grep (and likewise `\C-e`/`\C-k`/`\C-y`/`\C-i`/`\C-?`).
  - **`bind -P` format**: huck's `beginning-of-line can be found on "\C-a".` line is
    a prefix-match of bash's (bash lists more keyseqs); assert huck's exact line
    AND that bash's `bind -P` line for that function starts with
    `beginning-of-line can be found on "\C-a"`.
  - **user override**: `bind '"\C-a": kill-line'; bind -p | grep -F '"\C-a"'` →
    `"\C-a": kill-line` in BOTH shells (bash and huck identical).
  - **user unbind of a default**: `bind -r '\C-a'; bind -p | grep -c '"\\C-a"'` →
    `0` in both.
- **Unit tests** (`src/shell_state.rs` `mod tests`): the effective-keymap merge
  (default present; `active_binds` override replaces a default; a new user keyseq
  adds; an `unbound` keyseq drops a default); `active_bind_lines` emits the
  `"\C-a": beginning-of-line` line and a `# backward-kill-line (not bound)` line;
  `active_bind_lines_verbose` emits `beginning-of-line can be found on "\C-a".`.
- Full `cargo test` (0 failures); all harnesses + clippy green.

## Docs / close-out

Resolves the last coverage-sweep divergence. If `docs/bash-divergences.md` has a
`bind` entry, update/remove it; else no change. Record v191 in
`project_huck_iterations.md` + `MEMORY.md` (note ALL 3 coverage divergences now
done). **Deferred follow-ons to log:** vi-mode default keymap (`bind -p` under
`set -o vi` should show the vi keymap — huck shows the emacs table); expanding
`bind -l` toward bash's 173 function names; the unbound-function `# (not bound)`
set fully matching bash's larger function list.

## Scope boundary

In scope: `DEFAULT_EMACS_BINDS`; the `unbound` set; the effective-keymap merge;
rewriting `active_bind_lines`/`_verbose` for `-p`/`-P`; the harness + unit tests.
**Not** in scope: vi-mode keymap; `bind -l` expansion (stays huck's 33 honored
functions); `bind -v`/`-V` (readline variables — separate); `bind -s` macros;
`bind -x` shell-command bindings; matching bash's full 494-line set (impossible
honestly with rustyline).
