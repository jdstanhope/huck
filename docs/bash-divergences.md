# huck vs bash 5.x — Intentional Divergences

**Last updated:** 2026-07-09.

This document lists ONLY the divergences from bash 5.x that huck keeps **on
purpose** — deliberate design choices we do not intend to "fix." Each one is
also tracked as a closed, `by-design`-labelled GitHub issue for the record.

## Where the actionable divergences live

Every *actionable* divergence (bugs and missing bash features that we do intend
to address) now lives in the **GitHub issue tracker**, not in this file:

- **All divergences:** issues labelled [`divergence`](https://github.com/jdstanhope/huck/issues?q=is%3Aissue+label%3Adivergence)
- **Open / actionable:** [`divergence` + open](https://github.com/jdstanhope/huck/issues?q=is%3Aissue+is%3Aopen+label%3Adivergence)
  — filter further by `bug` / `enhancement` and `sev:high` / `sev:medium` / `sev:low`.
- **Kept by design:** [`by-design`](https://github.com/jdstanhope/huck/issues?q=is%3Aissue+label%3Aby-design) (the items below).

**Workflow:** before starting new work, review the open `divergence` issues and
either take an existing one or open a new issue to capture the work. When the
work is done, open a pull request that closes the issue (`Closes #N`) for review
and merge. See `CLAUDE.md` for the full loop.

---

## Intentional divergences (kept by design)

huck behaves differently from bash here on purpose. Each links to its
`by-design` tracking issue.


### `case` requires a separator before `esac`

[Issue #12 · by-design](https://github.com/jdstanhope/huck/issues/12)

- **huck**: `case x in foo) echo hi esac` errors with `UnterminatedCase` (`esac` is eaten as an argument to `echo`).
- **bash**: same as huck — POSIX-strict; bash also requires a separator. (Documented here because the v21 spec example was initially wrong and was corrected.)
- **Why**: matches POSIX and `fi`/`done` precedent.

---

### REPL silently neutralizes stray `break`/`continue`/`return`

[Issue #13 · by-design](https://github.com/jdstanhope/huck/issues/13)

- **huck**: a `return` (or `break`/`continue`) at the top-level prompt sets `$?` to 0 and continues.
- **bash**: prints an error and sets `$?` to 1.
- **Why**: deliberate friendly simplification.

---

### Multi-line commands collapse to one line in history

[Issue #14 · by-design](https://github.com/jdstanhope/huck/issues/14)

- **huck**: v19 collapses a multi-line `if`/`for`/`{…}`/etc. into a single physical line using `;` / space / no-sep joiners. Lossy for quotes that span lines.
- **bash**: stores embedded newlines.
- **Why**: keeps the history file format one-entry-per-line.

---

### `(`/`)`/`{`/`}`/`;;`/`;&`/`;;&` are metacharacters

[Issue #15 · by-design](https://github.com/jdstanhope/huck/issues/15)

- **huck**: unquoted `(` or `)` in arguments is a syntax error (v21); standalone `{`/`}` are keywords (v22).
- **bash**: same — `(` `)` and standalone `{`/`}` are all metacharacters.
- **Why**: required for `case`/subshell/brace-group recognition. Pre-v21 scripts using literal parens must quote them.

---

### Functions shadow regular builtins; control builtins are un-shadowable

[Issue #16 · by-design](https://github.com/jdstanhope/huck/issues/16)

- **huck**: a user-defined `cd() { … }` overrides the builtin; `return`/`exit`/`break`/`continue` cannot be shadowed.
- **bash**: distinguishes "special" vs "regular" builtins per POSIX, with similar (but more nuanced) precedence.
- **Why**: shadowing fundamental flow control would let a user break the shell.

---

### EOF mid-command exits the shell with status 2

[Issue #17 · by-design](https://github.com/jdstanhope/huck/issues/17)

- **huck**: Ctrl-D while a partial command is pending → "syntax error: unexpected end of input", exit 2.
- **bash**: interactive Ctrl-D mid-buffer abandons the line and returns to the prompt.
- **Why**: v19 spec called this a deliberate simplification; revisit if it becomes painful.

---

### HISTFILE defaults to `~/.huck_history`

[Issue #18 · by-design](https://github.com/jdstanhope/huck/issues/18)

- **huck/bash**: different shells, different defaults.

---

### Non-UTF8 command-sub output is lossy

[Issue #19 · by-design](https://github.com/jdstanhope/huck/issues/19)

- **huck**: invalid UTF-8 from `$(cmd)` → `U+FFFD` replacement.
- **bash**: byte-faithful.

---

### `name=(…)` array literal as a non-declaration/non-eval command argument

[Issue #20 · by-design](https://github.com/jdstanhope/huck/issues/20)

- **huck**: the lexer accepts an array literal `name=(…)` as a command ARGUMENT and `expand()` reconstructs the argument to its `name=(…)` text — so `echo x=(a b)` prints `x=(a b)`.
- **bash**: a parse-time syntax error (`echo x=(a b)` → `syntax error near unexpected token '('`).
- **Why**: replicating bash's parse-time gating would need command-context-aware lexing; the reconstruction is harmless and is what makes `eval x=(a b)` / `declare`-style array-literal args work (v136 resolved the prior panic via this reconstruction).

---

### `trap '' PIPE` ignore-form is not preserved inside a forked subshell/pipeline stage

[Issue #38 · by-design](https://github.com/jdstanhope/huck/issues/38)

Since v137 restores `SIGPIPE` to `SIG_DFL` at startup, `trap … PIPE` is now SETTABLE (it was previously rejected with `cannot reset ignored signal` because Rust's startup `SIG_IGN` put SIGPIPE in the ignored-at-startup set). A top-level PIPE trap fires via huck's flag-based dispatch. However, huck represents the ignore-form `trap '' PIPE` as a signal-hook handler (an empty closure), NOT a true OS `SIG_IGN` (the pre-existing model — see `src/traps.rs`), and v137's forked-stage child unconditionally resets `SIGPIPE` to `SIG_DFL` (matching bash's "a trapped signal is reset to default inside a subshell"). The net gap: a `trap '' PIPE` ignore that bash would keep ignored inside a pipeline subshell is instead reset to default in huck, so a producer in that subshell still dies on SIGPIPE. The handler-form and the top-level cases match bash; only the ignore-stays-ignored-in-a-subshell nuance diverges. Strictly more functional than pre-v137 (where the trap could not be set at all). Fixing it would require representing trap-installed ignores as real `SIG_IGN` distinct from startup-ignored signals.

---

### a SIGINT delivered to the shell while it waits on a foreground job that is NOT itself SIGINT-killed over-aborts

[Issue #39 · by-design](https://github.com/jdstanhope/huck/issues/39)

v138 aborts the running command list when an untrapped SIGINT is observed (`sigint_flag` set). bash's wait-path abort keys specifically on the foreground CHILD being terminated by SIGINT (`WTERMSIG==SIGINT`), not merely on the shell receiving a SIGINT during the wait. The narrow divergence: `seq 1 3 | while read x; do echo $x; kill -INT $$; done; echo after` — here `kill -INT $$` (a) signals the PARENT shell pid (`$$` is unchanged inside the pipeline subshell), while the pipeline's `while` subshell exits NORMALLY. bash keeps the parent's pending SIGINT from aborting (the job wasn't interrupted) and runs `after` (rc 0); huck sees `sigint_flag` set after the pipeline and aborts (`after` suppressed, rc 130). This requires `kill -INT $$` from inside a pipeline subshell — pathological. For a REAL interactive Ctrl-C the signal goes to the foreground job's process group (the job dies via SIGINT → huck's `WTERMSIG==SIGINT` trigger aborts, matching bash); and for `-c`/script the shell and children share the foreground pgroup so both receive it. So the in-scope v138 behavior matches bash; only this synthetic parent-only-SIGINT-during-a-surviving-job case diverges. Matching bash exactly would require gating the wait-path abort on the job's own SIGINT-termination rather than on the shell's flag.

---

### `command builtin <decl>` (a `command`-led nest wrapping a declaration builtin) errors instead of running

[Issue #41 · by-design](https://github.com/jdstanhope/huck/issues/41)

v142 adds the `builtin NAME [args]` builtin. huck correctly peels `builtin`-led nests around a declaration builtin (`builtin builtin local x=5`, `builtin command local x=5` both run and print the assignment). But any nest where a `command` wrapper sits immediately outside `builtin <decl>` — `command builtin local x=5`, and also the builtin-led `builtin command builtin local x=5` (the outer `builtin` is peeled, leaving `command builtin local`) — surfaces post-resolve with `decl_args` already discarded, so huck prints `huck: builtin: local: declaration builtins must not be wrapped by \`command builtin\`` and returns rc 1, whereas bash runs it (prints `x=5`). Maximally pathological — no real script nests `command builtin` around a declaration builtin; huck errors cleanly (rc 1, no panic) rather than running it. Matching bash would require carrying `decl_args` through the `command`-led resolution path.

---

### sourced-file syntax error points at the logical command's first line

[Issue #53 · by-design](https://github.com/jdstanhope/huck/issues/53)

- **huck**: a lex or parse (syntax) error raised while reading a sourced file (`source`/`.`, `--rcfile`, script-file mode) is reported as `huck: FILE: line N: syntax error: MSG`, where `N` is the physical line on which the offending logical command STARTED (`cmd_start_line` in `run_sourced_contents`). For a multi-line construct (a function body, a continued `if`, a `case`, etc.) that is the construct's opening line, not the line containing the offending token.
- **bash**: reports the physical line of the offending token itself, so for multi-line constructs bash's `line N` is later than huck's.
- **Why intentional**: huck parses a whole logical command (gathering continuation lines) before it can report a syntax error, so the precise within-command token line is not available without per-`Token` position tracking. The first-line report is accurate for the overwhelming majority of cases (single-line commands, where the two coincide) and is enough to locate the failing construct. Exact token `line:col` reporting is deferred (needs per-`Token` position tracking). Note: in huck unterminated quotes/braces become continuation rather than lex errors, so most user-visible cases are parse errors.
- **Workaround**: none needed — the reported line locates the failing construct; inspect from that line forward.

---

### `${var@OP}` scalar-transform edge divergences

[Issue #55 · by-design](https://github.com/jdstanhope/huck/issues/55)

- **huck**: three edge divergences in the v96 `${var@OP}` scalar transforms (M-86): (a) `@P` reuses `expand_prompt`, which always expands `$VAR`/command-substitution, whereas bash suppresses those in `@P` when `shopt -u promptvars` (default is ON, and backslash-escape processing matches; oh-my-posh's pre-rendered ANSI value has no `$VAR` so it is unaffected); (b) `@U`/`@u` non-ASCII case mapping inherits the pre-existing `case_modify` Rust `to_uppercase` quirk (e.g. `straße`@U → `STRASSE` vs bash `STRAßE`) — the SAME behavior as the existing `${v^^}`/`${v^}` modifier (M-17), not a v96 regression; (c) `@Q` of a high / non-UTF-8 byte renders char-wise (`$'\xff'`@Q → `'ÿ'`) rather than bash's byte-wise `$'\377'`, the same char-vs-byte gap as L-11 — the value still round-trips.
- **bash**: as described above — promptvars-gated `@P` $VAR suppression; locale/byte-faithful `@U`/`@Q`.
- **Why intentional**: (a) requires plumbing the `promptvars` shopt into prompt expansion for a default-on, rarely-unset option; (b)/(c) are pre-existing UTF-8/char-based architecture choices shared with M-17 and L-11, not new to `@OP`. In every common case (pre-rendered prompt values, ASCII text) output matches bash.
- **Workaround**: none needed for the oh-my-posh `@P` path or ASCII transforms.

---

### `&` ordering inside a command-substitution

[Issue #56 · by-design](https://github.com/jdstanhope/huck/issues/56)

- **huck**: **Command-substitution `&` ordering**: `$( a & b )` runs synchronously in source order in huck (the documented capture-context "ignore `&`" design — see the `execute_capturing` top-level `Amp`→`Semi` rewrite) rather than backgrounding `a`; the output *content* matches bash, but interleaving/ordering may differ when `a` would have overlapped `b`. (The former nested-`&`-inside-a-captured-subshell edge — `x=$( ( echo n1 & wait ) )` yielding empty vs bash's `n1` — is RESOLVED and now matches bash, verified 2026-07-07.)
- **bash**: `a` is genuinely backgrounded inside the substitution.
- **Why intentional**: the intentional capture-context design — backgrounding inside a substitution that synchronously drains stdout has no observable benefit and risks lost output, so huck serializes. Real scripts rarely background inside `$(…)`; the edge is pathological.
- **Workaround**: avoid backgrounding inside `$(…)`; run the `&`'d command at the top level and capture its output by other means.

---

### `command CMD` bare-form edges (`-p` PATH, `command declare -a`, function named `command`)

[Issue #57 · by-design](https://github.com/jdstanhope/huck/issues/57)

- **huck**: three edges in the v99 `command CMD` bare form (M-85): (a) `command -p CMD` resolves CMD via the CURRENT `$PATH`, not bash's guaranteed default PATH (`getconf PATH` / a "standard utilities" path); huck has no separate default-PATH search, so `-p` is accepted but effectively a no-op over the live `$PATH`. (b) `command declare -a a=(x y z)` (a compound array RHS reached via `command`): bash REJECTS it as a syntax error, but huck ACCEPTS it (the pre-resolve declaration-builtin reconstruction assigns the array) — a no-panic SUPERSET. Scalar `command export X=1` matches bash. (c) A user FUNCTION named `command` (`command() { …; }`) cannot shadow the builtin in huck — the interception is unconditional and runs BEFORE function lookup — whereas bash lets the function take precedence. The same holds for a function named `builtin` (added v142): the `builtin` pre-resolve interception also runs before function lookup, so `builtin() { …; }` cannot shadow the `builtin` builtin either.
- **bash**: (a) `-p` searches a guaranteed default PATH; (b) `command declare -a a=(…)` is a syntax error; (c) a function named `command` (or `builtin`) shadows the corresponding builtin.
- **Why intentional**: (a) huck has no built-in default-PATH constant; the live `$PATH` covers every real use of `-p` (recovering a real command past a shadowing function). (b) accepting the array assignment is strictly more permissive and panic-free — the only divergence is huck succeeding where bash errors on a pathological input. (c) POSIX discourages naming a function `command`; the unconditional interception is what makes the bare form reliably bypass functions in the first place.
- **Workaround**: (a) none needed — set `$PATH` explicitly if a default-PATH search is required; (b) avoid `command declare -a` for array RHS; (c) do not name a function `command`.

---

### `set -x` (xtrace) trace-format divergences

[Issue #58 · by-design](https://github.com/jdstanhope/huck/issues/58)

- **huck**: residual differences from bash's xtrace output, none of which affect the diagnostic value. (a) **Compound-header traces — IMPLEMENTED (v198)**: huck now emits a trace line for each compound-command header — the `for`-iteration variable (per loop pass, `+ for i in 1 2`), the `case` word (`+ case x in`), `select`, standalone `(( ))` and C-style `for ((;;))` arith clauses, and `[[ … ]]` LEAF-BY-LEAF with expanded operands and short-circuit (`+ [[ 1 -gt 0 ]]`; `[[ a && b ]]` → up to two lines; an untaken `||`/`&&` branch is not traced; `! leaf` folds into one line). Raw headers come from a `reconstruct_word_source` Word→source renderer; `[[ ]]` leaves from a `suppress`-threaded hook in `eval_test_expr`. Narrow RESIDUALS remain: (i) the rhs *pattern* of `[[ … == … ]]`/`=~` is shown as its expanded value, not bash's per-character quote-provenance escaping (`[[ $x == "p q" ]]` → bash `\p\ \q`, huck `p q`); (ii) the string-equality operator renders canonically as `==` (the AST collapses source `=` and `==`); (iii) reconstructed-header quote/brace STYLE is not recoverable from the parsed `Word` (`'x'`/`"x"` both render `"x"`; a bare `${x}` renders `$x`), and a deeply-nested compound command inside a `$()` header renders approximately; (iv) a command-substitution *operand* inside `[[ ]]` is expanded twice under `set -x` (once for the trace line, once for the comparison), so a `$(cmd)` with side effects may run twice — same family as (b). **(v219 provenance note)** the lexer now records source quote spans in `WordPart::Quoted` (added to make `declare -f` / `type` reconstruction bash-faithful, L-57); that provenance is available to the xtrace `reconstruct_part` path and would let residuals (i) and (iii) render with bash-faithful quote/brace style if wired up later — but the trace renderer does not yet consult it. (b) **Decl-RHS-with-command-substitution edge**: a `local`/`declare`/`export` RHS that contains a command substitution (e.g. `local x=$(cmd)`) is traced with the EXPANDED value, which re-runs the command substitution for the trace (double-execution) — rare. (c) Per M-90, `2>/dev/null` does NOT suppress the trace (builtin/executor stderr ignores `2>` — consistent with M-90). (d) **Pipeline-stage trace ORDER is best-effort**: the set of trace lines matches bash, but in-process (builtin/function) stages trace from a forked child while external stages trace from the parent, so the left-to-right ORDER of trace lines in a mixed pipeline may differ from bash (the lines themselves are identical).
- **bash**: renders the `[[ ]]` rhs pattern with quote-provenance escaping; echoes the source `=`/`==` spelling and the source quote/brace style; traces a decl RHS without re-executing a command substitution; left-to-right pipeline-stage trace order; `2>` can redirect the trace.
- **Why intentional**: the v198 compound-header traces match bash for the common cases (`xtrace_compound_diff_check.sh`); the four remaining `(a)` residuals are cosmetic display-only divergences (quote/brace style and the rare cmdsub-operand double-run). The decl-RHS-cmdsub double-execution and pipeline-stage ORDER edges are inherited from existing architecture; the `2>` gap is M-90's stderr-sink limitation. Every command still emits a trace line.
- **Workaround**: none needed for diagnostic use; the trace still shows each command before it runs.

---

### linear source-reader unit-boundary / resync edges

[Issue #59 · by-design](https://github.com/jdstanhope/huck/issues/59)

- **huck**: two edges in the v104 O(n) script source reader (M-99), both confined to already-divergent verbose / error paths. (a) **Trailing top-level `;`/`&` before a newline**: a trailing top-level `;` or `&` immediately before a newline (e.g. `set -v ;⏎cmd`) groups with the NEXT command into one parsed unit — only `&&`/`||`/`;`-on-a-line and bare top-level newlines bound units — so `set -v` / `set +v` taking effect via such a trailing-separator-then-newline line may echo one fewer / more line than bash. (`set -v; cmd` on ONE line already matched bash; only the rare trailing-`;`-then-newline differs.) Execution OUTPUT is unaffected — only the `set -v` echo of that boundary. (b) **Post-syntax-error resync**: after a syntax error the reader skips to the next top-level newline (the token-stream analogue of the old "clear the buffer, continue at the next line" recovery), so the cascade AFTER a syntax error may differ slightly from the old per-line resync.
- **bash**: (a) `set -v` echoes exactly the physical lines as read; (b) bash's own error-recovery boundaries.
- **Why intentional**: both are negligible and only affect the already-divergent-from-bash verbose / error edges; unit boundaries are intentionally `&&`/`||`/`;`-on-a-line / top-level-newline so the one-command-at-a-time linear reader (M-99) stays O(n). Normal execution output is identical.
- **Workaround**: none needed; put `set -v` / `set +v` on its own line (no trailing `;`/`&`) for an exact bash-matching echo boundary.

---

### FUNCNEST recursion backstop diverges from bash on pathological recursion

[Issue #66 · by-design](https://github.com/jdstanhope/huck/issues/66)

huck enforces `FUNCNEST` exactly like bash, but additionally clamps the effective nesting limit to an internal backstop `FUNCNEST_HARD_MAX = 2048` (below huck's ~2800 native-stack crash ceiling). So unbounded recursion with no/`0` FUNCNEST — or `FUNCNEST` set above 2048 — produces a clean `maximum function nesting level exceeded (2048)` error + rc 1 where bash would recurse deeper (and ultimately SIGSEGV). Intentional robustness improvement over bash's segfault; the message shows `2048` rather than a user-set value when clamped. Not a no-crash guarantee — a sufficiently stack-heavy recursive function can still overflow below 2048.

---
