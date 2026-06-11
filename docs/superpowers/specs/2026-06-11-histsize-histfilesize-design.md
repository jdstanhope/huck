# huck v139 — HISTSIZE / HISTFILESIZE environment variables (M-59) Design

**Status:** approved design, ready for implementation plan.
**Implements:** honor `$HISTSIZE` (in-memory history list cap) and `$HISTFILESIZE`
(history file cap) read from the shell's variable table, with bash semantics,
replacing the fixed compile-time `HISTORY_MAX = 1000`. Resolves M-59 (Tier-2,
medium).
**Branch (impl):** `v139-histsize-histfilesize`.

## Background — current behaviour

`src/history.rs` `History` has a single `max: usize` field set to a compile-time
`HISTORY_MAX = 1000` in `History::new()`. `max` governs BOTH the in-memory list
(eviction in `add`) and load-truncation (`load`). `save()` writes ALL in-memory
`entries` (so the file is implicitly capped at the in-memory `max`). The two
bash variables are not consulted.

Key constraints discovered:
- huck has **no special-variable assignment hook**; `IFS`/`PATH` are read at
  point-of-use (`lookup_var`). HISTSIZE/HISTFILESIZE will follow that idiom.
- `HISTSIZE`/`HISTFILESIZE` are usually **non-exported shell variables**, so they
  must be read from huck's variable table (`shell.lookup_var`), NOT the process
  env (`std::env::var` would miss a non-exported `HISTSIZE=200`).
- `shell.history` is `Rc<History>` (v127 COW); writes go through
  `Rc::make_mut(&mut shell.history)`.
- Startup order (`src/shell.rs` `run`): history `load()` at ~285 runs BEFORE
  `maybe_source_rc_file` at ~295 — the reverse of bash (rc then history).

## bash semantics to replicate (the agreed full-semantics rules)

