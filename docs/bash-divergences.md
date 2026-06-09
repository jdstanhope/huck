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
| Bugs (Tier 1) | 1 | Open bug to fix (M-114). |
| Missing features (Tier 2) | 25 | Deferred bash-compat backlog, ranked by severity within each group. |
| Intentional (Tier 3) | 9 | Deliberate divergences we're keeping. |
| Low-impact (Tier 4) | 24 | Open edge cases / cosmetic divergences (`[low]`/`[intentional]`/`[deferred]`). |

---

## Tier 1: Bugs

huck behaves wrong without a design reason; should be fixed.

### M-114: array literal as a command argument (`eval x=(…)` unescaped) panics
- **Status**: `[deferred]` (found v117)
- **Severity**: medium (a panic/abort, but off the common path)
- **huck**: an array literal `name=(…)` as a command ARGUMENT (not a leading assignment) — e.g. `eval x=(a b)` with UNESCAPED parens — reaches `expand()` as a parser-internal `WordPart::ArrayLiteral` and panics (`internal error: … must not reach expand(); try_split_assignment is supposed to consume it`, `src/expand.rs`). `try_split_assignment` only consumes the literal when it's the command's leading assignment.
- **bash**: treats `x=(…)` specially even as an argument and does not error.
- **Workaround / why low-urgency**: the real `_upvars` (and most code) ESCAPE the parens (`eval $2=\(…\)`), which lexes as a plain word and works; quoted `eval "x=(a b)"` also works. Only literal unescaped `cmd name=(…)` triggers it.
- **Next**: make a command-argument `ArrayLiteral` expand to its reconstructed `name=(…)` text (or otherwise not reach `expand()`). Own iteration.

---

## Tier 2: Missing bash features

Bash features huck doesn't implement. Listed roughly by impact within each
group.

### Functions & scoping

- **M-09a: Relaxed function-name characters** — `[deferred]` low. huck restricts function names to POSIX identifiers (`[A-Za-z_][A-Za-z0-9_]*`) in BOTH the `name() body` and `function name body` forms. Bash 5 accepts `.`, `-`, `+`, `:` and other non-POSIX-identifier characters when the function is defined via the keyword form (`function foo.bar { :; }`). Rarely used in practice.
- **M-09b: Definition-attached redirections** — `[deferred]` low. Both function-definition forms (`name() body > file` and `function name body > file`) currently reject trailing redirections. Bash allows attaching redirections to the function definition itself, taking effect at every call. Affects both forms equally.

### Compound commands

- **M-14b: `[[ -v arr[i] ]]` array-element form** — `[deferred]` low. `-v` supports
  scalar names and positional parameters; the array-element subscript form
  (`-v arr[1]`) is not parsed — the name falls through to a plain-name lookup
  (→ false). bash checks the specific element. Rarely used; bash-completion's
  `-v` uses are plain names.

### Parameter expansion modifiers

- **M-15b: operand quote/escape context not propagated** — `[deferred]` low. Two pre-existing divergences (NOT introduced by v84) when a `${...}` is itself inside double quotes: (a) single quotes in its operand are stripped by huck but kept literal by bash (`"${y:-'a|b'}"` → bash `'a|b'`, huck `a|b`); (b) a backslash-escaped char in an operand drops the backslash where bash keeps it (`"${y:-\*}"` → bash `\*`, huck `*`). Root cause: `parse_braced_operand` receives only the extracted operand body, not the enclosing quote context.
- **M-93: `${var@OP}` array/attribute transforms (`@A`/`@K`/`@k`/`@a`)** — `[deferred]` low. huck: the assignment-statement form `@A`, the key/value array forms `@K`/`@k`, and the attribute-flags form `@a` error (unsupported `@`-operator). bash: `@A` reproduces a `declare`-style assignment string, `@a` lists attribute flags, `@K`/`@k` expand associative-array key/value pairs. M-86 follow-on; the scalar transforms (`@P`/`@Q`/`@U`/`@L`/`@u`/`@E`) shipped in v96.

### Redirects

- **M-20: `n<>file` read-write open** — `[deferred]` low. huck: not implemented. bash: opens fd for read+write.
- **M-21: `>|` and `noclobber`** — `[deferred]` low. huck: no `set -o noclobber`, no `>|`. bash: `noclobber` blocks overwriting; `>|` forces.
- **M-51: `|&` pipe stdout+stderr** — `[deferred]` low. huck: parse error. bash: shorthand for `2>&1 |`.

