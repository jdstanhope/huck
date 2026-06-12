# huck v143 — `cd` completion after sourcing bashrc (fixes) Design

**Status:** approved design, ready for implementation plan.
**Fixes:** two root-caused bugs in huck's PROGRAMMABLE-completion path (driving a
`complete -F`/compgen-based spec), which only surface after `source ~/.bashrc`
registers bash-completion's `complete -o nospace -F _cd cd`.
**Branch (impl):** `v143-cd-completion`.

## Background — the two bugs

Pre-bashrc, `cd <TAB>` uses huck's DEFAULT file completer (basenames, replacement
anchored after the last `/`) — works. After `source ~/.bashrc`, `cd` is routed
through `_cd` (a `complete -F` spec), which returns FULL-PATH candidates
(`projects/alpha`) like bash. Two bugs then appear:

1. **Mis-join (`cd projects/projects`).** `cd pr<TAB>` → `cd projects/` (ok), then
   TAB → `cd projects/projects` (wrong). Root cause: `analyze` (src/completion.rs:119-122)
   anchors the replacement offset AFTER the last `/` (it assumes BASENAME
   candidates, which the default completer returns). The spec path returns
   FULL-PATH candidates, so rustyline replaces the empty suffix after `projects/`
   with the common prefix `projects/` → `cd projects/projects/`. bash avoids this
   because its `cur` = the whole `projects/` word (COMP_WORDBREAKS excludes `/`),
   and the full-path candidate replaces that whole word.

2. **`cd ~/<TAB>` does nothing.** `_filedir` passes the literal quoted `~/` to
   `compgen -d`; huck's compgen dir enumerator `list_dir_with_path_prefix` /
   `list_dir_filtered` (src/completion_spec.rs:530,566) calls `read_dir("~/")`
   with NO tilde expansion → empty → no candidates. bash tilde-expands internally.
   (huck's standalone `compgen -d -- ~/` works only because the UNquoted `~/` is
   word-expanded before compgen sees it.)

**Out of scope (deferred):** `-o nospace` is parsed (`CompletionOptions.nospace`)
but never honored. This is a no-op in huck because huck never appends the trailing
space that `nospace` would suppress (rustyline `CompletionType::List` inserts the
replacement verbatim; the only append is `/` for directories). The `cd` descend
flow already gets nospace's effect for free. Truly honoring nospace would first
require implementing bash's default trailing-space-after-completion — a separate,
larger feature touching the working default path. Deferred as a new low-impact
divergence entry (see Documented divergences).

## Approach — anchor the programmable path at the word start (bash's model)

bash's contract for programmable completion: the candidate replaces the WHOLE
`cur` word `[word_start, pos)`, and every candidate is the full text that replaces
`cur`. huck's DEFAULT file completer (basenames + after-slash anchor) is left
exactly as-is; ONLY the spec/programmable path changes.

### Fix 1a — `analyze` exposes the word start
`analyze` (src/completion.rs:24) currently returns `(usize /*start*/,
CompletionContext)` where `start` is the after-slash offset for a File-with-slash
word. Change it to return the word start too. New shape:

```rust
pub struct Analysis {
    pub word_start: usize, // start of the whole word [word_start, pos)
    pub start: usize,      // basename anchor (after last '/'; == word_start when no '/')
    pub context: CompletionContext,
}
pub fn analyze(line: &str, pos: usize) -> Analysis
```

For each existing `return`: Variable → `start = word_start + name_off`; File-with-
slash → `start = word_start + slash + 1`; File-no-slash & Command → `start =
word_start`. `word_start` is the loop's existing `word_start` local in every case.

Update the in-crate callers:
- `dispatch::resolve` (src/completion.rs:382): destructure `Analysis`.
- `bashdefault_strings` (src/completion.rs:536): `let ctx = analyze(line, pos).context;`.
- The `analyze` unit tests (src/completion.rs ~708-740): read `.start`/`.context`.

### Fix 1b — the spec path anchors at `word_start`
In `dispatch::resolve` (src/completion.rs:419-427), the `Some(spec)` (programmable)
branch returns the candidates anchored at `word_start` instead of `start`:

```rust
match spec_opt {
    Some(spec) => {
        let cands = run_spec_with_empty_fallback(&spec, line, pos, &cmd_name, shell);
        // Programmable completion replaces the WHOLE cur word with full-path
        // candidates (bash's model). Anchor at word_start, not the basename
        // offset, so `cd projects/<TAB>` → `cd projects/alpha` (not …/projects).
        (analysis.word_start, cands)   // even when cands.is_empty()
    }
    None => {
        // Default file completion: basenames anchored after the last '/'.
        let home = shell.get("HOME").unwrap_or("").to_string();
        (start, complete_file(dir, prefix, &home))
    }
}
```