**HISTSIZE** (in-memory list):
- unset / empty / non-numeric → default `1000` (huck's existing default, kept).
- a negative integer → unlimited (no eviction).
- `0` → keep no entries (empty list).
- a positive integer `n` → cap at `n`.

**HISTFILESIZE** (history file):
- unset → default to the *effective HISTSIZE* (bash: "default to HISTSIZE").
- set & negative / non-numeric → inhibit truncation (write all in-memory entries).
- `0` → truncate the file to empty.
- a positive integer `n` → cap the file to the last `n` lines.

## Architecture

### 1. `History` cap model → `Option<usize>`
Change `History.max` from `usize` to `Option<usize>` (`None` = unlimited,
`Some(n)` = cap, `Some(0)` = empty). `History::new()` defaults to `Some(1000)`.

- `add(&mut self, line)`: push, then while `Some(cap) = self.max` and
  `entries.len() > cap`, evict the oldest (incrementing `base_number`). When
  `max == None`, never evict. `Some(0)` evicts everything (the just-added entry
  too).
- `load(&mut self)`: after reading lines, truncate to the last `self.max` lines
  (all when `None`).
- New `set_max(&mut self, max: Option<usize>)`: set `self.max` AND immediately
  evict current entries past the new cap (so a shrink takes effect at once, not
  only on the next `add`).
- New `save_capped(&self, file_cap: Option<usize>)`: write the last `file_cap`
  entries (all when `None`; nothing when `Some(0)`), reusing the existing escape
  encoding. The old `save()` becomes `save_capped(self.max)`? NO — keep `save()`
  for the tests that call it directly, but the shell will call `save_capped` with
  the resolved HISTFILESIZE. (Implementer: keep `save()` as
  `self.save_capped(self.max)` for backward-compat of existing callers/tests, or
  update those tests — minimize churn; the production save path uses
  `save_capped`.)

`HISTORY_MAX` stays as the named constant `1000`, now used as the
`resolve_histsize` unset-default rather than a hard cap.

### 2. Shell-side resolution (`src/shell_state.rs`, reads the variable table)
```rust
/// Resolve $HISTSIZE → in-memory cap. None = unlimited. (v139, M-59)
pub fn resolve_histsize(&self) -> Option<usize> {
    match self.lookup_var("HISTSIZE") {
        Some(v) => match v.trim().parse::<i64>() {
            Ok(n) if n < 0 => None,            // negative → unlimited
            Ok(n) => Some(n as usize),         // 0 → empty, n → cap
            Err(_) => Some(crate::history::HISTORY_MAX), // non-numeric → default
        },
        None => Some(crate::history::HISTORY_MAX), // unset → default 1000
    }
}

/// Resolve $HISTFILESIZE → file cap. None = no truncation. Unset → effective
/// HISTSIZE (bash default). (v139, M-59)
pub fn resolve_histfilesize(&self) -> Option<usize> {
    match self.lookup_var("HISTFILESIZE") {
        Some(v) => match v.trim().parse::<i64>() {
            Ok(n) if n < 0 => None,    // negative → inhibit truncation
            Ok(n) => Some(n as usize), // 0 → empty file, n → cap
            Err(_) => None,            // non-numeric → inhibit
        },
        None => self.resolve_histsize(), // unset → default to HISTSIZE
    }
}
```
(`HISTORY_MAX` must become `pub(crate)` in `history.rs`. An empty string `""`
trims to empty → `parse::<i64>()` errors → HISTSIZE non-numeric default / HISTFILESIZE
inhibit; matches the "empty → default/inhibit" intent. Confirm `lookup_var`
returns `Some("")` for an explicitly-empty var; if it returns `None`, the unset
branch applies — either is acceptable per the rules.)

### 3. Wiring (two `Shell` helpers keep the 7 call sites clean)
- `Shell::record_history(&mut self, line: String)`:
  `let cap = self.resolve_histsize(); Rc::make_mut(&mut self.history).set_max(cap);
  Rc::make_mut(&mut self.history).add(line);`
  Replaces the `Rc::make_mut(&mut shell.history).add(history.clone())` at
  `src/shell.rs:~323`. Reading HISTSIZE per-command makes it dynamic.
- `Shell::save_history(&self)`:
  `let cap = self.resolve_histfilesize(); self.history.save_capped(cap);`
  Replaces all six `shell.history.save()` calls in `src/shell.rs` (~298, 314,
  341, 356, 377, 385).
- Startup re-cap: right AFTER `maybe_source_rc_file` returns (so a `HISTSIZE`
  from `~/.huckrc` is now known), apply
  `let cap = shell.resolve_histsize(); Rc::make_mut(&mut shell.history).set_max(cap);`
  This re-caps the already-loaded list to the rc's HISTSIZE — netting out to
  bash's rc-then-history effect without reordering startup. (The initial `load()`
  at ~285 still uses the default cap, then this line corrects it.)

## Behaviour matrix (target = bash)

| `$HISTSIZE` | in-memory cap | | `$HISTFILESIZE` | file cap |
|---|---|---|---|---|
| unset | 1000 | | unset | = effective HISTSIZE |
| `""` / `abc` | 1000 | | `""` / `abc` | no truncation |
| `0` | empty | | `0` | empty file |
| `200` | 200 | | `50` | last 50 lines |
| `-1` | unlimited | | `-1` | no truncation |

## Scope & must-not-regress
- **`histappend` is OUT OF SCOPE** (bash `shopt -s histappend`, where the file
  accumulates beyond the in-memory list). huck's `save_capped` overwrites the file
  from the in-memory list capped at HISTFILESIZE — bash-faithful for the default
  (non-append) case. A file growing past the in-memory list is M-46 territory.
- **`HISTFILE`** resolution (`resolve_histfile`) is unchanged.
- **`history -c`** and the rest of the `history` builtin are unchanged.
- Existing history round-trip / load-truncation / expansion tests must stay green
  (adjust only those that assert the old `max: usize` type, minimally).

