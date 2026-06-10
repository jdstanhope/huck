# huck v130 — `set -x` trace fidelity (match bash xtrace output) Design

**Status:** approved design, ready for implementation plan.
**Implements:** make huck's `set -x` (xtrace) output byte-match bash 5.x for the
common cases — proper per-word shell-quoting, `local`/`declare` argument
rendering, the `command` prefix, inline + bare assignment lines, and tracing of
external pipeline stages.
**Branch (impl):** `v130-setx-trace-fidelity`.

## Background — measured divergences (this session, huck vs bash 5.x)

huck's xtrace (v103, M-08) lives in ONE place — the trace block in
`run_exec_single` (`src/executor.rs:~2897`) — and emits the already-expanded
`program` + `args` joined by spaces, with NO quoting, NO inline-assignment
prefix, dropping `local`/`declare` args and the `command` prefix. Ground-truth
comparison (`set -x` in-script; huck has no `-x` CLI flag):

| fragment | bash | huck (current) |
|---|---|---|
| `x="a b"; echo "$x" c` | `+ echo 'a b' c` | `+ echo a b c` (AMBIGUOUS) |
| `[ 1 -lt 2 ]` | `+ '[' 1 -lt 2 ']'` | `+ [ 1 -lt 2 ]` |
| `echo "" "; foo"` | `+ echo '' '; foo'` | `+ echo  ; foo` (BROKEN) |
| `f(){ local DEF=x y; }; f` | `+ local DEF=x y` | `+ local` (args dropped) |
| `command printf "%s\n" hi` | `+ command printf '%s\n' hi` | `+ printf %s\n hi` |
| `FOO=bar echo hi` | `+ FOO=bar` / `+ echo hi` | `+ echo hi` (prefix dropped) |
| `A=1` (bare assignment) | `+ A=1` | *(nothing — not traced)* |
| `echo a \| cat` | `+ echo a` / `+ cat` | `+ echo a` (ext stage untraced) |

Six divergence classes. v130 scope (user-chosen) = the first five plus external
pipeline-stage tracing; **out of scope** (deferred): PS4-first-char-repeated-by-
substitution-depth (`++ echo hi` inside `$()`, L-21) and `$-` flag-content
completeness.

## Key finding — bash xtrace quoting is NOT `@Q`

huck's `${v@Q}` (v96, `shell_quote`) ALWAYS quotes, even safe words
(`${x@Q}` for `x=hello` → `'hello'` in both shells). But bash xtrace leaves safe
words BARE (`echo hello a-b a/b a=b` → all unquoted). So xtrace needs its own
"quote only when necessary" rule, distinct from `@Q`. Probed across the full
ASCII range, bash's predicate is `sh_contains_shell_metas`:

- **Always a shell-meta (→ single-quote the word):** space, tab, newline, and
  `'` `"` `\` `|` `&` `;` `(` `)` `<` `>` `!` `{` `}` `*` `[` `?` `]` `^` `$` `` ` ``
- **Contextual metas:** `~` is a meta only at index 0 or immediately after `=` or
  `:`; `#` is a meta only at index 0.
- **Safe (→ leave bare):** ASCII alphanumerics and `% + - . / : = @ _ ,`
  (and `~`/`#` when NOT in a meta position).
- **Empty string →** `''`. **Any control char (`is_control`) →** `$'…'` (ANSI-C,
  reuse `ansi_c_quote`).

Confirmed by probe: `,` and mid-word `~`/`#` are SAFE; `!` and `^` are META; the
sanity set (space `;` `*` `?` `$`) all quote. This matches v120's lesson — probe
the full ASCII range before baking the set in; the harness re-verifies it.

## Architecture

### Component 1 — `xtrace_quote` (new, `src/param_expansion.rs`)