The `Variable` and `Command` paths (incl. the `-E` empty-spec at line 397) keep
`start` — there `start == word_start`, so no behavior change. Only the File-
position spec branch moves to `word_start`. For a no-slash File word (`cd pr`),
`word_start == start`, so the common case is unchanged; the new anchor only bites
when `cur` contains a `/`.

### Fix 1c — the empty-fallback emits FULL cur-relative paths
Under the `word_start` anchor, the spec path's `-o default` / `-o bashdefault`
empty-fallback must also produce full cur-relative replacement text, or a spec
using `-o default` on a path argument would mis-replace (drop the dir). Today they
return BASENAMES (`complete_file` returns basenames). Make them full-path:

- `file_completion_strings(cur_word, shell)` (src/completion.rs:527): split
  `cur_word` at the last `/` into `(dir, prefix)`, call `complete_file(dir, prefix,
  home)`, and prepend `dir` to each replacement → full cur-relative paths.
- `bashdefault_strings` File arm (src/completion.rs:552): likewise prepend `dir`
  to each `complete_file(&dir, &prefix, &home)` replacement.

(`complete_file` itself is unchanged — it still returns basenames for the default
path; the prepend happens in these two spec-path callers.)

### Fix 2 — tilde expansion in the spec/compgen path
bash-completion's `_cd`/`_filedir` feed a literal `~/…` to `compgen -d/-f`. Add a
small shared helper and tilde-expand at the two spec-path sites (the DEFAULT path's
`complete_file` already tilde-expands via `resolve_dir`, so it is untouched):

```rust
/// Replace a leading `~/` with `home/` (the only tilde form _filedir emits).
fn expand_tilde_prefix(s: &str, home: &str) -> String {
    match s.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => format!("{home}/{rest}"),
        _ => s.to_string(),
    }
}
```

- **2a — list entries** (`src/completion_spec.rs`): thread `home` from
  `enumerate_action` (which already has `shell: &Shell`, line 393) into
  `list_dir_with_path_prefix(prefix, dirs_only, home)`. There (line 566), after
  splitting `prefix` into `(dir, base)`, scan `expand_tilde_prefix(scan_dir, home)`
  via `list_dir_filtered`, but REASSEMBLE with the original `dir` so candidates
  come back as `~/projects` (matching bash). `home = shell.get("HOME").unwrap_or("")`.
- **2b — trailing `/` on `~/` dirs** (`src/completion.rs`): in
  `run_spec_with_empty_fallback`'s `filenames` rendering (line 495), the is-dir
  metadata check runs on the raw candidate; for a `~/projects` candidate
  `std::fs::metadata("~/projects")` fails → no `/` appended → the descend stalls.
  Tilde-expand before the metadata probe:
  `std::fs::metadata(expand_tilde_prefix(&name, &home))` (home from the shell). The
  emitted `replacement`/`display` keep the original `~/…` text; only the is-dir
  probe uses the expanded path. This lets `cd ~/<TAB>` → `cd ~/projects/` → TAB
  descends. NOTE: `escape_filename` escapes `~`, so the `filenames` rendering must
  preserve a LEADING `~/` unescaped (escape only the remainder) — otherwise the
  candidate becomes `\~/projects` (a literal `~` dir). The default path is
  unaffected (it escapes basenames, which never start with `~/`).

