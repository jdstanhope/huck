# huck v156 — arbitrary-fd (fd > 2) redirections Design

**Status:** approved design, ready for implementation plan.
**Adds:** general numbered-fd and named-fd (`{var}`) redirections — `N>file`, `N<file`,
`N>>file`, `N>|file`, `N<>file`, `N>&M`, `N<&M`, `N>&-`, `N<&-`, `{fd}>file`, `{fd}>&-` —
replacing huck's fixed stdin/stdout/stderr redirect slots with one **ordered, fd-tagged**
redirect list. Retires M-124 (no fd>2 redirects), L-08 (redirect source-order), and M-20
(`n<>file` read-write open). Foundation for **v157 (coproc)**, which needs the shell to hold
and read/write high-numbered fds.
**Branch (impl):** `v156-fd-redirections`.

## Background

huck's redirect model is fd-0/1/2-centric: the lexer has only `>`, `>>`, `<`, `>&` (stdout
dup), `2>&` (stderr dup), `&>`, `&>>`, `>|`, heredocs, and here-strings; the AST hardcodes
three `Option<Redirect>` slots (`stdin`/`stdout`/`stderr`) on `ExecCommand`, `ResolvedCommand`,
and the `Redirected` compound, plus `Redirect::Dup { fd: 1|2 }`. So `exec 3>file` parses `3`
as a command word (`huck: exec: 3: not found`), `<&8` is a syntax error, and `3>&1` cannot be
expressed. There is no way to open, hold, dup, or close a fd above 2. This blocks any coprocess
or multi-fd plumbing and leaves L-08 (`2>&1 >file` source-ordering) and M-20 (`<>`) open.

## Scope (decided)

- **Full ordered fd-tagged redirect list** (replaces the 3 fixed slots) — bash-faithful, also
  fixes L-08. (Chosen over a "slots + fd>2 overflow" model.)
- **All numeric forms**: `N>`, `N<`, `N>>`, `N>|`, `N<>`, `N>&M`, `N<&M`, `N>&-`, `N<&-`.
- **Named-fd `{var}` form** (bash 4.1+): `{fd}>file` auto-allocates a free fd ≥10, assigns the
  number to `$fd`; `{fd}>&-` closes it.
- **`exec` integration**: `exec N>file` / `exec {fd}>log` hold a fd for the shell's life;
  `exec N>&-` / `exec {fd}>&-` close it.
- Out of scope (separate concern): `read -u FD` and `coproc` itself land in **v157**.

## Section 1 — Lexer & redirect AST model

### Lexer (`src/lexer.rs`)
A redirect operator may be preceded, with NO intervening whitespace, by a fd prefix that is
either a digit-run (`3>`, `10>>`, `2<&`) or a brace-wrapped identifier (`{fd}>`, `{n}<&-`). The
lexer emits a single fd-tagged redirect token carrying:
- **target**: `Default` (no prefix), `Fd(u16)` (numeric), or `Var(String)` (`{name}`);
- **op kind**: `<` `>` `>>` `>|` `<>` `<&` `>&` (plus existing `&>`/`&>>`, heredoc `<<`/`<<-`,
  here-string `<<<`).

Tokenization rule (bash-faithful): the digit/`{…}` prefix binds ONLY when glued to the operator
and starting a token — `echo 2>&1` → fd-2 dup, but `echo 2 >&1` (space) → arg `2` + default dup,
and `file2>x` → word `file2` + `>x` (the digits are part of the preceding word). The dup source
(`1`, `-`, or `{var}`) and file targets remain the following `Word`, as today.

### Redirect AST (`src/command.rs`)
Replace the three fixed `Option<Redirect>` slots on `ExecCommand`, `ResolvedCommand`, and the
`Redirected` compound with one ORDERED `redirects: Vec<Redirection>`:

