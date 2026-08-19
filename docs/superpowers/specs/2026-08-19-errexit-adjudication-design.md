# v364 — one adjudication per failure: `set -e` and ERR across control structures

**Issues:** [#676](https://github.com/jdstanhope/huck/issues/676) (`set -e` exits
on a failing `&&` list inside a compound body) and
[#685](https://github.com/jdstanhope/huck/issues/685) (a redirected compound
fires the ERR trap twice). Both are closed by this work; they are one root.

## The problem

`set -e` must not exit when the failing command was exempt — POSIX and bash
exempt "any command executed in a `&&` or `||` list except the command following
the final `&&` or `||`". huck applies that correctly at top level and loses it
the moment the list is the last command of a compound body:

```sh
set -e
X=no
if true; then
  [ "$X" = yes ] && [ "$X" = yes ] && exit 1
fi
echo REACHED
```

bash prints `REACHED` and exits 0. huck prints nothing and exits 1.

Found by the runtime sweep: `/usr/sbin/on_ac_power` ends in exactly this shape
and returns 1 under huck where bash returns 0.

### Measured: every in-place compound is affected

All rows are `set -e`, and all of them SHOULD reach the trailing command.

| fragment | bash | huck |
| --- | --- | --- |
| top level: `X=no; [ $X = yes ] && exit 1; echo R` | 0 | 0 ✅ |
| `if true; then false && true; fi` | 0 | **1** |
| `if false; then :; elif true; then false && true; fi` | 0 | **1** |
| `if false; then :; else false && true; fi` | 0 | **1** |
| `for i in x; do false && true; done` | 0 | **1** |
| `i=0; while [ $i = 0 ]; do i=1; false && true; done` | 0 | **1** |
| `i=0; until [ $i = 1 ]; do i=1; false && true; done` | 0 | **1** |
| `case x in x) false && true;; esac` | 0 | **1** |
| `{ false && true; }` | 0 | **1** |
| `{ false && true; } > /dev/null` | 0 | **1** |
| `if true; then ! true; fi` | 0 | **1** |
| `if true; then [[ 1 = 2 ]] && true; fi` | 0 | **1** |

⚠️ `while`/`until` were reported as unaffected in #676's original table. That was
wrong: those rows used `break`, and `break` resets the loop's status, which
masked it. Re-measured without it, they fail like the rest. The rule is uniform.

### Measured: what is already correct, and must stay correct

These are what stops the fix being "never adjudicate a compound".

| fragment (`set -e`) | bash | huck |
| --- | --- | --- |
| `( false && true )` — subshell | 1 | 1 ✅ |
| `f(){ false && true; }; f` — function call | 1 | 1 ✅ |
| `{ :; } > /nonexistent/x` — the compound's own redirect fails | 1 | 1 ✅ |
| `{ false; }` — plain inner failure | 1 | 1 ✅ |
| `if true; then false; fi` — plain inner failure | 1 | 1 ✅ |
| `[[ 1 = 2 ]]` / `(( 0 ))` | 1 | 1 ✅ |

A subshell's status is a wait status the parent sees fresh (its body ran in a
forked child). A function call's status belongs to the call. A failed redirect
is the wrapper's own failure. A plain inner failure exits via the INNER command's
adjudication, not the compound's — which is the point the fix turns on.

## The finding: the abstraction exists, and one line of its doc is the bug

`executor.rs::body_already_fired_err` already encodes precisely the needed
predicate — the same closed list (`BraceGroup`, `For`, `ArithFor`, `Case`,
`Select`, `If`, `While`), the same reasoning for every exclusion, and the `! !`
single-stage-pipeline unwrap from #481. Its doc says:

> errexit is deliberately NOT gated on this: `set -e; { false; }` must still
> exit, exactly as it does today.

That conclusion was checked against an OUTCOME rather than a mechanism.
`set -e; { false; }` does still exit — through the inner `false`'s own
adjudication, not the group's. So gating errexit on the same predicate is safe,
and the very case that motivated the exception is the acid test that it stayed
safe.

This is v355's lesson repeating: one question, two consumers, answered for one.

### The second bug in the same predicate

