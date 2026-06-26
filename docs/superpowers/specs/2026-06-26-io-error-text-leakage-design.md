# v229 — Rust io::Error text leakage — Design

**Status:** approved (brainstorm 2026-06-26)
**Iteration goal:** Stop huck leaking Rust error text into error messages — the
`std::io::Error` Display suffix ` (os error N)` and the synthesized
`stream did not contain valid UTF-8` — so the message body matches bash's bare
`strerror` / `cannot execute binary file`. Bundle the bash error-prologue
conversion on the file-IO error sites (cd, redirect-open, source) so the
category io-error LINES actually shrink. Broad-shrink iteration — no bash-test
category is expected to flip (each retains other blockers), but the io-error
lines drop out of alias / execscript / errors / dirstack / builtins.

## Background & measurement

v227/v228 identified two broad shared blockers; v228 took command-not-found word
order, v229 takes the other: Rust io::Error text leakage. Measurement (bash
5.2.21, file mode):

| case | huck now | bash |
|---|---|---|
| `cd /no/such` | `huck: cd: /no/such: No such file or directory (os error 2)` | `<src>: line N: cd: /no/such: No such file or directory` |
| `cd /etc/hostname` | `huck: cd: …: Not a directory (os error 20)` | `<src>: line N: cd: …: Not a directory` |
| `cat < /no/such` | `huck: /no/such: No such file or directory (os error 2)` | `<src>: line N: /no/such: No such file or directory` |
| `echo hi > /etc` | `huck: /etc: Is a directory (os error 21)` | `<src>: line N: /etc: Is a directory` |
| `. /no/such` | `huck: .: /no/such: file not found` | `<src>: line N: /no/such: No such file or directory` |
| `. /unreadable` | `huck: .: …: Permission denied (os error 13)` | `<src>: line N: …: Permission denied` |
| `. /etc` (a dir) | `huck: .: /etc: file not found` | `<src>: line N: .: /etc: is a directory` |
| `. /bin/true` (binary) | `huck: .: …: stream did not contain valid UTF-8` | `<src>: line N: .: …: cannot execute binary file` |

Two divergence classes co-occur on these lines: (1) the **io-text** (`(os error
N)` suffix; the synthesized UTF-8 string; huck's "file not found" where bash says
"No such file or directory" / "is a directory"); (2) the **prologue** (`huck:` vs
`<src>: line N:`). Per-category io-error line counts (pre-v229): alias 2,
execscript 4, errors 3, dirstack 1, builtins 1. Because both classes diverge on
each line, BOTH must be fixed on a site for that line to match bash — hence the
prologue is bundled with the io-text fix on the file-IO sites.

## Part 1 — the io helper (shell-wide io-text fix)

Add a shared helper that renders an io::Error the way bash does — the bare
`strerror`, without Rust's ` (os error N)` Display suffix:

```rust
/// Render an io::Error like bash: the bare strerror string, dropping Rust's
/// ` (os error N)` Display suffix. Rust-synthesized errors (no errno) keep
/// their Display text. The Display of an OS error is the documented
/// `"{strerror} (os error {errno})"`, so stripping that exact suffix yields
/// the same text bash gets from strerror(errno).
pub(crate) fn bash_io_error(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => {
            let s = e.to_string();
            match s.strip_suffix(&format!(" (os error {n})")) {
                Some(stripped) => stripped.to_string(),
                None => s,
            }
        }
        None => e.to_string(),
    }
}
```

Location: `crates/huck-engine/src/macros.rs` (the existing error-output helper
home, alongside the `e!` macro), exported `pub(crate)`.

Apply `bash_io_error(&e)` at **every** io::Error `{e}` formatting site. The known
sites (verify each `e` is a `std::io::Error` before converting — `printf`'s
`{e}` at builtins.rs:3431/3548 is a `parse_format` error, NOT io::Error, and must
be left alone):

- builtins.rs: cd (405, 411, 428, 444), pwd (502), echo (522, 528), unset (753,
  762 — verify io), export (979 — verify), readonly (1621 — verify), mapfile
  (2523, 2544), read (2703), jobs (3675), source `.` (6164 — see Part 4),
  pushd (7267), popd (7326), dirs (7392)
- expand.rs: redirection/expansion io errors (430, 794, 816, 847, 1237) — verify
- executor.rs: pipe (579, 596), fork (617), redirect-open (934, 951, 962, 996),
  heredoc (1031)
- history.rs: 144, 166

Sites that ALSO get a prologue conversion (Parts 2–4) compose the helper into
their new format; the rest keep their existing `huck:`/label prologue and only
swap `{e}` → `{bash_io_error(&e)}` (their prologue is deferred — see Non-goals).
Any `{e}` site whose `e` is not an io::Error is out of scope.

## Part 2 — cd error sites (prologue + io)

The three cd error sites (builtins.rs:405, 428, 444) become:
```rust
e!(err, "{}{target}: {}", shell.error_prefix(Some("cd")), bash_io_error(&e));
```
(444 has no `target` — `{}{}`, `error_prefix(Some("cd"))` + `bash_io_error(&e)`).
File-mode result: `<src>: line N: cd: <target>: <strerror>` — byte-identical to
bash. The cd "warning: could not read current dir" (411) keeps its wording but
swaps `{e}` → `bash_io_error` (verify bash's exact warning text in the harness;
if it diverges beyond the suffix, leave the prologue and just strip the suffix).

## Part 3 — redirect-open error sites (prologue + io)

