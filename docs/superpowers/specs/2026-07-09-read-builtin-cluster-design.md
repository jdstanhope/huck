# `read` builtin cluster — EOF status, IFS trailing delimiter, `-n`/`-N`, `-t`

**Status:** design (brainstormed 2026-07-09)
**Topic:** four real behavioral divergences in the `read` builtin, unified behind
one record-reader refactor. Resolves **B-02**, **B-03**, **M-162**, **M-163**
(docs/bash-divergences.md), all found in the 2026-07-09 differential sweep.

All facts below verified against bash 5.2 (file/pipe mode).

---

## 1. Problem

`read` is one of the most-used builtins; four gaps break real scripts:

- **B-02 — EOF exit status + variable clearing.** (a) `read` returns 0 when it
  reads data but hits EOF before the delimiter; bash returns 1 (`printf abc |
  read x` → bash rc 1, huck rc 0). This changes the ubiquitous `while read` / `if
  read` idiom on newline-less input. (b) On pure EOF (nothing read) bash sets
  each named var to empty; huck leaves them unchanged (`x=OLD; read x </dev/null`
  → bash `x=`, huck `x=OLD`).
- **B-03 — trailing IFS delimiter kept in the last variable.** `IFS=: read x y z`
  on `:a:b:` → bash `[][a][b]`, huck `[][a][b:]`.
- **M-162 — `read -n N` / `read -N N` unimplemented.** huck: `read: -n: invalid
  option`, reads nothing. `read -n1` (prompts, menus, byte reads) is common.
- **M-163 — `read -t TIMEOUT` unimplemented.** huck rejects `-t` (rc 2).

## 2. Architecture — one record reader

`read_one_line` (builtins.rs ~2187) hard-codes "read to delim/EOF" and returns
`Option<String>`, discarding *why* it stopped. All of B-02/M-162/M-163 need that
reason. Replace it with a config-driven record reader:

```rust
struct ReadCfg {
    raw: bool,            // -r: no backslash processing
    delim: u8,            // -d DELIM (default b'\n'; -d '' => NUL)
    delim_active: bool,   // false under -N (delimiter is an ordinary char)
    max_chars: Option<usize>, // Some(n) under -n/-N
    deadline: Option<std::time::Instant>, // Some under -t (>0)
}
enum ReadStop { Delim, Count, Eof, Timeout }
// Returns (decoded string, stop reason, any_byte_read).
fn read_record<R: Read + AsRawFd>(r: &mut R, cfg: &ReadCfg)
    -> std::io::Result<(String, ReadStop, bool)>
```

The reader loops byte-at-a-time (as today, required for the shared-fd-0 reason in
the existing comment). Per iteration:

1. If `deadline` is set, `poll(2)` the fd for the remaining time; on expiry return
   `(buf, Timeout, any)`.
