# Tech-debt hubs: a roadmap for the open divergence tracker

**Date:** 2026-09-17. **Tracker:** [#765](https://github.com/jdstanhope/huck/issues/765). **Status:** live — tick items off as PRs merge.

An audit of the 124 open issues on 2026-09-17 (114 labelled `divergence`) found
that roughly forty of them are cells of nine shared mechanisms. Fixing a cell
alone leaves its siblings inconsistent — several issue bodies say so outright
(#698 warns that fixing #347 alone would BREAK a currently-agreeing row; #691 is
"gated on #600"; #717 declined to fix one of its five consumers for exactly this
reason). This document names each hub, the issues it closes, the evidence that it
is one mechanism, and the order to take them in.

The ordering principle is **issues closed per unit of risk**: contained,
harness-provable hubs first; the two that reshape `VarValue` as two separate
passes (decided 2026-09-17 — the null-state pass lands first, the byte-string
pass later, accepting that expansion arms get re-audited twice).

Ceremony follows `CLAUDE.md`: hubs 1, 4 and 5 are bug-fix-round shaped (branch →
fix → harness → sweep → PR, self-merged after CI). Everything from hub 2 on
changes a shared subsystem's semantics and gets a spec + plan + hand-off PR.

## Order

| # | Hub | Closes | Shape | Status |
|---|-----|--------|-------|--------|
| 1 | Job table: bash's five cleanup points + stored `+`/`-`; fork-site signal dispositions | #475 #758 (round 1, PR #769); #185 (round 2, PR #771); #478 #766 (round 3, PR #773) | bug-fix rounds | ✅ done (#772 filed) |
| 4 | One AST printer (`generate` gains bash's outside-a-function style; diagnostics, `jobs`, `$BASH_COMMAND` use it) | #761 #770 (round 1, PR #774); #124 (round 2, PR #775); #589 moved to hub 5 (it is the matcher's escape form) | bug-fix rounds | ✅ done |
| 5 | One pattern-matching chokepoint (`glob_match::pattern_matches`), pattern text in bash's form | #717 #303 #589 (PR #776) | bug-fix round | ✅ done |
| 2A | Declared-but-unset variable state | #600 #225 #33 #691 | vNN | |
| 2B | One declaration policy table | #347 #697 #698 #734 #65 | vNN (after 2A) | |
| 3 | Piped-stdin reader feeds the lexer; parse the line before running it | #701 #81 #21 #79 #575 | vNN | |
| 6a | Brace expansion out of the lexer | #24 #44 #387 | vNN | |
| 6b | Tilde recognition out of the lexer | #295 #72 | vNN | |
| 7 | Nested list/and-or/pipeline AST → `time`, per-stage DEBUG | #5 #756 #268 #263 | vNN | |
| 6c | `${…}`/backtick body errors at expansion time | #493 #576 #574 #650 #608 #640 (terminator half) | vNN | |
| 8 | Byte-transparent variable values | #738 #52 #63 | vNN (second `VarValue` pass) | |

Smaller pairs, not scheduled: #101 + #30 (error routing: thread-local vs the
held writer); #106 / #536 (lexer decomposition — bank as a side effect of 6a/6b).

## The hubs

### 1. Job table — bash keeps a job until its status is *reported*

huck removes a job from the table when it is **reaped**; bash keeps it until its
status has been **reported** (the `[N]+ Done` notice, or `wait` returning it).
Every issue here is that one difference seen from a different angle:

- #475 — a just-exited background job is pruned from `jobs` sooner than bash.
- #185 — a coproc is torn down at the next command boundary (v306 moved reaping
  into `reap_and_notify` → `reap_coproc`); the round-trip test became a race.
- #476 — the coproc harness fails ~2 in 8 standalone on an idle box.
- #758 — `wait %` rarely returns 127 instead of 137 under the sweep.
- #200 — `$!` after a backgrounded builtin races the job's output.
- #428 — background children carry `SigIgn` for TSTP/TTIN/TTOU unconditionally;
  bash ignores those only when job control is OFF (`setup_async_signals`).

#158's headline (`kill -STOP` → `Stopped` in `jobs`) is already fixed — probed
2026-09-17; only its residuals remain (`%1` as a bare command resumes the job in
bash; `%string` / `%?substr` matching).