The path-bearing redirect-open sites in executor.rs (934, 951, 962) become:
```rust
e!(&mut *err, "{}{path}: {}", shell.error_prefix(None), bash_io_error(&e));
```
(951 uses `resolved_path(&resolved)` for the path). File-mode result:
`<src>: line N: <path>: <strerror>` — byte-identical to bash. The path-less
redirect error (996, `huck: {e}`) and heredoc (1031) get the helper only
(`{bash_io_error(&e)}`); their exact bash format is verified in the harness and,
if it is more than a suffix difference, left for a follow-up (note in the plan).
`shell` must be in scope at these sites (confirm during implementation; if a
redirect site lacks `shell`, apply helper-only there and note it).

## Part 4 — source (`.`) builtin (prologue + io + binary-exec)

bash distinguishes file-OPEN failures (reported like a redirect, NO `.:`) from
opened-but-unusable files (reported WITH `.:`):

| case | bash | huck site | fix |
|---|---|---|---|
| not found | `<src>: line N: <path>: No such file or directory` | 6156 (`resolve_source_path`→None) | `{error_prefix(None)}{filename}: No such file or directory` |
| a directory | `<src>: line N: .: <path>: is a directory` | 6156 (resolve→None today) | `{error_prefix(Some("."))}{filename}: is a directory` |
| permission/open io error | `<src>: line N: <path>: <strerror>` | 6164 (`read_to_string` Err, has errno) | `{error_prefix(None)}{path}: {bash_io_error(&e)}` |
| binary (non-UTF-8) | `<src>: line N: .: <path>: cannot execute binary file` | 6164 (`read_to_string` Err, `ErrorKind::InvalidData`) | `{error_prefix(Some("."))}{path}: cannot execute binary file` |

Implementation:
- **6156 (resolve→None):** today emits `huck: .: {filename}: file not found` for
  BOTH genuinely-missing files and directories. Distinguish: if the filename
  refers to an existing directory (`std::path::Path::new(filename).is_dir()`, or
  re-stat the resolved candidate) → the directory case
  (`error_prefix(Some("."))` + `is a directory`); otherwise the not-found case
  (`error_prefix(None)` + `No such file or directory`). Keep `posix_fatal(1)` and
  `Continue(1)`.
- **6164 (read_to_string Err):** branch on `e.kind()`: `ErrorKind::InvalidData`
  (the non-UTF-8 / binary case) → `error_prefix(Some("."))` + `path: cannot
  execute binary file`; any other kind (permission, etc.) → `error_prefix(None)`
  + `path: bash_io_error(&e)`. Keep `Continue(1)`.

The `huck: .: usage` (6141) and `maximum source depth` (6148) messages are NOT
io errors and are out of scope (builtin-usage prologue, a separate program).

binary-exec is fully covered by the 6164 `InvalidData` branch; the only path that
reads a file as a script and hits the UTF-8 error is `source_in_sink`'s
`read_to_string` (6161). No other script-read path exists (a directly-executable
binary is exec'd, not read).

## Non-goals

- The `huck:` prologue on the non-file-IO io sites (pwd, echo, read, mapfile,
  jobs, history, pushd/popd/dirs, pipe/fork/heredoc, expansion): they get the io
  helper now (suffix removed), but their prologue conversion is later staged
  prologue work. They rarely error and are not category-relevant; accepting a
  small future prologue touch there.
- The L-42 / L-08 redirect-routing order divergence (a `2>/dev/null` on a later
  failing redirect) — unaffected; this iteration changes message TEXT only.
- The source `.: usage` / `maximum source depth` messages (builtin-usage
  prologue, separate program).
- Any `{e}` site whose error is not a `std::io::Error` (e.g. printf's
  `parse_format` error).

## Testing & verification

- **Unit test** for `bash_io_error`: an OS error (e.g.
  `io::Error::from_raw_os_error(2)`) → `"No such file or directory"` (no suffix);
  a synthesized error (`io::Error::new(ErrorKind::InvalidData, "x")`) → keeps its
  Display.
- **New harness** `tests/scripts/io_error_diff_check.sh` — file mode,
  byte-identical stdout+stderr+rc vs bash (same temp path), covering: cd missing,
  cd into a file, redirect read-from-missing, redirect write-to-directory,
  source not-found, source a directory, source unreadable (permission — run only
  if not root, else skip), source a binary (`. /bin/true`). Each asserts the full
  `<path>: line N: …` line matches bash.
- **Regression:** `cargo test --workspace` (0 failed) + all `tests/scripts/*_diff_check.sh`
  harnesses (`funcnest_diff_check.sh` release-only). Watch the existing
  redirect / source / cd harnesses for any pinned old text.
- **Category re-measure:** alias / execscript / errors / dirstack / builtins —
  the io-error lines (`(os error N)`, `stream did not contain valid UTF-8`,
  `file not found` for a directory) must drop out of each diff; no category may
  regress to TIMEOUT/ERROR. No flip is expected.

## Risks

- **Helper breadth:** ~30 sites; a `{e}` that is not io::Error (printf) must be
  skipped. Each converted site is verified by the workspace sweep + harness.
- **Source restructuring** (Part 4) is the intricate part: the directory-vs-not-
  found distinction at 6156 and the kind-branch at 6164. The harness pins all
  four source forms byte-identically.
- **`shell` scope at redirect sites:** `error_prefix` needs `&shell`; if a
  redirect-open site lacks it, fall back to helper-only there and record it.
- **Permission test as root:** the source-unreadable case can't be exercised as
  root (root bypasses mode 000); the harness skips it when `id -u` is 0.