```rust
struct Redirection { fd: RedirFd, op: RedirOp }      // applied in source order
enum RedirFd  { Default, Number(u16), Var(String) }  // {name} ⇒ Var
enum RedirOp {
    File { mode: FileMode, target: Word },   // >  >>  <  <>  >|   (mode ⇒ default fd + open flags)
    Dup  { source: Word, output: bool },     // >&w / <&w   (source "-" ⇒ Close)
    Close,                                    // N>&- / N<&-  (normalized from Dup source "-")
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    HereString(Word),
}
enum FileMode { ReadOnly, Truncate, Append, Clobber, ReadWrite }  // < > >> >| <>  →  default fd 0/1/1/1/0
```

`RedirFd::Default` resolves to 0 for input ops, 1 for output ops, at apply time. Preserving
source order is what fixes L-08. The data in the old `Redirect` enum (Read/Truncate/Append/
Clobber/Heredoc/HereString/Dup) folds into `RedirOp` as shown.

## Section 2 — Parser & executor behavior

### Parser (`src/command.rs`)
`parse_simple_stage` / `parse_trailing_redirects` push each redirect token onto the ordered
`Vec<Redirection>` in source order (no last-wins merge into 3 slots). `finalize_stage` attaches
`redirects` to the `ExecCommand`; the assignment-only / empty-program check (from the recent
`VAR=val 2>err` and bare-`>file` work) becomes `redirects.is_empty()`. The `Redirected` compound
variant carries `redirects: Vec<Redirection>` the same way. Same-fd file redirects still net to
last-wins (each `dup2` overwrites); interleaved dups now order correctly (L-08).

