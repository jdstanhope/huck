# v178: arithmetic expansion errors are fatal (nonzero status, abort) — Design

**Status:** approved 2026-06-17
**Iteration:** v178
**Origin:** Divergence found while triaging the arith cluster: an arithmetic
**expansion** syntax/eval error returns exit 0 in huck where bash returns nonzero
and aborts. E.g. `echo $((1;2)); echo SECOND` — bash prints the error, exits 1,
and SECOND does NOT run; huck prints the error but expands to empty, runs both
commands, and exits 0.

## Problem

When arithmetic in a **word-expansion context** fails to parse/evaluate, huck
prints `huck: arithmetic: <msg>`, sets `$?`=1, and CONTINUES with an empty
expansion — it never aborts the command. Three sites:

1. `expand.rs:901` — `$((…))` in `expand` (`Err` arm: `set_last_status(1)`, then
   leaves the field empty).
2. `expand.rs:1116` — `$((…))` in `expand_assignment` (same swallow).
3. `param_expansion.rs:207/212` — substring offset/length `${v:off:len}`: on
   `eval_substring_index` `Err(())` it does `return ExpansionResult::Empty`.

bash treats an arithmetic expansion error as a **fatal expansion error**: it
aborts the command with a nonzero status (and, non-interactively, the rest of the
list doesn't run). huck **already implements exactly this** via
`shell.pending_fatal_pe_error` / `ExpansionResult::Fatal { status }`, used by
`${x:?}`, `set -u`, `${arr[unbound]}`, the array *read* path `${a[1+]}`, and the
substring *range* error (`param_expansion.rs:222`). Confirmed: those abort
correctly in huck (e.g. `echo ${x:?bad}; echo SECOND` → rc 1, SECOND absent). The
three arith arms simply don't use the flag.

Confirmed divergences (huck rc 0 + continues; bash rc 1 + aborts):
```
echo $((1;2)); echo SECOND          x=$((1+)); echo SECOND
echo $((1 2)); echo SECOND          echo $((a[1+])); echo SECOND
v=hi; echo ${v:1+:2}; echo SECOND   v=hi; echo ${v:1+}; echo SECOND
```
Already correct (untouched): standalone `(( 1+ ))` and `let '1+'` (both rc 1,
non-fatal — matching bash); the array *read* `${a[1+]}` (already fatal).

## Goal

Make arithmetic-expansion errors fatal — nonzero status, command aborts — by
routing the three arith arms through huck's existing fatal-PE mechanism, so they
behave like every other expansion error (and like bash).

## Design

Replace the swallow at each site with the fatal-PE path (mirroring the sibling
`ExpansionResult::Fatal` handling):

- **`expand.rs:901`** (`expand` → `Vec<Field>`), the `Err(e)` arm:
  ```rust
  Err(e) => {
      eprintln!("huck: arithmetic: {}", e);
      shell.pending_fatal_pe_error = Some(1);
      return result;
  }
  ```
- **`expand.rs:1116`** (`expand_assignment` → `String`), the `Err(e)` arm: same
  three lines (`return result;` — the string built so far), mirroring the
  `ExpansionResult::Fatal { status } => { shell.pending_fatal_pe_error = Some(status); return result; }`
  arm already present in `expand_assignment`.
- **`param_expansion.rs:209` and `:214`** (substring offset/length), each:
  ```rust
  Err(()) => return ExpansionResult::Fatal { status: 1 },
  ```
  (was `ExpansionResult::Empty`). `eval_substring_index` already printed the
  `huck: arithmetic: …` diagnostic, and `Fatal` is the variant the caller turns
  into `pending_fatal_pe_error` (e.g. `param_expansion.rs:222`).

No new types; the executor already consumes `pending_fatal_pe_error` to abort the
command (`executor.rs:211/251/1646/2649/2694`). The status is `1` — which matches
bash for arithmetic-expansion errors specifically (bash uses 1 here, not the 127
it uses for `${x:?}`/`set -u`; so for this case huck's status equals bash's).

### Behavior

- `echo $((1;2))`, `x=$((1+))`, `echo $((1 2))`, `echo $((a[1+]))`,
  `${v:1+:2}`, `${v:1+}` → print the error, abort the command, exit 1, the next
  command in the list does not run — matching bash.
- Unchanged: valid arithmetic (`$((1+2))`, `${v:1:2}`); standalone `(( ))` / `let`
  (already rc 1, non-fatal); the array *read* path (already fatal).

## Verification

- **New bash-diff harness** `tests/scripts/arith_error_status_diff_check.sh`:
  compares **stdout + exit code** (stderr discarded — the `huck:` vs bash error
  *wording* differs by the intentional prefix convention) of `bash -c` vs
  `huck -c`. Cases assert the fatal abort: each `<bad-arith>; echo SECOND` yields
  empty stdout + exit 1 + no SECOND in BOTH shells, for `$((1;2))`, `$((1+))`,
  `$((1 2))`, `$((a[1+]))`, `${v:1+:2}`, `${v:1+}`. Plus controls that must NOT
  abort: valid `echo $((1+2)); echo SECOND` and `v=hi; echo ${v:1:2}; echo SECOND`
  (both print the value + SECOND, exit 0), and `(( 1+ )); echo SECOND` (standalone
  stays non-fatal — SECOND runs).
- The parse sweep (`tools/parse_sweep.sh`) is unaffected — this is a RUNTIME
  status fix, not a parse fix; these inputs already parse. (Note this in the
  report; no `HUCK_GAP` movement expected.)
- Full `cargo test` (0 failures). CHECK for existing tests that assert the old
  swallow behavior (a `$((bad))` expanding to empty / continuing) — e.g. in
  `expand.rs` / `param_expansion.rs` unit tests; update any that encode the
  pre-fix tolerance to the new fatal behavior (like v177's lexer-test update).
- All `tests/scripts/*_diff_check.sh` harnesses green, clippy clean.

## Scope boundary

In scope: the three arith-expansion sites above (`$((…))` ×2 + substring index),
routed through `pending_fatal_pe_error` / `ExpansionResult::Fatal`. **Not** in
scope: array-subscript **assignment** (`a[1+]=5`) — a separate, tangled path with
its own "`a`: bad array subscript" pre-arith rejection that doesn't reach the
fatal-PE mechanism (logged as a follow-on); standalone `(( ))` / `let` (already
correct); the error-message **wording** (intentional `huck:`-prefix family); the
bash-127-vs-huck-1 status for `${x:?}`/`set -u` (pre-existing, affects all
expansion errors, not arith). No `bash-divergences.md` change (never a tracked
divergence). Record in `project_huck_iterations.md` + `MEMORY.md`; note the
array-subscript-assignment follow-on.