### Quoting

- **M-29: `$"…"` locale quoting** — `[deferred]` low. huck: parses as `$` + double-quoted word. bash: gettext lookup.

### Job control

- **M-41: Limited signal name set** — `[deferred]` medium. huck: 15 names (no SEGV/ABRT/FPE/BUS/ILL/TRAP/…). bash: full platform signal set.
- **M-42: `kill` with negative PID** — `[deferred]` low. huck: rejects. bash: passes to `kill(2)` as a pgrp / wildcard target.
- **M-96: first-class nested and-or AST (`list → and_or → pipeline → command`)** — `[deferred]` low. M-95 follow-on. v98 keeps the flat `Sequence` model with executor-side grouping (`partition_into_groups` + `run_andor_group`), which is correct and low-risk. A future first-class `list → and_or → pipeline → command` AST may eventually be wanted to express `time` on a whole group, per-group traps, and cleaner pipeline-status propagation — none of which the flat model represents natively. Not user-visible today; logged so the structural debt is tracked.

### Builtins (other)

- **M-26: `test -v VAR`** — `[deferred]` medium. huck: not implemented. bash: tests if a variable is set.
- **M-27: Other `test` operators** (`-p`/`-S`/`-b`/`-c`/`-O`/`-G`/`-N`/`-k`/`-u`/`-g`/`-t`) — `[deferred]` medium. huck: only `-e`/`-f`/`-d`/`-r`/`-w`/`-x`/`-s`/`-L`. bash: full set.
- **M-32: `cd -P` / `-L`** — `[deferred]` medium. huck: flags rejected. bash: physical/logical mode.
- **M-33: `pwd -P` / `-L`** — `[deferred]` low. huck: flags silently passed through. bash: physical/logical.
- **M-36b: system-data completion actions** — `[deferred]` low. `compgen -A
  hostname`/`user`/`group`/`service` are recognized but return nothing; bash reads
  `/etc/hosts`(`$HOSTFILE`)/`/etc/passwd`/`/etc/group`/`/etc/services`. Rarely the
  decisive completion source; deferred to avoid new filesystem/libc lookups.
- **M-46: `history -d`/`-w`/`-r`/`-a` flags** — `[deferred]` low. huck: only `-c`. bash: full set.
- **M-47: `history N`** — `[deferred]` low. huck: rejects numeric arg. bash: prints last N entries.
- **M-48: `export -p`/`-n`** — `[deferred]` medium. huck: flags treated as variable names. bash: `-p` lists, `-n` unexports.
- **M-92: prefix-name `${!prefix@}` / `${!prefix*}`** — `[deferred]` low.
  The variable-NAME-listing forms of `${!…}` (expand to the names of all
  set variables whose name begins with `prefix`) are not implemented —
  the lexer's `${!` branch handles only the scalar-indirect form (M-91).
  Not used by the bashrc / bash-completion; deferred. M-91 follow-on.
