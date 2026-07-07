# Unified error-message emitter (`sh_error!`) — design

**Iteration:** v269
**Date:** 2026-07-07
**Status:** design, pending plan

## Problem

huck emits error diagnostics two incompatible ways:

1. **The prologue is hand-baked at ~374 call sites.** The dominant idiom is
   `with_err(|err| e!(err, "huck: {name}: readonly variable"))` — every site
   hard-codes the literal `"huck: "` in its format string. bash instead prefixes
   non-interactive diagnostics with `<name>: [line N: ][cmd: ]` where `<name>` is
   `BASH_SOURCE[0]`/`$0`. huck has the bash-compatible prologue builder
   (`Shell::error_prefix`, `shell_state.rs:890`) but it is wired into only a
   handful of sites (arith, getopts, cd, redirects, source, umask/ulimit/enable).
2. **Three defects follow from the non-uniformity:**
   - **Double prefix.** Syntax errors surface through the central
     `eprintln!("huck: {e}")` in `repl.rs` while the error body has *already*
     acquired a `<name>: line N:` prologue on another path, yielding
     `huck: bash5: line 1: syntax error…` (observed in the `parser` bash-test
     category).
   - **Raw `eprintln!` sites bypass the sink.** A few sites (e.g.
     `cwd_scope.rs:29`) write directly to `io::stderr()` instead of the threaded
     `StderrSink`, so their output ignores `2>&1` and capture redirection.
   - **Missing `-c:` segment.** In `-c` mode bash attributes parser/syntax
     diagnostics to `<name>: -c: line N:`; huck's prologue omits the `-c:`
     invocation-context segment.

The prefix divergence is the single most widespread error-text mismatch against
bash — it appears in ~8 bash-test categories (`alias`, `parser`, `execscript`,
`type`, `dirstack`, `printf`, `getopts`, `shopt`, …).

### What this iteration does NOT try to do

Converting the prologue **flips zero bash-test categories on its own** — a
finding first measured in v227 and re-confirmed against the v268 sweep: every
prefix-mismatched category *also* diverges on an independent blocker (message
wording, a missing offending-source-line echo, an `unrecognized option: -o`
gap, `set -o`/usage-string gaps). Example: `parser` needs four distinct fixes,
of which the prefix is one. **This iteration's success is architectural, not a
category flip** — see Non-goals and Success criteria.

## Goals

1. **One emitter.** A single macro `sh_error!` (backed by one free function
   `emit_error`) is the *only* thing production code uses to emit an error
   diagnostic. It composes the bash prologue and writes to the current error
   stream in one call. Call sites state only *context* + *message*.
2. **`error_prefix` becomes an implementation detail.** No call site composes
   `prefix + tail` by hand; `error_prefix` is made non-`pub` and is reachable
   only through `emit_error`. (This is the crux of the user's objection to a
   prefix-*producing* function: there must be no way to obtain the prefix
   without also emitting through the one path.)
3. **Implicit stream.** `emit_error` resolves the destination through the
   existing thread-local sink (`with_err`), which already falls through to
   `io::stderr()` when no sink is installed. Call sites never name a stream, so
   they cannot write a diagnostic to the wrong one.
4. **Kill the three defects** that fall out of unification: no double prefix,
   no sink-bypassing `eprintln!`, and the **complete** prologue matrix including
   the `-c:` segment — the full bash attribution rules of §3, with nothing
   deferred.
5. **Enforce it.** A production invariant (a test in the v268 `include_str!`
   style) asserts that no literal `"huck: "` / invocation-name text survives in
   emission code outside `error_prefix` and the emitter family (§1).
6. **Prove parity.** A `tests/scripts/error_message_diff_check.sh` harness runs
   an error-producing fragment for **every cell of the §3 matrix** through bash
   and huck and asserts byte-identical stderr.

## Non-goals

- **No category flip.** We do not fix the *other* per-category blockers
  (message wording, offending-source-line echo, `-o`/`set -o` gaps, usage-string
  format). Those are separate iterations. The PASS count is expected to stay 10.