`expand_tilde_prefix` lives once (e.g. in `completion.rs`, re-exported to
`completion_spec.rs`, or duplicated minimally — implementer's call to keep it DRY).

## Behaviour matrix (target = bash, in a dir with `projects/{alpha,beta}/`)

| input (TAB) | before | after (this fix) |
|---|---|---|
| `cd pr` | `cd projects/` | `cd projects/` (unchanged) |
| `cd projects/` | `cd projects/projects/` ✗ | `cd projects/alpha` / `…/beta` listed ✓ |
| `cd projects/al` | mis-join | `cd projects/alpha/` ✓ |
| `cd ~/` | (nothing) ✗ | lists `~/<entries>`, dirs get `/` ✓ |
| `cd ~/pro` | (nothing) | `cd ~/projects/` ✓ |
| (pre-bashrc) `cd projects/al` | `cd projects/alpha/` | unchanged (default path) ✓ |

## Scope & must-not-regress
- The DEFAULT file-completion path (`None` branch in `dispatch::resolve` +
  `complete_file`) is UNCHANGED — pre-bashrc completion behaves exactly as before.
- Only the programmable/spec path (`Some(spec)`) moves to the `word_start` anchor
  and gains full-path empty-fallback + tilde expansion.
- `Variable`/`Command`/`-E` paths unchanged (`start == word_start` there).

## Documented divergences
- New low-impact `[deferred]` entry: **`-o nospace` is a no-op / no default
  trailing space after completion** — huck never appends the trailing space bash
  adds after a final completion, so `complete -o nospace` has nothing to suppress;
  honoring it needs the default trailing-space feature first. Low impact (the dir
  descend flow is unaffected). (No existing M-/L- entry covers this.)

## Files & responsibilities

| File | Change |
|------|--------|
| `src/completion.rs` | `analyze` → returns `Analysis{word_start,start,context}` (update callers + tests); `dispatch::resolve` spec branch anchors at `word_start`; `file_completion_strings` + `bashdefault_strings` File arm → full cur-relative paths; `filenames` is-dir probe tilde-expands; `expand_tilde_prefix` helper. |
| `src/completion_spec.rs` | `enumerate_action` threads `home` into `list_dir_with_path_prefix`; that fn tilde-expands the scan dir (reassembles with original `~/`). |
| `src/completion.rs` / `src/completion_spec.rs` mod tests | The core anchor/tilde tests live in-crate as `#[test]`s (dispatch is `pub(crate)`, so the binary can't drive completion). No separate integration `.rs` file — the binary-observable building block (`compgen`) is covered by the bash-diff harness below. |
| `tests/scripts/cd_completion_diff_check.sh` (NEW, 63rd) | Bash-diff over `compgen -d -- ~/…` / `compgen -f -- ~/…` (and slash-prefix `compgen -d -- projects/`). |
| `docs/bash-divergences.md` | New low-impact entry for the nospace/no-default-trailing-space deferral. |

## Testing

1. **`analyze` unit tests** (src/completion.rs mod tests): `analyze("cd projects/sub", pos)`
   returns `word_start` = offset of `projects/` and `start` = offset after the
   slash; for `cd pr`, `word_start == start`.
2. **Anchor unit test** (src/completion.rs mod tests, can call `pub(crate)
   dispatch::resolve`): register a synthetic `complete -F _fake cd` whose `_fake`
   sets `COMPREPLY=(projects/alpha projects/beta)` (NO dependency on the system
   bash-completion file); make a scratch tempdir with `projects/{alpha,beta}` and
   `cd` into it; call `dispatch::resolve("cd projects/", 12, &mut shell)` → assert
   the returned start == word_start (offset of `projects/`, = 3) and the candidates
   are the full paths `projects/alpha`/`projects/beta`. Add a `cd pr` case asserting
   start == word_start (no regression). Also a `-o default` empty-fallback case: a
   spec returning nothing on a `dir/<TAB>` falls back to full cur-relative paths
   (not basenames).
3. **Tilde unit test** (src/completion_spec.rs mod tests): set the shell's `HOME`
   to a tempdir containing `projects/` and `.config/`; `enumerate_action(Action::
   Directory, "~/", &shell)` → returns `~/projects` (and `~/.config` when the base
   starts with `.`); `enumerate_action(Action::Directory, "~/pro", &shell)` →
   `~/projects`. Confirm reassembly keeps the literal `~/` prefix.
4. **Bash-diff harness** `tests/scripts/cd_completion_diff_check.sh` (63rd):
   compare huck vs bash for `compgen -d -- ~/`, `compgen -f -- ~/`, `compgen -d --
   projects/` (run from a fixed scratch dir / a stable `$HOME`), asserting
   byte-identical sorted output. (Completion APPLICATION through readline is not
   harness-testable; this covers the compgen building block.)
5. **Full regression:** entire suite + ALL harnesses green; ESPECIALLY the existing
   completion tests (`completion*`/`compgen` unit tests + the PTY completion tests)
   — the default path must be untouched. `clippy` clean.
6. **Payoff (manual/PTY note):** after sourcing bash-completion + `complete -o
   nospace -F _cd cd`, `cd projects/<TAB>` descends and `cd ~/<TAB>` lists $HOME —
   verified via the synthetic-`_fake` anchor test + the tilde tests (the real `_cd`
   path is the same dispatch). An optional PTY test may assert the end-to-end
   descend if cheap; otherwise the unit tests + harness are the gate.

## Edge cases & notes
- A no-slash `cur` (`cd pr`) is unaffected: `word_start == start`.
- huck's COMP_WORDBREAKS is whitespace-only, so `analyze`'s `word_start` coincides
  with the COMP word start used by `-F` completers — the `word_start` anchor and
  the `$cur` passed to the function agree. (If COMP_WORDBREAKS is ever enriched,
  revisit.)
- Only `~/` (the form `_filedir` emits) is expanded; bare `~`/`~user` tilde
  completion is a separate, out-of-scope feature.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the
  controller verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