- **M-102: array-literal element word-splitting** — `[deferred]` medium. huck: an array literal whose element is an unquoted command substitution or variable expansion — `a=($(cmd))`, `a=($(echo "x y" z))`, `a=($var)` — produces ONE element containing the whole result string. bash: the multi-word result is word-split on `$IFS` into SEVERAL array elements (`a=($(echo a b c))` → 3 elements). Pre-existing (NOT introduced by v106); surfaced while writing the v106 M-101 array-literal tests (which therefore use single-word globs to sidestep it). Fix needs the array-literal element builder to run IFS field-splitting on each unquoted expansion result, reusing `emit_split_fields` (M-05).
- **M-107: `FUNCNAME` inside function bodies** — `[deferred]` low. huck: `$FUNCNAME` (and `${FUNCNAME[…]}`) is empty inside a function; bash sets it to the call-stack array (`FUNCNAME[0]` = the current function name). Surfaced as the blank `:` in bash_completion's `bash_completion: : \`-n'` diagnostic (`$FUNCNAME` empty), though that branch is no longer reached once `getopts` works (M-106). bash_completion reads `${FUNCNAME[…]}` in a few other diagnostics. Fix needs a per-call function-name stack exposed as the `FUNCNAME` array (huck already pushes `function_arg0` in `call_function` — the array surface from M-82 makes the variable wiring feasible).

### Globbing

- **M-53: `**` globstar** — `[deferred]` low. huck: `**` ≡ `*`. bash: `shopt -s globstar` makes `**` match across `/`.

### History

- **M-59: `HISTSIZE` / `HISTFILESIZE` env vars** — `[deferred]` medium. huck: compile-time `HISTORY_MAX = 1000`. bash: reads env vars.

---

## Tier 3: Intentional divergences

Things huck deliberately does differently from bash. Document and keep.

### I-01: `cd` always sets the physical PWD
- **Status**: intentional
- **Severity**: medium
- **huck**: after `cd symlink`, `PWD` is the canonical path (`std::env::current_dir()`).
- **bash**: defaults to logical PWD (the path you typed, through symlinks).
- **Why**: simpler implementation; canonical paths are less surprising for cross-language tooling.

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

### L-08: Redirect source-order not preserved (`2>&1 >file` anti-pattern)
- **Status**: intentional (v29)
- **Severity**: low
- **huck**: `cmd 2>&1 >file` is treated identically to `cmd >file 2>&1` — both fds end up at the file. The field-based `ExecCommand` AST (`stdin`/`stdout`/`stderr`) stores at most one redirect per fd and cannot preserve source order.
- **bash**: `cmd 2>&1 >file` puts stderr to the terminal and stdout to the file (because `2>&1` dups stderr to the CURRENT stdout, which is the terminal at that point, then `>file` redirects stdout). The canonical form is `cmd >file 2>&1`.
- **Why intentional**: source-order preservation requires refactoring `ExecCommand` to `redirects: Vec<(SourceFd, Redirect)>` — a substantial change. The canonical form covers >99% of real usage.
- **Workaround**: write `cmd >file 2>&1` (or `cmd &>file`).

### L-09: Regex `=~` is RE2-style, not POSIX ERE
- **Status**: intentional (v30)
- **Severity**: low
- **huck**: `[[ $s =~ regex ]]` uses the Rust `regex` crate (RE2-based). No lookbehind / lookahead (`(?<=...)`, `(?=...)`); minor syntax differences from POSIX ERE for some edge cases (e.g., `(?:...)` non-capturing groups are supported in both, but bash's POSIX-mode is stricter).
- **bash**: POSIX ERE. Has its own quirks.
- **Why intentional**: `regex` is a mature, fast, well-maintained Rust crate. Implementing POSIX-ERE-faithful regex isn't worth the cost for the rare divergences. Most real-world shell-regex usage works identically.
- **Workaround**: if a script relies on POSIX-ERE-specific features, fall back to `grep -E "pattern"` (which uses libc's POSIX ERE).

### L-10: `${var:…}` and `${var/…/…}` mis-split on `:` or `/` inside command substitutions
- **Status**: intentional (v33)
- **Severity**: low
- **huck**: `${s:$(echo 1:2)}` corrupts the split — the inner `:` inside `$(echo 1:2)` is treated as the offset/length delimiter, yielding offset `$(echo 1` and length `2)`. Same issue for `${var/$(cmd/with/slash)/repl}`. The `scan_braced_operand` helper handles brace depth and quoted spans but does not depth-track `$(…)` or `$((…))`, so the second-pass split scanners (`split_substring_body`, `split_substitution_body`) see the inner metacharacter at depth 0.
- **bash**: parses at the grammar level so the inner metacharacter is never visible to the split.
- **Why intentional**: real scripts almost never put a `:` or `/` literal inside a command substitution that itself sits inside a parameter expansion operand. The split scanners are simple by design, and adding `$(`/`$((` depth tracking would touch both v32 and v33 helpers for a vanishingly rare pattern.
- **Workaround**: stash the command-substitution result in a variable and reference the variable inside the operand.

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
- **huck**: three edges in the v99 `command CMD` bare form (M-85): (a) `command -p CMD` resolves CMD via the CURRENT `$PATH`, not bash's guaranteed default PATH (`getconf PATH` / a "standard utilities" path); huck has no separate default-PATH search, so `-p` is accepted but effectively a no-op over the live `$PATH`. (b) `command declare -a a=(x y z)` (a compound array RHS reached via `command`): bash REJECTS it as a syntax error, but huck ACCEPTS it (the pre-resolve declaration-builtin reconstruction assigns the array) — a no-panic SUPERSET. Scalar `command export X=1` matches bash. (c) A user FUNCTION named `command` (`command() { …; }`) cannot shadow the builtin in huck — the interception is unconditional and runs BEFORE function lookup — whereas bash lets the function take precedence.
- **bash**: (a) `-p` searches a guaranteed default PATH; (b) `command declare -a a=(…)` is a syntax error; (c) a function named `command` shadows the builtin.
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

- **Status**: `[intentional]`, all low (v103)
- **Severity**: low
- **huck**: five differences from bash's xtrace output, none of which affect the diagnostic value (every executed command still prints one trace line). (a) **Flat `$PS4`**: huck emits `$PS4` literally (default `+ `) with NO per-nesting-level character repeat (bash repeats the FIRST char of `$PS4` by call depth: `+`/`++`/`+++`) and NO `$PS4` escape / `$VAR` expansion. (b) **Inline-assignment prefix omitted**: for `VAR=v cmd`, bash traces `+ VAR=v cmd` but huck traces just `+ cmd` (a pure assignment `x=1` IS traced). (c) **Arg re-quoting**: bash re-quotes trace args containing special chars (`+ printf '%s\n' one`); huck emits them plain (`+ printf %s\n one`). (d) **Finer compound traces not emitted**: bash traces assignment commands, `for`-iteration variable sets, the `case` word, and `[[ ]]` / `(( ))`; huck traces the inner simple commands but not these (so a bare assignment `d=$-` is NOT traced — huck traces only commands with a program, plus pure-assignment-only commands). (e) Per M-90, `2>/dev/null` does NOT suppress the trace (builtin/executor stderr ignores `2>` — consistent with M-90).
- **bash**: depth-repeated `$PS4` with `$VAR`/escape expansion; traces the inline-assignment prefix, re-quotes special-char args, and emits finer compound-command traces; `2>` can redirect the trace.
- **Why intentional**: huck's flat single-line-per-command trace is the diagnostic tool's whole point — it pinpoints the next command to run (e.g. a hang). Depth-repeated `$PS4`, arg re-quoting, and finer compound traces are cosmetic for that purpose; the inline-assignment-prefix and `2>` gaps are inherited from existing architecture (the trace site sees the command after the assignment prefix is split off; M-90's stderr-sink limitation). Every command still emits exactly one trace line.
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

### L-24: a command substitution nested inside `$(( … ))` does not inherit extglob

- **Status**: `[intentional]`, low (noted v106)
- **Severity**: low
- **huck**: M-101 threads the parent's `LexerOptions` through every nested-body re-tokenization EXCEPT one: `arith_string_to_word` (which lexes an arithmetic body for `$(( … ))` / `(( … ))`) keeps its existing `pub(crate)` signature and passes `LexerOptions::default()`, so a command substitution nested inside arithmetic re-lexes with extglob OFF even when `shopt -s extglob` is on. An extglob pattern inside `$(( $(…!(x)…) ))` would therefore fail to lex.
- **bash**: extglob applies inside an arithmetic-nested command substitution too.
- **Why intentional**: a negligible edge — an extglob pattern inside a command substitution inside arithmetic does not occur in any real-world payload (bash_completion / nvm / etc.). Threading `opts` here would mean changing `arith_string_to_word`'s `pub(crate)` signature and all its callers for no observed benefit; deferred until a real case appears.
- **Workaround**: none needed in practice; lift the extglob command substitution out of the arithmetic context.

### L-25: a builtin's `2>&1` inside a capture context can't capture stderr

- **Status**: `[intentional]`, low (noted v109)
- **Severity**: low
- **huck**: a builtin's `2>&1` inside a CAPTURE context (`r=$(declare -p X 2>&1)`) does not capture the builtin's stderr. The M-90 redirect guard dup2's the dup-target onto the real fd 2, but a Capture sink writes the builtin's stdout to a Rust buffer (not real fd 1), so fd-level `2>&1` can't reach it. The file/pipe cases (`2>file`, `2>&1 | cmd`) work.
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