2. Read one byte. `n == 0` → return `(buf, Eof, any)`.
3. If `delim_active && byte == delim` → return `(buf, Delim, any)`.
4. Non-raw backslash handling (unchanged from `read_one_line`: `\<newline>` =
   continuation, `\X` → `X`, trailing `\` kept). Each *committed char* increments
   the character counter.
5. Otherwise push the byte; when a full UTF-8 char boundary is committed,
   increment the char counter. If `max_chars == Some(k)` and the counter reaches
   `k`, return `(buf, Count, any)`.

`read_one_record` (the mapfile helper at ~2164) is left as-is (separate caller);
only `read_one_line`'s callers move to `read_record`.

Character counting decodes UTF-8 incrementally: a byte that completes a scalar
value (or a lone invalid byte, counted as one — huck is lossy elsewhere) bumps the
count. This matches bash counting characters, not bytes (`read -N 3` on `héllo`
→ `hél`).

## 3. builtin_read — option parsing and status mapping

New options in the existing parse loop (builtins.rs ~2658):

- `-n` / `-N`: take a value (like `-u`/`-a`/`-d` via `take_opt_value`), parse as
  `usize`. Parse failure → `read: <val>: invalid number`, rc 1. `-n` sets
  `max_chars` + leaves `delim_active=true`; `-N` sets `max_chars` +
  `delim_active=false`. If both appear, last wins (bash: `-N` and `-n` share the
  nchars slot; last-specified governs).
- `-t`: take a value, parse as `f64 >= 0`. Failure/negative →
  `read: <val>: invalid timeout specification`, rc 1. `-t 0` → availability-probe
  mode (see §5). `-t N` (N>0) → `deadline = Instant::now() + N`.

Status mapping from `(string, stop, any)`:

| stop | exit | assignment |
|---|---|---|
| `Delim` | 0 | assign split fields |
| `Count` | 0 | assign split fields |
| `Eof` (any=true) | 1 | assign split fields (B-02a) |
| `Eof` (any=false) | 1 | assign EMPTY to each named var (B-02b); `REPLY=""` if no names |
| `Timeout` | 142 (`128 + libc::SIGALRM`) | assign split fields of partial data |

A readonly-assignment failure still forces exit 1 (as today). The pure-EOF path
must now still run the assignment loop with an empty line (so vars are cleared)
instead of the current early `return Continue(1)`.

## 4. B-03 — last-field trailing-delimiter rule

In `split_into_names`, the multi-name last field (builtins.rs ~2327) currently
strips only trailing IFS-whitespace. bash's rule (derived from a 12-row matrix,
2 vars / `IFS=:`):

| remainder | bash last var |
|---|---|
| `` (empty) | `` |
| `:` | `` |
| `::` | `::` |
| `:::` | `:::` |
| `b:` | `b` |
| `b::` | `b::` |
| `b:c` | `b:c` |

Rule: after stripping trailing IFS-whitespace, strip **one** trailing
non-whitespace IFS delimiter **iff it is the sole trailing delimiter** — i.e. the
character before it is not itself a non-whitespace IFS delimiter (or does not
exist). Two or more consecutive trailing delimiters → strip none. Then strip any
IFS-whitespace again. Concretely, on the ws-stripped remainder `R`:

```
if last(R) is non-ws-IFS AND (len(R)==1 OR char before last is NOT non-ws-IFS):
    drop last char; then drop trailing ws-IFS
```

The single-name path (`names.len()==1`) is UNCHANGED (strips only leading+trailing
IFS-whitespace — `IFS=: read x` on `a:b:` → `a:b:`, verified). `split_read_fields`
(the `read -a` unbounded splitter) is UNCHANGED.

## 5. `-t 0` availability probe

`-t 0` performs a single `poll(2)` with a 0 timeout and reads nothing:

- readable (or a regular file, always ready) → rc 0.
- not readable / EOF on a pipe → rc 1.

Inherently timing-dependent on pipes (matches bash's `select`-based probe; the
pipe-buffer race is not deterministic — an accepted caveat). No variable is
assigned.

## 6. TTY handling (best-effort caveat)

For `-n`/`-N`/`-t` on a terminal, bash switches to non-canonical mode for
per-keystroke behavior. huck reuses the existing `-s` termios machinery to
additionally clear `ICANON` (and `ECHO` when `-s`) for the read's duration, then
restores. Byte-exactness is GUARANTEED only for pipes/files/redirects (the
scriptable paths the harness covers); exact interactive terminal timing is
out of scope.

## 7. Non-goals / accepted divergences

- Exact interactive TTY timing for `-n`/`-t` (best-effort; §6).
- `read -t 0` pipe-race determinism (§5).
- Multibyte counting in a non-UTF-8 locale (huck is UTF-8-oriented; counts
  Unicode scalar values, a lone invalid byte = one char).
- `read -e` (readline editing) — separate, not in this cluster.

## 8. Success criteria

1. `read_cluster_diff_check.sh` byte-identical (stdout + rc) to bash across:
   B-02 (partial-line rc, pure-EOF rc + var clear, multi-var clear); B-03 (~20
   IFS/trailing-delimiter rows incl. the §4 table + `IFS=":"`, `IFS=": "`, default
   ws-IFS); `-n`/`-N` (counts, delimiter-stop vs literal, UTF-8, `-n 0`, EOF-short
   rc 1, leftover-in-stream, `-rn`); `-t` (data-ready, timeout→142, fractional,
   partial-assign, `-t 0` on a regular file, invalid `-n`/`-t` args).
2. No regression: `cargo test -p huck-engine` / `-p huck-syntax`.
3. No regression across the official bash-suite runner (PASS ≥ 15). Measure the
   `read`/`redir`/`vredir`/`procsub` categories (currently TIMEOUT/FAIL) — `-t`
   may unstick a read2.sub-style hang; not required to flip.
4. bash-divergences.md: delete **B-02, B-03, M-162, M-163**; add a `[deferred]`
   note for any residual (e.g. TTY interactive timing) if found.

## 9. Risks

- **Hot builtin.** `read` is pervasive; the reader refactor must preserve every
  current behavior (shared-fd-0 byte reader, `-r`/`-s`/`-p`/`-d`/`-a`/`-u`, line
  continuation, single-var trim). Gate: keep all existing `read` tests green plus
  the new matrix.
- **`poll` portability.** `-t` uses `libc::poll` on the raw fd; guarded `#[cfg(unix)]`
  like the existing termios code. Non-unix builds keep the no-timeout path.
- **Timeout rc constant.** `128 + libc::SIGALRM` (142 on Linux) — use the libc
  constant, not a hardcoded 142.
