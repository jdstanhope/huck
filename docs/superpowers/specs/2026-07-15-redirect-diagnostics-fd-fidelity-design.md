# v297 — redirect diagnostics & fd fidelity (cluster A) — Design

**Issues:** closes [#152](https://github.com/jdstanhope/huck/issues/152),
[#140](https://github.com/jdstanhope/huck/issues/140),
[#141](https://github.com/jdstanhope/huck/issues/141). All three were surfaced by the
2026-07-14 fd-management differential audit and live in the same redirect-lowering +
error-emit path (`crates/huck-engine/src/executor.rs`, `expand.rs`).

## Goal

Make huck's redirect **diagnostics** (error message text) and `{var}` fd **numbering**
byte-identical to bash 5.2.21 for the cases the audit flagged. Three threads, one cohesive
area:

1. **#152** — the `ambiguous redirect` message must name the offending (un-expanded) word.
2. **#140** — `{var}`/dup redirect error wording + source-order `$v`-visibility (incl. the
   external/pipeline-stage path).
3. **#141** — an external command's inherited `{var}` fd *number* must not shift when a file
   redirect precedes it (virtual `{var}` allocation in the child plan).

**Non-goal:** the broader error-model refactor (see [[huck-param-expansion-debt]]) — these are
localized fixes, not a rework. macOS/job-control (#97) and the other recent issues (#137/#144/
#142/#147/#151) are out of scope for v297.

## Shared machinery (already exists)

- `reconstruct_word_source(word: &Word) -> String` (`expand.rs:1474`) renders a `Word` back to
  its literal source text (`$(echo a b)`, `$v`). This is the key primitive for every
  "echo the un-expanded word" case — no new rendering needed.
- `expand_single(word, shell, err)` (`executor.rs:3023`) — expands a redirect *target* word;
  emits `ambiguous redirect` when the word yields ≠1 field (`3034`). A second bare site at `4858`.
  A third site already names the word: `{name}: ambiguous redirect` (`4716`). (Line numbers are
  approximate anchors — the plan verifies them against the live tree.)
- `resolve_fd_target(source: &Word, shell) -> Result<i32, io::Error>` (`executor.rs:3042`) —
  expands a `>&w`/`<&w` dup source and parses it as an fd; today errors `bad fd: {expanded}`.
- `lower_one_redirect(...)` (`executor.rs:4698`) → `PlanOp` list; the `{var}` (`NamedFd`) arm
  relocates the source via `dup_to_high_fd(raw_src, 10, false)` (`4817`) — this is where the
  child-plan fd *number* is chosen (#141). `PlanOp::NamedFd { high, name }` (`4654`).
- Differential gate: `tools/redirect_audit.sh` (157 cases, **16 DIVERGE** at baseline).

## Section 1 — #152: name the word in `ambiguous redirect`

huck already prints the `line N:` prefix; it only omits the offending word.

| case | bash 5.2.21 | huck (today) |
|---|---|---|
| `cat >$(echo a b)` | `line 1: $(echo a b): ambiguous redirect` | `line 1: ambiguous redirect` |
| `cat <$(echo a b)` | `line 1: $(echo a b): ambiguous redirect` | `line 1: ambiguous redirect` |

**Fix:** at the two bare emit sites (`executor.rs:3034`, `4858`), format the message as
`"{}: ambiguous redirect"` with `reconstruct_word_source(word)`. The site at `4716` already
uses this shape (`{name}: ambiguous redirect`) — align the other two to it. rc unchanged: the
redirect fails so the command does not run; both shells already rc=1 here — only the message text
changes.

## Section 2 — #140: `{var}`/dup diagnostics + source-order `$v`-visibility

Four sub-cases. bash processes redirects **left-to-right**, and a `{var}>…` redirect assigns
`$v` as a side effect that a *later* sibling redirect in the same command sees.

| # | case | bash 5.2.21 | huck (today) |
|---|---|---|---|
| a | `{v}>&9` (dup an unopened fd) | `redirection error: cannot duplicate fd: Bad file descriptor`  **then**  `line 1: 9: Bad file descriptor` (two lines) | `line 1: 9: Bad file descriptor` (one line) |
| b | `exec {v}>f; exec {v}>&-; echo x >&$v` | `line 1: $v: Bad file descriptor` (literal `$v`) | `line 1: 10: Bad file descriptor` (resolved number) |
| c | `2>&$v {v}>f` (use `$v` before it is assigned) | `line 1: $v: ambiguous redirect` | `line 1: bad fd: ` (single cmd) / `bad fd:` (pipeline) |
| d | `true {v}>f 2>&$v \| cat` (assign then use, **external/pipeline stage**) | silent success (`$v`→10, `2>&10` dups the `{var}` fd) | `bad fd:` (`$v` not visible on the external stage path) |

**Fix — message wording (b, c):** in `resolve_fd_target` and the dup-failure emit path, use
`reconstruct_word_source(source)` for the diagnostic instead of the resolved number / raw
expansion:
- **(c)** when the dup source expands to empty / ≠1 field (`$v` unset, `>&` with nothing) →
  `"{word}: ambiguous redirect"` (bash treats an empty dup target as ambiguous, not `bad fd`).
- **(b)** when the source resolves to a number but the subsequent `dup`/validity check fails →
  the error echoes the **raw word** (`$v`), not the resolved number. (huck currently formats
  with the resolved fd; switch to the word text.)

**Fix — the extra line (a):** for `{var}>&<badfd>`, bash emits an additional
`redirection error: cannot duplicate fd: <strerror>` line (no `line N:` prefix) *before* the
standard `line N: <fd>: Bad file descriptor`. In huck's `{var}` (`NamedFd`) dup-source failure
path, emit that leading line (with the `strerror` of the failing `dup`) then fall through to the
existing `<fd>: Bad file descriptor`. Order and prefixing must match the table exactly.

**Fix — source-order `$v`-visibility incl. external (d, and the general case behind c):**
huck must make each `{var}` redirect's assignment visible to *later* sibling redirects during
lowering, on **both** the in-process path (already correct per v292) **and the external
child-plan path** (`build_child_redir_plan`/`lower_one_redirect`) which #140 flags as still
broken. A later `>&$v` / `2>&$v` resolves against the `{var}` value assigned by an earlier op in
the same command; a `$v` used *before* any assignment expands empty → case (c)'s
`"$v: ambiguous redirect"`. This is the riskiest part of v297 (it threads `{var}` assignments
through the ordered child-plan resolution).

## Section 3 — #141: virtual `{var}` fd allocation on the child plan

An external command's inherited `{var}` fd number shifts when a file redirect precedes it,
because huck opens file redirects as `OwnedFd`s parked in the high range (≥10) until `fork`, and
then allocates the `{var}` fd as the lowest *real* free fd — which the parked temps have pushed up.

| case | bash 5.2.21 (child fd) | huck (today) |
|---|---|---|
| `cmd {v}>x` | 10 | 10 ✓ |
| `cmd 3>a {v}>x` | 10 | **11** |
| `cmd 3>a 4>b {v}>x` | 10 | **12** |

**Fix:** a *virtual* `{var}` allocator for the child plan. When lowering a `{var}` redirect for
the child (fork) path, choose the lowest number ≥10 that is **not** used as a *target* by an
earlier plan op and **not** by an earlier `{var}` in the same command, and emit
`dup2(source → that number)` in the child replay. The parked temps occupy the high range only in
the *parent*; in the child they are dup2'd to their targets and closed, so the virtual number is
free at replay time. bash gets fd 10 for the first `{var}` regardless of preceding file
redirects — the virtual allocator reproduces this. The in-process `{var}` numbering (which was
fixed by v292's Approach-B interleaving) is unaffected; this is the external/child-plan path.

## Error handling

Every message change keeps the existing rc semantics (rc=1 for a failing redirect; the message
text is the only change). The virtual allocator changes only the fd *number*, not success/failure
of the redirect. The source-order `$v`-visibility change must not alter the outcome of any
currently-passing case (only used-before-assign and cross-sibling-visible cases move).

## Testing

- **Primary gate:** `tools/redirect_audit.sh` must drop from **16 DIVERGE** toward 0 for the
  cluster-A cases (some of the 16 are these). Report the exact before/after DIVERGE set; any of
  the 16 that are NOT cluster-A stay divergent (tracked by their own issues) and must be listed
  so the count is honest.
- **New byte-identical `*_diff_check.sh`** cases for the message shapes (#152 ambiguous-word;
  #140 a/b/c; the source-order `2>&$v {v}>f` vs `{v}>f 2>&$v` pair; the external/pipeline `d`
  form), asserting stdout+stderr+rc byte-identical to bash. These belong in a new
  `redirect_diag_diff_check.sh` (or an extension of an existing redirect harness).
- **New fd-numbering harness** for #141: the `redirect_audit.sh` `$v` observable is blind to
  external child fd numbers, so add a small harness that runs `ls /proc/self/fd`-style
  fragments (`cmd 3>a {v}>x`, `cmd 3>a 4>b {v}>x`) through bash and huck and asserts the `{var}`
  fd number matches. Linux `/proc/self/fd` is fine (the box + CI are Linux); guard/skip on
  non-Linux like the existing platform-gated harnesses.
- **Regression:** the full sweep (`run_diff_checks.sh`) 188→ (189 after v296) + the new
  harness(es) green on both binaries; `fd_torture_diff_check.sh` 44 unchanged;
  `pipeline_redirect_audit.sh` 15/15 unchanged; engine lib green.

## Risk / sequencing notes for the plan

- Sections 1 and the message parts of 2 (a/b/c) are localized, low-risk emit-site changes —
  natural first tasks with the `redirect_audit.sh` gate.
- Section 2's source-order external `$v`-visibility (d) and Section 3's virtual allocator both
  live in the child-plan lowering (`lower_one_redirect`/`build_child_redir_plan`) and are the
  riskiest — group them and gate with the new fd-numbering + source-order harnesses.
- Watch [[huck-param-expansion-debt]]: keep these as targeted fixes; do not fold in the deferred
  error-model divergences. If a case turns out to require the error-model rework, STOP and split
  it to a follow-up issue rather than expand scope.
