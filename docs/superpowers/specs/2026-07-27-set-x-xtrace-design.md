# v339 — flip the `set-x` bash-suite category (xtrace fidelity)

**Issue:** [#310 — `set -x` xtrace divergences block the `set-x` bash test-suite category](https://github.com/jdstanhope/huck/issues/310)

**Goal:** flip the bash 5.2.21 test-suite `set-x` category from FAIL to a
byte-identical (0-diff) PASS, raising the runner's PASS count 26 → 27.

## Background

The `set-x` category (`run-set-x` → `set-x.tests` + `set-x1.sub`) checks that
`set -x` (xtrace) output matches bash exactly. Against huck it FAILs with a
44-line diff. Every diff line is explained by exactly **three independent
divergences**, all in the xtrace subsystem. None is env-dependent, so fixing
all three should produce a clean flip.

Diff triage (huck `<` vs bash `.right` `>`):

| Diff lines | Divergence |
|---|---|
| 4, 7, 10, 13, 16, 19 | Root 1 — arith-for update section trace spacing |
| 30 | Root 2 — append-assignment trace |
| 34–42 (huck) / 55–59 (bash) | Root 3 — `BASH_XTRACEFD` unsupported |

## Root 1 — arith-for section trace spacing

### Symptom
For `for ((i=0; i<=5; i++ ))`, huck traces the update section as `+ (( i++ ))`
(one space before `))`); bash traces `+ (( i++  ))` (two spaces). The init and
condition sections already match.

### Cause
`parse_arith_for_clause` (`crates/huck-syntax/src/parser.rs`) splits the
`for (( … ))` header into three sections and normalizes each with
`trim_section` (`parser.rs:4534`), which trims **both** leading *and* trailing
whitespace of the section's leading/trailing `Literal` parts.

Bash's rule, derived empirically against bash 5.2.21, is: **trim leading
whitespace only; preserve trailing whitespace verbatim.** Each section is then
rendered as `(( <section> ))`.

Evidence (`set -x; for ((<hdr>)); do :; done`):
- `i=0;i<=2;i++` → `(( i=0 ))` / `(( i<=2 ))` / `(( i++ ))` (all single space)
- `i=0 ; i<=2 ; i++ ` → `(( i=0  ))` / `(( i<=2  ))` / `(( i++  ))` (trailing kept, leading trimmed)
- `i=0;i<=2; i++ ` → `(( i=0 ))` / `(( i<=2 ))` / `(( i++  ))`

The same normalization drives `generate.rs::arith_for_to_source`
(`declare -f`), which diverges identically: bash prints
`for ((i=0; i<=5; i++ ))` (trailing space) where huck prints
`for ((i=0; i<=5; i++))`.

### Fix
Change `trim_section` to trim **leading** whitespace only (drop the `trim_end`
of the trailing `Literal`). The retained trailing whitespace flows through
`reconstruct_word_source_inner` — used by *both* the xtrace path
(`run_arith_for_inner`, `executor.rs:2033/2074/2135`) and `declare -f`
reconstruction — so a single change fixes the trace *and* the latent
`declare -f` divergence.

Arith **evaluation** ignores whitespace (`i++ ` evaluates identically to
`i++`), so there is no behavioral risk. All-whitespace / empty sections still
reduce to `None` (an all-whitespace section trims to empty via `trim_start` and
is removed), preserving current `for ((;;))` handling.

## Root 2 — append-assignment trace

### Symptom
For `foo=one; foo+=two`, huck traces `+ foo=onetwo`; bash traces `+ foo+=two`.

### Cause
The standalone-assignment trace block in `run_assignment_list`
(`executor.rs:4354`) always emits `name=<full current value>` — it uses `=`
regardless of the operator and reads the post-append variable contents via
`lookup_var`.

Bash traces `name` + the actual operator (`+=` or `=`) + the **RHS value this
statement assigned**, xtrace-quoted. Verified:
- `foo+=" $y"` (y=world) → bash `+ foo+=' world'` (the RHS expansion ` world`,
  *not* the full `hello world`).
- Plain `foo=one` → `+ foo=one` (RHS expansion == full value).

### Fix
The scalar bare-target branch of `apply_one_assignment` (`executor.rs:8123`)
already computes the exact string bash traces:
`let s = expand_assignment(&a.value, shell)`. Thread that value out of
`apply_one_assignment` (carry the expanded scalar RHS for the bare-scalar path;
`None` for array/associative/indexed paths) so the trace can reuse it — no
re-expansion, so command substitutions in the RHS never double-fire.

The trace then emits:
`{ps4}{name}{op}{xtrace_quote(rhs)}` where `op` is `+=` when `a.append` else
`=`, and `rhs` is the threaded scalar RHS. When the apply path returns no scalar
RHS (array/assoc/indexed target), fall back to the current `lookup_var`
behavior to avoid changing those paths.

### Scope
Bash also traces **array** assignments as their literal unexpanded source
(`a=($y $y)`, `b+=(3 4)`); huck currently traces garbage (`a=world`). This is a
separate pre-existing bug and is **not** exercised by `set-x`. It is explicitly
out of scope here and will be filed as a follow-up `divergence` issue.

## Root 3 — `BASH_XTRACEFD`

### Symptom
`set-x1.sub` does `exec 4>$TRACEFILE; BASH_XTRACEFD=4; set -x; echo 1..4;
unset BASH_XTRACEFD; …` then `cat $TRACEFILE`. Bash sends the `echo 1..4` and
`unset BASH_XTRACEFD` trace lines to fd 4 (the file), so they appear only later
when `cat` dumps the file (bash `.right` lines 55–59). huck ignores
`BASH_XTRACEFD` and sends them inline to stderr, so under `2>&1` they surface in
the wrong stream position (huck lines 34–42).

### Cause
`xtrace_emit` (`executor.rs:4215`) hardcodes `libc::write(2, …)`. huck has no
`BASH_XTRACEFD` support.

### Fix
Resolve the target fd at each emit site (**emit-time resolution**) via a helper:

```
fn xtrace_target_fd(shell: &Shell) -> i32 {
    // BASH_XTRACEFD, when it parses to a valid non-negative integer, is the
    // xtrace destination fd; otherwise (unset / empty / non-numeric) fd 2.
    match shell.lookup_var("BASH_XTRACEFD") {
        Some(v) => v.trim().parse::<i32>().ok().filter(|&n| n >= 0).unwrap_or(2),
        None => 2,
    }
}
```

Change `xtrace_emit(line)` → `xtrace_emit(fd, line)` (writing to `fd` instead of
the hardcoded `2`), and pass `xtrace_target_fd(shell)` at all **7** call sites
(`executor.rs:2677, 2740, 4244, 4357, 4714, 4759, 8871` — note 4357 is the
Root 2 assignment-trace site, so that edit lands there too). Every site already
has `shell` in scope (each computes `ps4(shell)`).

Emit-time resolution is chosen over assign-time capture because it is simpler
(no unset hook, no new `Shell` field) and naturally handles set→file,
`unset`→revert-to-stderr, and invalid→stderr — all the category needs. The mild
tradeoff is a variable lookup per trace line (cheap) and not matching a couple
of exotic assign-time edge cases (e.g. `BASH_XTRACEFD=4; exec 4>&-`) that no
category exercises.

Because the trace is emitted **before** the command runs, the trace for
`unset BASH_XTRACEFD` resolves fd 4 (still set) and lands in the file, matching
bash exactly (`.right` line 59). The subsequent `for f …` loop and `set +x`
resolve to fd 2 (now unset) and appear on the main stream (`.right` 38–53).

## Verification

- New harness `tests/scripts/set_x_diff_check.sh` (bash-vs-huck byte-identical)
  covering all three roots: arith-for section spacing (varied header spacing),
  scalar `+=`/`=` traces with expansion RHS, and a `BASH_XTRACEFD` redirect +
  `unset` round-trip.
- The official category runner must report `set-x` as 0-diff PASS
  (`HUCK_BASH_TEST_CATEGORY=set-x bash tests/bash-test-suite/runner.sh`).
- Full `tests/scripts/run_diff_checks.sh` sweep green — no regression,
  especially `setx_trace_fidelity`, `set_x`, arith-for, and any `declare -f`
  reconstruction harnesses (Root 1 changes `declare -f` arith-for output to
  match bash; confirm no harness encoded the old no-trailing-space form).
- Per-crate `cargo test` for `huck-syntax` (parser: `trim_section`) and
  `huck-engine` (executor: assignment trace, xtrace fd), plus the relevant
  `-p huck` integration binaries (`setx_trace_fidelity_integration`,
  `set_x_integration`, `arith_for_integration`, `declare_f_integration`) run
  single-threaded under a `ulimit -v` guard before push.

## Out of scope / follow-ups

- Array/associative assignment xtrace (literal unexpanded source) — new
  follow-up `divergence` issue (Root 2 scope note).
- Assign-time `BASH_XTRACEFD` capture and exotic fd-lifecycle edge cases — not
  needed for the flip; revisit only if a future category requires it.

## Summary of touched files

- `crates/huck-syntax/src/parser.rs` — `trim_section` (leading-only trim).
- `crates/huck-engine/src/executor.rs` — `apply_one_assignment` return
  (scalar RHS thread-out); `run_assignment_list` trace (operator + RHS);
  `xtrace_emit` fd parameter + `xtrace_target_fd` helper; 7 emit call sites.
- `tests/scripts/set_x_diff_check.sh` — new harness.
- `docs/bash-test-suite-baseline.md` — baseline update (PASS 26 → 27).
- Memory: `project_huck_iterations.md` + `MEMORY.md`.