```rust
/// Quote `s` the way bash's xtrace (`set -x`) does: bare unless it contains a
/// shell metacharacter, in which case single-quote (with `'` → `'\''`); empty →
/// `''`; any control char → ANSI-C `$'…'`. Distinct from `${v@Q}`/`shell_quote`,
/// which always quotes.
pub(crate) fn xtrace_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars().any(|c| c.is_control()) {
        return ansi_c_quote(s);
    }
    if contains_shell_metas(s) {
        return format!("'{}'", crate::builtins::escape_alias_value(s)); // '\'' escaping
    }
    s.to_string()
}

/// bash `sh_contains_shell_metas`: true if `s` needs quoting for re-read.
fn contains_shell_metas(s: &str) -> bool {
    let bytes: Vec<char> = s.chars().collect();
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            ' ' | '\t' | '\n' | '\'' | '"' | '\\' | '|' | '&' | ';' | '(' | ')'
            | '<' | '>' | '!' | '{' | '}' | '*' | '[' | '?' | ']' | '^' | '$' | '`' => {
                return true;
            }
            '~' => {
                if i == 0 || bytes[i - 1] == '=' || bytes[i - 1] == ':' {
                    return true;
                }
            }
            '#' => {
                if i == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
```
(`escape_alias_value` already does the `'` → `'\''` single-quote-body escaping
used by `${v@Q}`; control-char strings never reach it because the `is_control`
branch handles them first. The `ansi_c_quote` helper is already `pub(crate)`.)

### Component 2 — `run_exec_single` trace block (rewrite, `src/executor.rs`)

The block currently at ~2897 is replaced. New behaviour (still emitted AFTER
inline-assignment apply + `command`-collapse, BEFORE dispatch, to stderr):

1. **Inline-assignment lines:** for each `a` in `cmd.inline_assignments`, emit
   `{ps4}{name}={xtrace_quote(value)}` where `value = shell.lookup_var(name)`
   (read post-apply; `unwrap_or_default()`). One line per assignment, in source
   order. (Matches bash `+ FOO=bar`.)
2. **Command line** (only if `resolved.program` is non-empty): build a word list
   = `command_prefix` ++ `[program]` ++ rendered-args, join the `xtrace_quote`'d
   words with single spaces, emit `{ps4}{joined}`.
   - **`command_prefix`**: a new `Vec<String>` captured during the
     `while resolved.program == "command"` collapse loop (~2835): on each
     bare-form iteration push `"command"` followed by the leading flag tokens it
     consumed (`resolved.args[..idx]`, e.g. `-p`, `--`). Empty in the common
     (no-`command`) case. Each token is `xtrace_quote`'d (all are safe → bare).
   - **rendered-args**: if `resolved.decl_args` is `Some(v)` (declaration
     command), render each `DeclArg`: `Plain(s)` → `xtrace_quote(s)` (already an
     expanded string); `Assign(a)` → `{name}={xtrace_quote(rhs)}` where
     `name = a.target.name()` and `rhs = word_literal_text(&a.value)` when the
     value Word is purely literal (common case, `DEF=x`), else
     `expand::expand_assignment(&a.value, shell)` (see decl-RHS edge note).
     Else render each `resolved.args[i]` → `xtrace_quote(arg)`.

`program` itself is also `xtrace_quote`'d (so `[` → `'['`).

### Component 3 — bare-assignment tracing (`src/executor.rs`, `run_single` Assign arm ~2676)

A bare `A=1` is `SimpleCommand::Assign`, never reaching `run_exec_single`, so it
is currently untraced. Add: when `shell.shell_options.xtrace`, after each
successful `apply_one_assignment(a, shell)`, emit
`{ps4}{name}={xtrace_quote(value)}` with `value = shell.lookup_var(name)`. One
line per assignment, in order (matches bash `+ A=1`, `+ B='x y'`). PS4 is read
once via `lookup_var("PS4").unwrap_or("+ ")`.

### Component 4 — external pipeline-stage tracing (`src/executor.rs`, `spawn_external_with_fds`)

In-process stages already trace (they run `run_exec_single` in the forked child);
external stages spawned via `spawn_external_with_fds` do not. Add, right after
the `resolve(...)` in that function (near the v129 flush), when
`shell.shell_options.xtrace`: emit `{ps4}{program} {quoted args}` (same
`xtrace_quote` join; external stages carry no `command` prefix or decl_args —
declaration builtins are InProcess). Emitted from the parent before the spawn, to
the inherited stderr.

### Shared helper

`fn xtrace_command_line(ps4, prefix: &[String], program: &str, args: &[String]) -> String`
builds the joined, quoted command line; used by Components 2 (args path) and 4.
The decl_args and inline/bare-assignment rendering are small inline loops at
their sites (different shapes). Keep one PS4-lookup helper
`fn ps4(shell) -> String`.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/param_expansion.rs` | Add `pub(crate) fn xtrace_quote` + private `contains_shell_metas`. |
| `src/executor.rs` | Capture `command_prefix` in the collapse loop; rewrite the `run_exec_single` trace block (assignment lines + prefix + decl_args/args, all quoted); add bare-assignment tracing in the `Assign` arm; add external-stage tracing in `spawn_external_with_fds`; shared `xtrace_command_line`/`ps4` helpers. |
| `tests/setx_trace_fidelity_integration.rs` (NEW) | Exact-byte trace assertions. |
| `tests/scripts/setx_trace_fidelity_diff_check.sh` (NEW) | Bash-diff harness (ASCII-range quoting + every divergence row). |
| `docs/bash-divergences.md` | NARROW the existing **L-21** (`set -x` trace-format divergences, `[intentional]`) entry: REMOVE the now-fixed items — (b) inline-assignment prefix, (c) arg re-quoting, and the bare-assignment clause of (d). KEEP the residual: (a) flat `$PS4` (no depth-repeat / no PS4 expansion), the finer-compound clause of (d) (`for`-iteration var sets, `case` word, `[[ ]]`/`(( ))`), the decl-RHS-expansion edge, and (e) `2>`-doesn't-suppress (M-90). Reword so it no longer claims arg-quoting/inline-prefix gaps. Do NOT claim full xtrace parity. |
| `tests/set_x_integration.rs` (v103, EXISTING) | UPDATE assertions that locked in the OLD unquoted / prefix-dropped output to the new bash-matching output (the reviewer re-runs bash to confirm each changed assertion). |

## Testing

1. **Integration `#[test]`s** (`tests/setx_trace_fidelity_integration.rs`) — run a
   fragment through huck with `set -x`, capture STDERR, assert exact bytes:
   - `set -x` + `x="a b"; echo "$x" c` → stderr contains `+ echo 'a b' c`
   - `[ 1 -lt 2 ]` → `+ '[' 1 -lt 2 ']'`
   - `echo "" "; foo"` → `+ echo '' '; foo'`
   - `f(){ local DEF=x y; }; f` → trace has `+ local DEF=x y`
   - `command printf "%s\n" hi` → `+ command printf '%s\n' hi`
   - `FOO=bar echo hi` → two lines `+ FOO=bar` then `+ echo hi`
   - `A=1; B="x y"` → `+ A=1` then `+ B='x y'`
   - `echo a | cat` → trace has both `+ echo a` and `+ cat`
   - safe-word baseline: `echo hello a-b a/b a=b a,b` → `+ echo hello a-b a/b a=b a,b` (all bare)
   (Assertions target the relevant trace line(s); other shell noise on stderr is
   tolerated by substring/line matching, not full-stderr equality, since job/
   error noise is orthogonal.)

2. **Bash-diff harness** `tests/scripts/setx_trace_fidelity_diff_check.sh` —
   gold-standard byte-identical bash↔huck on stderr-only (`2>&1 >/dev/null`), with
   `set -x` prefixed. Fragments cover every divergence row above PLUS an
   ASCII-punctuation sweep: for each printable punctuation char `c`, trace
   `: "a${c}b"` and assert bash==huck (locks the `contains_shell_metas` set).
   Strip nothing; compare raw. (PS4 default `+ ` in both.)

3. **Full regression:** entire unit + integration suite and ALL existing
   harnesses green; clippy clean. In particular the existing xtrace test(s) from
   v103 must be updated to the new quoted output (the reviewer should re-run bash
   to confirm any changed assertion).

## Edge cases & notes
- **Declaration-RHS edge:** for `local`/`declare` the builtin has not run at
  trace time (trace fires before dispatch, per v103). A purely-literal RHS
  (`local DEF=x y`) renders exactly via `word_literal_text` (no eval). A RHS with
  an expansion (`local DEF=$y`) is rendered by `expand_assignment`, matching
  bash's expanded value for side-effect-free expansions (`$VAR`, arithmetic); the
  ONLY divergence is a command substitution in a decl RHS (`local x=$(cmd)`),
  which then executes a second time for the trace — a rare documented edge,
  deferred (not a v130 target).
- The `name` of a subscripted decl/inline assignment target (`arr[0]=x`) renders
  via `target.name()` (scalar name); subscript rendering is best-effort.
- **Array** inline/bare assignments in a trace (`arr=(a b)` → bash
  `+ arr=([0]="a" [1]="b")`) are rare; v130 renders SCALAR assignments exactly and
  treats array value rendering as best-effort (a `lookup_var` of an array yields
  its element-0 / join form) — documented as a minor known divergence, not a
  target. Do NOT crash on an array assignment.
- The inline-assignment value is read via `lookup_var` AFTER apply (post-
  expansion), matching bash which traces the expanded value.
- PS4 is taken verbatim (no recursive expansion of PS4 itself — bash expands PS4,
  but huck's v103 already treats PS4 as a literal default `+ `; keeping that is
  consistent and out of this iteration's scope).
- **Pipeline-stage trace order (best-effort, deferred residual):** external-stage
  traces are emitted from the parent pre-spawn while in-process stage traces come
  from the forked child; both reach the same stderr, but their relative ORDER in a
  mixed pipeline can race. v130 guarantees the SET of trace lines matches bash
  (every stage traced, correctly quoted); strict left-to-right order is
  best-effort. The harness compares pipeline fragments order-independently
  (sorted); integration tests assert membership (`contains`), not order. Forcing
  strict order would require either double-expanding stage args in the parent (a
  correctness regression) or a fork-plumbing refactor — out of scope; logged as an
  L-21 residual.
- No new shell state, no `set -o` interaction beyond reading
  `shell_options.xtrace`.
