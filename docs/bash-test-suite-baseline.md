# bash 5.2.21 test-suite baseline

bash source: 5.2.21 (GNU, GPLv3+; not vendored, run from `$BASH_SOURCE_DIR`).
huck commit: dfe1c78 (v313: readonly-assignment error discards the current command #31).
**Updated by v342 (#32/#321, 2026-07-29 UTC):** `casemod` flipped to PASS (0-diff)
— two roots: (1) **L-44** associative-array iteration order (#32) — bash iterates
assoc arrays in hash-table order, not insertion order. Reverse-engineered and
validated bash 5.2.21's exact order (FNV-1 hash, **1024** buckets, bucket-ascending
+ within-bucket newest-inserted-first; 400 randomized cross-checks, 0 mismatches),
implemented as a pure iteration-order VIEW (`assoc_order.rs`) over huck's
insertion-ordered `Vec<(String,String)>` storage, routed at every assoc
enumeration site (`${m[@]}`/`${!m[@]}`/transforms/slice via the `expand_assoc_param`
snapshot; `declare -p`/`@A`/`@K`/`@k` via `render_declare_value_part`). (2)
**`declare -c`** (#321) capitalize-first attribute + a pre-existing `case_modify`
fix (`${v^pat}`/`${v,pat}` tested only the first char, not a forward scan). Summary
PASS 30→31, FAIL 52→51. Only `casemod` flipped; L-44 also shrank `assoc` (499→358)
and `appendop` (21→19); NO regressions (all PASS categories held vs an origin/main
baseline). Follow-ups: #322 (assoc `${a[@]:o:l}` slice offset), #323 (assoc-cluster
residuals blocking `assoc`/`appendop`/`quotearray`).
**Updated by v341 (#44/#318, 2026-07-28 UTC):** `braces` flipped to PASS (0-diff)
— four brace-expansion roots (all in `brace_expand.rs` + the lexer reconstruction):
(1) negative step was rejected — bash ignores the step SIGN (magnitude only,
direction from endpoints), so `{10..1..-2}` → `10 8 6 4 2`; (2) a nested outer
brace with no top-level comma (`a-{b{d,e}}-c`) and an UNMATCHED outer brace
(`a-{bdef-{g,i}-c`, `{a{b,c}`) left the inner balanced brace unexpanded — bash
expands it (`expand_into` now recurses the body/remainder); (3) a char range
emits an EMPTY element for `\` (0x5C), matching bash's `{A..a}` quirk (needed a
sentinel so the empty survives as a field while `{a,,b}`'s empty still vanishes);
(4) bare `$var{x,y}` merges the brace suffix's leading name-continuation run into
the variable name (`$varx $vary` → `vx vy`), while braced `${var}{x,y}` does not
— required a new `braced` flag on `WordPart::Var` because the parser demotes
modifier-less `${var}` to a plain `Var`. Summary PASS 29→30, FAIL 53→52. Only
`braces` flipped; no regressions (all big expansion categories byte-identical vs
an origin/main baseline). Follow-ups: #44 stays open for the broader
brace-before-param ordering; a pre-existing `declare -f`/`type` divergence
(`$var` printed for a plain braced ref instead of `${var}`) was surfaced (the new
`braced` flag could fix it later).
**Updated by v340 (#314, 2026-07-28 UTC):** `nquote2` + `nquote3` BOTH flipped to
PASS (0-diff) — a double flip. Single shared root: positional `${@<op>}` /
`${*<op>}` per-element transforms (pattern removal `#`/`##`/`%`/`%%`,
substitution `/`/`//`, case, `@Q`) applied the operator to only the FIRST
positional parameter, and the quoted `"${@<op>}"` form joined all params into
one word — while the array form `${arr[@]<op>}` already worked. Fixed by routing
`$@`/`$*`+per-element-transform through the array's per-element path
(`expand_positional_transform` in `expand.rs`: map `scalar_apply_per_element`
over `positional_args`, returning `WordList` for quoted `@` and `Value(IFS-join)`
otherwise — mirroring `expand_array_param`). The Ctrl-A (`$'\001'`) bytes in the
tests were incidental data, not the cause (reproduces on plain `set aXa bXb cXc;
"${@/X/-}"`). Summary PASS 27→29, FAIL 55→53. Only these two flipped; no
regressions (`dollars` diff even shrank 251→246 from the same fix). The
`arith-for` category was investigated first this iteration and abandoned — its
residual is a non-behavioral `$0` program-name artifact in `-c` error output
(`huck:` vs `bash:`), not fixable by huck code (see the shelved
`2026-07-28-arith-for-category-design.md` spec / #64 / #313). Follow-ups: #315
(same bug in the assignment-RHS/no-split dispatch), #26 (the `${@:-word}`
default-op half of L-88, still deferred).
**Updated by v339 (#310, 2026-07-27 UTC):** `set-x` flipped to PASS (0-diff) —
fixed three `set -x` xtrace divergences: (1) each `for (( … ))` header section's
TRAILING whitespace is now preserved (`trim_section` trims leading only), so the
trace reads `(( i++  ))` and `declare -f` reconstructs `for ((…; i++ ))`, both
matching bash; (2) `BASH_XTRACEFD` is honored (`xtrace_target_fd` resolves the
target fd at emit time; trace goes to that fd, reverting to stderr on unset);
(3) a standalone assignment now traces its real operator + the RHS this
statement assigned (`foo+=two`, not `foo=onetwo`) — the scalar RHS is threaded
out of `apply_one_assignment` via a transient `Shell` field mirroring
`last_cmd_sub_status`. Summary PASS 26→27, FAIL 56→55. Only `set-x` flipped
(verified against a main-baseline build: `set-x` FAIL→PASS, `parser` PASS→PASS —
its FAIL row below was already stale); no regressions across the full runner.
The `## Summary` count block below is refreshed to the authoritative full-runner
numbers (it had been stale at 19/63 since an old sweep; per-category rows may
still lag — the count is the authoritative signal). Array/associative
assignment-trace (literal-source) remains a deferred divergence: #311.
**Updated by v338 (#306, 2026-07-26 UTC):** `lastpipe` flipped to PASS (0-diff) —
implemented `shopt lastpipe`: when set with job control off and a Terminal sink,
the last pipeline stage runs in the current shell (a `PipelineStage::InProcess`
variant; run before reaping the forked stages to avoid deadlock; PIPESTATUS +
control-flow outcomes integrated), so its assignments persist. Summary PASS 25→26,
FAIL 57→56. Only `lastpipe` flipped; no regressions. Follow-ups: #307
(capture-context `$()` lastpipe), #308 (last-stage-own-redirect-failure PIPESTATUS).
**Updated by v337 (#302, 2026-07-25 UTC):** `posixpat` flipped to PASS (0-diff) —
implemented POSIX.2 collating symbols `[[.name.]]` and equivalence classes
`[[=x=]]` in the bracket matcher (`glob_match.rs`: name→char table + `collsym`,
`parse_class` atom-based ranges, `has_collating_symbol`/`has_equivalence_class`
routing at 4 sites) plus the `[[:ascii:]]` class. Summary PASS 24→25, FAIL 58→57.
Only `posixpat` flipped; `glob-test` (212→190) and `minimal` (1268→1247) shrank
(no regression). Follow-up: completion-filter routing site (`completion_spec.rs`)
still unwired for collating/equivalence patterns.
**Updated by v335 (#294, 2026-07-24 UTC):** `tilde2` flipped to PASS (0-diff) —
two lexer-side tilde-recognition roots: `~` after an embedded `=` in an
assignment value is literal (Root A), and a word-start `~` in an unquoted
`${x:-word}`/`${x:=word}` value operand expands (Root B). Summary PASS 23→24,
FAIL 59→58. Only `tilde2` flipped; `more-exp` (111→109) and `minimal` (1270→1268)
shrank (no regression). Follow-ups: #295 (recognition→expander refactor), #296
(pattern-operand tilde `${x#~}`).
**Updated by v334 (#291, 2026-07-24 UTC):** `array2` flipped to PASS (0-diff).
Summary PASS 22→23, FAIL 60→59.
**Updated by v333 (#289, 2026-07-24 UTC):** `nquote` flipped to PASS (0-diff).
Summary PASS 21→22, FAIL 61→60.
**Updated by v332 (#286, 2026-07-24 UTC):** `dynvar` flipped to PASS (0-diff).
Summary PASS 20→21, FAIL 62→61.
**Updated by v331 (#27/#283, 2026-07-23 UTC):** `parser` flipped to PASS
(0-diff). Summary PASS 19→20, FAIL 63→62.
**Updated by v322 (#255, 2026-07-22 UTC):** `dbg-support2` flipped to PASS (0-diff)
— the DEBUG trap now fires before bare assignments, the action's `$LINENO` equals
the pending command's line (without leaking into a function the action calls), and
under `shopt -s extdebug` a non-zero DEBUG-action status skips the pending command
(status 2 inside a function/sourced script simulates `return 2`)
(`tests/scripts/debug_trap_extdebug_diff_check.sh`, 10/10). Summary PASS 18→19,
FAIL 64→63. Only `dbg-support2` changed.

**Updated by v321 (#253, 2026-07-22 UTC):** `rhs-exp` flipped to PASS (0-diff) —
inside a nested `"…"` span of a value-family parameter-expansion word, huck now
drops a backslash before a non-special char when the enclosing `${…}` is
double-quoted (`\p`→`p`), matching bash 5.2.21 (`tests/scripts/rhs_exp_nested_quote_diff_check.sh`,
10/10). Summary PASS 17→18, FAIL 65→64. Only `rhs-exp` changed.

**Updated by v318 (#218, 2026-07-20 UTC):** `procsub` flipped to PASS (0-diff) —
process substitution now sets `$!` (waitable via the saved-status ring) and
`f=<(…)` assignment parses/expands correctly. Summary PASS 16→17, FAIL 66→65.
Only `procsub` changed.

**Updated by v315 (#209, 2026-07-20 UTC):** the `eval:` context marker +
eval line base flipped `posix2` to PASS — see the "v315 targeted
re-sweep" paragraph below and the Summary block (PASS 15→16, FAIL 67→66).
The v313/v314 full/targeted-sweep narrative below is left as the
historical record of those sweeps and is otherwise unchanged by v315
(confirmed via the harness-level `syntax_error_diag_diff_check.sh` and
the full `run_diff_checks.sh` sweep — no other category's output moved).
Sweep date: 2026-07-19 UTC (v313 full re-sweep — verifies the v299–v313 arc: the job-control batch (v299–v306), the heredoc-in-process / builtin write-error work (v307–v308), the huck-engine fork-guard (v309), in-process group stderr under `2>&1` in `$()` (v310), and the three error-fatality-funnel fixes — negated-pipeline errexit (v311/#1), arithmetic-expansion discard (v312/#3), readonly-assignment discard (v313/#31)). **NO change and NO regression: PASS holds at 15/82, TIMEOUT 0, ERROR 0 — the same 15 categories pass, none regressed, no new hangs.** Why no category flipped despite real fixes: each failing category is gated by SEVERAL independent divergences, and the arc fixed narrow ones that shrink diffs below the flip threshold. Case in point — v313 resolved L-43 (readonly-assignment abort), the note previously cited as `case`'s SOLE blocker, but the current `case` diff exposes two more live divergences (see its row), so it stays FAIL. **The 67 FAIL notes below have drifted since v268/v298 — treat them as approximate; the count is the authoritative signal.** Near-miss ranking (smallest current diffs, closest to PASS): `posix2` (5 lines — error-diagnostic format on `case esac in esac)`, [#209](https://github.com/jdstanhope/huck/issues/209)), `procsub` (9), `nquote` (12). Prior sweep provenance: 2026-07-15 UTC (v298 re-sweep, PASS 10→15: +getopts, input-test, iquote, nquote1, tilde; TIMEOUT 4→2 then 2→0 via v299 harness correction); 2026-07-07 UTC (v268 full re-sweep); 2026-06-25 UTC (v218 full sweep with recho/zecho/printenv helpers; v219 cprint+herestr flip; v220 herestr; v225 func).

**v314 targeted re-sweep** (2026-07-19 UTC, syntax-error 3-shape alignment
#211 — top-level `ParseError`/`LexError` diagnostics now render as one of
bash's three shapes: near-token (`syntax error near unexpected token
`X'`), unexpected-EOF (`syntax error: unexpected end of file`), or
unterminated-quote/delimiter (`unexpected EOF while looking for matching
`X'`); `tests/scripts/syntax_error_diag_diff_check.sh` is 27/27
byte-identical incl. exit code, and the full sweep is 209/209).
Re-swept only the categories most likely to move
(`parser`, `errors`, `comsub`, `comsub-posix`, `array`, `posix2`) rather
than the full 82 — **no category flipped**, matching the pattern the
v299–v313 arc already established (a shrunk diff below the flip
threshold, not a flip, unless a category's diff was ENTIRELY the fixed
class). Per-category evidence:
- `parser`: diff shrank to 13 lines. The `for`/`case`-in-`for` fragments'
  wording now matches bash's Shape 1/2 text; the residual is (a) an
  unrelated `not a valid identifier` diagnostic that wrongly carries a
  `line N:` prefix (not a top-level syntax error, so out of `render_syntax_diag`'s
  scope) and (b) a line-alignment artifact from an earlier divergence in
  the same fixture file.
- `posix2`: the `case esac in esac) ...` fragment's MESSAGE TEXT and
  echoed source line now match bash exactly (`syntax error near
  unexpected token `)'` / the quoted line) — the sole remaining diff was
  the diagnostic prefix: bash prints `./posix2.tests: eval: line 199:`
  (the `eval:` marker plus the outer calling script's physical line
  number), while huck printed `./posix2.tests: line 1:` (no `eval:`
  marker; the error occurs inside an `eval`, and huck's prefix was
  numbering from the eval-body's own internal line count instead of the
  calling script's physical line). This was exactly the gap
  [#209](https://github.com/jdstanhope/huck/issues/209) attributed to
  v315's planned `eval:` marker — confirms the diagnosis. **(v315/#209
  RESOLVED this — posix2 now 0-diff PASS; see the v315 note below.)**
- `comsub` / `comsub-posix`: diffs (25 / 41 lines) are dominated by
  unrelated pre-existing gaps — a structural over-acceptance inside a
  nested `case`/`$(` fixture, `unsupported expansion` fallback wording on
  an unimplemented expansion form, and a `$THIS_SH`-path test-fixture
  mismatch — none are 3-shape wording. Not moved by v314 (expected).
- `errors`: diff (230 lines) is still dominated by unimplemented
  `alias -x`/`unalias -x`/`readonly -f`/`declare +r` option surface, as
  the existing row's note already describes; unaffected by v314.
- `array`: diff (793 lines) unaffected — the existing row's note (missing
  `set +a`, array-literal-with-`&` parsing) already covers its causes,
  none of which are top-level syntax-error wording.

**Side effect resolved:** `tests/scripts/cmdsub_comment_diff_check.sh`
(a comment-only `$()` body at EOF, previously a known pre-existing gap —
see MEMORY.md's `huck-cmdsub-comment-only-body-eof` entry, 1/8 PASS) is
now 8/8 — the old `MissingCommand`-fallback wording for that case is
superseded by the aligned Shape 3 (`unexpected EOF while looking for
matching`) rendering, which happens to match bash's actual behavior for
this fragment too.

**v315 targeted re-sweep** (2026-07-20 UTC, `eval:` nested-context marker
+ `$LINENO`/error-line base #209 — a syntax error raised while parsing an
`eval` string now prints an `eval: line N:` marker with the OUTER
(calling) line number instead of leaking the eval-body's internal line
count, and `$LINENO` reads correctly inside `eval` too, both via a
shell-global `eval_frame`/`line_base()` that `render_syntax_diag`'s
`Diag::Syntax` arm consults). Re-ran `posix2` in isolation (single
category, `HUCK_BASH_TEST_CATEGORY=posix2`): the diagnostic-prefix diff
v314 confirmed as the sole remaining line — huck's `line 1:` (no `eval:`
marker) vs bash's `eval: line 199:` — is gone; **`posix2` is now a
byte-identical 0-diff PASS**, closing out the near-miss #209 opened (the
"other pre-existing POSIX compliance failures" the earlier row speculated about did not
materialize as separate diff lines once the prefix was fixed). This is
the one category the v315 change was expected to move; the harness-level
guard (`syntax_error_diag_diff_check.sh`, 27/27) and the full 82-category
sweep confirm no other category's output changed (the whole behavioral
change is `line_base()` being non-zero only inside `eval`, which is a
no-op everywhere else).

**v316 provenance note** (#213, 2026-07-20 UTC): closed the backtick
command-substitution-body syntax-error gap — a syntax error inside a
`` `...` `` body now prints bash's `command substitution:` marker
(byte-identical on stderr; `$()` bodies are unaffected). No category
flip — `posix2` was already PASS (v315/#209); this is a harness-level
alignment (`tests/scripts/comsub_marker_diag_diff_check.sh`, 8/8) with
no effect on the 82-category sweep. The pre-existing stdout/rc
divergence (bash recovers with an empty substitution and continues,
rc 0; huck aborts the `-c` string, rc 2) is out of scope for #213 and
tracked as a follow-on, [#215](https://github.com/jdstanhope/huck/issues/215).

Front-end-rearchitecture check (v266–v268): NO regression. The parser-driven front-end (oracle deletion, `${…}`/subscript/assignment paths) cost zero bash-suite compatibility — every previously-passing category still passes, no new TIMEOUTs, and the array/subscript/assignment categories (`array`, `array2`, `assoc`, `appendop`, `tilde`, `posixpat`) stay FAIL for the same pre-existing non-front-end reasons recorded below.

## Summary

- Categories run: 82
- PASS: 31
- FAIL: 51
- TIMEOUT: 0
- ERROR: 0
- SKIP (from known-skips.txt): 4

(Counts refreshed by the v342 full-runner sweep, 2026-07-29 UTC — authoritative.
The 31 PASS categories are: array2, braces, casemod, cprint, dbg-support2, dynvar,
extglob2, extglob3, func, getopts, herestr, ifs, input-test, invert, iquote,
lastpipe, nquote, nquote1, nquote2, nquote3, nquote5, parser, posix2, posixpat,
precedence, procsub, rhs-exp, set-x, strip, tilde, tilde2. Some per-category FAIL
rows below still lag reality — the count above is the authoritative signal.)

**v299 harness correction:** the two categories previously recorded as TIMEOUT
(`jobs`, `minimal`) were NOT hangs and NOT huck performance bugs — they are
inherently long-running (real `sleep`/`wait` in `jobs.tests`, ~62s in bash
itself; deliberate `read -t` timeout tests in `minimal`/`read.tests`, ~17s in
both shells; huck is as-fast-or-faster than bash on both). They exceeded only
the harness's 30s default cap. `runner.sh` now gives such categories
(`LONG_CATEGORIES`) a 180s cap, so they report their true FAIL status. No
remaining TIMEOUTs; a TIMEOUT anywhere now signals a genuine hang/regression.

## Per-category status

| Category | Status | Note |
|---|---|---|
| alias | FAIL | Error-message format divergence — huck uses its own name as the command-not-found prefix rather than the running script's filename; also some alias-expansion differences in non-interactive script mode. |
| appendop | FAIL | L-43 (readonly-assignment abort) RESOLVED by v313 (#31). Remaining: an array-element append subscript form that huck fails to parse; assoc-array iteration-order divergence (L-44). |
| arith | FAIL | The `set -o posix` cascade was resolved in v215 (test now runs end-to-end). v216 aligns arith error-message format with bash: the source-file + line-number prologue, leading-trimmed expression echo, and `(error token is "...")` suffix now match byte-for-byte for the both-error cases verified by `arith_error_diff_check.sh` (10/10 PASS). Remaining failures are the behavioral divergences catalogued in L-56: signed-integer overflow wrapping (literals ≥ 2^63 wrap to min-int in bash; huck rejects as out-of-range); `++`/`--` applied to non-lvalue literals (bash treats as repeated unary `+`/`-` and yields the number; huck errors); lazy dead-branch evaluation in ternary expressions (dead branch must not be evaluated even if it contains an unset variable); array-element lvalue expressions inside arith (`a[n]=n++`); substring offset/length with arith ternary colons; standalone `(( ))` command line-number attribution (off vs bash because `Command::Arith` carries no source line); and minor error-kind wording for malformed base-N numbers. |
| arith-for | FAIL | The `declare -f` trailing-space format divergence is resolved by v218. Remaining divergences: huck leaves empty `for ((` sections empty (`for ((; i<3; i++))`), whereas bash normalizes a missing section to `1` (`for ((1; i<3; i++))`) — an arith-for reconstruction-fidelity gap (L-59); and error-message wording for malformed `for ((` headers (wrong section count or a quoted string as a section value) still differs between huck and bash. |
| array | FAIL | `set +a` (all-export off) not supported, misconfiguring the test environment. Also an array literal whose element contains a background `&` operator is parsed differently than bash expects. |
| array2 | FAIL | With helpers provisioned, the real divergence is in how certain array subscript/expansion forms pass word counts to commands: huck collapses some `${a[@]}`-style expansions into fewer arguments than bash produces, treating them more like `${a[*]}` in specific subscript contexts. |
| assoc | FAIL | `BASH_ALIASES` and `BASH_CMDS` built-in assoc arrays are not present in huck. Also L-46 (bare attribute-only `declare -A` prints an empty-string assignment in `declare -p`) and L-44 (assoc-array iteration order). |
| attr | FAIL | `readonly -a` (array readonly flag) not recognized — huck rejects the `-a` option. Error-message prefix format differs throughout. New bug. |
| braces | PASS | v341 (#44/#318): 0-diff PASS. Four roots fixed — negative step (sign ignored, `{10..1..-2}`→`10 8 6 4 2`); nested/unmatched outer brace still expands a balanced inner one (`a-{b{d,e}}-c`, `a-{bdef-{g,i}-c`); char range emits an empty element for `\`; and bare `$var{x,y}`→`$varx $vary` name-merge (new `braced` flag on `WordPart::Var`, since modifier-less `${var}` demotes to `Var`). #44 stays open for the broader brace-before-param ordering. |
| builtins | FAIL | Multiple unimplemented `set -o` options (`posix`, `+p`) abort the test preamble. `ulimit` and `fc` are not found as commands. |
| case | FAIL | L-43 (readonly-assignment abort) RESOLVED by v313 (#31) — a standalone readonly assignment now discards the current command, so the old cascade is gone. Two divergences remain (v313 re-sweep): (1) control-character case-PATTERN matching — patterns built from control bytes (soh/stx/del) match differently, yielding `ok1ok2ok3ok4ok5` where bash produces `fail1fail2fail3ok4fail5`; (2) arithmetic assignment to a readonly inside `(( ))` — `((xx++))` on a readonly emits one error + computes `1.1`, where bash emits a second `xx++: … (error token is "")` diagnostic and computes `1.0` (an arith-lvalue-on-readonly path, distinct from the run_assignment_list fix). |
| casemod | PASS | v342 (#32/#321): 0-diff PASS. Two roots — L-44 associative-array hash iteration order (now a bash-faithful view: FNV-1, 1024 buckets, bucket-asc + newest-first), and `declare -c` (capitalize-first attribute) + a pre-existing `case_modify` `${v^pat}` first-char-only fix. |
| complete | FAIL | M-92 (`${!prefix@}` variable-name-listing expansion) not implemented — `complete.tests` uses this inside a `[[ ]]` expression, causing an unterminated-compound-test parse error that prevents the entire suite from running. |
| comsub | FAIL | Error-message format divergence (huck uses its own name as prefix, not the script-file-and-line form). Unterminated heredoc inside a command substitution is treated as a hard error that aborts the substitution, losing many expected output lines. Huck also fails to parse several complex nested-comsub forms (command substitutions containing `esac` tokens, bare-word `case` clauses, and `nest`/`DO`/`DONE` patterns) that bash handles by treating `)` as the comsub terminator. |
| comsub-eof | FAIL | Unterminated heredoc inside a command substitution is treated as a hard error in huck (aborts the substitution) while bash issues a warning and treats the EOF as the delimiter. New divergence in error-vs-warning handling. |
| comsub-posix | FAIL | Unterminated heredoc inside a command substitution causes a hard error in huck, losing many expected output lines across multiple sub-tests. Additionally, huck rejects several command-substitution forms where POSIX allows a bare `)` to end the substitution (e.g., heredoc-terminated-by-parenthesis patterns and substitutions containing `if/then` fragments without a closing `fi`). |
| cond | FAIL | M-94 (`${!@}` / `${!*}` as indirect expansion of the positional list) causes a parse error, aborting the test early. Additional `[[ ]]` compound-test edge case with an incomplete expression. |
| coproc | FAIL | Coproc pipe file-descriptor numbers diverge — huck allocates low-numbered fds while bash allocates high-numbered fds. Also `<&N-` / `>&N-` dup-and-close fd redirect operator not supported (same root cause as the redir TIMEOUT). M-126 and a new dup-close gap. |
| cprint | PASS | v218 resolved the `declare -f` trailing-space format; v219's `WordPart::Quoted` quote-provenance fix (L-57) resolves the remaining reconstruction divergences — `echo`-argument quoting and adjacent-double-quoted-substring reconstruction now match bash byte-for-byte. 0-diff PASS (verified via the runner 2026-06-25). |
| dbg-support | FAIL | `set -o functrace` (DEBUG/RETURN/ERR trap inheritance through function calls) not yet supported. Entire debug-trap test suite fails from the first rejected option. |
| dbg-support2 | PASS | v322 (#255): DEBUG trap fires before bare assignments; the action's `$LINENO` tracks the pending command's line without leaking into a function the action calls; extdebug non-zero DEBUG-action status skips the pending command (status 2 in a function/sourced script simulates `return 2`). 0-diff PASS. |
| dirstack | FAIL | `pushd -m` / `popd -m` / `dirs -m` argument is treated as an invalid option rather than a numeric argument (huck and bash differ on which flags these commands accept). Error-message prefix and format differences throughout. |
| dollars | FAIL | No longer TIMEOUTs (the v220-recorded hang — a blocking read/process-wait around `${!*}`/`${!@}` indirect expansion — is resolved): the category now runs to completion with output divergences across the `$@`/`$*`/`${!*}` dollar-special tests (error-message wording and expansion-count differences). |
| dynvar | FAIL | `BASH_ARGV0` is not updated to reflect the running script's `$0` — tests that check `BASH_ARGV0` report a mismatch. `EPOCHREALTIME` not implemented (L-41 computed-dynamics gap). |
| errors | FAIL | Multiple `set -o <option>: not yet supported` rejections misconfigure the test environment (posix, allexport, etc.). Also `alias -x` / `unalias -x` flags not recognized. Cascading from missing set options. |
| execscript | FAIL | Error-message format differences — huck uses its own name as prefix rather than the script-file-and-line-number form bash uses. Executing a binary file produces a UTF-8 decoding error instead of bash's "cannot execute binary file" message. |
| exp-tests | FAIL | Several real divergences now visible: `$'...'` strings containing control characters are displayed in `$'...'` escape notation by huck rather than as raw bytes; certain `${a[@]}` expansions collapse to fewer arguments than bash produces; `${!}` and similar empty-name parameter-expansion forms cause a huck syntax error while bash returns a value; high-byte characters in variable keys and values are formatted differently (huck uses a plain-string representation while bash uses `$'...'` notation in `declare -p` output); and word-splitting with non-standard IFS diverges for some adjacent-field cases. |
| exportfunc | FAIL | M-09a (relaxed function-name characters) — function names containing hyphens (e.g., `foo-a`) are rejected by huck's identifier parser. Additional divergences in heredoc-count limits and export-flag error handling. |
| extglob | FAIL | A subset of extglob patterns involving backslash-escaped metacharacters inside extglob brackets diverge from bash (e.g., some `!([*)*`-class patterns are not correctly rejected). A temp-directory permission or working-directory issue also causes certain filesystem-based extglob tests to produce wrong results. Core extglob matching is mostly correct; edge cases remain. |
| extglob2 | PASS | |
| extglob3 | PASS | |
| func | PASS | All blockers cleared across v221–v225: v221 prefix-assignment leak; v222 redirected-brace-body + nested-`function`-keyword reconstruction; v223 `declare -xF` export filter/format + FUNCNAME write protection; v224 FUNCNEST enforcement + recursion backstop (func4.sub byte-identical); v225 posix-gated special-builtin prefix-assignment persistence + the inline_scopes enclosing-restore-survival fix (func3.sub line 155). 0-diff PASS (verified via the runner 2026-06-26). |
| getopts | PASS | v298 re-sweep: 0-diff PASS (the L-26 usage-message-format blocker recorded at v268 is resolved). |
| glob-test | FAIL | A missing locale warning appears in huck's output but not bash's (locale check position differs). Multibyte character handling diverges: a Unicode character is rendered differently (huck produces different byte sequences vs bash). Globbing correctness diverges for some patterns — cases that should fail to match succeed in huck, and vice versa. Backslash-escaped glob metacharacters passed as arguments are handled differently. Glob results omit the `./` prefix that bash includes when the pattern starts with `./`. L-04/L-11 (character vs byte in multibyte globbing) class divergence remains. |
| globstar | FAIL | Test environment mismatch — `globstar.tests` expects to run from the bash build directory (where compiled object files are present to glob over); huck runs it from the tests directory, where those files do not exist. Also M-53 (bare `**` globstar matches directories only, not files). |
| heredoc | FAIL | Several heredoc edge cases: a `$PS4` literal appears in huck's heredoc output where bash expects an expanded (or empty) value; fd-based heredoc reads via an `exec`-opened descriptor generate bad-fd errors; and an unterminated heredoc inside a complex script aborts where bash would continue. |
| herestr | PASS | v219's `WordPart::Quoted` quote-provenance fix removed the reconstruction hunks (adjacent double-quoted here-string operands and double-quoted-vs-single-quoted function-body lines now match bash); v220 task 1 resolved the last runner residual — `declare -p` of an indexed array whose element holds an embedded control byte now renders the value in bash's ANSI-C `$'i\n'` escape form. 0-diff PASS (verified via the runner 2026-06-25). A separate empty-leading-word `command not found:` bug (L-57; an empty-expanded command name, e.g. `${THIS_SH} ./herestr1.sub` with `THIS_SH` unset) surfaces only on a direct invocation and is masked under the runner, which exports `THIS_SH=$HUCK`. |
| histexpand | FAIL | `set: history: not yet supported`, and history-expansion flags (`-p`, `-a`, `-s`, `-w`) not implemented (M-46). Entire test suite fails from the first rejected option. |
| history | FAIL | M-46 (`history -d/-w/-r/-a` not supported), M-47 (`history N` numeric argument not supported), `fc` not found as a command, `set: history: not yet supported`. Multiple history-command gaps. |
| ifs | PASS | Flipped FAIL→PASS since the v220 sweep: the v220-recorded divergence (joining `${a[*]}`/`$*` with a space instead of the first `IFS` character when `IFS` is non-whitespace) is resolved; the category is now byte-identical. |
| ifs-posix | FAIL | IFS splitting semantics with the `read` builtin diverge when IFS contains both whitespace and non-whitespace characters — huck does not correctly handle certain adjacent mixed-class IFS-separator edge cases. New bug, separate from the unimplemented posix set option. |
| input-test | PASS | v298 re-sweep: 0-diff PASS (the v268 piped-input-to-child-script `read` divergence is resolved). |
| invert | PASS | |
| iquote | PASS | v298 re-sweep: 0-diff PASS (the v268 `$'...'` control-char/high-byte escape-expansion divergence is resolved). |
| jobs | FAIL | v299: NOT a hang. `jobs.tests` is inherently long — its real foreground `sleep`/`wait` budget runs **~62s in bash 5.2.21 itself** vs ~43s in huck (huck is *faster*). It only showed TIMEOUT because the harness default cap was 30s; it is now in `LONG_CATEGORIES` (180s cap) and reports its true status. Now FAILs on job-control output divergences (non-interactive job-control message formats, `%job` notation, `disown`/`bg`/`fg` error wording) — needs triage. |
| lastpipe | PASS | v338 (#306): 0-diff PASS. `shopt lastpipe` implemented — with lastpipe set, job control off, and a Terminal sink, the last pipeline stage runs in the current shell (`PipelineStage::InProcess`, run before the forked-stage reap to avoid pipe deadlock); its assignments persist, `$?`/`PIPESTATUS`/pipefail include it, and control-flow (`exit`/`return`) propagates. Capture-context (`$()`) lastpipe deferred (#307). |
| mapfile | FAIL | L-34 (`mapfile -C` callback and `mapfile -u` fd-argument flags not implemented). Documented deferred gap from v140. |
| minimal | FAIL | v299: NOT a hang. `minimal` is a meta-runner (~25 sub-runners); its time is dominated by `read.tests`' deliberate `read -t` timeout tests (~17s in **both** huck and bash — `read -t` sleeps for its timeout by design, it does NOT block indefinitely), plus func (~5s) and dynvar (~2s), all within ~0.1s of bash. It only showed TIMEOUT because the ~25s+ inherent runtime exceeded the harness's 30s default cap; it is now in `LONG_CATEGORIES` (180s cap). Now FAILs on the aggregate output divergences of its sub-runners — needs triage. |
| more-exp | FAIL | Several remaining divergences: `${a[@]}` in contexts where IFS-splitting interacts with leading-space preservation produces fewer fields than bash; tilde in certain variable assignment contexts is not expanded when it should be (or expands to an unrelated value); a backslash at the end of a word in `"$@"` contexts splits incorrectly; an unterminated command substitution causes an abort where bash would produce output; and word-splitting with embedded bracket characters diverges. |
| nameref | FAIL | L-47 (nameref follow-on gaps). A `declare -p` call on a nameref variable dumps the entire variable table instead of just the named variable — new bug in the nameref plus `declare -p` interaction path. |
| new-exp | FAIL | A parse or expansion error early in the test file (involving `}` as an unexpected token in an arith/expansion context) causes huck to abort the script, losing nearly all expected output. Error-message format also differs (huck says `unexpected character: '}'` while bash says `syntax error: operand expected (error token is "}")`). The `set: posix: not yet supported` issue is gone but the early-abort prevents the remainder from running. |
| nquote | FAIL | Several divergences: `$'\t'` and similar `$'...'` escape sequences produce the literal escape notation rather than the actual character in some contexts; `set: history` and `set: -H` are not supported, causing format divergences; an unterminated `${...}` inside a multi-line quoted string errors in huck while bash produces output; byte-level differences in high-byte character sequences passed through quoting operations; and a helper glue-file source operation fails in huck. |
| nquote1 | PASS | v298 re-sweep: 0-diff PASS (the v268 embedded-Ctrl-A word-count/empty-field divergence is resolved). |
| nquote2 | PASS | v340 (#314): 0-diff PASS. Root was positional `${@/pat/rep}`/`${@//pat/rep}` applying the substitution to only the first positional param (quoted form joined into one word); fixed by mapping the transform over each param via the array per-element path (`expand_positional_transform`). The Ctrl-A bytes were incidental test data, not the cause. |
| nquote3 | PASS | v340 (#314): 0-diff PASS. Same root as nquote2 — positional `${@%pat}`/`${@#pat}`/`${@##pat}` pattern-removal transforms now apply per-param. Flipped by the same one-branch fix (double flip). |
| nquote4 | FAIL | The braced hex-escape form `\x{NN}` inside `$'...'` strings is not implemented in huck: the sequence is passed through literally while bash expands it to the corresponding byte. Unbraced `\xNN` and other escape forms may have separate issues. |
| nquote5 | PASS | |
| parser | FAIL | v314 (#211) shrank the diff to 13 lines: the `for`/`case`-in-`for` syntax-error TEXT now matches bash's near-token/unexpected-EOF shapes byte-for-byte. Remaining: an unrelated `not a valid identifier` diagnostic wrongly carries a `line N:` prefix (not a top-level parse error, outside `render_syntax_diag`'s scope), plus a line-alignment artifact downstream of it. |
| posix2 | PASS | v315 (#209): the `eval:` marker + eval line base resolved the diagnostic-prefix diff v314 (#211) had narrowed this to — huck now prints `eval: line 199:` with the correct outer line number, matching bash exactly. 0-diff PASS. |
| posixexp | FAIL | Multiple real divergences: quoting-aware pattern removal (`${var//pattern}`) strips more content than bash; an unterminated `${...}` form that bash accepts causes a syntax error in huck; `$*` with a non-whitespace IFS joins with a space instead of the IFS character (producing `1 2` where bash produces `12`); IFS-splitting at word boundaries diverges (huck splits where bash keeps tokens joined); and the test-case label printed for IFS diagnostic output shows `(null)` in huck versus the actual IFS value in bash for some edge cases. |
| posixexp2 | FAIL | `set: posix: not yet supported` misconfigures the test environment; also an unterminated `${...}` handling difference when posix mode is presumed active. |
| posixpat | PASS | v337 (#302): 0-diff PASS. Implemented POSIX.2 collating symbols `[[.name.]]` (POSIX.2 name→char table + `collsym`, `parse_class` atom-based ranges) and equivalence classes `[[=x=]]` (C-locale = match the char), wired to `extglob_match` via `has_collating_symbol`/`has_equivalence_class` at 4 match sites, plus the `[[:ascii:]]` glibc class. The char-class half already passed pre-v337. |
| posixpipe | FAIL | `time` builtin output format differs (huck emits the system `time(1)` format while bash uses its own built-in format with `real`/`user`/`sys` labels). Also lastpipe behavior divergence. |
| precedence | PASS | |
| printf | FAIL | Usage-message prefix format (`huck: printf: usage:` vs bare `printf: usage:`). Also some format-specifier differences (string width and `%b` handling). |
| procsub | PASS | v318 (#218): `$!` from a process substitution + `wait "$!"` resolves (saved-status ring), and `f=<(…)` process-substitution assignment now parses/expands (lexer glues `<(…)` onto the assignment value; `expand_assignment` realizes it; drained per-command like bash). 0-diff. |
| quote | FAIL | Backslash quoting edge cases — an escaped space inside a word is treated differently, and a backslash-newline line continuation produces two separate values rather than joining the words. New bugs in backslash-quote-in-word handling. |
| quotearray | FAIL | Assoc-array keys containing escaped special characters (brackets, dollar signs, backslashes) cannot be used as arithmetic subscripts — the arith parser fails on the key content. New bug in special-character key handling in arithmetic array contexts. |
| read | FAIL | v298 re-sweep: no longer TIMEOUT (the v268 hang — the foreground-wait latency / `read -t` block — is resolved; the category now runs to completion). Now FAILs with remaining output divergences (residual `read -t`/`read -u` fd-source edge cases, L-34 class); needs re-triage. |
| redir | FAIL | v298 re-sweep: no longer TIMEOUT (the v268 hang — `<&N-`/`>&N-` dup-and-close leaving fd state inconsistent so a later `read` blocked on the tty — is resolved; move-fd redirects now supported and the category runs to completion). Now FAILs with remaining output divergences; needs re-triage. |
| rhs-exp | PASS | v321 (#253): inside a nested `"…"` span of a value-family parameter-expansion word (e.g. `${v:+a="\p"b}`), a backslash before a non-special char is now DROPPED when the enclosing `${…}` is double-quoted (`\p`→`p`), matching bash — the old divergence (huck retained `\'` where bash produced `'`) is resolved. 0-diff. |
| set-e | FAIL | `set -e` interaction with `&&`/`||` compound lists, `!` negation, and `eval` diverges — some cases where bash would abort the script huck continues (or vice versa). New bug area in `set -e` compound-list abort semantics. |
| set-x | PASS | v339 (#310): 0-diff PASS. Three `set -x` xtrace fixes — (1) `for (( … ))` header sections preserve trailing whitespace (`trim_section` leading-only trim → `(( i++  ))`, and `declare -f` reconstructs `for ((…; i++ ))`); (2) `BASH_XTRACEFD` honored (`xtrace_target_fd` resolves the target fd at emit time; reverts to stderr on unset); (3) standalone assignments trace their real operator + assigned RHS (`foo+=two`, not `foo=onetwo`) via a transient `Shell` field. Array/associative assignment-trace (literal source) deferred: #311. |
| shopt | FAIL | Error-message prefix format difference. Many `set -o <option>: not yet supported` rejections (allexport, braceexpand, hashall, histexpand, keyword, monitor, notify, onecmd, privileged, history, ignoreeof, interactive-comments, posix, emacs, vi). Significant missing set-option surface. |
| strip | PASS | |
| test | FAIL | `test <` and `test >` lexicographic string-comparison operators not supported — huck rejects them with "unexpected argument". Also `/dev/tty` inaccessible in the test runner environment (test infrastructure). |
| tilde | PASS | v298 re-sweep: 0-diff PASS (the v268 `set -o posix` preamble rejection + colon-delimited-assignment tilde divergence are resolved). |
| tilde2 | PASS | v335 (#294): 0-diff PASS. Two lexer-side tilde-recognition roots fixed — Root A: `~` after an embedded `=` in an assignment value is literal (`h=HOME=~`), eligibility re-enables on `:` only; Root B: a word-start `~` in an unquoted `${x:-word}`/`${x:=word}` value operand expands. The posix-`eval` tail was downstream of Root A. Pattern-operand tilde (`${x#~}`) stays literal — separate divergence #296. |
| trap | FAIL | `trap -p` display format divergence — huck prints bare signal names (`HUP`, `INT`, etc.) while bash prints them with the `SIG` prefix (`SIGHUP`, `SIGINT`, etc.). Subshell EXIT trap not firing when expected. Signal-number display differences in job-notification lines. Multiple trap formatting gaps. |
| type | FAIL | Error-message prefix format difference (L-class), `set: posix: not yet supported`, and `declare -f` output format issues cascade into function-display comparisons. |
| varenv | FAIL | `set -k` (keyword mode: treating `key=val` tokens anywhere on a command line as variable assignments) is not supported, causing wrong argument counts in the first several tests. Multiple other `set` options (`ignoreeof`, `monitor`, `-a`, `-m`) are not supported, cascading through later tests. Additional divergences: a `declare` call with an inline array-value token is rejected; function-local variable scoping differs from bash in one case; `SHELLOPTS` content differs; and error-message prefix format differs for some `declare` errors. |
| vredir | FAIL | Variable fd redirection (`exec {varname}>file`) not implemented — huck does not support the `{varname}` syntax that allocates a fresh file descriptor and assigns its number to the named variable. New missing feature. |

## Skipped categories

| Category | Reason |
|---|---|
| loadable | huck has no loadable-builtin support; bash-specific. |
| intl | depends on locale/i18n infrastructure; out of huck's compat scope. |
| strict-posix | depends on POSIX-strict mode huck doesn't implement. |
| rsh | restricted shell (`set -r`) is not implemented and is not a huck feature. |

## How to regenerate

1. `curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp`
2. `export BASH_SOURCE_DIR=/tmp/bash-5.2.21`
3. `bash tests/bash-test-suite/runner.sh > /tmp/sweep.md`
4. Hand-triage non-PASS categories using the per-category diffs printed
   in the runner's header path.
5. Update this document with the new status column and prose Notes.
6. Commit.

## Licensing reminder

This document contains only huck-authored content (category names,
status counts, prose notes). NEVER copy verbatim bash test output or
test-script contents into the Note column — those bytes are GPL'd.
The full per-category diffs live in `/tmp/huck-bash-tests-<timestamp>/`
and stay local.
