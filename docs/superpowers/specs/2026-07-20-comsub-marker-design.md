# v316 — `command substitution:` syntax-error marker for backtick bodies

**Issue:** [#213](https://github.com/jdstanhope/huck/issues/213). Phase 3 (final)
of the syntax-error-diagnostic work: v314 = top-level shapes (#211), v315 = the
`eval:` marker (#209), v316 = the `command substitution:` marker for backtick
command-substitution bodies.

**Goal:** a syntax error raised while re-parsing a backtick `` `…` `` command-
substitution body is reported with bash's `command substitution:` marker, the
body-local line offset by the outer line where the backtick sits, and a Shape-1
source-echo of the backtick **body** — e.g. `` echo `case esac in esac)` `` →
`<name>: command substitution: line 1: syntax error near unexpected token \`)'`
+ `` <name>: command substitution: line 1: `case esac in esac)' ``.

---

## The measured bash model

The divergence is **entirely backtick**; `$()` already matches bash. Verified
against bash 5.2.21 (`<name>:` prefix normalized to `SH:`):

| input (`-c`) | bash | huck (post-v315) |
|---|---|---|
| `` echo `case x in` `` (unterminated, Shape 2) | `command substitution: line 2: syntax error: unexpected end of file` | `-c: line 2: …` |
| `` echo `case esac in esac)` `` (near-token, Shape 1) | `command substitution: line 1: syntax error near unexpected token \`)'` + `command substitution: line 1: \`case esac in esac)'` | `-c: line 1: … \`)'` + `-c: line 1: \`echo \`case esac in esac)\`'` |
| `` echo `echo "hi` `` (Shape 3) | `command substitution: line 1: unexpected EOF while looking for matching \`"'` | `-c: line 1: …` |
| `echo $(case x in)` / `echo $(esac)` / `echo $(echo ))` | `-c: line 1: …` (top-level) | `-c: line 1: …` — **already matches** |

Two differences from huck, for backtick only:

1. **Marker:** `command substitution:` replaces `-c:` (or the script name
   segment) — across all three shapes.
2. **Shape-1 source-echo:** bash echoes the backtick **body**
   (`` `case esac in esac)' ``); huck echoes the whole **outer** line
   (`` `echo `case esac in esac)`' ``).

**Line number** matches the eval model: bash's comsub line = the outer line
where the backtick sits + (body-local line − 1). Verified with a backtick on
script line 3: near-token → `command substitution: line 3`; unterminated
(body EOF on body-line 2) → `command substitution: line 4`.

### Why `$()` already matches and backtick doesn't

bash parses a `$()` body as syntax during the *initial* parse pass → a failure
there is a normal top-level syntax error (`-c:`). A backtick body is treated as
an opaque string initially and re-lexed/re-parsed at *expansion* time; a failure
in that re-parse gets the `command substitution:` context. huck happens to
re-parse the backtick body too (see below), so the fix mirrors bash's structure.

### Scope

- **Backtick only.** `$()` / `$(())` / `${}` are out of scope (they match).
- Single-line backtick bodies byte-exact; multi-line best-effort (the same
  bash off-by-one quirk documented for eval in v315). posix2 already PASSes;
  no bash-suite category flips here — this is `sev:low` polish closing #213.

---

## Architecture

Because backtick fails at **parse** time (no `Shell`, unlike eval's execution-
time `eval_frame`), the `command substitution:` context rides the `ParseError`
itself, and the renderer recurses.

### Front layer (huck-syntax) — carry the context

`parse_backtick_sub` (`crates/huck-syntax/src/parser.rs:1958`) phase 3 re-parses
the cooked body with a fresh sub-lexer: `parse_sequence(&mut sub)?`. Replace the
bare `?` with a wrap-on-failure:

```rust
    let sequence = match parse_sequence(&mut sub) {
        Ok(Some(mut seq)) => { zero_lines_in_sequence(&mut seq); seq }
        Ok(None) => empty_sequence(),
        Err(inner) => {
            return Err(ParseError::InCommandSub {
                inner: Box::new(inner),
                body: cooked,
                err_pos: sub.cursor_pos(),
            });
        }
    };
```

New variant (in `crates/huck-syntax/src/command.rs`):

```rust
/// v316 (#213): a syntax error raised while re-parsing a backtick command-
/// substitution body. Carries the body error, the cooked body (for the echo +
/// body-local line), and the body-relative error offset. Rendered with bash's
/// `command substitution:` marker.
InCommandSub { inner: Box<ParseError>, body: String, err_pos: usize },
```

A placeholder `Display`/message arm renders `inner`'s message (the real render is
the engine's job); this variant is `#[derive]`-compatible (`Clone, Debug,
PartialEq, Eq`) since `String`/`Box<ParseError>` are.

### Back layer (huck-engine) — renderer recursion + marker

Refactor `render_syntax_diag` into a thin public wrapper + a shared worker that
takes the marker and line base explicitly:

```rust
pub fn render_syntax_diag(shell: &Shell, err: &ParseError, source: &str, token_line: u32) {
    // Top-level entry: derive marker + base from the eval frame (v315), unchanged.
    let (marker, base) = match shell.eval_frame {
        Some(n) => (Marker::Eval, n.saturating_sub(1)),
        None => (Marker::Default, 0),
    };
    render_diag_inner(shell, err, source, token_line, marker, base);
}

fn render_diag_inner(shell: &Shell, err: &ParseError, source: &str,
                     local_line: u32, marker: Marker, line_base: u32) {
    match err {
        ParseError::InCommandSub { inner, body, err_pos } => {
            // The backtick's own display line = line_base + local_line.
            // The comsub body numbers from 1; offset it by (backtick line − 1).
            let comsub_base = line_base + local_line.saturating_sub(1);
            let body_local = 1 + body.as_bytes()[..(*err_pos).min(body.len())]
                .iter().filter(|&&b| b == b'\n').count() as u32;
            render_diag_inner(shell, inner, body, body_local, Marker::CommandSub, comsub_base);
        }
        // ... the existing Shape 1/2/3 arms, but every emitted line becomes
        //     `line_base + <the shape's local line>`, and the emit call passes
        //     `marker` so the prologue prints eval:/command substitution:/-c:.
    }
}
```

`Marker` is a small enum: `Default` (→ the existing `-c:`/script logic),
`Eval` (→ `eval:`), `CommandSub` (→ `command substitution:`).

**Unify the line base into the `line_base` param.** v315 applies the eval base
in two places — `shell.rs` pre-adds `shell.line_base()` to the syntax-error
`ln` (Shape 1), and `render`/`emit_matching` read `shell.line_base()` for the
Shape 2/3 EOF line. Keeping that *and* adding a separate comsub base would
double-count (and fail to compose for backtick-in-eval). Instead, make
`line_base` the single mechanism:

- `shell.rs`'s `Err` arm passes the **raw local line** (`1 + newlines`, no
  `line_base()` pre-add) as `token_line`.
- `render_diag_inner` adds its `line_base` param to every shape's local line
  (Shape 1 token line, Shape 2/3 EOF/delim line). The internal
  `shell.line_base()` reads in `render`/`emit_matching` are removed.
- The top-level `render_syntax_diag` sets `line_base = shell.line_base()` (the
  eval base) — so eval's byte output is identical, just applied in one place.
- The `InCommandSub` arm composes: `comsub_base = line_base + local_line − 1`.

`Shell::eval_frame` / `line_base()` stay as-is (they still derive the top-level
base + `Marker::Eval`); only the *consumption* moves to the param.

**Marker emission.** The emit path gains the marker. Keep `error_prefix`'s
`Diag::Syntax` for `Default`/`Eval` (still reads `eval_frame` for the `eval:`
segment). Add `Diag::SyntaxNested { line, marker: &'static str }` printing
`<name>: <marker>: line N: ` (name via the same is_interactive/argv0 logic, no
`-c:`) for `Marker::CommandSub`. `render_diag_inner`'s emit picks the prologue by
`marker`.

The Shape-1 echo uses `source` (the body inside the recursion) so it echoes the
backtick body verbatim, matching bash.

**Regression guard:** the refactor is byte-neutral for `Marker::Default` (base 0,
no marker) and `Marker::Eval` (base = eval_frame−1, `eval:` marker) — pinned by
`syntax_error_diag_diff_check.sh` (27) + `eval_line_diag_diff_check.sh` (10),
which must stay green.

### Nesting

A backtick inside an eval (or backtick-in-backtick) composes through
`line_base`: the outer eval frame sets the initial base; the `InCommandSub` arm
adds the backtick offset on top. Exotic; single-line-exact, not separately
guaranteed for deep multi-line nesting.

---

## Testing

- **New `tests/scripts/comsub_marker_diag_diff_check.sh`** (byte-diff huck vs
  bash, normalize only the `<name>:` prefix, compare stderr + rc without a pipe
  clobbering `$?`): backtick unterminated-case (Shape 2), near-token (Shape 1 —
  pins the body echo), unterminated-quote (Shape 3), bad-paren (Shape 1);
  backtick on a later **script** line (line-base, via a temp-file script); and a
  `$()` **control** (`echo $(case x in)` → still `-c:`, marker only for backtick).
- **Guards:** `eval_line_diag_diff_check.sh` and `syntax_error_diag_diff_check.sh`
  stay green (eval + top-level unaffected — the refactor is byte-neutral for
  `Marker::Default`/`Marker::Eval`).
- **Full `run_diff_checks.sh`** green (backtick harnesses `backtick_escape`,
  `cmdsub_comment`, etc. unaffected — only the *error* path changes).
- Closes #213; no follow-on expected.

## Rejected alternatives

- **A Shell context flag like `eval_frame`.** Backtick fails at parse time,
  before execution — there is no Shell frame to set. The context must ride the
  `ParseError`.
- **Duplicate the Shape 1/2/3 classification in the `InCommandSub` arm.** The
  recursion reuses the existing shape logic against the body; duplicating it
  would drift.
- **Keep v315's eval base separate from the comsub base** (two mechanisms).
  Rejected: it double-counts and doesn't compose for backtick-in-eval. Unifying
  the base into the single `line_base` param is required for correctness, not
  just tidiness — and stays byte-neutral for eval (pinned by the v315 harnesses).
  `eval_frame` itself is kept; only where the base is *consumed* moves.