**Why it is one hub — corrected by round 1 (2026-09-17):** the *notified*
bit already existed. What huck had that bash does not was a FUSED
reap+report+prune pass between every command. bash reaps whenever `SIGCHLD`
arrives and reports/prunes at exactly five points — the end of a foreground
`wait_for`, `jobs`, each loop iteration (`REAP()`), `wait`, and every new
input line the parser reads (`parse.y shell_getc`) — and what it reports
splits on `startup_state`: a script file (or piped stdin) reports a
background job only when a signal killed it; `-c` marks a normal exit
reported silently. Round 1 ported that, plus bash's stored `j_current`/
`j_previous` (a dead job keeps `+`; a stop takes it), `max+1` job numbering,
`kill`'s re-arm/skip-dead rules, and job-control-gated `WUNTRACED`. #428's
headline turned out already fixed; #200 is a test-hygiene item whose fix
landed earlier. Round 2 (#185): `coproc_reap` belongs inside
`cleanup_dead_jobs`, not the reap. Round 3 (#766 + #478): the fork-site
signal dispositions.

**Gate:** a `job_lifecycle_diff_check.sh` covering `jobs` after `wait`, after a
signal-interrupted `wait`, a coproc exiting mid-script at several delays (#185's
table), and `kill -TSTP %1` under `set -m` on and off. Run the three flaky
tests in a loop against the frozen binary before and after.

### 4. Three printers for one AST

**Corrected by round 1 (2026-09-18):** there were three, not two — the
`jobs` column had its own (`render_job_*`, with a `background job` fallback
for compounds). bash's `print_cmd.c` has ONE printer with a mode,
`inside_function_def`: in a function body `;` ends the line and `{ }` is
multi-line; everywhere else (a `$( )` body's text at parse time, the `jobs`
column) `;` joins inline and `{ }` is one line, while `if`/`for`/`while`
keep their multi-line shape in both. `generate.rs` now carries that mode;
the diagnostic printer delegates comsub/procsub bodies to it and the jobs
column and `$BASH_COMMAND` call it directly. #589 turned out to be the
pattern MATCHER's escape form leaking into xtrace, not the printer — it
belongs to hub 5.

`crates/huck-syntax/src/generate.rs` (used by `declare -f`, `type`, `export -f`)
and `crate::expand::reconstruct_word_source` (45 callers — every diagnostic and
xtrace line that names a word) both render the AST back to text, and they have
drifted. Probed 2026-09-17: `declare -f` prints `$(echo X 1>&2)` in bash's
normalised form; `reconstruct_word_source` drops the redirect (#761).

- #761 — a diagnostic names `$(echo X)` for `$(echo X 1>&2)`. Closes outright
  by routing the diagnostic renderer through `generate.rs`'s word printer.
- #124 — `&>` is expanded at parse time into two redirections, so BOTH printers
  render it as `> f 2>&1`. Needs the operator preserved in the AST (a `RedirOp`
  variant); with one printer that is one rendering site, not two.
- #589 — `set -x` renders a quoted `[[ == ]]` glob operand with huck's internal
  escape. Same seam: the xtrace renderer must not see an internal representation.

**Gate:** extend `declare_f_diff_check.sh` and the ambiguous-redirect rows; add
an xtrace row for a quoted glob operand.

### 5. Pattern compilation has no chokepoint

Twelve `glob::Pattern::new` sites in `huck-engine`. bash's rule — an unmatched
`[` in a pattern is an ordinary character — is applied in one of five consumers,
and the other four have grown *deliberate* handling around the wrong behaviour
(`case` swallows the error with `unwrap_or(false)`; `[[ == ]]` turns it into a
diagnostic; four `param_expansion.rs` sites branch on `is_err()`; completion has
its own arm).

- #717 — the five consumers × unmatched `[` table.
- #303 — completion `-X` filter does not route collating symbols / equivalence
  classes / POSIX classes: the same "translate before compile" seam.
- #589 — the internal escape that leaks into xtrace is produced on this seam.

**Corrected by the round (2026-09-21):** the chokepoint is the MATCHER, not
the compile — four copies of the same engine dispatch (own matcher for
extglob/classes, `glob` crate otherwise) lived in `case`, `[[`, `${…}` and
completion, and completion's copy skipped the class routing (#303). One
`glob_match::pattern_matches` now; bash's bracket rules ported from
`sm_loop.c` (an unmatched `[` is literal; a dangling range `[a-` is NO
match; PATSCAN decides a group before brackets, so `@([x)` is literal) and
`gm_loop.c` MATCHLEN (`${v/…}` tries only a fixed length when the pattern has
no `*` outside a bracket). The pattern text is bash's `\c` form for quoted
spans (#589 — `set -x` prints `\a\*`), translated for the `glob` crate at
the chokepoint. **Gate:** `pattern_chokepoint_diff_check.sh`, 51 rows.

### 2A. A "declared but unset" variable state

`VarValue` is `Scalar | Indexed | Associative` (`shell_state.rs:38`) with no
null variant, so a bare `declare -a x`, `readonly FOO` or `local v` materialises
an empty value.

- #600 — nine measured rows (`declare -p`, `@A`, `[[ -v ]]`, `set -u`, the
  export snapshot, `local v` creating nothing).
- #225 — `mark_readonly` on an unset name creates it set-to-empty (restricted
  mode reports `ENV`/`BASH_ENV` as set).
- #33 — `declare -p` prints `NAME=""` for every attribute-only declaration.
- #691 — bare `local V` over an exported outer drops the export attribute
  ("gated on #600" in its own body).

**Blast radius (from #600):** `scalar_view`, the `${…}` expansion arms, `-v`,
nounset, `declare -p`/`@A` rendering, the export snapshot, `local`'s declaration
path. This is the first of two `VarValue` passes; the second is hub 8.

### 2B. One declaration policy table

`declare` / `local` / `readonly` / `export` with `-a` / `-A` / `-i` / `+i` on an
existing variable is a single table — *(builtin, flag, value supplied?, existing
shape, readonly?) → convert / select / refuse* — that huck implements as
scattered guards, each right for some cells:

- #347 — `readonly -a` on an associative silently replaces (missing guard).
- #697 — `declare -A` on a scalar errors; bash promotes to `([0]="value")`
  (a huck-invented diagnostic; three unit tests and two harness rows pin it).
- #698 — `readonly -A` / `export -a` with no value are SELECTORS in bash, not
  conversions; adding #347's guard naively breaks this row.
- #734 — an attribute change on a readonly is refused; bash protects only the
  VALUE (one cell left behind `Shell::assign`'s check, load-bearing for
  restricted mode).
- #65 — `export`/`readonly` persist the WHOLE inline-assignment prefix; bash
  persists only the names the builtin actually names.

Depends on 2A: "a declared-but-unset name has nothing to promote" (#697).

### 3. The piped-stdin reader pre-processes text above the lexer

Four divergences exist ONLY on the piped-stdin driver (file and `-c` are right):

- #701 — the reader joins `\`+newline before the lexer sees it.
- #81 — a heredoc body line ending in `\` is treated as a continuation.
- #21 — history expansion runs non-interactively.
- #79 — line numbers are always `line 1`.

One fix: the piped reader feeds the same incremental lexer the file driver uses
instead of classifying physical lines itself (`continuation::classify`).
See the memory note on the three top-level drivers before touching this.

- #575 — a command BEFORE a syntax error on the same line RUNS in huck
  (`rm -rf x;;` deletes). bash parses the whole line first. Driver-wide, and
  the most dangerous open divergence in the tracker; it belongs to the same
  "what is the unit the driver hands to the parser" question.

### 6. The front-end does expansion-phase work

Three sub-hubs with the same smell, in ascending risk:

**6a. Brace expansion at lex time** (`lexer.rs:7694` calls
`brace_expand::expand`). #24 (`set +B` cannot retroact within one lex batch),
#44 (`$v{1,2}` expands `$v` first; a scalar RHS is brace-expanded), #387 (a
65536-element cap raised as a SYNTAX error). Moving it to the expander closes
all three and the cap becomes an allocation question, as in bash.

**6b. Tilde recognition in the lexer.** #295 is the refactor (delete
`TokenKind::Tilde`, `assign_val_tilde_ok` and the assignment-value state
machine; recognise prefixes in the expander with quoting available); #72
(`~user:` in a command word) is the bug it closes. Both shrink `lexer.rs`
(#106).

**6c. `${…}` and backtick bodies parsed at parse time** (#493). The
discriminator is whether the command BEFORE the error runs. Cells: #576 (a
backtick body error aborts where bash yields empty and continues), #574 (`$( )`
body errors raise huck-specific shapes), #650 (nested `${` is "unsupported
expansion", not a bad substitution), #608 (`${#?:-D}`), and the terminator half
of #640. The `${…}` scanner debts were consciously deferred as a *refactor*
(see the param-expansion memory); #493 reframes the work as moving the ERROR to
expansion time, not rewriting the scanners. Still the riskiest of the three —
do 6a and 6b first.

### 7. The flat `Sequence` AST

#5 keeps `list → and_or → pipeline → command` as executor-side grouping and
names `time` on a group as the thing it cannot express. #756 (sev:medium — huck
runs GNU `/usr/bin/time`; report format, `-p`, `TIMEFORMAT`, the failure cases)
needs `time` as a pipeline prefix. #268 and #263 are DEBUG-trap decisions that
cannot apply because a pipeline stage is a forked leaf rather than a node the
parent owns.

### 8. Byte-transparent values (second `VarValue` pass)

`VarValue::Scalar(String)` cannot carry a non-UTF-8 byte; 37 lossy-conversion
sites. #738 (a child's environment is CORRUPTED on the way through), #52
(`$'\xHH'` yields a codepoint), #63 (C1 controls). `exported_env` is the
chokepoint for the child half. Scheduled after 2A so the null state's read-site
audit is not redone mid-flight — accepted cost: the arms get a second audit.

## Working the list

- Each round takes its issues from the table above (`Closes #N` per PR), files
  neighbours it turns up as their own `divergence` issues, and ticks the Status
  column here in the same PR.
- A hub whose measurement shows it is smaller than its row claims drops to a
  bug-fix round (`CLAUDE.md`, "Abort the ceremony when the design turns out
  small"); one that turns out larger stops and hands back.
- One blog entry per hub, not per PR.