## Documented divergences
- **Ordering** (existing, now bounded): the initial `load()` runs before rc, so
  the loaded list is briefly capped at the default 1000 until the post-rc
  `set_max` re-caps it. Net observable state after startup matches bash. No new
  divergence entry needed (the re-cap closes the gap).
- **Non-interactive history recording** (pre-existing, L-27-adjacent): huck records
  history for piped/non-interactive stdin where bash does not. This is why a
  bash-diff harness comparing histfiles is N/A (non-interactive bash writes no
  history). Not introduced by v139; not re-logged here.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/history.rs` | `max: Option<usize>` (default `Some(1000)`); `add`/`load` honor `Option`; add `set_max(Option<usize>)` (sets + evicts) and `save_capped(Option<usize>)`; make `HISTORY_MAX` `pub(crate)`. Update the directly-affected unit tests. |
| `src/shell_state.rs` | Add `resolve_histsize()` + `resolve_histfilesize()` (bash-semantic parsing) + their unit tests. |
| `src/shell.rs` | Add `Shell::record_history` + `Shell::save_history`; replace the 1 add site + 6 save sites; add the post-rc `set_max` re-cap. |
| `tests/histsize_integration.rs` (NEW) | Deterministic piped-stdin + `HISTFILE` tests of the behaviour matrix (assert exact histfile contents). |
| `docs/bash-divergences.md` | DELETE M-59 (Tier-2 21→20). |

## Testing

1. **Unit tests — resolution** (`src/shell_state.rs` tests): build a `Shell`,
   `set("HISTSIZE", …)`, assert `resolve_histsize()` for each row: unset→`Some(1000)`,
   `""`→`Some(1000)`, `"abc"`→`Some(1000)`, `"0"`→`Some(0)`, `"200"`→`Some(200)`,
   `"-1"`→`None`. Same matrix for `resolve_histfilesize()`: unset→equals
   `resolve_histsize()`, `"abc"`/`"-1"`→`None`, `"0"`→`Some(0)`, `"50"`→`Some(50)`.
2. **Unit tests — `History`** (`src/history.rs` tests): `set_max(Some(3))` after
   adding 5 evicts to the last 3 (with correct `base_number`); `set_max(None)`
   keeps all; `set_max(Some(0))` empties; `save_capped(Some(2))` writes the last 2
   lines; `save_capped(None)` writes all; `save_capped(Some(0))` writes an empty
   file; round-trip save_capped→load honors the cap.
3. **Integration tests** (`tests/histsize_integration.rs`) — run the huck binary
   with piped stdin and a temp `HISTFILE` (env), then read the histfile:
   - `HISTSIZE=2\necho a\necho b\necho c\n` → histfile is `echo b`,`echo c`
     (in-memory capped at 2; HISTFILESIZE unset → defaults to 2).
   - `HISTFILESIZE=1\necho a\necho b\n` → histfile is the last 1 line (`echo b`).
   - `HISTSIZE=-1\n` + 5 echoes → all 6 commands present (unlimited).
   - `HISTSIZE=0\necho a\n` → histfile empty.
   - `HISTFILESIZE=0\necho a\n` → histfile empty.
   - control: no HISTSIZE set, a few commands → all present (default 1000 path).
   Use a unique temp `HISTFILE` per test (no cross-test interference); assert exact
   contents.
4. **Full regression:** entire suite + all bash-diff harnesses green; clippy clean.
   (No new bash-diff harness — see the divergence note on non-interactive history.)

## Edge cases & notes
- `Some(0)` in `add`: push-then-evict-all leaves the list empty; `last()`/
  `last_number()` return `None` — fine for `$!`/expansion (already `Option`).
- `set_max` called every `record_history` is a cheap `lookup_var` + parse per
  command — negligible.
- `save_capped(Some(n))` with `n > entries.len()` writes all entries (no padding).
- A `HISTSIZE`/`HISTFILESIZE` assignment is itself recorded as a history command
  (matches bash); its effect applies to subsequent adds / the next save.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the
  controller verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
