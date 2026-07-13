# v286 — Move-fd redirect (`<&N-` / `>&N-`) + faithful redirect regeneration

**Issue:** [#121](https://github.com/jdstanhope/huck/issues/121) — the dup-and-close
"move fd" redirect operator is unsupported, causing a deadlock. `divergence` +
`bug` + `sev:medium`.

**Deferred follow-up:** [#124](https://github.com/jdstanhope/huck/issues/124) —
`&>` / `&>>` operator preservation in `declare -f` (a separate AST change, NOT
part of the "best-effort" comment this iteration retires).

## Problem

Two related redirect-fidelity gaps:

1. **The move-fd operator is unsupported (the #121 deadlock).** bash's
   `[n]<&digit-` / `[n]>&digit-` duplicates `digit` onto `n` and then closes
   `digit`. huck's `dup_op` (command.rs) treats only a bare `-` as `Close`; the
   operand of `>&5-` is the word `"5-"`, which becomes `Dup { source: "5-" }` and
   fails fd-parse at runtime with `bad fd: 5-`, silently dropping the redirect.
   So `exec 0<&5-` never moves the file onto stdin, and a following `read` blocks
   on the terminal forever (bash-suite `redir` category; redir5.sub:21-23).

2. **Redirect regeneration is "best-effort".** `generate.rs` regenerates the
   redirects on `Command::Redirected` / `exec` / hoisted-brace-group function
   bodies by collapsing them to the fixed 0/1/2 `RedirectSlot` representation
   (`append_slot_redirects` → `slots_for_simple_path`), which **drops** fd>2
   targets, `Close` (`N>&-`), `<&` dup-in, `<>` (ReadWrite), `{var}` named fds,
   and source ordering. The code comment (generate.rs:124) admits this. Now that
   huck is close to bash-faithful, this stance should be retired.

## Design

### Section 1 — AST + parse: the move operator

Add a variant to `RedirOp` (command.rs):

```rust
/// `[n]>&digit-` (output) / `[n]<&digit-` (input): dup `source` onto the
/// target fd, THEN close `source`. bash's "move fd".
Move { source: Word, output: bool },
```

In `dup_op` (command.rs:1033), classify the operand three ways:

```rust
pub(crate) fn dup_op(source: Word, output: bool) -> RedirOp {
    match word_literal_text(&source) {
        Some("-") => RedirOp::Close,
        Some(t) if is_move_operand(t) => {
            // strip the trailing '-'; the numeric fd is the move source.
            RedirOp::Move { source: lit_word(&t[..t.len() - 1]), output }
        }
        _ => RedirOp::Dup { source, output },
    }
}
```

`is_move_operand(t)` = `t` matches `^[0-9]+-$` (one or more digits then a single
trailing `-`). This is bash's documented literal-`digit-` form; a non-numeric or
expanded source ending in `-` is left as a `Dup` (unchanged behavior). The
directional default fd (when no `n` prefix) is 1 for `>&` and 0 for `<&`, same as
`Dup`.

### Section 2 — executor: apply a Move

Add a `Move` arm alongside the `Dup` and `Close` arms in the ordered redirect
applier (executor.rs, the `apply` function ~line 1090). A move is exactly
"Dup then Close the source", both routed through the existing save/restore scope:

```rust
RedirOp::Move { source, output: _ } => {
    let src = resolve_fd_target(source, shell)?;   // same error path as Dup
    // validate src is open (bash: bad fd error), then dup onto target
    if fcntl(src, F_GETFD) < 0 { error "src: Bad file descriptor"; return Err }
    self.redirect(shell, src, target, ...)?;       // saves target, dup2(src->target)
    self.close_target(src);                         // saves src, closes src
    Ok(())
}
```

Because both `redirect` (target) and `close_target` (source) record prior state
in the `RedirScope`, a **command-scoped** move (`cmd 3<&5-`) restores both fds
after the command, while an **`exec`** move persists both — matching bash. Move
must also be handled anywhere `Dup`/`Close` are handled on the pipeline-stage
fast path (`build_child_extra_ops`); Rust's exhaustiveness checks will surface
every site. Move behaves like `Dup` for `default_fd()` (output→1, input→0) and is
dropped from `slots_for_simple_path` like `Close`/`<&` (handled additively).

### Section 3 — faithful ordered redirect regeneration

Replace `append_slot_redirects` (generate.rs) with an **ordered** `append_redirects`
that renders each `Redirection` in source order via a new
`redirection_to_source(r: &Redirection) -> String`, repointing all three call
sites (hoisted brace-group, `Command::Redirected`, `exec_to_source`). Delete the
now-dead slot renderer (`redirect_to_source` + `RedirDefault`) and remove the
"best-effort" comment.

`redirection_to_source` matches bash's canonical `declare -f` rendering, reverse-
engineered from bash 5.2.21 (each row verified):

| `RedirOp` | fd prefix rule | operator | space? |
|---|---|---|---|
| `File{ReadOnly}` (`<`) | drop iff fd is default **0** | `<` | yes |
| `File{Truncate}` (`>`) | drop iff fd is default **1** | `>` | yes |
| `File{Append}` (`>>`) | drop iff fd is default **1** | `>>` | yes |
| `File{Clobber}` (`>\|`) | drop iff fd is default **1** | `>\|` | yes |
| `File{ReadWrite}` (`<>`) | **always show** (default `0`) | `<>` | yes |
| `Dup{output}` | **always show** (default 1/0) | `>&` / `<&` | no |
| `Move{output}` | **always show** (default 1/0) | `>&`…`-` / `<&`…`-` | no |
| `Close` | show fd (parser resolves default→1/0) | `>&-` (direction normalized) | no |
| `Heredoc` / `HereString` | (unchanged from current renderer) | | |

FD-prefix encoding: `RedirFd::Var(name)` → `{name}`; `RedirFd::Number(n)` → `n`
(or `""` when droppable per the rule); `RedirFd::Default` → `""` (droppable File)
or the directional default digit (Dup/Move/ReadWrite).

Worked examples (bash-verified): `1>file`→`> file`, `2>file`→`2> file`,
`0>file`→`0> file`, `>&2`→`1>&2`, `3<&0`→`3<&0`, `3<&-`→`3>&-`, `0<&-`→`0>&-`,
`0<&5-`→`0<&5-`, `>&2-`→`1>&2-`, `<>file`→`0<> file`, `{v}<&3-`→`{v}<&3-`,
`3>&1 4<&0`→`3>&1 4<&0` (order preserved).

`slots_for_simple_path` and the executor pipeline path are **left untouched** —
only the generate side switches to the ordered renderer.

### Section 4 — out of scope

- **`&>` / `&>>` preservation** — deferred to #124 (needs an AST change to keep
  the combined operator; not named by the "best-effort" comment being retired).
  Regenerating `&>file` as `> file 2>&1` is unchanged from today (not a
  regression).
- No change to `slots_for_simple_path` or any executor path other than adding the
  `Move` arms.

## Testing

1. **Move-fd correctness harness** `tests/scripts/move_fd_redirect_diff_check.sh`
   — byte-identical bash↔huck for: the `exec 0<&5-` file-to-stdin move (the #121
   repro, reading a fixture so it never blocks), `>&N-` output move, exit status,
   command-scoped restore (`{ cmd 3<&5-; } ; use fd 3 after`), `exec` persistence,
   and the degenerate `N>&N-` (target == source).

2. **Faithful-regeneration harness** `tests/scripts/redirect_regen_diff_check.sh`
   — `declare -f` over a function body containing every redirect form in the
   Section 3 table (fd>2, `<&` dup-in, `<>`, `{var}`, `N>&-` close, move, and a
   multi-redirect ordered sequence), byte-identical to bash.

3. **Unit tests** (huck-syntax): `dup_op` classifies `-`/`5-`/`5` → Close/Move/Dup;
   `redirection_to_source` renders each row of the Section 3 table.

4. **Regression:** all existing redirect diff harnesses stay green —
   `func_redirect`, `function_redirect`, `fd_redirect`, `compound_redirects`,
   `assign_redirect`, `pipe_compound_redirect`, `pipeline_redirect_pipe`,
   `heredoc_redir_v266`, `declare_f`, plus the full `run_diff_checks.sh` sweep;
   and `cargo test -p huck-syntax` / `-p huck-engine` (per-crate, single-threaded).

## Non-goals

- `&>` / `&>>` operator preservation (#124).
- Any change to runtime redirect *execution* beyond adding move support.
- `read`/`minimal`/`jobs` bash-suite timeout budget (#123).
