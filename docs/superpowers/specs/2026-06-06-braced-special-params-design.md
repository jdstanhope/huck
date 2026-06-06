# huck v102 — braced special parameters `${-}`/`${?}`/`${$}`/`${!}` Design

**Status:** approved design, ready for implementation plan.
**Implements:** the single-char special parameters in BRACED form — `${-}` (option
flags `$-`), `${?}` (exit status `$?`), `${$}` (shell pid `$$`), `${!}` (last bg
pid `$!`) — and with modifiers (`${-#*e}`, `${?:-default}`, `${$:+set}`, …). Today
huck supports these UNBRACED (`$-`/`$?`/`$$`/`$!`) and `${#}`/`${@}`/`${*}` braced,
but `${-}`/`${?}`/`${$}`/`${!}` braced raise `syntax error: parameter expansion
with empty name`.
**Primary driver:** `~/.nvm/nvm.sh`'s `nvm()` (line 2972) tests its option flags
with `${-#*e}` / `${-#*a}` / `${-#*E}` (the `$-` special in braced form with a
remove-prefix modifier). The line-2963 blocker.
**Closes:** a new low Tier-2 entry (braced single-char special params)
`[fixed v102]`.
**Branch (impl):** `v102-braced-special-params`.

## Verified bash 5.2 semantics (the contract)

- `${-}` → current option flags (e.g. `hBs`). `${?}` → last status. `${$}` →
  shell pid. `${!}` → last background pid (empty if none).
