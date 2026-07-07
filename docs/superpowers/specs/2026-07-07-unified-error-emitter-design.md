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
   no sink-bypassing `eprintln!`, and add the `-c:` segment.
5. **Enforce it.** A production invariant (a test in the v268 `include_str!`
   style) asserts that no literal `"huck: "` string survives in engine emission
   code outside `error_prefix` and a small allowlist.
6. **Prove parity.** A `tests/scripts/error_message_diff_check.sh` harness runs
   representative error-producing fragments through bash and huck and asserts
   byte-identical stderr for the message forms this iteration covers.

## Non-goals

- **No category flip.** We do not fix the *other* per-category blockers
  (message wording, offending-source-line echo, `-o`/`set -o` gaps, usage-string
  format). Those are separate iterations. The PASS count is expected to stay 10.
- **No message-body changes.** Body composition (the text after the prologue,
  and helpers like `bash_io_error`) is unchanged — only the prologue and the
  emission path are unified.
- **No new prologue semantics beyond the `-c:` segment.** The exact bash matrix
  of *which* error classes get `-c:` is harness-verified; any residual
  attribution edge case that proves deep is captured as a `[deferred]`
  divergence rather than expanded here.

## Design

### 1. The emitter

Add to huck-engine one function and one macro. The macro is `eprintln!`-shaped:
first the shell (for the prologue), then the context, then a format string +
args.

```rust
// crates/huck-engine/src/macros.rs (or a new error_emit.rs)

/// Emit one bash-compatible diagnostic line to the current error sink.
/// `cmd` is the builtin/context name (`Some("cd")`) or `None`. The prologue
/// (`<name>: [line N: ][cmd: ]`) is prepended; the body is the caller's message.
pub fn emit_error(shell: &Shell, cmd: Option<&str>, body: std::fmt::Arguments) {
    with_err(|err| {
        let _ = ::std::io::Write::write_fmt(err, format_args!("{}", shell.error_prefix(cmd)));
        let _ = ::std::io::Write::write_fmt(err, body);
        let _ = ::std::io::Write::write_all(err, b"\n");
    });
}

#[macro_export]
macro_rules! sh_error {
    ($shell:expr, $cmd:expr, $($arg:tt)*) => {
        $crate::emit_error($shell, $cmd, format_args!($($arg)*))
    };
}
```

`sh_error!` is `#[macro_export]`ed and `emit_error` is `pub` so huck-cli's
central-printer sites (`repl.rs`) can use them; `$crate` resolves to huck-engine
regardless of the invoking crate. In-engine sites use it directly.

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

### 3. Prologue completeness — the `-c:` segment

bash attributes syntax/parser diagnostics in command-string mode to
`<name>: -c: line N:`. Add an `is_command_string: bool` flag to `Shell`, set
when huck is invoked with `-c`, and extend `error_prefix` to insert the `-c:`
segment for the invocation-context class:

```rust
// inside error_prefix, after the `<name>: ` and before `line N: `
if !self.is_interactive && self.is_command_string && /* invocation-context class */ {
    out.push_str("-c: ");
}
```

The class distinction (bash uses `-c:` for parser/startup diagnostics but not
for every runtime builtin error) is verified by the harness. If matching bash's
exact attribution matrix proves to require per-error-class plumbing beyond a
single flag, this iteration covers the common syntax-error forms and records the
residual as an `L-*` `[deferred]` divergence — the emitter unification (Goals
1–5) does not depend on it.

### 4. Eliminate the double prefix

Parser/lexer errors originate in huck-syntax, which has no `Shell` and must not
carry a prologue. Guarantee that `ParseError`/`LexError` `Display`
(`command.rs:676`, `lexer.rs:36`, via `errors::parse_error_message_impl`)
renders **body only** — no `<name>:`/`line N:`/`huck:` text. The central
runners (`repl.rs:47/229/…` and the script/`-c` entry path) stop using
`eprintln!("huck: {e}")` and instead emit through `sh_error!(shell, None,
"{e}")`, so the prologue is applied exactly once, by the one emitter. Audit the
paths that currently inject `<name>: line N:` into a syntax-error body and remove
that injection (it moves into `error_prefix`).

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

### 7. Pre-shell CLI diagnostics (allowlist)

`repl.rs:47` emits `huck: {e}` for a `parse_cli` failure that happens **before a
`Shell` exists**, so there is no `error_prefix` state (`$0`/argv0 unset). bash
still prefixes with its name here, but huck has nothing to resolve. These few
pre-shell sites are an **explicit, enumerated allowlist**: they keep a literal
`"huck: "` and are exempt from the invariant. The allowlist is small (CLI
argument parsing and line-editor init) and listed in the invariant test so it
cannot silently grow.

## Enforcement invariant

Add a production test in the v268 `lexer_has_no_production_parser_dependency`
style: read the engine emission sources with `include_str!`, strip
`#[cfg(test)]` modules, and assert zero literal `"huck: "` occurrences outside
(a) `error_prefix`'s definition and (b) the enumerated pre-shell allowlist.
This is the durable guard that "all sites use the one emitter" — a new hand-baked
`"huck: "` fails the test.

## Testing

- **`tests/scripts/error_message_diff_check.sh`** — bash↔huck byte-comparator
  (the standard `checkf`/`checkd` harness shape) over representative
  error-producing fragments in each mode:
  - script-file mode (`<name>: line N: <msg>`): readonly assignment, unknown
    builtin option, a redirect failure, a `cd` failure.
  - `-c` mode (`<name>: -c: line N: <msg>`): a syntax error, a runtime error.
  - the double-prefix regression (a syntax error must produce exactly one
    prologue).
  - a `2>&1`-captured error (proves sink routing: the diagnostic lands on stdout
    when redirected).
  Guard the sweep with `ulimit -v` + `timeout` per the repo convention.
- **Unit tests** for `emit_error`: prologue composition for
  interactive/non-interactive, with/without `cmd`, with/without `line N`, and
  the `-c:` segment — asserting against a capture sink.
- **Full existing suites green**: `huck-syntax` (~408) and `huck-engine`
  (~1740), run per-crate single-threaded per the repo memory.

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
  an adjacent parameter) or, if genuinely shell-less and pre-shell, added to the
  §7 allowlist. The plan flags any site that needs a signature change.
- **Borrow conflicts.** Sites inside `&mut self` methods calling
  `emit_error(self, …)` take a shared reborrow; a few may need a small scope
  adjustment. Mechanical, caught by the compiler.
- **Warnings that are really notices.** `warning:`-class lines that bash routes
  without the error prologue must NOT be forced through `sh_error!`. The plan
  classifies each `warning:` site (bash parity is the arbiter).
- **`-c:` attribution depth** (see §3) — the one place with real bash nuance;
  bounded by the deferral clause.

## Success criteria

1. `sh_error!`/`emit_error` exist and are the sole error-emission path in engine
   production code.
2. `error_prefix` is `pub(crate)` with no non-emitter caller.
3. The invariant test passes (zero literal `"huck: "` outside `error_prefix` +
   the enumerated allowlist).
4. No double prefix; syntax errors carry exactly one prologue.
5. Raw-`eprintln!` error sites route through the sink (a `2>&1` test proves it).
6. `error_message_diff_check.sh` passes for the covered forms.
7. `huck-syntax` and `huck-engine` suites green.
8. bash-test PASS count unchanged (10) — this is expected and correct; the win
   is architectural uniformity, which turns the prefix from "1 of N blockers"
   into "0 blockers" for every affected category, enabling future single-fix
   category flips.