### Executor — one ordered applier, two contexts
- **In-process** (builtins, compound commands, `exec`, the shell itself): a generalized
  `RedirectScope` (evolving today's `CompoundRedirectScope`) walks the list in order. Per entry:
  resolve the target fd (`Number(n)`→n; `Var(name)`→allocate a free fd ≥10 and assign `$name`;
  `Default`→0/1/2 by op); then open-file / `dup2(source,target)` / close / spawn-heredoc-writer;
  save the prior target fd. On Drop it restores in reverse (temporary). `exec` keeps them (no
  restore — closes the saved originals), as v155 already does.
- **External (forked)**: files are opened in the PARENT (file `open` is not async-signal-safe in
  the child), producing an ordered list of `dup2`/`close` ops replayed by a single `pre_exec`
  closure in the child. Generalizes `run_subprocess`'s current dup `pre_exec`; the
  `std::process::Command` 0/1/2 stdio setters give way to the uniform ordered application.

### `{varname}` allocation
`{fd}>file`: open → `fcntl(F_DUPFD, 10)` to a high fd (≥10, NON-cloexec so a child inherits it);
assign `$fd` = the high number. **Lifetime (corrected vs an earlier draft):** for an IN-PROCESS
command (compound / builtin / function) bash leaves the allocated fd OPEN in the shell after the
command — it is only closed by an explicit `{fd}>&-` (or `exec {fd}>&-`) or at shell exit (a
deliberate bash fd-"leak"); `$fd` holds the live number. For an EXTERNAL command bash does NOT
set the parent's `$fd` at all (the redirect + assignment happen in the forked child); the parent
allocates+inherits the fd into the child and closes its own copy after. `{fd}>&-` reads `$fd` and
closes that fd.

### `exec` integration
Rewrite v155's `apply_redirects_permanently` to consume the ordered list — arbitrary fds,
`{var}` allocation/assignment, and close — so `exec 3<file` / `exec {fd}>log` hold a fd for the
shell's life and `exec 3>&-` / `exec {fd}>&-` close it.

## Section 3 — Error handling

- A redirect that fails to open (missing file / permission / `EISDIR`) prints `huck: <target>:
  <reason>`, the command fails, and already-applied redirects in that command roll back
  (in-process: scope Drop; external: the child exits). For `exec`, a failed redirect returns
  failure WITHOUT exiting (v155 behavior).
- Dup of a closed/invalid source (`>&9` when 9 isn't open) → `Bad file descriptor`, rc 1
  (`EBADF`). `N>&-` on an already-closed fd is lenient (no error), per bash. `{var}` allocation
  under fd exhaustion → error. `>&$x` where `$x` is not a single fd number → `ambiguous
  redirect` (extends the existing `resolve_fd_target` checks).
- If an earlier redirect in the ordered list fails, later ones in the same command do not apply.

## Divergences retired (delete from `docs/bash-divergences.md`)
- **M-124** — arbitrary-fd (fd > 2) redirections (the headline).
- **L-08** — redirect source-order (`2>&1 >file`), fixed by the ordered list.
- **M-20** — `n<>file` read-write open (`RedirOp::File` ReadWrite mode).

(Unblocks v157 coproc.)

## Behaviour matrix (byte-identical to bash unless noted)
- `exec 3>f; echo x >&3; exec 3>&-; cat f` → `x`.
- `exec 3<f; read line <&3; echo "$line"` → first line of `f`.
- L-08: `cmd 2>&1 >file` (stderr→terminal, stdout→file) vs `cmd >file 2>&1` (both→file) now differ.
- `cmd 3>&1 1>&2 2>&3 3>&-` → classic stdout/stderr swap.
- `<>`: `exec 3<>f; echo data >&3; exec 3>&-` opens `f` read-write.
- `{fd}>f; echo "$fd"` → a number ≥10; `{fd}>&-` closes it.
- Existing 0/1/2 redirects, heredocs, here-strings, `&>`, `>|` — unchanged (regression).

## Edge cases / documented divergences
- **Dup source that is not an fd number or `-`:** a BARE `>&word` / `&>word` (no leading fd)
  where `word` is a filename is the legacy "redirect both stdout+stderr to the file" form —
  preserved as huck handles `&>` today (it is NOT a `RedirOp::Dup`). A dup with a LEADING fd and
  a non-numeric/non-`-` source (`3>&file`) is `ambiguous redirect` (rc 1), per bash. So
  `RedirOp::Dup { source }` only ever carries an fd-number word or `-`; the both-to-file case
  routes through the existing combined-redirect path.
- Error-message wording differs from bash (`huck:` vs `bash: line N:`) — status-compared, not
  byte-compared, in the harness (house style).
- fd numbers huck assigns for `{var}` may differ from bash's exact choice (both ≥10); the
  harness checks `$fd ≥ 10` and round-trip behavior, not the literal number.
- A redirect on a builtin to a fd the builtin ignores (`echo x 5>f`) opens fd 5 for the builtin
  (which ignores it) then restores — matches bash's observable result (nothing on fd 5).

## Testing
1. **Unit (lexer):** fd-prefix tokenization (`3>`, `10>>`, `2<&`, `{fd}>`, `{n}<&-`); the
   no-space rule (`echo 2>&1` vs `echo 2 >&1` vs `file2>x`); dup source `-` → Close.
2. **Unit (parser):** ordered `Vec<Redirection>` for `>a 2>&1` vs `2>&1 >a`, `3<>f`, `{fd}>f`.
3. **`fd_redirect_diff_check.sh`** (byte-identical bash↔huck): hold+write+close, read, the L-08
   ordering pair, `<>`, `{fd}` allocate/assign/close, the 3-way fd swap, and status-only error
   cases (bad fd, missing file, stderr suppressed where wording diverges).
4. **Regression:** full suite + ALL existing harnesses — especially `compound_redirects`,
   `function_redirect`, `assign_redirect`, `exec` — stay byte-identical. This is the safety net
   for the ~88-site refactor.

## Implementation staging (for the plan)
Lexer fd-prefix tokens → Redirect AST + parser building the ordered list → in-process applier
(`RedirectScope`) → external/`pre_exec` applier → `{varname}` allocation → `exec` integration →
`fd_redirect_diff_check.sh` + delete M-124/L-08/M-20. Each step gated by the full suite +
existing harnesses (the refactor's safety net).

## Notes
- macOS-portable: all calls (`dup2`, `close`, `fcntl(F_DUPFD_CLOEXEC)`, `open`) are POSIX.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Implementer subagents must NOT `git checkout <sha>`; controller verifies the branch tip before
  merge (see [[feedback-verify-branch-tip-before-merge]]).
