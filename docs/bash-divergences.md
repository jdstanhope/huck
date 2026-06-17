# huck vs bash 5.x — Divergence Reference

**Last updated:** 2026-06-09 (slimmed to only the current/open divergences — deferred bugs/features + intentional divergences. Resolved divergences and the full shipped-iteration change log are preserved in git history.)

This is the running audit of where huck differs from bash 5.x. As of the
2026-06-09 slim, it lists ONLY the divergences that are still current:
`[deferred]` entries (pending work) and `[intentional]` entries (kept on
purpose by design). Resolved divergences — every `[fixed vNN]` entry — and the
shipped-iteration change log have been removed; they live in git history,
under `docs/superpowers/` (per-iteration specs + plans), and in the project
iteration memory. Reference an ID (e.g. `M-114`) in commit messages so the doc
stays in sync.

## How to read

- Each entry has an ID (`B-` bugs, `M-` missing features, `I-` intentional,
  `L-` low-impact), status, severity, the two behaviours, and (when known)
  the fix location.
- **Status**: `[deferred]` (a real divergence still to be addressed, ranked by
  severity) or `[intentional]` (a deliberate divergence we're keeping). A
  compound status (e.g. "(a) `[deferred]` / (b) `[intentional]`") means the
  entry has both open and kept-by-design parts.
- **Severity**: `high` (likely to surprise users / break scripts), `medium`
  (rare but real), `low` (cosmetic / edge case).

## Summary

| Tier | Count | Notes |
| --- | --- | --- |
| Bugs (Tier 1) | 0 | None open. |
| Missing features (Tier 2) | 17 | Deferred bash-compat backlog, ranked by severity within each group. |
| Intentional (Tier 3) | 9 | Deliberate divergences we're keeping. |
| Low-impact (Tier 4) | 39 | Open edge cases / cosmetic divergences (`[low]`/`[intentional]`/`[deferred]`). |

---

## Tier 1: Bugs

huck behaves wrong without a design reason; should be fixed.

_None currently open._

---

## Tier 2: Missing bash features

Bash features huck doesn't implement. Listed roughly by impact within each
group.

### Functions & scoping

- **M-09a: Relaxed function-name characters** — `[deferred]` low. huck restricts function names to POSIX identifiers (`[A-Za-z_][A-Za-z0-9_]*`) in BOTH the `name() body` and `function name body` forms. Bash 5 accepts `.`, `-`, `+`, `:` and other non-POSIX-identifier characters when the function is defined via the keyword form (`function foo.bar { :; }`). Rarely used in practice.
- **M-09b: Definition-attached redirections** — `[deferred]` low. Both function-definition forms (`name() body > file` and `function name body > file`) currently reject trailing redirections. Bash allows attaching redirections to the function definition itself, taking effect at every call. Affects both forms equally.

### Parameter expansion modifiers

- **M-15b: operand quote/escape context not propagated** — `[deferred]` low. Two pre-existing divergences (NOT introduced by v84) when a `${...}` is itself inside double quotes: (a) single quotes in its operand are stripped by huck but kept literal by bash (`"${y:-'a|b'}"` → bash `'a|b'`, huck `a|b`); (b) a backslash-escaped char in an operand drops the backslash where bash keeps it (`"${y:-\*}"` → bash `\*`, huck `*`). Root cause: `parse_braced_operand` receives only the extracted operand body, not the enclosing quote context.
- **M-93: `${var@OP}` array/attribute transforms (`@A`/`@K`/`@k`/`@a`)** — `[deferred]` low. huck: the assignment-statement form `@A`, the key/value array forms `@K`/`@k`, and the attribute-flags form `@a` error (unsupported `@`-operator). bash: `@A` reproduces a `declare`-style assignment string, `@a` lists attribute flags, `@K`/`@k` expand associative-array key/value pairs. M-86 follow-on; the scalar transforms (`@P`/`@Q`/`@U`/`@L`/`@u`/`@E`) shipped in v96.
- **M-127: case modification on a whole array (`${a[@]^^}` / `${a[@],,}` / `${a[@]^}` / `${a[@],}`)** — `[deferred]` low (found in the v157 runtime sweep, batch 4). huck errors `${a[…]}: modifier Case { … } not supported on array in v71` when a case-modification modifier (`^^`/`,,`/`^`/`,`, with or without a pattern) is applied to the `[@]`/`[*]` form. bash applies the case fold to EVERY element (`a=(foo bar); echo "${a[@]^^}"` → `FOO BAR`). The per-element form `${a[1]^^}` and the scalar form `${v^^}` both already work (v37), so the case-fold machinery exists — the gap is that the array-iteration path in the modifier dispatch doesn't map the `Case` modifier over each element the way it does for substring/replace. Fix: in the `[@]`/`[*]` modifier branch, apply `case_modify` per element (mirroring how the other per-element modifiers are handled). The same v71 "not supported on array" guard also rejects a few other modifiers on `[@]`; only `Case` was observed in the sweep.

### Redirects

- **M-125: a non-final pipeline stage with an explicit stdout redirect doesn't get an inter-stage pipe** — `[deferred]` low (found in v156 review; pre-existing, NOT introduced by v156). `cmd >file | next`: bash gives `next` an immediate-EOF stdin (the explicit `>file` overrides the pipe for `cmd`'s stdout, and `next` reads end-of-pipe → EOF); huck skips creating the inter-stage pipe when a stage has an explicit stdout fd, so `next` inherits the PARENT's stdin instead. With an already-EOF parent stdin the observable result matches bash; with a blocking parent stdin (terminal/FIFO) huck HANGS where bash returns. Edge: requires a non-final stage with `>file`/`>>file` AND a blocking parent stdin. Fix lives in the pipeline executor (`run_multi_stage`, ~the `explicit_stdout_fd`/`!is_last` pipe-creation branch): still create the inter-stage pipe for the downstream reader even when the upstream stage's stdout is explicitly redirected (the redirect overrides the upstream's pipe write-end, but the downstream still needs the read-end).

### Quoting

- **M-29: `$"…"` locale quoting** — `[deferred]` low. huck: parses as `$` + double-quoted word. bash: gettext lookup.

### Job control

- **M-41: Limited signal name set** — `[deferred]` medium. huck: 15 names (no SEGV/ABRT/FPE/BUS/ILL/TRAP/…). bash: full platform signal set.
- **M-42: `kill` with negative PID** — `[deferred]` low. huck: rejects. bash: passes to `kill(2)` as a pgrp / wildcard target.
- **M-126: multiple simultaneous coprocs** — `[deferred]` low. v157 reliably supports ONE active coproc and emits a `… still exists` warning (`huck: warning: execute_coproc: coproc [PID:NAME] still exists`) when a second `coproc` is started while one is still running. bash 5 supports fully independent concurrent coprocs, each with its own `NAME[0]`/`NAME[1]`/`NAME_PID` triple tracked separately. The `Shell.coprocs` field is already a `Vec`, so supporting multiple active coprocs is a policy relaxation (remove the single-active-coproc guard and allow concurrent entries) rather than a structural change; deferred as a low-priority follow-on. Note: coproc fd numbers and the exact warning wording are NOT byte-compared in the coproc_diff_check.sh harness. Related (v157): a coproc that exits in a non-interactive script WITHOUT an explicit `wait` is not auto-unset until the next `wait`/shell exit — huck reaps coprocs LAZILY (like bash) so that reading a coproc's final output after its body exits stays reliable (an eager between-statement drain was tried in v157 but reverted because it raced the canonical write-then-read idiom, reaping + unsetting `COPROC` ~6-10% of the time before the read). The `wait`-then-use path and the interactive REPL both auto-unset correctly.
- **M-96: first-class nested and-or AST (`list → and_or → pipeline → command`)** — `[deferred]` low. M-95 follow-on. v98 keeps the flat `Sequence` model with executor-side grouping (`partition_into_groups` + `run_andor_group`), which is correct and low-risk. A future first-class `list → and_or → pipeline → command` AST may eventually be wanted to express `time` on a whole group, per-group traps, and cleaner pipeline-status propagation — none of which the flat model represents natively. Not user-visible today; logged so the structural debt is tracked.

### Builtins (other)

- **M-36b: system-data completion actions** — `[deferred]` low. `compgen -A
  hostname`/`user`/`group`/`service` are recognized but return nothing; bash reads
  `/etc/hosts`(`$HOSTFILE`)/`/etc/passwd`/`/etc/group`/`/etc/services`. Rarely the
  decisive completion source; deferred to avoid new filesystem/libc lookups.
- **M-46: `history -d`/`-w`/`-r`/`-a` flags** — `[deferred]` low. huck: only `-c`. bash: full set.
- **M-47: `history N`** — `[deferred]` low. huck: rejects numeric arg. bash: prints last N entries.
- **M-122: bare `declare` (no args) omits function definitions** — `[deferred]` medium (v146). bash's bare `declare` (no flags, no names) prints ALL variables AND every function's body (equivalent to the variable listing followed by `declare -f`). huck's bare `declare` lists only variables. `declare -f` (no name) DOES print all function bodies (wired in v146), so the building block exists — the gap is that the bare-`declare` listing path doesn't append the `generate::function_to_source` output for each function after the variables. Low-risk follow-on now that the serializer exists.
- **M-92: prefix-name `${!prefix@}` / `${!prefix*}`** — `[deferred]` low.
  The variable-NAME-listing forms of `${!…}` (expand to the names of all
  set variables whose name begins with `prefix`) are not implemented —
  the lexer's `${!` branch handles only the scalar-indirect form (M-91).
  Not used by the bashrc / bash-completion; deferred. M-91 follow-on.

### Globbing

- **M-53: `**` globstar** — `[deferred]` low. huck: `**` ≡ `*`. bash: `shopt -s globstar` makes `**` match across `/`.

---

## Tier 3: Intentional divergences

Things huck deliberately does differently from bash. Document and keep.

### I-02: `case` requires a separator before `esac`
- **Status**: intentional
- **Severity**: low
- **huck**: `case x in foo) echo hi esac` errors with `UnterminatedCase` (`esac` is eaten as an argument to `echo`).
- **bash**: same as huck — POSIX-strict; bash also requires a separator. (Documented here because the v21 spec example was initially wrong and was corrected.)
- **Why**: matches POSIX and `fi`/`done` precedent.

### I-03: REPL silently neutralizes stray `break`/`continue`/`return`
- **Status**: intentional
- **Severity**: low
- **huck**: a `return` (or `break`/`continue`) at the top-level prompt sets `$?` to 0 and continues.
- **bash**: prints an error and sets `$?` to 1.
- **Why**: deliberate friendly simplification.

### I-05: Multi-line commands collapse to one line in history
- **Status**: intentional
- **Severity**: low
- **huck**: v19 collapses a multi-line `if`/`for`/`{…}`/etc. into a single physical line using `;` / space / no-sep joiners. Lossy for quotes that span lines.
- **bash**: stores embedded newlines.
- **Why**: keeps the history file format one-entry-per-line.

### I-06: `(`/`)`/`{`/`}`/`;;`/`;&`/`;;&` are metacharacters
- **Status**: intentional
- **Severity**: low
- **huck**: unquoted `(` or `)` in arguments is a syntax error (v21); standalone `{`/`}` are keywords (v22).
- **bash**: same — `(` `)` and standalone `{`/`}` are all metacharacters.
- **Why**: required for `case`/subshell/brace-group recognition. Pre-v21 scripts using literal parens must quote them.

### I-07: Functions shadow regular builtins; control builtins are un-shadowable
- **Status**: intentional
- **Severity**: low
- **huck**: a user-defined `cd() { … }` overrides the builtin; `return`/`exit`/`break`/`continue` cannot be shadowed.
- **bash**: distinguishes "special" vs "regular" builtins per POSIX, with similar (but more nuanced) precedence.
- **Why**: shadowing fundamental flow control would let a user break the shell.

### I-11: EOF mid-command exits the shell with status 2
- **Status**: intentional (per v19 spec)
- **Severity**: medium
- **huck**: Ctrl-D while a partial command is pending → "syntax error: unexpected end of input", exit 2.
- **bash**: interactive Ctrl-D mid-buffer abandons the line and returns to the prompt.
- **Why**: v19 spec called this a deliberate simplification; revisit if it becomes painful.

### I-13: HISTFILE defaults to `~/.huck_history`
- **Status**: intentional
- **Severity**: low
- **huck/bash**: different shells, different defaults.

### I-15: Non-UTF8 command-sub output is lossy
- **Status**: intentional
- **Severity**: low
- **huck**: invalid UTF-8 from `$(cmd)` → `U+FFFD` replacement.
- **bash**: byte-faithful.

### I-16: `name=(…)` array literal as a non-declaration/non-eval command argument
- **Status**: `[intentional]`
- **Severity**: low (v136)
- **huck**: the lexer accepts an array literal `name=(…)` as a command ARGUMENT and `expand()` reconstructs the argument to its `name=(…)` text — so `echo x=(a b)` prints `x=(a b)`.
- **bash**: a parse-time syntax error (`echo x=(a b)` → `syntax error near unexpected token '('`).
- **Why**: replicating bash's parse-time gating would need command-context-aware lexing; the reconstruction is harmless and is what makes `eval x=(a b)` / `declare`-style array-literal args work (v136 resolved the prior panic via this reconstruction).

---

## Tier 4: Low-impact / edge cases

- **L-01**: `~user` lookup capped at 16 KiB buffer. (Never hit in practice.)
- **L-02**: Glob sort order is byte-lexicographic, not `LC_COLLATE`-aware.
- **L-03**: Non-integer variable in `$((…))` errors instead of bash's "treat as recursive arith expression."
- **L-04**: `${#var}` counts Unicode chars; bash counts bytes (matches in UTF-8 locale). v33 extends the same char-counting convention to `${var:off:len}` — offset/length units are codepoints, never byte indices. Slices never split a multi-byte UTF-8 codepoint. v37 `${var^^}` / `${var,,}` uses Rust's `char::to_uppercase` / `char::to_lowercase` Unicode iterators — locale-independent (matches bash with UTF-8 locale; may differ in non-UTF-8 locales).
- **L-05**: `[N] PID` spawn notification shows only the last pipeline stage's PID; bash shows all.
- **L-06**: `jobs` column width is fixed at 24; bash uses terminal width.
- **L-07**: `wait` polls (50ms) rather than blocking — small latency / minor CPU usage.
- **L-27: history expansion runs on piped (non-interactive) stdin** — `[low]`. huck applies `!`-history expansion to commands read from piped stdin (`printf 'echo hi!there\n' | huck` → `huck: !there: event not found`), whereas bash disables history expansion when non-interactive (piped stdin or a script) and prints `hi!there`. huck's file-arg path (`huck script.sh`) and `source` are unaffected — they match bash. Root: the REPL/piped-stdin reader (`src/shell.rs` `read_logical_command`) runs the history scanner regardless of interactivity; bash gates `histexpand` on an interactive shell. Surfaced repeatedly while testing `[!…]`/`[^…]` glob fragments (which contain `!`) via piped stdin; the v116 bracket-negation harness and integration tests run fragments as file-args to avoid it. Low impact: interactive use and scripts/`source` (the common paths) are correct; only literal piped-to-stdin command streams containing `!` diverge.

- **L-29: residual prompt/`$PS4` xtrace quirks** — `[deferred]`, low. The original L-29 gap (`$LINENO` empty in prompts/`$PS4`) is **resolved**: v141 added `$(...)`/`$((...))`/backticks to `prompt::expand_prompt`, and **v152 added the `LINENO` variable**, so `$LINENO` now expands in `$PS4`, prompts, and `${var@P}` byte-identically to bash. Two unrelated residuals remain (same low tier): (a) a `PS4=…` assignment command traces its OWN line with the POST-assignment `PS4` value (huck emits e.g. `+2+ PS4=…`), whereas bash uses the PRE-assignment value (`+ PS4=…`) — huck's bare-assignment xtrace emits after applying the assignment; narrow and cosmetic. (b) (v141, arcane) an ESCAPED backtick inside a backtick prompt body (`` `echo a\`b` `` via `@P`) — huck's prompt backtick scanner treats `\`` as a literal and runs `echo a`b`, whereas bash treats it as a nested-cmdsub delimiter (errors on the unmatched inner). Far outside common use; deferred.

- **L-31: the `read` builtin issues one `libc::read` syscall per byte** — `[deferred]`, low (perf, not a hang). `read`/`read -r` reads stdin one byte at a time (`src/builtins.rs` ~line 2027), so consuming a large body via `read` is O(n) syscalls — slow on big inputs (e.g. a 200KB heredoc piped into `while read`). NOT a deadlock (it completes); off the `nvm ls-remote` path (nvm feeds the big index.tab to `awk`, which reads in blocks). Fix: buffer `read`'s stdin (read a chunk, scan for the delimiter, push back the remainder) — but `read` must not over-consume past its delimiter from a shared fd, so the buffering needs care. Found v134.

- **L-42: redirect-open error routing ignores an earlier same-command `2>…`** — `[deferred]`, low. When one redirect on a command fails to open, huck prints the error (`huck: /path: No such file…`) to the REAL stderr regardless of an earlier `2>/dev/null` on the same command, whereas bash processes redirects strictly left-to-right, so a later failing redirect's error lands under the earlier `2>…` (`cmd 2>/dev/null < /missing` → bash: silent, huck: prints the error). Affects normal commands AND the assignment/redirect-only command equally — a routing/ordering limitation of the field-based `ExecCommand` redirect model (related to L-08, which covers the same source-order-not-preserved root cause for the `2>&1 >file` anti-pattern). The exit status (1) matches; only the error-message routing differs.
- **L-43: a readonly assignment does not abort a non-interactive shell** — `[deferred]`, low. Assigning to a readonly variable (`UID=5`, `BASH_VERSINFO[0]=9`) correctly prints `huck: NAME: readonly variable` and yields status 1, but huck CONTINUES executing the rest of the script; bash treats a variable-assignment error as fatal in a non-interactive shell and EXITS immediately (`bash -c 'UID=5; echo done'` prints only the error and exits 1 — `done` never runs). Independent of any redirect (reproduces with a bare `UID=5; echo done`). The common interactive case (where bash also keeps going) matches; only non-interactive scripts diverge. Fix: in the readonly-assignment error path (`run_assignment_list` and the inline-assignment apply), propagate a fatal-exit outcome when the shell is non-interactive, mirroring bash's special-builtin/assignment-error abort rule.
- **L-44: `declare -p` associative-array serialization format** — `[deferred]`, low (found in the v157 runtime sweep, batch 4). For an associative array huck emits `declare -A m=(["k"]="v" ["x"]="y")` whereas bash emits `declare -A m=([x]="y" [k]="v" )` — three differences: (a) huck quotes the subscript key (`["k"]`) where bash leaves a bareword key unquoted (`[k]`, quoting only keys that need it); (b) bash appends a trailing space before the closing `)`; (c) bash lists elements in its internal hash order, huck in insertion order. Both forms round-trip correctly through `eval`/`source` (the value is the point), so this is cosmetic. (c) is impractical to match — bash's hash order is an implementation detail and huck's insertion order is arguably more useful; (a)/(b) are matchable in the `declare -p` serializer (`generate::*`) if exact byte-parity is ever wanted. Indexed arrays and scalars already match bash. Related: M-122 (bare `declare` omits functions), L-41 (`declare -p` omits computed dynamics).
- **L-45: inline `shopt -s extglob; <use>` on a single logical line** — `[deferred]`, low (found in the v157 runtime sweep, batch 4). extglob pattern matching (`+(…)`/`@(…)`/`!(…)`/`*(…)`/`?(…)`) is fully implemented (`glob_match::extglob_match`) and works in `[[ == ]]`, `case`, and pathname globbing whenever `shopt -s extglob` runs on an EARLIER logical line (separate line, script file, or piped stdin — all verified). It fails only when the `shopt -s extglob` and the extglob-using command are joined on the SAME logical line (`shopt -s extglob; [[ aaa == +(a) ]]`, or a single newline-free `-c` string), because huck lexes the entire logical line up front — with extglob still OFF — so the `(` in the pattern is a metacharacter and the `[[ ]]` fails to parse (`unterminated '[[ ]]'`). bash handles the same one-liner because its parser re-evaluates after the `shopt`. The inter-logical-line re-lex mechanism (`process_line`, the extglob-flip re-tokenize tail) already covers the common case; this same-line case would need re-lexing after a `;`-separated `shopt` flips the option mid-line. Real scripts put `shopt -s extglob` on its own line near the top, so this rarely bites. Related: I-06 (`(` is a metacharacter), M-101/L-24 (extglob `LexerOptions` threading through nested re-tokenization).

- **L-46: bare attribute-only declaration prints `NAME=""` in `declare -p`** — `[deferred]`, low (found during v158). A declaration that sets an attribute but assigns NO value — `declare -i x`, `declare -r x`, `declare -x x`, `declare -l x`, `declare -A m` — creates the variable as a SET empty-string value in huck, so `declare -p x` prints `declare -i x=""` where bash prints `declare -i x` (no `=…`, because the variable is declared-but-UNSET). Affects every attribute flag equally (the attribute mutators — `mark_integer`/`mark_readonly`/`set_case_fold`/etc. — create the var with `VarValue::Scalar(String::new())` rather than a distinct "declared, unset" state). Pre-existing; not specific to v158's `-l`/`-u`. Round-trips fine via `eval`/`source` (an empty-string assignment is harmless). Fix would need a tri-state value (unset-but-declared vs set-empty vs set-value) on `Variable`, or a per-var "has been assigned" flag consulted by `declare -p`. Low impact: the bare-attribute-then-`declare -p` pattern is rare and the `=""` is cosmetic.

- **L-47: nameref follow-on gaps (`for ref in …` and `[[ -R ref ]]`)** — `[deferred]`, low (v160 follow-ons; v160 shipped comprehensive-core `declare -n` namerefs). (a) A nameref used as a `for`-loop VARIABLE: huck routes the loop assignments THROUGH the nameref (writes the target each iteration), whereas bash does NOT route a for-loop var through a nameref (bash itself is inconsistent here — the loop var reads empty and the target is untouched). huck's behavior is arguably more sensible, but diverges; rare (pass-by-ref code seldom uses the ref as a loop var). (b) The `[[ -R name ]]` test operator (true iff `name` is a set nameref) is not implemented — `[[ -v ref ]]` IS implemented (tests the target). Both are completeness edges; the common nameref idioms (`local -n out=$1`, scalar/array/element targets, chains, `${!ref}`, `unset`/`unset -n`, `declare -p`) all match bash (`nameref_diff_check.sh`, 21 cases).

- **L-48: spurious parse diagnostics on a single-quoted `awk` C-style-for body inside `$(…)`** — `[deferred]`, low (parse-sweep follow-on, found during v180 on `fcnal-test.sh`). A multi-line single-quoted `awk '{ for (i = 3; i <= NF; ++i) { … } }'` program used as a pipeline stage INSIDE command substitution (`addr=$(… | awk '{ … }')`) is not kept fully opaque by huck's parser: awk's C-style `for (`, `{`, and `}` leak into the shell parse, producing cascading spurious `syntax error: invalid variable name in 'for' loop` / `unexpected '}'` messages on unrelated later lines. The parse ultimately RECOVERS — `huck -n` exits 0, byte-matching `bash -n` rc 0 — so this is diagnostic NOISE, not a hard parse divergence (the script runs); the parse-sweep's any-stderr heuristic over-flags it as a HUCK_GAP. Not the v180 for-loop-var feature (that is correct and verified) — a distinct command-substitution / single-quote-tokenization gap. Low impact: cosmetic stderr; affects scripts embedding awk C-style-for programs in `$()` (e.g. kernel selftests).

- **L-50: `bind` builtin — partial readline surface** — `[deferred]`, low (v161 shipped the `bind` builtin over huck's rustyline editor). v161 implements: the 5 readline VARIABLES that map to live rustyline config (`editing-mode` vi/emacs, `bell-style`, `show-all-if-ambiguous`, `completion-query-items`, `keyseq-timeout`), real key REBINDING for the ~33 readline functions with a rustyline `Cmd` equivalent (`bind '"\C-w":backward-kill-word'` genuinely takes effect), and the listing/error flags (`-v/-V/-l/-p/-P`, unknown-option → rc 2 + usage). KNOWN GAPS vs bash: (a) **`bind -x`** (bind a key to a SHELL COMMAND) is accepted but a no-op. FEASIBILITY (investigated 2026-06-15, deliberately KEPT DEFERRED — robust support is high-effort/high-risk for a rarely-used feature): rustyline's `ConditionalEventHandler` (`Send + Sync`, returns `Option<Cmd>`) CAN run the command if the handler holds only the command STRING and reaches the single-threaded `Shell` via a `thread_local!` the run loop populates before each `readline()` (sidesteps `Send+Sync` since the `Rc<RefCell<Shell>>` lives in the thread-local, not the handler). `EventContext::line()` gives the buffer → `READLINE_LINE` (read) works; `Cmd::Replace(Movement::WholeLine, Some(new))` writes it back. BUT two hard limits make full bash parity impractical: (i) `EventContext` exposes NO cursor position, so `READLINE_POINT` can't be read (only faked at end-of-line); (ii) the command runs while rustyline owns the terminal in RAW mode, so a `bind -x` command that PRINTS (the common fzf-style use) renders mangled unless huck manually toggles `tcsetattr` cooked→run→raw and forces a redraw — re-entrant with rustyline's own terminal management and fiddly/risky. A line-transform-only subset (READLINE_LINE in/out, no output handling) would be feasible and lower-risk if a real need appears. (b) **`bind -f file`** (read an `.inputrc`) is a no-op. (c) **readline functions with no rustyline `Cmd`** (e.g. `dump-functions`, `re-read-init-file`, keyboard macros) can't be bound — warn/no-op. (d) **`bind -v` lists 5 variables**, not bash's ~30 (the ones huck genuinely models); other `set VAR value` are recorded for round-trip but inert. (e) **a bogus function name** (`bind '"\C-x":no-such-fn'`) → huck rc 1 (validates) vs bash rc 0 (bash skips bind-arg validation when non-interactive — "line editing not enabled"); huck's stricter validation is deliberate. (f) **`bind -p`/`-P` list nothing in non-interactive `-c`/script mode** — bindings are applied (and recorded into the active list) by the REPL run-loop seam, which only runs interactively. All editor-effecting paths are proven by `tests/bind_pty.rs` (live rebind) + `bind_diff_check.sh` (13 cases); coupling to rustyline is confined to `src/readline_bind.rs` + the run loop.

- **L-32: `trap '' PIPE` ignore-form is not preserved inside a forked subshell/pipeline stage** — `[intentional]`, low (v137). Since v137 restores `SIGPIPE` to `SIG_DFL` at startup, `trap … PIPE` is now SETTABLE (it was previously rejected with `cannot reset ignored signal` because Rust's startup `SIG_IGN` put SIGPIPE in the ignored-at-startup set). A top-level PIPE trap fires via huck's flag-based dispatch. However, huck represents the ignore-form `trap '' PIPE` as a signal-hook handler (an empty closure), NOT a true OS `SIG_IGN` (the pre-existing model — see `src/traps.rs`), and v137's forked-stage child unconditionally resets `SIGPIPE` to `SIG_DFL` (matching bash's "a trapped signal is reset to default inside a subshell"). The net gap: a `trap '' PIPE` ignore that bash would keep ignored inside a pipeline subshell is instead reset to default in huck, so a producer in that subshell still dies on SIGPIPE. The handler-form and the top-level cases match bash; only the ignore-stays-ignored-in-a-subshell nuance diverges. Strictly more functional than pre-v137 (where the trap could not be set at all). Fixing it would require representing trap-installed ignores as real `SIG_IGN` distinct from startup-ignored signals.

- **L-33: a SIGINT delivered to the shell while it waits on a foreground job that is NOT itself SIGINT-killed over-aborts** — `[intentional]`, low (v138). v138 aborts the running command list when an untrapped SIGINT is observed (`sigint_flag` set). bash's wait-path abort keys specifically on the foreground CHILD being terminated by SIGINT (`WTERMSIG==SIGINT`), not merely on the shell receiving a SIGINT during the wait. The narrow divergence: `seq 1 3 | while read x; do echo $x; kill -INT $$; done; echo after` — here `kill -INT $$` (a) signals the PARENT shell pid (`$$` is unchanged inside the pipeline subshell), while the pipeline's `while` subshell exits NORMALLY. bash keeps the parent's pending SIGINT from aborting (the job wasn't interrupted) and runs `after` (rc 0); huck sees `sigint_flag` set after the pipeline and aborts (`after` suppressed, rc 130). This requires `kill -INT $$` from inside a pipeline subshell — pathological. For a REAL interactive Ctrl-C the signal goes to the foreground job's process group (the job dies via SIGINT → huck's `WTERMSIG==SIGINT` trigger aborts, matching bash); and for `-c`/script the shell and children share the foreground pgroup so both receive it. So the in-scope v138 behavior matches bash; only this synthetic parent-only-SIGINT-during-a-surviving-job case diverges. Matching bash exactly would require gating the wait-path abort on the job's own SIGINT-termination rather than on the shell's flag.

- **L-34: `mapfile`/`read` unimplemented flags + two `mapfile` edges** — `[deferred]`/`[low]` (v140). v140 ships `mapfile`/`readarray` with `-t -d -n -O -s` (+ default `MAPFILE`) and `read -a`. NOT YET implemented: `mapfile -u FD`/`-C callback`/`-c quantum`, and `read -n`/`-N`/`-t`/`-u` (nchars/timeout/fd). `-C`/`-c` need callback eval; `-u` needs reading from an arbitrary fd; rare in practice, deferred. Two minor edges in the shipped set: (a) a malformed numeric option arg (`mapfile -n xyz`) exits rc 2 with `huck: mapfile: xyz: invalid number`, vs bash rc 1 `invalid line count` (program-name-prefix class + a pathological-input rc); (b) a high-byte raw delimiter `-d $'\xff'` doesn't split (the `0xFF` becomes U+FFFD through huck's UTF-8 `String` word model and never matches the stream byte) — inherited from the general non-UTF-8-byte limitation (L-04/L-11 class), not specific to mapfile; multi-byte UTF-8 delimiters split on the first byte like bash. All common usage matches bash (12/12 bash-diff harness).

- **L-35: `command builtin <decl>` (a `command`-led nest wrapping a declaration builtin) errors instead of running** — `[intentional]`, low (v142). v142 adds the `builtin NAME [args]` builtin. huck correctly peels `builtin`-led nests around a declaration builtin (`builtin builtin local x=5`, `builtin command local x=5` both run and print the assignment). But any nest where a `command` wrapper sits immediately outside `builtin <decl>` — `command builtin local x=5`, and also the builtin-led `builtin command builtin local x=5` (the outer `builtin` is peeled, leaving `command builtin local`) — surfaces post-resolve with `decl_args` already discarded, so huck prints `huck: builtin: local: declaration builtins must not be wrapped by \`command builtin\`` and returns rc 1, whereas bash runs it (prints `x=5`). Maximally pathological — no real script nests `command builtin` around a declaration builtin; huck errors cleanly (rc 1, no panic) rather than running it. Matching bash would require carrying `decl_args` through the `command`-led resolution path.

- **L-36: `complete -o nospace` is a no-op (no default trailing space after completion)** — `[deferred]`, low (v143). huck never appends the trailing space bash adds after completing a final (non-directory) word — rustyline (`CompletionType::List`) inserts the replacement verbatim; the only append is `/` for directories. So `complete -o nospace` has nothing to suppress (it parses into `CompOptions.nospace` but is unread at tab-dispatch). Honoring nospace meaningfully would first require implementing bash's default trailing-space behavior. Low impact: the directory-descend flow is unaffected (`cd dir/<TAB>` already adds no space).

- **L-37: indexed subscripted array element with a literal brace** — `[deferred]`, low (v144). v144 brace-expands BARE array-literal elements. An INDEXED subscripted element whose value contains a literal brace — `a=([2]=x{a,b})` — keeps the value literal in huck (`a[2]="x{a,b}"`), whereas bash brace-expands the whole `[i]=…` word into BARE literals dropping the subscript (`[0]="[2]=xa" [1]="[2]=xb"`). ASSOCIATIVE subscripts (`declare -A m=([k]=x{a,b})`) keep the brace literal in BOTH shells (huck matches bash). Pathological; bash's own indexed-vs-associative behavior here is surprising. Low impact.

- **L-38: brace expansion ordering vs parameters and scalar assignments** — `[deferred]`, low (v144; pre-existing, command-word path). Two related spots where huck's brace expansion diverges from bash's textual-first model: (a) a brace FOLLOWING a parameter — `v1=A v2=B; echo $v{1,2}` — bash expands `$v{1,2}`→`$v1 $v2` textually FIRST → `A B`, huck expands `$v` first → `1 2`; (b) a scalar assignment RHS — `v={1,2}; echo "$v"` — bash assigns the literal `{1,2}` (no brace expansion on a scalar assignment RHS), huck brace-expands the assignment word (`v=1 v=2`) leaving `v=2` (`x={a,b}` → `b`). Both pre-existing (NOT introduced by v144's array-element brace expansion, which is correct); surfaced during v144 review. Low/rare.
- **L-39: process-substitution edge cases** — `[deferred]`, low (v150). The v150 process substitution `<(…)`/`>(…)` covers command-argument and redirect-target usage (foreground + pipelines + compound commands + background). Three residual edge gaps: (a) **assignment-RHS context** — `x=<(cmd)` is NOT realized (the `expand_assignment` path is a no-op for `ProcessSub`); bash assigns `/dev/fd/N`. Realizing there would fork a child with no command to consume the fd. (b) **setup-failure path** — if `pipe()`/`fork()` fails while realizing a process sub, huck prints an error and emits an EMPTY field, so the outer command still runs (with an empty arg / failing-open redirect); bash aborts the command on process-sub setup failure. Only reachable under fd/process exhaustion. (c) **background long-running inner producer** — `cmd < <(slow_producer) &` reaps the inner via `waitpid(WNOHANG)` after spawning the bg job (to avoid blocking `&`), so a still-running inner producer leaves a bounded zombie until SIGCHLD/shell-exit (its fd IS closed — no fd leak). Also: the FIFO fallback (`/dev/fd` absent) is verified by inspection only — unreachable on Linux/macOS, which both provide `/dev/fd`.
- **L-41: computed builtin-variable edge cases** — `[deferred]`, low (v154). v154 added the shell-maintained builtin variables. The STATIC ones (`UID`/`EUID`/`PPID`/`GROUPS`/`HOSTNAME`/`HOSTTYPE`/`OSTYPE`/`MACHTYPE`/`BASH`/`BASH_VERSION`/`BASH_VERSINFO`/`HUCK_VERSION`/`SHLVL`) are stored in the vars table and match bash; the DYNAMIC ones (`RANDOM`/`SECONDS`/`EPOCHSECONDS`/`BASHPID`) are computed in `lookup_var` and therefore NOT in the vars table, which yields several edge divergences: (a) **`set`/`declare -p` omit them** — bare `set` and `declare -p RANDOM` don't list/print the computed dynamics (bash does); they DO complete via `compgen -v`/`$<TAB>` (the v154 completion registry). (b) **`[[ -v RANDOM ]]` is false** — `is_set` checks the vars table, so `-v` on a computed dynamic reports unset (bash: set). (c) **`BASHPID`/`EPOCHSECONDS` assignment** silently stores a shadowed ghost (the computed value still wins on read) rather than erroring as bash's readonly does. (d) **inline-assignment scoping** — `RANDOM=n cmd` / `SECONDS=n cmd` reseed/reset GLOBALLY (the reseed bypasses the vars snapshot/restore), whereas bash SCOPES the reseed to that command; standalone `RANDOM=n; …` (the common case) matches bash. (e) **`GROUPS` order** — huck returns the raw `getgroups(2)` order; bash orders egid-first (the SET matches; only order differs). (f) `RANDOM`'s LCG is huck's own — only range (0–32767) and reseed-determinism are guaranteed, not bash's exact sequence; `BASH_VERSION`/`BASH_VERSINFO` are a deliberate bash masquerade (`HUCK_VERSION` is the true identity). All low-impact; the common read/assign/completion paths match bash.

### L-08: Redirect source-order not preserved on PIPELINE STAGES
- **Status**: `[deferred]` low
- **Severity**: low
- **huck**: v156 RESOLVED the source-order bug for SINGLE commands — bare builtins, external commands, compound commands (brace groups, subshells, if/for/while/case), functions, and `exec` all now process redirects in strict left-to-right order via an ordered `Vec<Redirect>` list, so `cmd 2>&1 >file` (puts stderr to terminal, stdout to file) and `cmd >file 2>&1` (both to file) are correctly distinguished on a single command. The gap REMAINS for PIPELINE STAGES: the fast-path `slots_for_simple_path` used to set up 0/1/2 for pipeline stages applies redirects in a last-wins fashion rather than preserving source order, so `cmd1 2>&1 >file | cmd2` may not produce the same result as bash. Additionally, a heredoc or here-string targeting fd>2 on a pipeline stage is dropped (the pipeline stage setup path does not plumb fd>2 heredocs). Two residual edges in this narrow scope:
  - **Pipeline-stage ordering**: `cmd 2>&1 >file | …` — stage-level redirect setup is last-wins for fds 0/1/2.
  - **fd>2 heredoc on a pipeline stage**: `cmd 3<<EOF … | …` — the heredoc for fd>2 is silently discarded on a pipeline stage.
- **bash**: processes all redirects in strict left-to-right order on every command, including pipeline stages.
- **Why deferred**: the pipeline-stage fast-path (`slots_for_simple_path`) would need to be replaced with the same ordered-redirect walk used for single commands; the heredoc-on-fd>2-in-pipeline gap would require extending the pipeline setup to pass through fd>2 heredoc fds. Single-command usage (the overwhelmingly common case) now matches bash.
- **Workaround**: write `cmd >file 2>&1` (canonical form, correct on both single commands and pipeline stages); avoid fd>2 heredocs on pipeline stages.

### L-09: Regex `=~` is RE2-style, not POSIX ERE
- **Status**: intentional (v30)
- **Severity**: low
- **huck**: `[[ $s =~ regex ]]` uses the Rust `regex` crate (RE2-based). No lookbehind / lookahead (`(?<=...)`, `(?=...)`); minor syntax differences from POSIX ERE for some edge cases (e.g., `(?:...)` non-capturing groups are supported in both, but bash's POSIX-mode is stricter).
- **bash**: POSIX ERE. Has its own quirks.
- **Why intentional**: `regex` is a mature, fast, well-maintained Rust crate. Implementing POSIX-ERE-faithful regex isn't worth the cost for the rare divergences. Most real-world shell-regex usage works identically.
- **Workaround**: if a script relies on POSIX-ERE-specific features, fall back to `grep -E "pattern"` (which uses libc's POSIX ERE).

### L-11: `$'\xHH'` and `$'\nnn'` produce Unicode codepoints, not raw bytes

Bash inserts the raw byte value (0x00–0xFF) directly into the output
string. huck, whose strings are Rust `String` (UTF-8), interprets the
numeric value as a Unicode codepoint via `char::from_u32`. For ASCII-range
values (< 0x80) the two encodings are bit-identical. For high-bit values
the divergence is visible: bash's `$'\xFF'` is a single byte (`0xFF`),
huck's `$'\xFF'` is the two-byte UTF-8 encoding of U+00FF
(`0xC3 0xBF`).

This aligns with L-04 (huck's Unicode-by-default convention for parameter
expansion). Scripts that depend on injecting raw high bytes via
`$'\xHH'` — rare in practice — will see different output sizes.
Surrogate-range escapes (`\uD800`..`\uDFFF`) and codepoints above
U+10FFFF are rejected with a `LexError::AnsiCInvalidCodepoint` rather
than silently producing invalid UTF-8.

### L-15: sourced-file syntax error points at the logical command's first line

- **Status**: `[intentional]` (v94)
- **Severity**: low
- **huck**: a lex or parse (syntax) error raised while reading a sourced file (`source`/`.`, `--rcfile`, script-file mode) is reported as `huck: FILE: line N: syntax error: MSG`, where `N` is the physical line on which the offending logical command STARTED (`cmd_start_line` in `run_sourced_contents`). For a multi-line construct (a function body, a continued `if`, a `case`, etc.) that is the construct's opening line, not the line containing the offending token.
- **bash**: reports the physical line of the offending token itself, so for multi-line constructs bash's `line N` is later than huck's.
- **Why intentional**: huck parses a whole logical command (gathering continuation lines) before it can report a syntax error, so the precise within-command token line is not available without per-`Token` position tracking. The first-line report is accurate for the overwhelming majority of cases (single-line commands, where the two coincide) and is enough to locate the failing construct. Exact token `line:col` reporting is deferred (needs per-`Token` position tracking). Note: in huck unterminated quotes/braces become continuation rather than lex errors, so most user-visible cases are parse errors.
- **Workaround**: none needed — the reported line locates the failing construct; inspect from that line forward.

---

### L-16: `${!…}` / `[[ ]]` error-text divergences (stderr only)

- **Status**: `[intentional]` (v95)
- **Severity**: low
- **huck**: in the v95 `${!var}` indirect-expansion (M-91) area, three error cases produce the right exit code and stdout but a different stderr message than bash (the established program-name-prefix class — `huck:` vs `bash:`): (a) a SET-but-empty source (`ref=""; ${!ref}`) → huck `huck: ref: invalid indirect expansion` vs bash `bash: : invalid variable name`; (b) a `set -u` unbound effective name → huck `huck: {N}: unbound variable` vs bash `!ref: unbound variable`; (c) a malformed / space-containing effective name (`ref="a b"; ${!ref}`) → huck silently yields empty (rc 0) where bash also yields empty but additionally prints `invalid variable name` to stderr.
- **bash**: as quoted above — the diagnostics name the bare/offending variable and, in case (c), emit a diagnostic huck omits.
- **Why intentional**: rc and stdout match bash byte-for-byte in every case; only the human-facing error TEXT differs (same class as L-13/L-15's `huck:`-vs-`bash:` prefix divergence). Matching bash's exact wording would buy nothing for scripts that key off rc/stdout.
- **Workaround**: none needed for rc/stdout-driven scripts.

---

### L-17: `${var@OP}` scalar-transform edge divergences

- **Status**: `[intentional]` (v96)
- **Severity**: low
- **huck**: three edge divergences in the v96 `${var@OP}` scalar transforms (M-86): (a) `@P` reuses `expand_prompt`, which always expands `$VAR`/command-substitution, whereas bash suppresses those in `@P` when `shopt -u promptvars` (default is ON, and backslash-escape processing matches; oh-my-posh's pre-rendered ANSI value has no `$VAR` so it is unaffected); (b) `@U`/`@u` non-ASCII case mapping inherits the pre-existing `case_modify` Rust `to_uppercase` quirk (e.g. `straße`@U → `STRASSE` vs bash `STRAßE`) — the SAME behavior as the existing `${v^^}`/`${v^}` modifier (M-17), not a v96 regression; (c) `@Q` of a high / non-UTF-8 byte renders char-wise (`$'\xff'`@Q → `'ÿ'`) rather than bash's byte-wise `$'\377'`, the same char-vs-byte gap as L-11 — the value still round-trips.
- **bash**: as described above — promptvars-gated `@P` $VAR suppression; locale/byte-faithful `@U`/`@Q`.
- **Why intentional**: (a) requires plumbing the `promptvars` shopt into prompt expansion for a default-on, rarely-unset option; (b)/(c) are pre-existing UTF-8/char-based architecture choices shared with M-17 and L-11, not new to `@OP`. In every common case (pre-rendered prompt values, ASCII text) output matches bash.
- **Workaround**: none needed for the oh-my-posh `@P` path or ASCII transforms.

### L-18: `&` inside a captured subshell / command-substitution ordering

- **Status**: `[deferred]` (a) / `[intentional]` (b), both low (v98)
- **Severity**: low
- **huck**: two capture-context `&` edges, neither introduced by v98's M-95 work. (a) **Nested `&` inside a subshell within `$(…)`**: `x=$( ( echo n1 & wait ) )` yields `[]` (empty) in huck vs `[n1]` in bash. `execute_capturing` rewrites only TOP-LEVEL `Amp`→`Semi` and does not recurse into `Subshell` bodies, so a `&` *inside* a captured subshell still spawns a real background child that writes outside the capture buffer; the `wait` then sees no in-buffer output. Confirmed byte-identical on the parent commit — pre-existing, not a v98 regression. (b) **Command-substitution `&` ordering**: `$( a & b )` runs synchronously in source order in huck (the documented capture-context "ignore `&`" design — see the `execute_capturing` top-level `Amp`→`Semi` rewrite) rather than backgrounding `a`; the output *content* matches bash, but interleaving/ordering may differ when `a` would have overlapped `b`.
- **bash**: (a) the inner `&`'d command writes into the captured output, so `wait` collects `n1`; (b) `a` is genuinely backgrounded inside the substitution.
- **Why low / intentional**: (a) requires recursing the top-level `Amp`→`Semi` rewrite into nested `Subshell` bodies inside a capture (a `[deferred]` refinement of `execute_capturing`); (b) is the intentional capture-context design — backgrounding inside a substitution that synchronously drains stdout has no observable benefit and risks lost output, so huck serializes. Real scripts rarely background inside `$(…)`; both edges are pathological.
- **Workaround**: avoid backgrounding inside `$(…)`; run the `&`'d command at the top level and capture its output by other means.

### L-19: `command CMD` bare-form edges (`-p` PATH, `command declare -a`, function named `command`)

- **Status**: `[intentional]`, all low (v99)
- **Severity**: low
- **huck**: three edges in the v99 `command CMD` bare form (M-85): (a) `command -p CMD` resolves CMD via the CURRENT `$PATH`, not bash's guaranteed default PATH (`getconf PATH` / a "standard utilities" path); huck has no separate default-PATH search, so `-p` is accepted but effectively a no-op over the live `$PATH`. (b) `command declare -a a=(x y z)` (a compound array RHS reached via `command`): bash REJECTS it as a syntax error, but huck ACCEPTS it (the pre-resolve declaration-builtin reconstruction assigns the array) — a no-panic SUPERSET. Scalar `command export X=1` matches bash. (c) A user FUNCTION named `command` (`command() { …; }`) cannot shadow the builtin in huck — the interception is unconditional and runs BEFORE function lookup — whereas bash lets the function take precedence. The same holds for a function named `builtin` (added v142): the `builtin` pre-resolve interception also runs before function lookup, so `builtin() { …; }` cannot shadow the `builtin` builtin either.
- **bash**: (a) `-p` searches a guaranteed default PATH; (b) `command declare -a a=(…)` is a syntax error; (c) a function named `command` (or `builtin`) shadows the corresponding builtin.
- **Why intentional**: (a) huck has no built-in default-PATH constant; the live `$PATH` covers every real use of `-p` (recovering a real command past a shadowing function). (b) accepting the array assignment is strictly more permissive and panic-free — the only divergence is huck succeeding where bash errors on a pathological input. (c) POSIX discourages naming a function `command`; the unconditional interception is what makes the bare form reliably bypass functions in the first place.
- **Workaround**: (a) none needed — set `$PATH` explicitly if a default-PATH search is required; (b) avoid `command declare -a` for array RHS; (c) do not name a function `command`.

### L-20: `case`-pattern bare `)` inside a command substitution

- **Status**: `[deferred]`, low (v101)
- **Severity**: low
- **huck**: a `case` statement whose pattern's terminating `)` sits inside a command substitution — `x=$( case y in a) echo hit;; esac )` — closes the `$(…)` early at the pattern's `)`. The `$(…)` body scanner `scan_paren_substitution` (`src/lexer.rs`) counts parens; v101 (M-97) made it balance `(`/`)` pairs correctly for subshells and nested `$((…))`, but a `case`-pattern terminator is an UNMATCHED `)` (no opening `(`) at depth 0, so the naive counter treats it as the command-sub's closing `)` and truncates the body. **Pre-existing** — not introduced or worsened by v101 (v101 only added depth-increment on a bare `(`; the unmatched-`)` case was wrong before and after).
- **bash**: parses the command-sub body with the full recursive grammar, so it recognizes the `case`-pattern `)` as a pattern terminator (not the command-sub close) and the substitution ends at the real `)` after `esac`.
- **Why low / deferred**: a `case` statement INSIDE a command substitution that itself uses the unparenthesized `a)` pattern form is rare; distinguishing a pattern-terminator `)` from the command-sub close requires full recursive command-grammar parsing inside the lexer's body scanner (huck's scanner is deliberately a paren-counter). The parenthesized pattern form `(a)` inside `$(…)` balances correctly post-M-97.
- **Workaround**: parenthesize the `case` pattern (`(a) echo hit;;`), or assign the `case` to a variable outside the substitution.

### L-21: `set -x` (xtrace) trace-format divergences

- **Status**: `[intentional]`, all low (v103; narrowed v130 — per-word arg quoting, inline + bare assignment prefixes, and `command`/`local`/`declare` arg tracing now match bash; v131 — PS4 depth-repeat + `$VAR`/escape expansion now match bash)
- **Severity**: low
- **huck**: four residual differences from bash's xtrace output, none of which affect the diagnostic value (every executed command still prints one trace line). (a) **Finer compound traces not emitted**: bash separately traces the `for`-iteration variable set on each loop pass, the `case` word, and the `[[ ]]` / `(( ))` test/arith conditions; huck traces the inner simple commands but not these per-construct lines. (b) **Decl-RHS-with-command-substitution edge**: a `local`/`declare`/`export` RHS that contains a command substitution (e.g. `local x=$(cmd)`) is traced with the EXPANDED value, which re-runs the command substitution for the trace (double-execution) — rare. (c) Per M-90, `2>/dev/null` does NOT suppress the trace (builtin/executor stderr ignores `2>` — consistent with M-90). (d) **Pipeline-stage trace ORDER is best-effort**: the set of trace lines matches bash, but in-process (builtin/function) stages trace from a forked child while external stages trace from the parent, so the left-to-right ORDER of trace lines in a mixed pipeline may differ from bash (the lines themselves are identical).
- **bash**: emits finer compound-command traces; traces a decl RHS without re-executing a command substitution; left-to-right pipeline-stage trace order; `2>` can redirect the trace.
- **Why intentional**: huck's flat single-line-per-command trace is the diagnostic tool's whole point — it pinpoints the next command to run (e.g. a hang). Finer per-construct compound traces are cosmetic for that purpose; the decl-RHS-cmdsub double-execution and pipeline-stage ORDER edges are inherited from existing architecture (the trace site sees the expanded decl word; in-process vs external stages trace from different processes); the `2>` gap is M-90's stderr-sink limitation. Every command still emits exactly one trace line.
- **Workaround**: none needed for diagnostic use; the trace still shows each command before it runs.

### L-22: linear source-reader unit-boundary / resync edges

- **Status**: `[intentional]`, both low (v104)
- **Severity**: low
- **huck**: two edges in the v104 O(n) script source reader (M-99), both confined to already-divergent verbose / error paths. (a) **Trailing top-level `;`/`&` before a newline**: a trailing top-level `;` or `&` immediately before a newline (e.g. `set -v ;⏎cmd`) groups with the NEXT command into one parsed unit — only `&&`/`||`/`;`-on-a-line and bare top-level newlines bound units — so `set -v` / `set +v` taking effect via such a trailing-separator-then-newline line may echo one fewer / more line than bash. (`set -v; cmd` on ONE line already matched bash; only the rare trailing-`;`-then-newline differs.) Execution OUTPUT is unaffected — only the `set -v` echo of that boundary. (b) **Post-syntax-error resync**: after a syntax error the reader skips to the next top-level newline (the token-stream analogue of the old "clear the buffer, continue at the next line" recovery), so the cascade AFTER a syntax error may differ slightly from the old per-line resync.
- **bash**: (a) `set -v` echoes exactly the physical lines as read; (b) bash's own error-recovery boundaries.
- **Why intentional**: both are negligible and only affect the already-divergent-from-bash verbose / error edges; unit boundaries are intentionally `&&`/`||`/`;`-on-a-line / top-level-newline so the one-command-at-a-time linear reader (M-99) stays O(n). Normal execution output is identical.
- **Workaround**: none needed; put `set -v` / `set +v` on its own line (no trailing `;`/`&`) for an exact bash-matching echo boundary.

### L-23: quoted substring of an `=~` regex is not matched literally

- **Status**: `[intentional]`, low (pre-existing, noted v105)
- **Severity**: low
- **huck**: bash matches a *quoted* substring of an `=~` regex operand LITERALLY — quoting escapes the regex metacharacters in that substring (e.g. in `[[ $x =~ "a.b" ]]` the `.` is a literal dot, not "any char"). huck expands the operand `Word` (M-100's `scan_regex_operand` keeps quotes as ordinary word quoting) and passes the resulting string to `regex::Regex`, so a quoted metacharacter stays ACTIVE (`.` still matches any char). This is PRE-EXISTING (the operand has always been expanded as a word) and is NOT introduced or worsened by M-100's regex-operand lexing.
- **bash**: a quoted span inside an `=~` regex is treated as a literal string (regex metacharacters within it are escaped).
- **Why intentional**: bash_completion's regexes (the M-100 driver) use `\`-escapes for literal characters, not quoting-for-literal-match, so this gap doesn't affect the real-world payload; matching it would require tracking quote spans through expansion and re-escaping them for the regex engine.
- **Workaround**: use a `\`-escape (`\.`) rather than quoting (`"."`) to match a regex metacharacter literally inside `=~`.

### L-25: a builtin's `2>&1` inside a capture context can't capture stderr

- **Status**: `[intentional]`, low (noted v109)
- **Severity**: low
- **huck**: a builtin's `2>&1` inside a CAPTURE context (`r=$(declare -p X 2>&1)`) does not capture the builtin's stderr. The M-90 redirect guard dup2's the dup-target onto the real fd 2, but a Capture sink writes the builtin's stdout to a Rust buffer (not real fd 1), so fd-level `2>&1` can't reach it. The file/pipe cases (`2>file`, `2>&1 | cmd`) work. Also applies to a function-call's `2>&1` under capture (`r=$(func 2>&1)`) — same in-memory-buffer cause; v125's function-call redirects fixed the divert/suppress directions but not capture-of-stderr.
- **bash**: the builtin's stderr is merged into the captured stdout.
- **Why intentional**: capturing a builtin's stdout via a Rust buffer (rather than a real fd 1) is the design that makes `$(builtin …)` work without forking; a real `dup2(1,2)` has no in-buffer fd 1 to target. The file/pipe redirect cases (the common ones) are correct.
- **Workaround**: redirect the builtin's stderr to a file and read the file, or run the builtin in a forked subshell.

### L-26: `getopts` verbose error messages use huck's program-name prefix

- **Status**: `[intentional]`, low (noted v111)
- **Severity**: low
- **huck**: `getopts` verbose error messages match bash's body (`illegal option -- c` / `option requires an argument -- c`) but use huck's `huck:` prefix instead of bash's `$0`/script-name prefix — stderr text only. The `name`/`OPTARG`/`OPTIND`/rc are byte-identical to bash.
- **bash**: prefixes the same message body with `$0` (or the script name).
- **Why intentional**: the same program-name-prefix class as L-13/L-15/L-16 (`huck:` vs `bash:`); rc and the `name`/`OPTARG`/`OPTIND` outputs that scripts key off match bash exactly.
- **Workaround**: none needed for rc/variable-driven scripts; use the silent error mode (leading `:` in the optstring) to suppress the message entirely.
