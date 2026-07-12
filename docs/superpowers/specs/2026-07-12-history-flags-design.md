# v284 — Extend the `history` builtin: `history N` (#7) and `-d`/`-w`/`-r`/`-a` (#6)

**Issues:** [#7](https://github.com/jdstanhope/huck/issues/7) (`history N`) and
[#6](https://github.com/jdstanhope/huck/issues/6) (`-d`/`-w`/`-r`/`-a` flags).
The PR `Closes #7` and `Closes #6`. Both are `enhancement`+`divergence`, sev:low.

## Problem

huck's `history` builtin supports only bare `history` (list all) and `history -c`
(clear). Everything else errors `history: <arg>: invalid option`. bash supports a
listing count and file/edit flags. This adds the missing bash-compatible forms.

## Current state

- `builtin_history` (`crates/huck-engine/src/builtins.rs:5275`): no-arg lists all
  via `shell.history.entries()`; `-c` clears; else errors.
- `History` (`crates/huck-engine/src/history.rs:32`): `entries: Vec<String>`,
  `base_number: usize` (absolute number of `entries[0]`), `max: Option<usize>`,
  `file: Option<PathBuf>`. Methods: `add`, `clear`, `get(number)`, `entries()`
  (yields `(base_number+i, cmd)`), `load()` (reads `self.file`, REPLACES entries),
  `save_capped(cap)` (writes last `cap` entries to `self.file`, escaped).
  `escape_for_save`/`unescape_for_load` encode `\`→`\\`, newline→`\n` so each
  entry is one physical line.
- **Latent format divergence:** the row renderer uses `"{number:>5}\t{command}"`
  (TAB); bash uses `"%5d  %s"` (two spaces).

## Design

All behavior confined to `builtin_history` (rewritten as a small option parser)
plus new `History` methods. `history` records interactively (`repl.rs`), but the
flags operate on `shell.history` regardless of mode; `-r <file>` populates history
even non-interactively (matching bash), which is what makes the flags testable in
a script harness.

### Row format (fixes the latent divergence)

Render every listing row (bare `history` and `history N`) as bash does:
`format!("{number:>5}  {command}")` — a 5-wide right-justified number, **two
spaces**, the command. Replaces the current `\t`. Verify no existing test/harness
pins the tab; update any that do.

### `history N` (#7)

A single non-option numeric argument prints the last `N` entries (with numbers).

- `N` parses as a non-negative integer. `N == 0` prints nothing (bash). `N` larger
  than the list prints the whole list.
- A non-numeric bare argument that is not a recognized option is an error:
  `history: <arg>: invalid option` (unchanged from today for the non-numeric case).
- New `History::tail(n) -> impl Iterator<Item = (usize, &str)>`: the last `n`
  entries as `(absolute_number, command)`, oldest-first among the tail.

### `history -d` (#6) — delete, full bash forms

Accepts three operand forms (bash 5.2):

- **Single offset** `history -d N` — delete the entry with absolute number `N`.
- **Negative offset** `history -d -K` — delete the `K`-th entry from the end;
  `-1` = last. Resolved to absolute `last_number - K + 1`.
- **Range** `history -d START-END` — delete every entry with absolute number in
  `START..=END` (inclusive). `START`/`END` may each be negative (resolved as
  above). An empty/reversed range deletes nothing, rc 0 (bash).

Errors (rc 1, no deletion): a non-numeric operand, or an offset resolving outside
`base_number..=last_number`, emit `history: <operand>: history position out of
range` (matching bash, which uses this message even for non-numeric input). A
missing operand emits `history: -d: option requires an argument` (bash) rc 1.

Deletion renumbers the remaining entries contiguously — huck's `base_number+index`
model already yields this once an element is removed from `entries` (the entries
after the removed index shift their absolute number down by one; `base_number` is
unchanged). New `History` methods:

- `delete(number: usize) -> bool` — remove the entry with absolute `number`;
  `false` if out of range. If the removed index is `< unwritten_start`, decrement
  `unwritten_start` (see `-a`).
- `delete_range(start: usize, end: usize) -> usize` — remove all entries with
  absolute number in `start..=end`; returns the count removed. Implemented by
  deleting from the highest number down so indices stay valid.

### `history -w [file]`, `-r [file]`, `-a [file]` (#6) — file ops

Each takes an optional filename; with none, use the resolved histfile
(`History::file`). If no file is given and `History::file` is `None`, error
`history: cannot use history file` (bash-ish) rc 1. All use huck's existing
escape/unescape encoding (consistent with huck's own histfile round-trip via
`load`/`save_capped`); for single-line entries this is byte-identical to bash.

- **`-w [file]`** — write the **whole** current list to `file` (truncate). New
  `History::write_all_to(path) -> Option<String>` (warning body on I/O error, like
  `save_capped`). Sets `unwritten_start = entries.len()` (everything is now
  written).
- **`-r [file]`** — read `file`, unescape, **append** its lines to the current
  list (then `enforce_max`). New `History::read_append_from(path) -> Option<String>`.
  Read lines are on-disk-origin, so set `unwritten_start = entries.len()` after
  (they are not re-appended by a later `-a`). A missing file is an error
  (`history: <file>: <os error>`) rc 1 — bash reports it.
- **`-a [file]`** — **append** only the entries added since the last
  `-a`/`-w`/session start (`entries[unwritten_start..]`) to `file` (append mode,
  create if absent), then advance `unwritten_start = entries.len()`. New
  `History::append_new_to(path) -> Option<String>`.

New `History` field: `unwritten_start: usize` (init 0; reset to 0 by `clear`;
`add` leaves it, so freshly-added entries are "unwritten"; `set_max`/`enforce_max`
eviction of the oldest entries decrements `unwritten_start` by the number evicted,
saturating at 0, so it keeps pointing at the same logical boundary).

### Option parsing / dispatch

Rewrite `builtin_history` to parse leading options in a getopt-like loop over
`args`, matching bash's "process all options, then act" order:

1. Collect flags: `-c` (clear), `-d <operand>` (delete; consumes the next arg as
   its operand), and the file ops `-w`/`-r`/`-a` (each consuming an optional
   following non-option arg as the filename). An unknown `-X` →
   `history: -X: invalid option` + `history: usage: history [-c] [-d offset] [n]
   or history -anrw [filename] or history -ps arg [arg...]` rc 2 (bash usage).
   (`-p`/`-s`/`-n` are recognized as valid option letters for the usage string but
   **not implemented** — see Out of scope; if given, emit
   `history: -X: not yet implemented` rc 1 so they don't silently no-op. Decision:
   keep them rejected, not silently accepted.)
2. Apply in bash's order: `-c` first (clear), then `-d` (delete), then the file op.
3. If NO options were given and a single bare argument remains, it is the `N`
   listing count (or the invalid-option error if non-numeric). If no args at all,
   list everything.

Clarify one interaction: `history -c` combined with `-d`/file ops is allowed (bash
processes both); after `-c` the list is empty so a following `-d` errors out of
range — matching bash.

## Testing

### Unit tests (`crates/huck-engine/src/history.rs` test module)

- `tail(n)`: n<len, n==0, n>len.
- `delete(number)`: middle (renumbers), first, last, out-of-range (`false`),
  and `unwritten_start` adjustment when deleting before the marker.
- `delete_range(start,end)`: inclusive multi-delete, reversed range (no-op).
- `write_all_to`/`read_append_from` round-trip (append semantics + `unwritten_start`
  = len after read); `append_new_to` writes only the tail and advances the marker;
  a second `append_new_to` after more `add`s appends only the newer lines.
- eviction decrements `unwritten_start`.

### Bash-diff harness — `tests/scripts/history_diff_check.sh` (new)

Populate via `history -r <fixture>` (works non-interactively in both shells), then
compare byte-for-byte (`HISTFILE=/dev/null` to isolate). Cases:

- `history` (row format, two spaces) and `history 2`, `history 0`, `history 99`.
- `history -d 2` (single), `history -d -1` (negative), `history -d 2-3` (range),
  then `history` to show renumbering; plus `history -d 9` / `history -d abc`
  error text + rc.
- `history -r a` then `history -r b` (append) → `history`.
- `history -w out; cat out` (whole list, no numbers).
- `history -a out`: after `history -r fix`, `-a` appends **nothing** (the loaded
  lines are not "new this session" in either shell) — assert `out` is empty. This
  is the marker behavior both shells agree on in a non-interactive script.
  (Exercising `-a` appending genuinely session-added lines needs `history -s`,
  which huck does not implement, so that path is covered by the `History` **unit
  tests** — `add` then `append_new_to` — rather than the diff harness.)

### Suites

- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
- Full diff-check sweep `tests/scripts/run_diff_checks.sh` stays green (182 with
  the new harness).

## Out of scope

- `-p` (expand-and-print), `-s` (store as one entry), `-n` (read unread lines) —
  not in #6/#7; rejected with a "not yet implemented" message (rc 1), not silently
  accepted, so no parses-but-mis-runs surprise.
- Timestamps (`HISTTIMEFORMAT`), history expansion changes.

## Notes

- Both #6 and #7 are real (not intentional) divergences; the merged PR auto-closes
  both. No `docs/bash-divergences.md` entry.
- The `history` row-format fix (`\t` → two spaces) is a latent-divergence fix
  bundled here because `history N` must share the renderer and match bash.
