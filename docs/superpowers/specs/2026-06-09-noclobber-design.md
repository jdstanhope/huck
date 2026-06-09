# huck v123 — `noclobber` (`set -C` / `set -o noclobber`) + `>|` redirect (M-21) Design

**Status:** approved design, ready for implementation plan.
**Implements:** M-21 in full — the `noclobber` shell option (`set -C` /
`set -o noclobber`) that refuses to truncate an existing regular file via `>`,
plus the `>|` (and `1>|` / `2>|`) "force-clobber" redirect operators that
override it.
**Why now:** `cmd >| file` is currently a parse error
(`expected a filename after redirection` — the lexer emits `>` then a bare `|`
pipe). mise's generated completion uses `mise usage >| "$spec"`. More broadly,
`noclobber` is a standard, commonly-set bash safety option and `>|` is the
idiomatic escape hatch scripts use against it; both are listed as not-yet-done
in the README and bundled under M-21.
**Branch (impl):** `v123-noclobber`.

## Background — probed bash 5.x semantics (this session)

All of the following were probed against the system bash and are **observable**,
so all are in scope:

| case | bash result |
|---|---|
| `set -C; echo new > f` where `f` exists & is regular | `bash: …: f: cannot overwrite existing file`, **rc 1**, file untouched |
| `set -C; echo new >\| f` (force) | overwrites, rc 0 |
| `set -C; echo more >> f` | appends, rc 0 (append always allowed) |
| `set -C; echo new > newfile` (does not exist) | creates, rc 0 |
| `set -C; echo x > /dev/null` (and FIFOs) | **rc 0** — non-regular files are exempt |
| `set -C; ... 2>\| f` | force-clobber on fd 2 |
| `set -C; echo new &> f` where `f` exists & regular | **fails** rc 1 — the `>file` part of `&>` is subject to noclobber |
| `&>\|` | **syntax error** in bash — no such operator |
| `set -C; set +C; echo new > f` | clobbers (toggle off works) |
| `set -C; echo new > symlink-to-regular` | **fails** — bash follows the symlink, sees a regular file |
| noclobber **off**: `echo new > f` (exists) | clobbers (today's huck behavior; unchanged) |

Derived rules:
- `noclobber` (default off) gates **only** the truncating `>` open (and the
  `>file` half of `&>`). `>>` (append) and `>|` (force) are never gated.
- When gated and the target **exists as a regular file** → error
  `cannot overwrite existing file`, exit status 1, target untouched.
- When gated and the target **does not exist** → create normally.
- When gated and the target **exists but is not a regular file** (device, FIFO,
  socket, directory-handled-elsewhere) → open for write **without** truncation
  (the `/dev/null` exemption). bash determines "regular" by following symlinks.
- `set -C` ⇔ `set -o noclobber`; `set +C` ⇔ `set +o noclobber`. `$-` gains `C`.

### `$-` note (pre-existing partial — NOT made byte-identical)
huck's `$-` already diverges from bash: bash file-mode `$-` includes `h`
(hashall) and `B` (braceexpand) which huck does not emit (`set -f` → bash `fhB`,
huck `f`). v123 adds `C` to huck's `$-` when noclobber is on, placed in the
trailing (uppercase) position, so `set -Cf; echo $-` → huck `fC` vs bash `fhBC`
— differing only by the pre-existing `hB` gap. We therefore **do not** put a
bare `echo $-` in the byte-identical diff harness; a huck-only test asserts `$-`
contains `C`.

## Current wiring (explored this session)

- **Lexer** `src/lexer.rs:670` — the `>` arm peeks for `>` (→ `RedirAppend`)
  and `&` (→ `DupOut`), else `RedirOut`; a trailing `|` falls through to plain
  `RedirOut`, leaving `|` to be lexed as a pipe → parse error. The `1>` arm
  (`:689`) and `2>` arm (`:703`) mirror this.
- **AST** `src/command.rs:252` — `Redirect::{Read, Truncate(Word),
  Append(Word), Heredoc, HereString, Dup}`. The parser
  (`src/command.rs:1634-1657`) maps `RedirOut→stdout=Truncate`,
  `RedirErr→stderr=Truncate`, and `AndRedirOut` (`&>`) →
  `stdout=Truncate(target)` + stderr dup. `is_redirect_op`
  (`src/command.rs:1597`) gates which operators enter this loop.
- **Open sites** — the truncating open (`OpenOptions::new().write(true)
  .create(true).truncate(true)`) appears in:
  - `open_resolved` `src/executor.rs:2459` (the `ResolvedRedirect` path used by
    forked pipeline stages); `ResolvedRedirect` is defined at `:2026` as
    `Truncate(String)` / `Append(String)`, built at `:564`, `:2159`, `:2184`.
  - inline builtin/single-stage paths at `src/executor.rs:1655`, `:1718`,
    `:3388`, `:3447` (these match a raw `Redirect` and have `shell` in scope).
  None consult `noclobber` today.
- **Options** — `ShellOptions` (`src/shell_state.rs:107`) has explicit bool
  fields (`errexit/nounset/pipefail/verbose/xtrace/noglob`); `noclobber` is
  **not** a field. `option_get`/`option_set` (`src/builtins.rs:4307`/`:4321`)
  return `Unimplemented` for `noclobber` (it is in `SETO_TABLE` at `:4279` with
  `default:false`). The `set` short-flag loop (`src/builtins.rs:4423`) handles
  `e/f/u/x`; `dollar_dash_value` (`src/shell_state.rs:427`) emits `e f i u v x`.

This is exactly the shape `noglob` had before v120 — the option plumbing
follows that precedent.

## Architecture

Three independent units: (1) lex/parse `>|`; (2) wire the `noclobber` option;
(3) a single guarded-open helper that all truncating opens funnel through.

### Unit 1 — `>|` surface syntax

**Lexer** (`src/lexer.rs`): in each of the `>`, `1>`, `2>` arms, add a
`'|'` peek branch **before** the `else` (order: `>`/`&`/`|`/else):
- `>` arm and `1>` arm: `Some(&'|')` → consume → `Operator::RedirClobber`.
- `2>` arm: `Some(&'|')` → consume → `Operator::RedirErrClobber`.

**Operators** (`src/lexer.rs`): add `RedirClobber`, `RedirErrClobber` to the
`Operator` enum (and any `Display`/`as_str` impl if present, formatting as `>|`
and `2>|`).

**AST** (`src/command.rs`): add variant
```rust
/// `>|file` — force truncate, overriding `noclobber` (`set -C`).
Clobber(Word),
```
to `enum Redirect`.

**Parser** (`src/command.rs`): add to `is_redirect_op` and to the match:
```rust
Operator::RedirClobber    => stdout = Some(Redirect::Clobber(target)),
Operator::RedirErrClobber => stderr = Some(Redirect::Clobber(target)),
```

Scope of fds: `>|`, `1>|`, `2>|` only (matching huck's existing fd-1/fd-2
redirect support). Arbitrary `n>|` and `&>|` are out of scope (`&>|` is a bash
syntax error; huck has no general `n>` redirect today).

### Unit 2 — `noclobber` option

- `src/shell_state.rs`: add `pub noclobber: bool` to `ShellOptions` (after
  `noglob`); initialize `false` everywhere `ShellOptions` is constructed
  (the `..Default`/explicit-init sites — there is one near `:446`).
- `dollar_dash_value` (`src/shell_state.rs:427`): append
  `if self.shell_options.noclobber { out.push('C'); }` **after** the lowercase
  letters (i.e. after the `x` push) — trailing uppercase position.
- `option_get` (`src/builtins.rs:4307`): add
  `"noclobber" => Some(shell.shell_options.noclobber),`.
- `option_set` (`src/builtins.rs:4321`): add
  `"noclobber" => { shell.shell_options.noclobber = value; Ok(()) }`.
- `set` short flags (`src/builtins.rs:4423`): add `b'C'` to the `-`-flag set
  (`shell.shell_options.noclobber = true`) and the `+`-flag clearing path
  (`= false`), symmetric to how `f` is handled.

This makes `set -C` / `set +C` / `set -o noclobber` / `set +o noclobber` all
toggle the one bool, surfaced in `$-`, `set -o`, and `[[ -o noclobber ]]`
(the existing OptEnabled test already reads `option_get`).

### Unit 3 — guarded-open helper + open-site routing

**Helper** (`src/executor.rs`, free function near `open_resolved`):
```rust
fn open_writable(path: &str, guard_noclobber: bool) -> io::Result<File> {
    if !guard_noclobber {
        // current behavior: truncate-or-create
        return OpenOptions::new().write(true).create(true).truncate(true).open(path);
    }
    // noclobber: refuse to clobber an existing regular file (O_EXCL),
    // but exempt non-regular files (e.g. /dev/null, FIFOs) — matching bash.
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            match std::fs::metadata(path) {           // follows symlinks
                Ok(md) if !md.is_file() => {
                    // special file: open for write, no O_EXCL / no truncate
                    OpenOptions::new().write(true).open(path)
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "cannot overwrite existing file",
                )),
            }
        }
        Err(e) => Err(e),
    }
}
```
The error's `Display` is exactly `cannot overwrite existing file`, so existing
callers' `eprintln!("huck: {path}: {e}")` produce
`huck: PATH: cannot overwrite existing file` (rc 1), matching bash modulo the
documented `huck:` vs `bash: line N:` prefix (L-01).

**`ResolvedRedirect`** (`src/executor.rs:2026`): add variant
`NoclobberTruncate(String)` (a `>` open that must honor noclobber). The
resolve sites (`:564`, `:2159`, `:2184` — they have `shell` access) map:
- `Redirect::Truncate(w)` → if `shell.shell_options.noclobber`
  → `NoclobberTruncate(path)` else `Truncate(path)`.
- `Redirect::Clobber(w)` → `Truncate(path)` (force is never guarded).
- `Redirect::Append(w)` → `Append(path)` (unchanged).

`open_resolved` (`:2459`) routes:
- `Truncate(p)` → `open_writable(p, false)`
- `NoclobberTruncate(p)` → `open_writable(p, true)`
- `Append(p)` → unchanged append open.
`resolved_path` (`:2473`) adds the `NoclobberTruncate` arm.

**Inline open sites** (`src/executor.rs:1655`, `:1718`, `:3388`, `:3447`):
these match a raw `Redirect` with `shell` in scope. Extend each
`Some(Redirect::Truncate(w))` arm to also accept `Clobber`, computing the
guard, and replace the `OpenOptions…truncate…` call with `open_writable`:
```rust
Some(r @ (Redirect::Truncate(w) | Redirect::Clobber(w))) => {
    let force = matches!(r, Redirect::Clobber(_));
    let guard = shell.shell_options.noclobber && !force;
    // … expand w to `path` exactly as today …
    match open_writable(&path, guard) { Ok(f) => …, Err(e) => { eprintln!("huck: {path}: {e}"); … } }
}
```
(Each site keeps its existing error-cleanup block verbatim; only the pattern
and the open call change.)

**`&>` is covered transitively**: it lowers to `stdout = Truncate(target)`, so
its truncate honors noclobber automatically. No special handling.

**Classification sites**: any match that lists `Truncate` for "is this a
stdout/stderr write redirect?" (e.g. `src/executor.rs:493`, `:2145`) gains
`| Redirect::Clobber(_)` so `>|` is treated like `>` for fd-targeting and the
"never a stdin redirect" guard.

## Scope & correctness
- noclobber **off** (the default and only prior state) → `guard` is always
  false → every open is the exact pre-v123 `truncate(true)` open. **Zero
  behavior change** when the option is off.
- `>|` always opens with `guard=false` → identical to today's `>` regardless of
  noclobber.
- Only the truncating `>`/`&>`-stdout open is gated. `>>`, `<`, dup, heredoc,
  here-string untouched.
- The regular-file test uses `fs::metadata` (follows symlinks), matching bash's
  symlink-to-regular behavior.

## Must-not-regress
- All existing redirect behavior with noclobber off (the default): truncate,
  append, `&>`, `2>`, dup, heredoc, here-string — byte-identical.
- `set -o` table output, `set -e/-u/-f/-x`, `$-` for those flags.
- The `Redirect::Truncate(ww(...))` parser/exec unit tests (untouched — new
  variant, not a changed signature).
- `[[ -o optname ]]` for the other options.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `>|`/`1>|`/`2>|` lexing; `Operator::RedirClobber`/`RedirErrClobber` (+ Display); unit tests |
| `src/command.rs` | `Redirect::Clobber(Word)`; parser arms + `is_redirect_op`; unit tests |
| `src/shell_state.rs` | `ShellOptions.noclobber`; `dollar_dash_value` `C`; unit tests |
| `src/builtins.rs` | `option_get`/`option_set` noclobber; `set -C/+C` short flags; unit tests |
| `src/executor.rs` | `open_writable` helper; `ResolvedRedirect::NoclobberTruncate`; route open sites; classification arms; unit tests |
| `tests/noclobber_integration.rs` | NEW — the probed cases vs bash (file-arg per L-27) |
| `tests/scripts/noclobber_diff_check.sh` | NEW — 46th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | drop M-21; README: move `>|`/`set -C`/noclobber out of "not yet implemented" into the supported lists |

## Testing
1. **Unit**
   - `lexer`: `>|`, `1>|`, `2>|` tokenize to the new operators;
     `>>`/`>&`/`>` unaffected; `> |` (space) still parses as redirect-then-pipe.
   - `command`: `cmd >| f` parses to `stdout = Clobber`; `cmd 2>| f` →
     `stderr = Clobber`.
   - `shell_state`: `noclobber` toggles `$-`'s `C`; off by default.
   - `executor` `open_writable`: in a tempdir — guard creates a new file; guard
     errors `AlreadyExists`/"cannot overwrite existing file" on an existing
     regular file (and leaves it untouched); guard on `/dev/null` succeeds;
     `guard=false` truncates an existing file.
   - `builtins`: `set -C` then `option_get("noclobber")` true; `set +C` false.
2. **Integration** (`tests/noclobber_integration.rs`, binary vs bash,
   file-arg): the probed table — blocked `>` (rc 1, file intact + stderr text
   without the prefix), `>|` force, `>>` append, `>` new file, `> /dev/null`,
   `2>| f`, `&> f` blocked, `set +C` re-enables clobber, noclobber-off baseline.
   Assert byte-identical stdout and exit status; for the error line compare the
   text after the shell-prefix.
3. **46th bash-diff harness** `tests/scripts/noclobber_diff_check.sh` —
   ~8 fragments covering force/append/new/`/dev/null`/`&>`/toggle, byte-identical.
   (No bare `echo $-` — see the `$-` note.)
4. **Regression**: full suite, all 46 harnesses, `cargo clippy --all-targets`.
5. **Payoff smoke**: `printf 'set -C\nmise usage >| /tmp/o 2>/dev/null || echo ran\n'`
   parses and runs (the `>|` no longer a parse error); and a direct
   `set -C; echo hi >| existing` overwrites. Report honestly: this closes M-21
   (`>|` + noclobber); it does not by itself complete `mise<TAB>` (still the
   2.12 bash-completion env gap).

## Edge cases & notes
- **Directory target** `> somedir`: `open_writable` with guard true →
  `create_new` fails with `AlreadyExists`, `metadata` says not-a-file → it would
  try `OpenOptions::write(true).open(dir)` which fails with the OS "Is a
  directory" error — bash also errors here; the message differs but both are
  rc 1. Acceptable (not a regular file, naturally rejected by the OS open).
- **TOCTOU**: using `create_new` (O_CREAT|O_EXCL) is the same race-free
  primitive bash uses; the `metadata` retry for special files has a benign race
  identical in spirit to bash's stat-then-reopen.
- **Readonly/permission errors** propagate as today (the `Err(e)` passthrough).
- This does **not** implement `set -n` (noexec) or any other inert
  `SETO_TABLE` option — only `noclobber`.