`Command::Redirected` is missing from the list, so a redirected compound fires
ERR twice (#685):

```sh
trap "echo ERRFIRE" ERR; { false; } > out.txt
```

`out.txt` holds ONE `ERRFIRE` under bash and TWO under huck — bash fires at the
inner `false`, inside the redirect; huck fires there AND again at the wrapper
after the redirect is torn down.

⚠️ This was nearly missed, and the near-miss is worth recording: counting fires
on STDOUT showed `bash: 0, huck: 1`, because bash's only fire happened inside the
redirect and went to `/dev/null`. Counting on stderr showed the truth. Any ERR
assertion in this area must direct the action's output outside the redirect
under test.

## The model

**Every failure is adjudicated exactly once, at the site that produced it. A
status a command INHERITED is never re-judged.**

Two mechanisms, no new state:

1. **A pass-through kind is not adjudicated.** `If`, `While`, `For`, `ArithFor`,
   `Select`, `Case` and `BraceGroup` run their bodies in place through
   `execute_sequence_body`, which already ran every inner command through
   `finish_command`. Their `Continue` is inherited. Everything else keeps
   adjudicating: `Simple`, `Pipeline`, `Subshell`, a function call,
   `DoubleBracket`, `Arith`.

2. **`Redirected` decides per branch.** `run_redirected` already knows whether
   the redirect failed or the inner command ran. A redirect failure is a fresh
   failure of the wrapper and is adjudicated there; otherwise the inner outcome
   passes through untouched.

The predicate keeps its list and its reasoning and is renamed
`status_produced_by_body` — it is no longer only about ERR — and both the
`fire_err_trap` call and `maybe_errexit` are gated on it in `finish_command`.
Its doc comment loses the "errexit is deliberately NOT gated on this" paragraph
and gains the reason that paragraph was wrong, so the next reader cannot
re-derive the original conclusion.

### Why not the alternatives

* **Carry provenance in `ExecOutcome::Continue`.** A type change across the whole
  executor, and every construct would have to decide what to propagate.
* **A sticky "the failure was exempt" flag on `Shell`.** `run_list_element`'s own
  doc warns that a leak of exactly this kind "would have made `set -e` silently
  stop working for the rest of the list" — the worst failure mode available here,
  because it is invisible: a script that should have exited and does not.

The chosen shape has nothing to leak, and "which kinds inherit" is answered by
reading one match that the compiler keeps exhaustive.

## Prerequisite: consolidate the four loop-body matches

`run_while_inner`, `run_for_inner`, `run_arith_for_inner` and `run_select_inner`
each carry the same ~20-line body-outcome match — decrement `LoopBreak(n)` and
`LoopContinue(n)`, forward `Exit`/`FunctionReturn`/`Interrupted`, keep `Continue`
as `last`. `run_case_inner` has a fifth, simpler variant; `BraceGroup` has none.

Threading a new concept through four copies is how the next person fixes a bug in
one of them. So the four collapse into one helper FIRST, as its own task, with a
**zero-diff gate**: a behaviour-preserving refactor must change no expected value
anywhere (v343's rule). `case` and `BraceGroup` are deliberately left alone —
their shapes differ and folding them in would be scope creep.

## Testing

A dedicated `tests/scripts/errexit_adjudication_diff_check.sh`, built as a
MATRIX rather than a list of the shapes in the issues:

* **constructs**: `if` / `elif` / `else`, `for`, `while`, `until`, arith-`for`,
  `select`, `case`, `{ }`, `( )`, function call, `Redirected` (redirect OK and
  redirect failing), pipeline, `[[ ]]`, `(( ))`, and nested combinations;
* **failure shapes**: plain `false`, exempt `&&` list, exempt `||` list,
  `! cmd`, a failing condition, a failing redirect;
* **both consumers**: the `set -e` exit status, AND the ERR fire COUNT — with the
  trap action writing to stderr, for the reason recorded above.

Gates:

1. the new harness goes RED against the pre-fix binary, and the spec's PR states
   by how many rows;
2. the consolidation task shows a zero diff — no expected value moves;
3. the full `tests/scripts/run_diff_checks.sh` sweep stays green;
4. the runtime sweep (`tools/runsweep`) is re-run and compared PER SCRIPT against
   `tools/run_results.aug19.tsv`. `set -e` is in nearly every system script and a
   wrongly-kept exemption is silent — a script that should have exited and does
   not cannot be seen in a status check, only in output.

## Out of scope

* **#683** (`return` with too many arguments is fatal per-driver) — a different
  root, in v358's error-fatality classifier.
* **#679/#680** (message wording, line numbers) — unrelated families.
* `case` and `BraceGroup` body-outcome handling — left as they are.
