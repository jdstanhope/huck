# v29: FD Duplication Redirects `n>&m` / `&>file` — Design Spec

## Goal

Implement POSIX fd-duplication redirects (`2>&1`, `1>&2`) and the
common bash extensions (`&>file`, `&>>file`). Closes M-18 from
`docs/bash-divergences.md`.

Pre-v29:
- `cmd 2>&1` is a parse error (`2>` lexes, then `&1` doesn't compose).
- `cmd >file 2>&1` likewise fails.
- `cmd &>file` lexes `&` as Background → parse error.

These are arguably the most-used POSIX-required redirect patterns in
real shell scripts — `2>&1` for merging stderr into stdout is in
nearly every CI/CD pipeline, build script, and `do-X || log-and-exit`
idiom. Tracked TWICE in v28 test workarounds.

After v29:
```sh
cmd 2>&1              # stderr -> wherever stdout goes
cmd >file 2>&1        # both stdout and stderr to file
cmd &>file            # bash shorthand for >file 2>&1
cmd &>>file           # bash shorthand for >>file 2>&1
cmd 1>&2              # stdout -> wherever stderr goes
echo "error" >&2      # convenience shorthand
```

## Scope

User-confirmed:
- **`2>&1` / `1>&2`** (POSIX fd-duplication for fds 1 and 2).
- **`&>file` / `&>>file`** (bash combined-redirect shortcuts).
- **NOT** arbitrary `n>&m` for n other than 1 or 2 (e.g. `3>&2`).
- **NOT** `>&-` / `2>&-` (fd close); POSIX but rarely used.
- **NOT** redirects on compound commands (`(cmd) > file`, `if cmd; fi > file`) —
  separate audit gap, deferred.

## Semantics

**`n>&m`** (POSIX 2.7.6): "the file descriptor denoted by `n` is made
to be a copy of the file descriptor denoted by `m`". Implementation:
`dup2(m, n)` after stdio setup but before exec.

**`&>file`** (bash extension): equivalent to `>file 2>&1`. Parser
desugars to two redirects (stdout=Truncate(file), stderr=Dup{fd:2,
source:lit_word("1")}).

**`&>>file`**: equivalent to `>>file 2>&1`. Same desugaring with Append
instead of Truncate.

**Application order**: per POSIX, redirects apply left-to-right. Huck
stores redirects in per-fd fields (stdin/stdout/stderr) on
`ExecCommand`, which loses source-order. The executor applies them in
a fixed order: stdout-redirect first, then stderr-redirect. Result:

- `cmd >file 2>&1` (canonical): bash and huck both → both fds to file. ✓
- `cmd 2>&1 >file` (rare/anti-pattern): bash → stderr to terminal,
  stdout to file. Huck → BOTH to file (wrong).

**Document this as an intentional v29 simplification.** Moving to
`Vec<Redirect>` to preserve source order is a larger refactor,
out-of-scope.

**Target word expansion**: the word after `>&` is expanded via
`expand_assignment` (no split/glob), then parsed as an i32. So
`cmd 2>&$STDOUT_FD` works if `$STDOUT_FD=1`. Non-numeric expansion
errors at runtime: `huck: bad fd: <text>`.

**Pipeline composition**: each pipeline stage's redirects apply per
stage. `cmd1 2>&1 | cmd2` — cmd1's stderr is merged into its
stage-stdout pipe, which cmd2 reads. Standard pipeline behavior.

**Subshell composition (v28)**: `(cmd) 2>&1` doesn't parse today
(compound-command redirects, separate gap). `(cmd 2>&1)` works — the
redirect is on the inner cmd inside the subshell.

**Inline assignment composition (v23)**: `FOO=hi cmd 2>&1` — inline
assignment applies; cmd runs with FOO=hi and stderr->stdout. Standard.

## Lexer changes

`src/lexer.rs`:

**Four new Operator variants**:
```rust
pub enum Operator {
    // existing...
    DupOut,             // >&  (source fd defaults to 1 stdout)
    DupErr,             // 2>& (source fd is 2 stderr)
    AndRedirOut,        // &>  (bash: > + 2>&1)
    AndRedirAppend,     // &>> (bash: >> + 2>&1)
}
```

**Lexer dispatch extensions**:

`>` arm: extend the existing peek-chain:
```rust
'>' => {
    if has_token { /* flush */ }
    if chars.peek() == Some(&'>') {
        chars.next();
        tokens.push(Token::Op(Operator::RedirAppend));
    } else if chars.peek() == Some(&'&') {       // NEW
        chars.next();
        tokens.push(Token::Op(Operator::DupOut));
    } else {
        tokens.push(Token::Op(Operator::RedirOut));
    }
}
```

`2` arm (existing `2>` recognition): after consuming `2>`, extend the
peek-chain:
```rust
if chars.peek() == Some(&'>') {
    chars.next();
    tokens.push(Token::Op(Operator::RedirErrAppend));
} else if chars.peek() == Some(&'&') {           // NEW
    chars.next();
    tokens.push(Token::Op(Operator::DupErr));
} else {
    tokens.push(Token::Op(Operator::RedirErr));
}
```

`&` arm: extend the existing peek-chain:
```rust
'&' => {
    if has_token { /* flush */ }
    if chars.peek() == Some(&'&') {
        chars.next();
        tokens.push(Token::Op(Operator::And));
    } else if chars.peek() == Some(&'>') {       // NEW
        chars.next();
        if chars.peek() == Some(&'>') {
            chars.next();
            tokens.push(Token::Op(Operator::AndRedirAppend));
        } else {
            tokens.push(Token::Op(Operator::AndRedirOut));
        }
    } else {
        tokens.push(Token::Op(Operator::Background));
    }
}
```

These are mechanical peek-chain extensions; no new state machine or
context-sensitivity needed.

## AST changes

`src/command.rs`:

```rust
pub enum Redirect {
    Read(Word),
    Truncate(Word),
    Append(Word),
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    HereString(Word),
    Dup { fd: i32, source: Word },               // NEW
}
```

- `fd`: 1 (for `>&`) or 2 (for `2>&`). Identifies which fd the redirect
  targets (i.e. which `ExecCommand` field this Redirect is stored in).
- `source`: the Word after `>&`; expands at runtime to the fd number to
  dup FROM.

Naming convention: `Redirect::Dup { fd: 2, source: word_of_1 }`
represents `2>&1` — "make fd 2 a dup of fd 1".

## Parser changes

`src/command.rs`:

**Two simple arms** in the per-stage redirect-consumption code:

```rust
Token::Op(Operator::DupOut) => {
    let target = consume_next_word(iter)?;  // existing helper or inline
    cmd.stdout = Some(Redirect::Dup { fd: 1, source: target });
}
Token::Op(Operator::DupErr) => {
    let target = consume_next_word(iter)?;
    cmd.stderr = Some(Redirect::Dup { fd: 2, source: target });
}
```

**Two desugaring arms** for the bash combined-redirect forms:

```rust
Token::Op(Operator::AndRedirOut) => {
    let target = consume_next_word(iter)?;
    cmd.stdout = Some(Redirect::Truncate(target));
    cmd.stderr = Some(Redirect::Dup { fd: 2, source: lit_word("1") });
}
Token::Op(Operator::AndRedirAppend) => {
    let target = consume_next_word(iter)?;
    cmd.stdout = Some(Redirect::Append(target));
    cmd.stderr = Some(Redirect::Dup { fd: 2, source: lit_word("1") });
}
```

`lit_word("1")` constructs `Word { parts: [Literal { text: "1".to_string(), quoted: false }] }`. A small helper or inline `Word(vec![WordPart::Literal { text: "1".to_string(), quoted: false }])`.

**Last-wins for stderr in combined-redirects**: `cmd 2>file &>out` —
the `&>out` arm overwrites `cmd.stderr` with the Dup. Matches bash's
left-to-right semantics for the cases huck preserves.

## Executor changes

`src/executor.rs`:

The dup must happen in the CHILD process, AFTER stdio configuration
but BEFORE exec (for externals) or before running the body (for
in-process fork).

**For external commands** via `std::process::Command` (`spawn_external_with_fds`):
- Configure stdout/stderr via existing Stdio path (file open or piped).
- After spawn-config and before exec, run a `pre_exec` closure that
  performs the dup2 for any Dup redirects.
- Parent-side: pre-expand the Dup's `source` Word to an i32 BEFORE
  fork (so the child's pre_exec doesn't need to allocate).

```rust
// Parent (pre-fork): resolve target fds.
let stdout_dup: Option<i32> = match &cmd.stdout {
    Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
    _ => None,
};
let stderr_dup: Option<i32> = match &cmd.stderr {
    Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
    _ => None,
};
// Configure stdio normally for non-Dup redirects (file open etc.); for Dup,
// use Stdio::inherit() since we'll dup2 in pre_exec.
// ...
// pre_exec closure (runs in child between fork and exec):
process.pre_exec(move || {
    if let Some(fd) = stdout_dup {
        if unsafe { libc::dup2(fd, 1) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if let Some(fd) = stderr_dup {
        if unsafe { libc::dup2(fd, 2) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
});
```

The existing `reset_job_control_signals_in_child` pre_exec composes
via successive `pre_exec` calls — both run in the child in order.

**For in-process fork** (`fork_and_run_in_subshell`):
- Same pattern: resolve target fds in parent, dup2 in child after the
  existing stdio dup2.

**Application order in the child**: stdout-Dup happens before
stderr-Dup. This matches the canonical `>file 2>&1` semantics (stdout
goes to file first, then stderr dups from the now-file fd 1).

**`resolve_fd_target(source: &Word, shell: &Shell) -> Result<i32, io::Error>`**:
1. Expand via `expand_assignment(source, shell)`.
2. Parse as i32. On parse failure: `Err(io::Error::other("bad fd: ..."))`.
3. Return the i32.

Called pre-fork.

## Edge cases

- **`cmd 2>&1` alone** (no other redirect): stdout stays at parent's
  terminal; stderr dups to fd 1 (also terminal). Net: no observable
  change. Per bash.
- **`cmd 2>&1 > file`** (rare anti-pattern): documented divergence.
  Huck applies stdout-redirect-first; result is both to file. Bash
  would put stderr to terminal, stdout to file. Document.
- **`cmd >file 2>&1`** (canonical): both to file. ✓
- **`cmd &>file`**: parser desugars; same end-state as canonical. ✓
- **`echo error >&2`**: stdout dups to fd 2; echo's output goes to
  stderr. Common idiom.
- **`cmd 2>&3`** (arbitrary fd, OUT OF SCOPE): parser accepts the
  syntax (target Word can be any number); runtime resolves "3" to fd
  3 and dup2's. May or may not work depending on whether fd 3 is open.
  Bash behavior: same. We don't actively prevent it but don't claim
  support — uncommon and bash-compat-by-accident.
- **`cmd 2>&-`** (close fd, OUT OF SCOPE): the target Word would
  expand to literal `-`. `resolve_fd_target` fails parse → runtime
  error. Acceptable; documented as out-of-scope.
- **`cmd 2>&$FD`** with `$FD=1`: target Word has Var part; expansion
  yields "1"; parses to fd 1. ✓
- **`cmd 2>&$FD` with `$FD=foo`**: runtime error "bad fd: foo".
- **`cmd &>file 2>err.log`**: parser sees `&>file` (sets
  stdout=Truncate(file), stderr=Dup{2,"1"}), then `2>err.log`
  overwrites stderr=Truncate(err.log). Last-wins-per-field. Final:
  stdout=file, stderr=err.log. Probably what the user wants.
- **Pipeline composition** (`cmd1 2>&1 | cmd2`): cmd1's stage has
  stderr=Dup{2,"1"}; v25 pipeline machinery sets up cmd1's stdout pipe
  to cmd2; child dup2(1, 2) merges stderr into the same pipe. cmd2
  reads both streams interleaved. ✓
- **Subshell composition** (`(cmd 2>&1)`): the inner cmd has the dup;
  works through existing subshell machinery. ✓
- **`(cmd) 2>&1`** (compound-command redirect): doesn't parse today
  (separate gap, deferred). Will work when compound-command redirects
  land.

## Out of scope

- **Arbitrary `n>&m`** for n != 1, 2.
- **`>&-` / `2>&-`** to close fds.
- **`<&` / `n<&m`** (input fd-duplication — symmetric to output but
  uncommon and adds parser surface).
- **Source-order preservation** (`Vec<Redirect>` refactor) — see
  Semantics note above; documented divergence.
- **Redirects on compound commands** (`(cmd) > file`,
  `if cmd; fi > file`) — separate audit gap; track as new entry if
  not already.

## Tests

### Lexer (`src/lexer.rs::tests`)

| Test | Covers |
| --- | --- |
| `tokenize_dup_out_basic` | `>&` → `Operator::DupOut` |
| `tokenize_dup_err_basic` | `2>&` → `Operator::DupErr` |
| `tokenize_and_redir_out` | `&>` → `Operator::AndRedirOut` |
| `tokenize_and_redir_append` | `&>>` → `Operator::AndRedirAppend` |
| `tokenize_redir_out_still_works` | regression: `>` alone, `>>` |
| `tokenize_redir_err_still_works` | regression: `2>`, `2>>` |
| `tokenize_background_and_and_still_work` | regression: `&` alone, `&&` |
| `tokenize_dup_in_context` | `cmd 2>&1` lexes as Word + DupErr + Word("1") |

### Parser (`src/command.rs::tests`)

| Test | Covers |
| --- | --- |
| `parse_dup_stdout_from_fd2` | `cmd >&2` → stdout = Dup{fd:1, source:lit("2")} |
| `parse_dup_stderr_from_fd1` | `cmd 2>&1` → stderr = Dup{fd:2, source:lit("1")} |
| `parse_and_redir_out_desugars` | `cmd &>file` → stdout=Truncate(file), stderr=Dup{2,"1"} |
| `parse_and_redir_append_desugars` | `cmd &>>file` → stdout=Append(file), stderr=Dup{2,"1"} |
| `parse_dup_with_var_target` | `cmd 2>&$FD` → source Word has Var part |
| `parse_dup_in_pipeline_stage` | `cmd 2>&1 \| grep` → stage 0 has dup, stage 1 doesn't |
| `parse_combined_dup_and_file_redirect` | `cmd >file 2>&1` → both redirects present |

### Integration (new `tests/fd_dup_integration.rs`)

| Test | Script | Expected |
| --- | --- | --- |
| `dup_stderr_to_stdout_canonical` | `sh -c 'echo stderr-msg >&2' 2>&1\nexit\n` | `stderr-msg` appears on stdout |
| `dup_stdout_to_stderr` | `echo hi 1>&2 2> /tmp/v29_$$\ncat /tmp/v29_$$\nrm -f /tmp/v29_$$` | `hi` appears in the file |
| `combined_redirect_canonical_form` | `sh -c 'echo out; echo err >&2' >/tmp/v29c_$$ 2>&1\nwc -l < /tmp/v29c_$$\nrm -f /tmp/v29c_$$` | `2` (both lines in file) |
| `and_redir_out_to_file` | `sh -c 'echo out; echo err >&2' &>/tmp/v29a_$$\nwc -l < /tmp/v29a_$$\nrm -f /tmp/v29a_$$` | `2` |
| `and_redir_append_to_file` | `echo first > /tmp/v29ap_$$\nsh -c 'echo second; echo err >&2' &>>/tmp/v29ap_$$\nwc -l < /tmp/v29ap_$$\nrm -f /tmp/v29ap_$$` | `3` |
| `dup_in_pipeline_stage` | `sh -c 'echo a; echo b >&2' 2>&1 \| grep -c .\nexit\n` | `2` (grep sees both lines) |
| `dup_with_inline_assignment` | `FOO=hi sh -c 'echo $FOO >&2' 2>&1\nexit\n` | `hi` on stdout |
| `dup_with_subshell` | `(sh -c 'echo from-sub >&2') 2>&1\nexit\n` — note: outer `2>&1` on subshell will FAIL today (compound-command-redirect gap); use inner form: `(sh -c 'echo from-sub >&2' 2>&1)\nexit\n` | `from-sub` |
| `dup_runtime_bad_fd_target` | `sh -c true 2>&notanumber` | runtime error stderr (acceptable shape — non-zero exit + error message) |
| `echo_to_stderr_shorthand` | `echo error >&2 2> /tmp/v29s_$$\ncat /tmp/v29s_$$\nrm -f /tmp/v29s_$$` | `error` in the file |

### Doc updates

- `docs/bash-divergences.md`: M-18 → `[fixed (2026-05-26)]` with note about
  the order-divergence for the `2>&1 >file` anti-pattern. Add a new
  tracking entry for the order limitation (e.g. `B-11` or similar — a
  Tier 1 bug if it counts as one). Tier 2 count drops by 1.
- `README.md`: v29 row.

## Change log

- **2026-05-26**: Spec drafted; scope = `2>&1` / `1>&2` POSIX fd-dup
  + `&>file` / `&>>file` bash extensions; `Redirect::Dup { fd, source }`
  AST; dup2-in-child execution; documented order-divergence for
  `2>&1 >file` (non-canonical anti-pattern).
