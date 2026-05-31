# huck v60 — `PS1` / `PS2` prompt customization (M-76)

## Goal

Replace huck's two hardcoded prompt strings with bash-style
prompt expansion: read `PS1` (primary) and `PS2` (continuation)
from shell variables and expand a Tier-A set of `\X` escapes
plus `$VAR` / `${VAR}` interpolation.

After v60:

- Setting `PS1='\u@\h \w \$ '` produces the expected
  `user@host /path/to/cwd $ ` prompt.
- Continuation lines use `PS2` (defaulting to `> `).
- Defaults still print `huck> ` and `> ` when the variables are
  unset.
- Color/non-printing markers (`\[ ... \]`) translate to
  rustyline's `\x01`/`\x02` cursor-tracking markers so prompts
  with ANSI escape sequences line up correctly.

New tracked divergence: **M-76: `PS1`/`PS2` customization**.

## Scope decisions (locked via AskUserQuestion)

**Tier A** (common bash escapes). Out: floats time/date escapes
(`\d \t \T \@ \A \D{fmt}`), `\v`/`\V`/`\s`, octal `\nnn`,
`PROMPT_COMMAND`, `PS3` (used by `select` — huck doesn't have
it), `PS4` (used by `set -x` — huck doesn't have it either).

## Out of scope (deferred)

- `\d \t \T \@ \A \D{fmt}` — date/time escapes (would require
  strftime; huck has no other strftime user).
- `\v \V \s` — bash version & shell name.
- `\nnn` octal escapes (rare in PS1).
- `PROMPT_COMMAND` (run a command before each prompt).
- `PS3` / `PS4` (depend on features huck doesn't have).
- Command substitution `$(...)` inside the prompt template
  (bash supports it; defer until needed).

## Architecture

New `src/prompt.rs` module with one public function:

```rust
/// Expands a bash-style prompt template (PS1/PS2 contents)
/// into the final byte string handed to rustyline.
pub fn expand_prompt(template: &str, shell: &Shell) -> String;
```

Plus the wire-in at `src/shell.rs` (around line 140) — replaces
the hardcoded `PROMPT`/`CONT_PROMPT` constants with a lookup +
expansion call.

### Algorithm

Walk `template` by byte index. The outer loop alternates between:
- **Fast path**: scan forward to the next `\` or `$` byte and
  push the slice (preserves non-ASCII bytes correctly).
- **Special path**: handle `\X` or `$VAR` / `${VAR}`.

For `\X` the supported letters are:

| Escape | Result |
|---|---|
| `\u` | current username (env `USER` or libc `getpwuid(getuid())`) |
| `\h` | hostname truncated at first `.` |
| `\H` | full hostname |
| `\w` | CWD with `$HOME` collapsed to `~` (uses `shell.lookup_var("PWD")` then `current_dir()` fallback) |
| `\W` | basename of CWD (no collapse needed; `~` if CWD == HOME) |
| `\$` | `#` when euid==0, `$` otherwise (uses `libc::geteuid`) |
| `\n` | newline `0x0A` |
| `\r` | carriage return `0x0D` |
| `\\` | literal backslash |
| `\?` | `shell.last_status()` as decimal |
| `\j` | `shell.jobs.iter().count()` |
| `\!` | `shell.history.last_number().map(\|n\| n+1).unwrap_or(1)` (history number that the NEXT command will get) |
| `\#` | reuses `\!` for v60 (small divergence — bash distinguishes; we don't until there's a separate counter) |
| `\e` | ESC `0x1B` |
| `\a` | bell `0x07` |
| `\[` | `\x01` (rustyline non-printing start) |
| `\]` | `\x02` (rustyline non-printing end) |
| `\033` | ESC `0x1B` (alias for `\e`) |

Unknown escapes (e.g. `\z`) pass through as `\z` literally.

For `$VAR`: walk while next char is `_` or alphanumeric (first
char must be `_` or alpha). Look up via `shell.lookup_var(name)`;
substitute the value or empty string if unset.

For `${VAR}`: scan until closing `}`; same lookup.

Both forms tolerate empty-name lookups gracefully (just emit `$`
or `${}` literally if the name is empty / malformed).

### Wire-in (`src/shell.rs`)

Current:
```rust
const PROMPT: &str = "huck> ";
const CONT_PROMPT: &str = "> ";
// ...
let prompt = if pending.is_none() { PROMPT } else { CONT_PROMPT };
```

After:
```rust
const DEFAULT_PS1: &str = "huck> ";
const DEFAULT_PS2: &str = "> ";
// ...
let template_var = if pending.is_none() { "PS1" } else { "PS2" };
let default = if pending.is_none() { DEFAULT_PS1 } else { DEFAULT_PS2 };
let template = shell.lookup_var(template_var).unwrap_or_else(|| default.to_string());
let expanded = crate::prompt::expand_prompt(&template, &shell);
match editor.readline(&expanded) { /* ... unchanged */ }
```

### Module wiring

`src/main.rs` gets `mod prompt;` in alphabetical position
(between `param_expansion` and `shell`).

## Behavior table

| Template | Expansion |
|---|---|
| `huck> ` | `huck> ` (no escapes; passthrough) |
| `\u@\h:\w\$ ` | `alice@laptop:~/work$ ` (sample env) |
| `\$ ` (as root) | `# ` |
| `\$ ` (as user) | `$ ` |
| `[\?] $ ` (after `false`) | `[1] $ ` |
| `\j job(s) ` (with 2 jobs) | `2 job(s) ` |
| `${USER}> ` | `alice> ` |
| `${UNSET}>` | `>` (empty substitution) |
| `\[\e[31m\]err\[\e[0m\] ` | `\x01\x1B[31m\x02err\x01\x1B[0m\x02 ` |
| `\033[1mbold\033[0m` | `\x1B[1mbold\x1B[0m` |
| `\z plain` | `\z plain` (unknown escape preserved) |
| `\\` | `\` (literal backslash) |
| `` (empty PS1) | `` (empty prompt) |

## Test plan

### Unit tests in `src/prompt.rs::tests` (16 tests)

1. `literal_text_passes_through` — `"hello "` → `"hello "`.
2. `expand_user` — set `USER=alice` via shell.set, `\u` → `alice`.
3. `expand_hostname_short` — `\h` does NOT contain a `.`.
4. `expand_cwd_with_home_collapse` — set `HOME=/h/me` and
   `PWD=/h/me/x`; `\w` → `~/x`.
5. `expand_cwd_basename` — `PWD=/a/b/c`; `\W` → `c`.
6. `expand_dollar_user_vs_root` — for current process: `\$`
   produces `#` iff `getuid() == 0`. (Test asserts it's one of
   `$`/`#` and matches `is_root()`.)
7. `expand_n_r_backslash` — `\n` → `\n`; `\r` → `\r`; `\\` → `\`.
8. `expand_status` — set shell.set_last_status(42); `\?` → `42`.
9. `expand_jobs_count_zero` — fresh shell; `\j` → `0`.
10. `expand_escape_e_and_033` — `\e` → `\x1B`; `\033` → `\x1B`.
11. `expand_bell` — `\a` → `\x07`.
12. `expand_bracket_markers` — `\[X\]` → `\x01X\x02`.
13. `expand_dollar_var_with_braces` — set `XYZ=hi`; `${XYZ}` → `hi`.
14. `expand_dollar_var_bare` — set `XYZ=hi`; `$XYZ ` → `hi `.
15. `expand_unknown_escape_preserved` — `\z` → `\z`.
16. `expand_undefined_var_empty` — `${UNSET}>` → `>`.

(Tests for `\!` and `\H` are env-dependent; will rely on
integration / manual verification rather than unit asserts on
specific values. The implementation paths are still exercised
implicitly through other tests since they go through the same
escape dispatch.)

### Integration tests

**None** for v60. The prompt is only visible in interactive
mode; non-tty stdin doesn't print prompts (rustyline's
readline_direct path). PTY-based prompt assertions would
require a meaningful expansion of the existing v15 PTY harness
— deferred.

The unit tests cover the expansion logic; the wire-in is a
straight var-lookup-plus-call so the risk surface is small.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Foundation + 16 unit tests** — new `src/prompt.rs`; wire-in
   at `src/shell.rs`; `mod prompt;` in `src/main.rs`.

2. **Docs** — new M-76 entry; change-log; README v60 row.

## Acceptance criteria

- 16 unit tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- Default behavior unchanged when PS1/PS2 are unset (prints
  `huck> ` / `> `).
- Setting `PS1='\u@\h:\w\$ '` produces a sensible prompt in
  interactive use.
- Color escape sequences wrapped in `\[ ... \]` track rustyline's
  cursor correctly.
- All existing tests still pass.
- M-76 doc entry added.