- **No message-body changes.** Body composition (the text after the prologue,
  and helpers like `bash_io_error`) is unchanged — only the prologue and the
  emission path are unified. (The `<name>: -c: line N:` *prologue* on a syntax
  error is in scope; the message *body* — e.g. `syntax error near unexpected
  token` vs huck's wording — is not.)

There is **no deferral** of the prefix itself: every prologue form in §3,
including `-c:` attribution and the pre-shell CLI form, is implemented this
iteration.

## Design

### 1. The emitter family

A small family covers every diagnostic huck emits; these functions are the
*only* code that writes the invocation-name/prologue text. They live in
huck-engine (new `error_emit.rs`) and share one prologue builder (`error_prefix`,
now taking a `Diag` kind — §3). Two emit to a **caller-provided writer**
(`emit_error_to`/`sh_error_to!` — the builtin path, redirect-aware) and the rest
route through the **thread-local sink** (`emit_error`/`sh_error!`,
`emit_syntax_error`, `emit_cli_error`) for sites with no writer in hand. See §1(a2)
for why the writer variant is mandatory for builtins.

**(a) Runtime errors — the common case (`sh_error!`).** `eprintln!`-shaped:
shell, context, format string + args.

```rust
/// Emit one bash diagnostic to the current error sink for a RUNTIME error.
/// Prologue: `<name>: [line N: ][cmd: ]`. `cmd` = builtin/context (Some("cd")) or None.
pub fn emit_error(shell: &Shell, cmd: Option<&str>, body: std::fmt::Arguments) {
    with_err(|err| {
        let _ = write!(err, "{}", shell.error_prefix(Diag::Runtime(cmd)));
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
    });
}

#[macro_export]
macro_rules! sh_error {
    ($shell:expr, $cmd:expr, $($arg:tt)*) => {
        $crate::emit_error($shell, $cmd, format_args!($($arg)*))
    };
}
```

**(a2) Runtime errors WITH a writer in hand — `sh_error_to!` (the builtin path).**
This is the variant most builtins use, and it is why the family is a hybrid.
Builtins receive `out`/`err` **writer parameters** from the executor
(`run_builtin(program, args, out, err, shell)`). Those writers are the
redirect-aware channel: for a *bare* builtin with a trailing `2>&1`/`>&2`, the
executor's `run_builtin_with_redirects` does an **in-memory stream swap**
(`route_err_to_out` / `route_out_to_err`, resolving L-25) that exists ONLY in
those writer params — the thread-local sink is not told about it. So a builtin
that emits via the thread-local (`sh_error!`) instead of its `err` param loses
the diagnostic under captured `2>&1` (verified: `x=$(cd /nope 2>&1)` captures the
error in bash but was empty in huck when routed thread-local). Therefore a site
that HOLDS a writer must emit to THAT writer:

```rust
/// Emit a runtime diagnostic to a CALLER-PROVIDED writer (redirect-aware).
pub fn emit_error_to(shell: &Shell, w: &mut dyn std::io::Write, cmd: Option<&str>, body: std::fmt::Arguments) {
    let _ = write!(w, "{}", shell.error_prefix(Diag::Runtime(cmd)));
    let _ = w.write_fmt(body);
    let _ = w.write_all(b"\n");
}

#[macro_export]
macro_rules! sh_error_to {
    ($shell:expr, $w:expr, $cmd:expr, $($arg:tt)*) => {
        $crate::emit_error_to($shell, $w, $cmd, format_args!($($arg)*))
    };
}
```

**The rule (which variant a site uses):** if an `out`/`err` writer param
descending from `run_builtin` is in scope, use `sh_error_to!(shell, err, cmd, …)`
— this is the redirect-correct channel and covers the overwhelming majority of
builtin error sites. Only when NO writer is reachable (deep helpers in
`shell_state.rs`/`expand.rs` internals, non-builtin executor paths) use the
thread-local `sh_error!(shell, cmd, …)`. Both share `error_prefix`; the choice
is purely "do I have the writer the executor handed me?"

**(b) Syntax/parser errors (`emit_syntax_error`).** Prologue is
`<name>: [-c: ]line N:` — the `-c:` segment present iff the shell was invoked
with `-c` (`is_command_string`), and **no `cmd` segment**. The line comes from
the `ParseError`'s own location, not the runtime line counter.

```rust
/// Emit a SYNTAX/parser diagnostic. Prologue: `<name>: [-c: ]line N:`.
pub fn emit_syntax_error(shell: &Shell, line: u32, body: std::fmt::Arguments) {
    with_err(|err| {
        let _ = write!(err, "{}", shell.error_prefix(Diag::Syntax { line }));
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
    });
}
```

**(c) Pre-shell CLI errors (`emit_cli_error`).** For failures that occur before
a `Shell` exists (bad CLI option, line-editor init) — bash prints
`<basename>: <msg>` with **no line, no `-c:`**. `prog` is `basename(argv[0])`
(the CLI's `args[0]`), matching bash's use of the invocation basename.

```rust
/// Emit a diagnostic with no shell state: `<prog>: <msg>`.
pub fn emit_cli_error(prog: &str, body: std::fmt::Arguments) {
    with_err(|err| {
        let _ = write!(err, "{prog}: ");
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
    });
}
```

`sh_error!` is `#[macro_export]`ed and all three functions are `pub` so
huck-cli's central printers (`repl.rs`) can use them; `$crate` resolves to
huck-engine regardless of the invoking crate. In-engine sites use them directly.
Together they are the "group of functions" through which all emission flows —
there is no fourth path.

Call sites transform mechanically:

```rust
// before
with_err(|err| e!(err, "huck: {name}: readonly variable"));
eprintln!("huck: cwd: {}: {}", path.display(), bash_io_error(&e));
with_err(|err| e!(err, "huck: {cmd}: {sub}: {msg}"));

// after
sh_error!(shell, None,          "{name}: readonly variable");
sh_error!(shell, Some("cwd"),   "{}: {}", path.display(), bash_io_error(&e));
sh_error!(shell, Some(cmd),     "{sub}: {msg}");
```

The first macro argument is any expression that borrows as `&Shell`. At the many
sites inside `Shell` methods it is `self`; inside builtins it is the `shell`
parameter. `emit_error` takes `&Shell` (shared borrow) so it composes cleanly
even where the surrounding method holds `&mut self` — the emit is a
self-contained shared borrow for the duration of the call.

`e!` and `bash_io_error` **remain** — `e!` is the low-level one-line writer
`sh_error!` is layered on conceptually, and body helpers are unchanged. Sites
that legitimately write **non-error** lines to stderr (warnings that bash also
prefixes with the prologue are errors for our purposes; genuine
progress/notice output) keep using `e!`/`with_err` directly.

### 2. `error_prefix` becomes internal

Change `pub fn error_prefix` → `pub(crate) fn error_prefix` and confirm no
caller outside the emitter remains. The existing `error_prefix` unit tests
(`shell_state.rs:3963+`) stay (in-crate). Any current external caller is
rewritten to `sh_error!`.

### 3. The prologue matrix (complete)

Empirically characterized against bash 5.2.21 across every mode × error class.
`<name>` is `$0` **verbatim** (never basenamed): the `-c` NAME arg, else the
script path, else `bash`→`huck` for stdin/default.

| Error class | `-c` mode | script-file mode | stdin (non-interactive) | interactive |
|---|---|---|---|---|
| **Runtime** (builtin, cd, cmd-not-found, arith, `set -o`) | `<name>: line N: <cmd>: <msg>` | `<name>: line N: <cmd>: <msg>` | `<name>: line N: <cmd>: <msg>` | `<name>: <cmd>: <msg>` (no line) |
| **Syntax/parser** | `<name>: -c: line N: <msg>` | `<name>: line N: <msg>` | `<name>: line N: <msg>` | `<name>: <msg>` (no line) |
| **Pre-shell CLI** (bad option, editor init) | `<basename>: <msg>` (no line) | — | — | — |

Rules distilled:
- **`-c:` segment** ⟺ **syntax error AND `-c` mode**. Never for runtime errors;
  never in script/stdin mode. (Verified: `bash -c 'if'` → `bash: -c: line 1:`;
  `bash -c 'readonly x=1;x=2'` → `bash: line 1:` (no `-c:`); `bash s.sh` syntax
  error → `s.sh: line N:`.)
- **`cmd:` segment** only on runtime errors (never syntax; the parser has no
  builtin context).
- **`line N:`** on every non-interactive diagnostic; **absent** interactively
  and for pre-shell CLI errors. Syntax errors take the line from the
  `ParseError` location; runtime errors from `current_lineno`.
- `<name>` carries embedded slashes unchanged (`bash -c '…' /a/b/prog` →
  `/a/b/prog: …`); pre-shell errors use the invocation **basename**.

This maps onto `error_prefix` taking a `Diag` kind that selects the segments:

```rust
enum Diag<'a> {
    Runtime(Option<&'a str>), // <name>: [line N: ][cmd: ]
    Syntax { line: u32 },     // <name>: [-c: (iff is_command_string)]line N:
}
pub(crate) fn error_prefix(&self, kind: Diag) -> String { … }
```

Requires one new `Shell` field, `is_command_string: bool`, set when huck is
invoked with `-c`. The pre-shell form has no `Shell` and is produced by
`emit_cli_error` (§1c), not `error_prefix`.

### 4. Eliminate the double prefix

Parser/lexer errors originate in huck-syntax, which has no `Shell` and must not
carry a prologue. Guarantee that `ParseError`/`LexError` `Display`
(`command.rs:676`, `lexer.rs:36`, via `errors::parse_error_message_impl`)
renders **body only** — no `<name>:`/`line N:`/`-c:`/`huck:` text. The central
runners (`repl.rs:47/229/…` and the script/`-c` entry path) stop using
`eprintln!("huck: {e}")` and instead emit through `emit_syntax_error(shell,
line, format_args!("{e}"))`, passing the line from the `ParseError` location.
The prologue (including the `-c:` segment when `is_command_string`) is then
applied exactly once, by `emit_syntax_error`. Audit every path that currently
injects `<name>: line N:` into a syntax-error body — that today produces the
`huck: bash5: line 1:` double prefix — and remove the injection; it now lives
solely in `error_prefix(Diag::Syntax{..})`.

### 5. Sink routing

Every raw `eprintln!("huck: …")` **error** site (grep: `cwd_scope.rs:29`, the
`repl.rs` set, any other) becomes `sh_error!`, routing through `with_err` so the
output honors `2>&1` and capture. Where the site is in the CLI layer, it calls
the engine's `emit_error` (huck-cli already depends on huck-engine).

### 6. Error-value constructor sites

A few `shell_state.rs` methods (e.g. lines 189/192) *return* a `"huck: {cmd}:
…"` string for the caller to print (the "callers translate" contract at
`shell_state.rs:173`). Convert these to return the **body only**; the
translating caller emits with `sh_error!`. This keeps the invariant (no literal
`"huck: "`) true and centralizes the prologue.

### 7. Pre-shell CLI diagnostics

`repl.rs:47/129` emit `huck: {e}` for a `parse_cli` / line-editor-init failure
that happens **before a `Shell` exists**. bash prints `<basename>: <msg>` with
no line here. These route through `emit_cli_error(prog, format_args!("{e}"))`
(§1c), where `prog = basename(args[0])` — so they match bash's basename-only
form and are **not** a literal-`"huck: "` exception: the invocation name lives
inside `emit_cli_error`, the sole place besides `error_prefix` that renders it.
No scattered allowlist; the invariant permits the name text only inside the
emitter family.

## Enforcement invariant

Add a production test in the v268 `lexer_has_no_production_parser_dependency`
style: read the emission sources with `include_str!`, strip `#[cfg(test)]`
modules, and assert zero literal invocation-name text (`"huck: "`) outside the
**emitter family** (`error_prefix`, `emit_error`, `emit_syntax_error`,
`emit_cli_error`). This is the durable guard that "all sites use the emitter" —
a new hand-baked `"huck: "` fails the test.

## Testing

**Primary driver — the bash test suite, iteratively.** The exact set of error
messages to match is discovered and validated by running the prefix-touched
bash-test categories and reading the diffs, not by enumerating cases a priori.
The loop: run each of `parser`, `execscript`, `type`, `dirstack`, `alias`,
`printf`, `errors`, `comsub` via the runner
(`HUCK_BASH_TEST_CATEGORY=<cat> bash tests/bash-test-suite/runner.sh`), read the
per-category `.diff`, and for each line that is a *prologue* mismatch, fix the
emitter/matrix and re-run. A prologue diff line that persists means the matrix
(§3) is wrong for that case — the suite is the oracle. (Diff lines that are
*body* mismatches — wording, source-line echo, `-o` gaps — are out of scope and
expected to remain; the PASS status of these categories will not necessarily
flip.) These runs stay local (GPL: never copy bash test bytes into committed
files).

**Regression harness — `tests/scripts/error_message_diff_check.sh`.** A
bash↔huck byte-comparator (standard `checkf`/`checkd` shape) with one fragment
per **cell of the §3 matrix**:
- runtime × {`-c`, script-file, stdin}: readonly assignment, unknown builtin
  option, a `cd` failure (each `<name>: line N: <cmd>: <msg>`, no `-c:`).
- syntax × `-c`: `<name>: -c: line N: <msg>`; syntax × script-file:
  `<name>: line N: <msg>` — same fragment, asserting `-c:` appears iff `-c`.
- custom `$0` (`huck -c '…' myprog`) → `myprog:` prefix.
- the double-prefix regression: a `-c` syntax error yields exactly one prologue.
- a `2>&1`-captured runtime error lands on stdout (proves sink routing).
- pre-shell: `huck --badoption` → `huck: <msg>` (basename, no line).
Guard the sweep with `ulimit -v` + `timeout` per the repo convention.

**Unit tests** for the prologue builder: `error_prefix(Diag::Runtime/Syntax)`
composition for interactive/non-interactive, with/without `cmd`, with/without
`is_command_string`, asserting against a capture sink; plus `emit_cli_error`.

**Full existing suites green**: `huck-syntax` (~408) and `huck-engine` (~1740),
run per-crate single-threaded per the repo memory.

## Site inventory (for task decomposition)

Non-test `"huck: "` emission sites by file (≈374 total):

| File | count |
|---|---|
| `crates/huck-engine/src/builtins.rs` | 201 |
| `crates/huck-engine/src/executor.rs` | 85 |
| `crates/huck-engine/src/expand.rs` | 22 |
| `crates/huck-engine/src/completion_builtins.rs` | 17 |
| `crates/huck-engine/src/shell_state.rs` | 14 |
| `crates/huck-engine/src/restricted.rs` | 7 |
| `crates/huck-engine/src/shell.rs` | 6 |
| `crates/huck-cli/src/repl.rs` | 6 |
| `crates/huck-engine/src/param_expansion.rs` | 4 |
| `crates/huck-syntax/src/errors.rs` | 3 |
| `crates/huck-engine/src/stdin_pipe.rs` | 3 |
| `crates/huck-engine/src/history.rs` | 2 |
| `crates/huck-engine/src/engine.rs` | 1 |
| `crates/huck-engine/src/cwd_scope.rs` | 1 |

The conversion is mechanical and per-file-independent, which suggests one task
per file (or per small file-group), with the emitter + invariant + harness as
the first task and the double-prefix/`-c:`/central-printer work as its own task.

## Risks / edge cases

- **`&Shell` availability.** A minority of free functions emit without a `Shell`
  in scope. Each is resolved by threading `&Shell` (usually already available on
  an adjacent parameter) or, if genuinely pre-shell, routed through
  `emit_cli_error` (§1c/§7). The plan flags any site that needs a signature
  change.
- **Borrow conflicts.** Sites inside `&mut self` methods calling
  `emit_error(self, …)` take a shared reborrow; a few may need a small scope
  adjustment. Mechanical, caught by the compiler.
- **Warnings that are really notices.** `warning:`-class lines that bash routes
  without the error prologue must NOT be forced through `sh_error!`. The plan
  classifies each `warning:` site (bash parity is the arbiter).
- **Syntax-error line source.** `emit_syntax_error` needs the line from the
  `ParseError` location (v237 spanned tokens; AST `line: u32`). The plan
  confirms the exact accessor; huck already emits *a* line for `-c` syntax
  errors today, so the source exists — this iteration routes it correctly and
  adds `-c:`.

## Success criteria

1. The emitter family (`sh_error!`/`emit_error`, `emit_syntax_error`,
   `emit_cli_error`, plus the writer variant `sh_error_to!`/`emit_error_to`)
   exists and is the sole error-emission path in production. Builtin sites (any
   site holding an `out`/`err` writer from `run_builtin`) use `sh_error_to!` so
   the bare-builtin `2>&1`/`>&2` redirect swap is respected — verified by a
   `$(builtin 2>&1)` capture test.
2. `error_prefix` is `pub(crate)`, takes `Diag`, and has no non-emitter caller.
3. The invariant test passes (zero literal invocation-name text outside the
   emitter family).
4. **The full §3 matrix is implemented** — `-c:` appears iff syntax-error-in-`-c`;
   runtime/script/stdin/pre-shell forms all match bash. Nothing deferred.
5. No double prefix; syntax errors carry exactly one prologue.
6. Raw-`eprintln!` error sites route through the sink (a `2>&1` test proves it).
7. `error_message_diff_check.sh` passes for **every matrix cell**, and the
   prefix-touched bash-test categories show **no remaining *prologue* diff
   lines** (body-only diffs may remain — see Testing).
8. `huck-syntax` and `huck-engine` suites green.
9. bash-test PASS count unchanged (10) — expected and correct; the win is
   architectural uniformity, turning the prefix from "1 of N blockers" into "0
   blockers" for every affected category, enabling future single-fix flips.
