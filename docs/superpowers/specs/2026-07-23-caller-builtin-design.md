# v330 — `caller` builtin

Issue: [#281](https://github.com/jdstanhope/huck/issues/281) — fourth step of the `dbg-support` bash-suite category sub-arc.

## Problem

huck lacks the `caller` builtin (`caller: command not found`, rc 127). bash's
`caller` prints the current subroutine's call context, walking the call stack.
It is a genuinely useful standalone builtin (users call `caller` for stack
traces) and clears the `caller: command not found` errors interleaved through
the `dbg-support` diff — though measurement (below) shows its category impact
is modest (509→475): the dominant residual there is a separate DEBUG-output vs
command-substitution line-ordering issue, not `caller`.

Verified against bash 5.2.21 (`f` called from `g` at line 8, `g` from main at
line 9):
```
caller           -> 8 /tmp/x.sh        (LINE FILE),      rc 0
caller 0         -> 8 g /tmp/x.sh      (LINE FUNC FILE), rc 0
caller 1         -> 9 main /tmp/x.sh                     rc 0
caller 2         -> (out of range)                       rc 1  (no output)
caller foo       -> "caller: foo: invalid number" + "caller: usage: caller [expr]", rc 2
caller 0 99      -> 8 g /tmp/x.sh       (extra args ignored, first used)
caller  (top level, no subroutine)                       rc 1  (no output)
```

## Design

`caller` reads huck's existing `call_stack` (`Vec<Frame>`, `Frame { funcname,
source, call_line, kind }`), whose indexing already backs the correct
`FUNCNAME`/`BASH_SOURCE`/`BASH_LINENO` arrays. From `sync_call_arrays`:
`FUNCNAME[i] = call_stack[n-1-i].funcname`, `BASH_SOURCE[i] =
call_stack[n-1-i].source`, `BASH_LINENO[i] = call_stack[n-1-i].call_line`
(i = 0 is the current/top frame; `n = call_stack.len()`).

So `caller N` needs `BASH_LINENO[N]`, `FUNCNAME[N+1]`, `BASH_SOURCE[N+1]`:
- `LINE = call_stack[n-1-N].call_line`
- `FUNC = call_stack[n-2-N].funcname`
- `FILE = call_stack[n-2-N].source`
valid iff `n >= N + 2`.

### Registration

Add `"caller"` to the builtins name list and a dispatch arm
`"caller" => builtin_caller(args, out, err, shell)` in `crates/huck-engine/src/builtins.rs`.
`caller` is a regular (non-special) builtin.

### `builtin_caller(args, out, err, shell) -> ExecOutcome`

```rust
fn builtin_caller(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let n = shell.call_stack.len();
    match args.first() {
        // No arg: LINE FILE from the frame that called the current one.
        None => {
            if n >= 2 {
                let line = shell.call_stack[n - 1].call_line;
                let file = shell.call_stack[n - 2].source.clone();
                let _ = writeln!(out, "{line} {file}");
                ExecOutcome::Continue(0)
            } else {
                ExecOutcome::Continue(1)
            }
        }
        Some(a) => {
            // bash: `legal_number` — a plain non-negative decimal integer.
            let k: usize = match a.parse::<u64>() {
                Ok(v) => v as usize,
                Err(_) => {
                    crate::sh_error_to!(shell, err, Some("caller"), "{a}: invalid number");
                    let _ = writeln!(err, "caller: usage: caller [expr]");
                    return ExecOutcome::Continue(2);
                }
            };
            // LINE FUNC FILE for frame k (extra args ignored).
            if n >= k + 2 {
                let line = shell.call_stack[n - 1 - k].call_line;
                let func = shell.call_stack[n - 2 - k].funcname.clone();
                let file = shell.call_stack[n - 2 - k].source.clone();
                let _ = writeln!(out, "{line} {func} {file}");
                ExecOutcome::Continue(0)
            } else {
                ExecOutcome::Continue(1)
            }
        }
    }
}
```

Match the exact error format against bash: `caller: foo: invalid number` on
the first line (via `sh_error_to!` with name `"caller"` and message
`"{a}: invalid number"`, which yields the `<script>: line N: caller: foo:
invalid number` prefix bash also emits), then `caller: usage: caller [expr]`.
Verify the byte-exact stderr in the harness.

### Notes / edges

- **Top level** (`n < 2`): rc 1, no output — matches bash's true top-level
  `caller`/`caller N`. (bash's script-file `0 NULL` quirk is not reproduced;
  the `dbg-support` uses of `caller` are all inside functions.)
- **Negative / non-decimal arg** (`caller -1`, `caller 0x1`): `u64::parse`
  fails → invalid-number error, rc 2. Confirm bash agrees (bash rejects
  non-`legal_number` args); if bash accepts arithmetic (`caller 1+1`), that is
  out of scope (dbg-support only uses `caller`/`0`/`1`/`foo`).
- `caller`'s own invocation does not push a call frame (it is a builtin), so
  the stack reflects the enclosing subroutine — exactly what bash reads.

## Testing

Gate = bash 5.2.21 fidelity + `dbg-support` diff shrinkage.

1. **Bash-diff harness** `tests/scripts/caller_diff_check.sh` (model on
   `trap_zero_diff_check.sh`). Cases (byte-identical incl. exit + stderr):
   - `caller` / `caller 0` / `caller 1` inside a two-deep call (`g`→`f`).
   - `caller N` out of range → rc 1, no output.
   - `caller foo` → invalid-number error + usage, rc 2.
   - `caller 0 99` → extras ignored.
   - `caller` at top level → rc 1.
   - a stack-trace loop like `dbg-support.sub` (`for ((i=0; i<${#FUNCNAME[@]}; …))`
     with `caller`) — deep-ish nesting.
   Use `check_file`/`check_stdin`-style comparison as needed (the FILE path
   gives real `caller` file names).
2. **`dbg-support` diff shrinkage**: re-run `HUCK_BASH_TEST_CATEGORY=dbg-support`
   and record the new size (measured: 509 → 475 — the `caller`
   errors resolve; the residual is dominated by a separate DEBUG-output vs command-sub line-ordering issue, not caller). Note the residual for the sub-arc's next
   step / possible flip.
3. **Regression**: `dbg-support2` stays PASS; `type`/`command`/`enable`
   builtins now recognize `caller` (it's a builtin — check `type caller`,
   `command -v caller` if those are tested); the DEBUG harnesses stay green;
   `funcname` / `functions_integration` green; full `run_diff_checks.sh` sweep
   green; huck-engine lib green. A new `caller_integration` `-p huck` binary is
   optional but nice.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` builtin/function integration binaries single-threaded before push; NO
GPL bash text.

## Scope

**In scope.** The `caller` builtin (registration + `builtin_caller`); the
harness; the `dbg-support` measurement; regressions. `type`/`command -v`
recognition falls out of adding it to the builtins list.

**Out of scope.** Arithmetic-expression `caller` args (`caller 1+1`); the
script-file top-level `0 NULL` quirk; `$BASH_COMMAND`. If a `dbg-support`
residual remains after this, it is the sub-arc's next step (a possible flip
assessment).

## Documentation

- `docs/architecture.md`: add `caller` to the builtin list / the "where to add
  a new builtin" cheatsheet if it enumerates builtins.
- Removes a divergence (no new intentional one). #281 auto-closes via the PR
  body (`Closes #281`).