- With modifiers: `${-#*e}` → `$-` with the shortest `*e`-matching prefix removed
  (nvm's errexit test: `[ "${-#*e}" != "$-" ]`). `${?:-default}`, `${$:+set}`,
  `${-:-x}` all apply the modifier to the special param's value.
- `${@}`/`${*}`/`${#}` already work (unchanged). Positional `${0}`/`${1}` already
  work.

## Root cause (verified)

`read_braced_param_expansion` (`src/lexer.rs:1784`) dispatches `@`/`*` (top match),
`#` (count / Length), digit-only (positional), and `!` (v95 indirect / array
keys) — but has NO arm for the single-char specials `-`/`?`/`$`, so they fall to
`read_braced_name`, which returns an empty name → `EmptyParamName`. The bare
`${!}` falls into the `!`-indirect branch and then `read_braced_name` returns
empty → same error. On the eval side, `Shell::lookup_var` (`src/shell_state.rs:431`)
already resolves `-`, `$`, `!`, `0`, `#` — but NOT `?` (unbraced `$?` is a
separate `WordPart::LastStatus`).

## Section 1 — Lexer (`src/lexer.rs`)

In `read_braced_param_expansion`:
- Extend the initial special-char dispatch (the `match chars.peek()` that handles
  `@`/`*`) with arms for `-`, `?`, `$`: consume the char and route through
  `dispatch_braced_modifier(ch.to_string(), quoted, /*subscript*/ None, chars,
  parts, /*indirect*/ false)`. `dispatch_braced_modifier` already emits a bare
  `ParamExpansion { name: ch, modifier: None }` when `}` follows, and the right
  modifier (`RemovePrefix`, `UseDefault`, …) when an operator follows — so both
  `${-}` and `${-#*e}` are handled with no new modifier code.
- In the `!` branch (`:~1888`): before the existing indirect/keys handling, if
  the char immediately after the consumed `!` is `}`, this is the bare `$!`
  special param — emit `WordPart::Var { name: "!", quoted }` (or route through
  `dispatch_braced_modifier("!", …, indirect=false)` for the bare case),
  consume `}`, return. Only a NON-`}` follow continues into the v95 indirect path
  (`${!var}`/`${!arr[@]}`). (`${!}`-with-modifier, e.g. `${!:-x}`, is a rare
  ambiguous edge — out of scope; documented.)

These specials are NOT subscriptable, so pass `subscript: None` (matching the
positional-param handling).

## Section 2 — Eval (`src/shell_state.rs`)

Add `"?" => return Some(self.last_status().to_string())` to the special-params
match in `lookup_var` (beside the existing `"0"`/`"$"`/`"!"`/`"-"` arms). This is
the only special param not already resolved there; with it, `${?}` (bare) and
`${?:-x}`/`${?#…}` (modifier) all resolve. `is_set` already includes `?`
(`src/shell_state.rs:496`), so `${?+set}` / `set -u` interactions are consistent.

No other eval change: a braced `${-#*e}` lexes to `ParamExpansion { name: "-",
modifier: RemovePrefix{…} }`; the existing modifier evaluator reads the value via
`lookup_var("-")` (→ `dollar_dash_value()`) and applies remove-prefix — all reused.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `read_braced_param_expansion`: dispatch `-`/`?`/`$` to `dispatch_braced_modifier`; handle bare `${!}` → `$!` in the `!` branch |
| `src/shell_state.rs` | `lookup_var`: add `"?"` → `last_status` |
| `tests/braced_special_params_integration.rs` | NEW — `${-}`/`${?}`/`${$}`/`${!}` bare + modifiers; the nvm `${-#*e}` flag-test shape |
| `tests/scripts/braced_special_params_diff_check.sh` | NEW — 27th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | new Tier-2 entry `[fixed v102]`; the `${!<modifier>}` edge note; changelog; README row |

## Testing

1. **Lexer unit tests**: `${-}` → `ParamExpansion{name:"-", modifier:None}` (or
   `Var{name:"-"}`); `${-#*e}` → `ParamExpansion{name:"-", modifier:RemovePrefix}`;
   `${?}`/`${$}`/bare `${!}` parse (no `EmptyParamName`); `${!var}` still parses as
   indirect (regression).
2. **Integration** (`tests/braced_special_params_integration.rs`) — stdout vs bash:
   - `${?}`: `false; echo "${?}"` → `1`; `true; echo "${?}"` → `0`.
   - `${?:-x}` on a nonzero/zero status; `${?#0}` style.
   - `${$}`: `[ "${$}" = "$$" ] && echo same` → `same` (or compare `$$` and `${$}`
     are equal numerics).
   - `${-}`: `case "${-}" in *h*) echo has-h;; esac`-style (deterministic: check
     it's non-empty and equals `$-`).
   - `${-#*e}`: `set +e 2>/dev/null; [ "${-#*e}" = "$-" ] && echo no-e || echo
     has-e` — pick a form whose output is deterministic across bash/huck (compare
     `${-#*e}` to `$-`).
   - bare `${!}`: with no background job → empty (`[ -z "${!}" ] && echo empty`);
     with one (`sleep 0 & ; [ -n "${!}" ] && echo set; wait`).
   - the exact nvm shape: `f() { if [ "${-#*e}" != "$-" ]; then echo errexit; else
     echo no; fi; }; f` (under default flags → `no`; `set -e; f` → `errexit`).
   - regression: `${#}`, `${@}`, `${*}`, `${0}`, `${!var}` indirect, `$-`/`$?`
     unbraced all unchanged.
3. **bash-diff harness** `tests/scripts/braced_special_params_diff_check.sh`
   (27th): deterministic forms (compare-and-echo, not raw `$-`/`$$`/`$!` values
   which vary) byte-identical to bash 5.2. NOTE: `$-`'s exact letters and `$$`/`$!`
   pids differ across shells/runs — do NOT byte-compare raw values; use
   equality/`case` membership tests that yield stable `yes`/`no` output.
4. **Regression**: full suite — param-expansion, special-params, indirect (v95),
   `${var@OP}` (v96) suites.
5. **End-to-end**: re-bisect `nvm.sh` — `nvm()` (de-wrapped line 2963) now parses;
   report the next gap (if any).

## Edge cases & notes

- **`${!}` vs `${!var}`**: bare `${!}` (next char `}`) → `$!`; anything else after
  `!` → v95 indirect. `${!<modifier>}` (e.g. `${!:-x}`, `${!#p}`) is ambiguous with
  indirect and is OUT OF SCOPE (documented low edge); nvm doesn't use it.
- **Not subscriptable**: `${-[0]}` etc. are nonsensical — these specials take no
  subscript (pass `None`), matching positional-param handling.
- **No regression**: `${@}`/`${*}`/`${#}`/`${0}`/digit/`${!var}` paths are
  untouched; only the new `-`/`?`/`$` arm + the bare-`${!}` check + the `?`
  lookup_var arm are added.
