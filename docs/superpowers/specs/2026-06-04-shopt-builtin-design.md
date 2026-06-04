# huck v86 — `shopt` builtin Design

**Status:** approved design, ready for implementation plan.
**Implements:** the `shopt` builtin (bash's second shell-option namespace, distinct
from `set -o`). huck currently has no `shopt` at all, so `shopt -s checkwinsize`,
`shopt -s histappend`, and `if ! shopt -oq posix` — all present in a stock Debian
`~/.bashrc` — fail with "command not found: shopt". This is the last builtin gap
that blocks a clean `~/.bashrc` load (after v84's `${var:+…}` and v85's `!`).
**Divergence tracker:** new sub-entry **M-08d** (companion to M-08 `set` flags / M-08c `!`).
**Branch (impl):** `v86-shopt` (created from `main` at plan time).

## Scope

Three decisions were made during brainstorming and bound this iteration:

1. **Mechanics + a glob/match behavioral subset.** Implement the full `shopt`
   builtin surface (`-s`/`-u`/`-q`/`-p`/`-o`, listing, exit statuses) AND wire
   *real behavior* for five high-value options: `nullglob`, `dotglob`,
   `nocaseglob`, `failglob` (pathname expansion) and `nocasematch` (`[[ == ]]`
   and `case`). All other options are faithful **inert toggles**: they
   round-trip through set/unset/query/list at their bash default value but do
   not change huck's behavior.
2. **Full bash 5.2 option table.** Recognize bash 5.2's complete set of **57**
   `shopt` names at their **non-interactive default** values, so bare `shopt`,
   `shopt -p`, and any `shopt -s <name>` from a real bashrc/bash-completion all
   work and the listings are byte-identical to bash.
3. **Query-faithful, set-conservative `-o` bridge.** `shopt -o` operates on the
   `set -o` namespace. Recognize bash's full **27**-name `set -o` table for
   listing (`set -o`, `set +o`, `shopt -o`, `shopt -po`) and querying
   (`shopt -oq posix` → rc 1 silently, `posix` off). The three implemented
   set-o options (`errexit`/`nounset`/`pipefail`) stay read/write + behavioral.
   Attempting to **enable** an unimplemented set-o option (`set -o xtrace`,
   `shopt -so xtrace`) still errors "not yet supported in this version"
   (preserving v69's no-lie policy and keeping `$-` honest).

**Out of scope** (remain deferred, inert toggles): `extglob` (extended glob
patterns), `globstar` (`**`), `histappend`/`cmdhist`/`lithist` (history file
behavior), `checkwinsize`, `expand_aliases` (huck's alias expansion stays
interactive-only), `autocd`/`cdspell`/`dirspell`/`cdable_vars`, `xpg_echo`,
`huponexit`, the `compat*` levels, and every other name not in the five-option
behavioral subset. They are recognized, settable, queryable, and listed — just
inert.

## Verified bash 5.2.21 semantics (the implementation targets these byte-for-byte)

### Listing format

Both `shopt` and `shopt -o` use bash's `printf "%-15s\t%s\n"` format: the name
left-justified in a 15-wide field (space-padded; names ≥15 chars print in full,
no truncation), a literal TAB, then `on` or `off`.

```
nullglob       <TAB>off
assoc_expand_once<TAB>off      # 17-char name: no padding, just name+TAB+value
```

### The `shopt` table (57 names, exact bash order, non-interactive defaults)

Order is bash's internal table order (NOT alphabetical — note `assoc_expand_once`
follows `autocd`). Bare `shopt` and `shopt -p` both emit in this order.

```
autocd off, assoc_expand_once off, cdable_vars off, cdspell off, checkhash off,
checkjobs off, checkwinsize ON, cmdhist ON, compat31 off, compat32 off,
compat40 off, compat41 off, compat42 off, compat43 off, compat44 off,
complete_fullquote ON, direxpand off, dirspell off, dotglob off, execfail off,
expand_aliases off, extdebug off, extglob off, extquote ON, failglob off,
force_fignore ON, globasciiranges ON, globskipdots ON, globstar off,
gnu_errfmt off, histappend off, histreedit off, histverify off, hostcomplete ON,
huponexit off, inherit_errexit off, interactive_comments ON, lastpipe off,
lithist off, localvar_inherit off, localvar_unset off, login_shell off,
mailwarn off, no_empty_cmd_completion off, nocaseglob off, nocasematch off,
noexpand_translation off, nullglob off, patsub_replacement ON, progcomp ON,
progcomp_alias off, promptvars ON, restricted_shell off, shift_verbose off,
sourcepath ON, varredir_close off, xpg_echo off
```

13 default-ON: `checkwinsize cmdhist complete_fullquote extquote force_fignore
globasciiranges globskipdots hostcomplete interactive_comments
patsub_replacement progcomp promptvars sourcepath`. All others default off.

### The `set -o` table (27 names, exact bash order, defaults)

```
allexport off, braceexpand ON, emacs off, errexit off, errtrace off,
functrace off, hashall ON, histexpand off, history off, ignoreeof off,
interactive-comments ON, keyword off, monitor off, noclobber off, noexec off,
noglob off, nolog off, notify off, nounset off, onecmd off, physical off,
pipefail off, posix off, privileged off, verbose off, vi off, xtrace off
```

3 default-ON: `braceexpand hashall interactive-comments`. Implemented (real
state, read/write): `errexit`, `nounset`, `pipefail`. The other 24 list/query
at their default; enable attempts error.

Note the name in the set-o table is `interactive-comments` (hyphen); the shopt
table has the distinct `interactive_comments` (underscore). They are different
options in different namespaces.

### Invocation behavior & exit statuses

| Invocation | Behavior | rc |
|---|---|---|
| `shopt` | list all 57 in table order | 0 |
| `shopt -s` (no names) | list only the ON options (13) | 0 |
| `shopt -u` (no names) | list only the OFF options (44) | 0 |
| `shopt -p` | list all 57 in `shopt -s NAME` / `shopt -u NAME` re-input form | 0 |
| `shopt -s NAME...` | enable each named option | 0, or 1 if any name invalid |
| `shopt -u NAME...` | disable each named option | 0, or 1 if any name invalid |
| `shopt NAME...` | query: print each `NAME\t{on,off}` (table format) | 0 iff **all** named are set, else 1 |
| `shopt -q NAME...` | quiet query: no output | 0 iff **all** named are set, else 1 |
| `shopt -q` (no names) | quiet, no names → vacuously "all set" | 0 |
| `shopt -p NAME...` | like the `NAME...` query but prints `shopt -s/-u NAME` re-input form (rc still 0 iff all set) | 0/1 |

Invalid name: emit `huck: shopt: NAME: invalid shell option name` to stderr,
rc 1, but **still process the valid names in the same call** (bash sets the
valid ones and reports the invalid; verified `shopt -s nullglob bogus extglob`
sets nullglob+extglob, errors on bogus, rc 1). (bash's message is
`bash: line N: shopt: NAME: invalid shell option name`; huck uses its standard
`huck: shopt:` prefix — see Testing for the harness consequence.)

### The `-o` bridge

`-o` makes every form above operate on the `set -o` table instead:

| Invocation | Behavior |
|---|---|
| `shopt -o` | list all 27 set-o names, table format |
| `shopt -po` / `shopt -p -o` | list in `set -o NAME` / `set +o NAME` re-input form (identical to `set +o`) |
| `shopt -so NAME` | enable: `errexit`/`nounset`/`pipefail` → set real state; any other → "not yet supported in this version", rc 1 |
| `shopt -uo NAME` | disable: same three real; others → "not yet supported" |
| `shopt -o NAME...` | query (print + rc); reads real state for the 3, default for the rest |
| `shopt -qo NAME...` / `shopt -oq NAME...` | quiet query (flag order is free); `shopt -oq posix` → rc 1 silently |

### `set -o` / `set +o` listing consequence (existing builtin reformatted)

huck's current `set -o` prints space-padded output for 3 names
(`errexit␠␠␠␠␠␠␠␠␠off`); bash prints `%-15s\t%s` for 27. To make the listings
byte-identical and consistent with `shopt -o`, `set -o`/`set +o`'s listing is
reformatted to `%-15s\t%s` and expanded to the full 27-name `SETO_TABLE`:

- `set -o` (no args) → 27-name table, `%-15s\t%s`.
- `set +o` (no args) → 27 lines of `set -o NAME` / `set +o NAME` (re-input).
- The `set -o NAME` / `set +o NAME` **enable/disable** paths are unchanged from
  v69: only `errexit`/`nounset`/`pipefail` are accepted; others still error
  "not yet supported in this version". `$-` is unchanged.

Existing `set -o`/`set +o` unit & integration tests that assert the old 3-line
space-padded output are updated to the new format/length.

## Behavioral wiring (the five live options)

### Pathname expansion — `glob_expand_fields` (`src/expand.rs:1012`)

The function gains access to the four glob-relevant flags (threaded from its
call site, which has `&Shell`; exact plumbing — extra param vs. small `GlobOpts`
struct — is the plan's choice). Per field with unquoted glob metacharacters:

- **`nocaseglob`** → `MatchOptions.case_sensitive = false`.
- **`dotglob`** → force `require_literal_leading_dot = false` so `*`/`?` match
  leading-dot names. The existing `.`/`..` retain-filter still excludes those two.
- **no match + `failglob`** → abort: emit `huck: no match: PATTERN` to stderr and
  fail the whole simple command with status 1 (the command does not run), like a
  redirection error. `failglob` takes precedence over `nullglob`.
- **no match + `nullglob`** (and not failglob) → the field expands to **zero**
  words (contributes nothing) instead of the literal pattern.
- **no match, neither flag** → unchanged: the literal pattern (current behavior).

All four default off → identical behavior to today when shopt is untouched.

### Pattern matching — `nocasematch`

When `nocasematch` is on, glob matching becomes case-insensitive in three places,
all in `src/executor.rs` (each already has `&Shell` in scope):

- `eval_binary` — `[[ x == pat ]]` / `[[ x != pat ]]`: match via
  `glob::Pattern::matches_with(s, MatchOptions { case_sensitive: false, .. })`
  instead of the default `matches`.
- `eval_binary` — `[[ x =~ re ]]`: prepend `(?i)` to the regex source.
- `case_item_matches` — `case` clause patterns: same `matches_with` case-fold.

Default off → existing `[[`/`case`/`=~` tests unaffected.

## Data model

`src/shell_state.rs`:

- New `const SHOPT_TABLE: &[ShoptInfo]` (57 rows: `name: &'static str`,
  `default: bool`) in exact bash order.
- New `pub struct ShoptOptions` holding the 57 booleans. Implementation may use a
  `[bool; 57]` indexed by table position (seeded from `SHOPT_TABLE` defaults) or
  named fields; either way it exposes typed accessors for the five live options
  (`nullglob()`, `dotglob()`, `nocaseglob()`, `failglob()`, `nocasematch()`).
  `Default` seeds every option to its bash default.
- New `pub shopt_options: ShoptOptions` field on `Shell`, default-initialized.

`src/builtins.rs`:

- Expand the existing 3-entry `SHELL_OPTIONS` into the full 27-name `SETO_TABLE`
  (name + default). `option_get` returns real state for the 3 implemented and
  the table default for the rest; `option_set` accepts only the 3 (others →
  "not yet supported"). `print_options_table`/`print_options_reinput` iterate the
  27 and use `%-15s\t%s` / `set ±o NAME`.
- New `builtin_shopt` + helpers (`shopt_get`/`shopt_set`/`print_shopt_table`/
  `print_shopt_reinput`/`format_shopt_line`). The `-o` forms delegate to the
  set-o helpers.

## File-change map

| File | Change |
|------|--------|
| `src/shell_state.rs` | `ShoptInfo`, `SHOPT_TABLE` (57), `ShoptOptions` struct + `Default`, `Shell.shopt_options` field + init |
| `src/builtins.rs` | new `"shopt"` dispatch arm + `BUILTIN_NAMES` entry; `builtin_shopt` + helpers; expand `SHELL_OPTIONS`→`SETO_TABLE` (27); reformat `print_options_table`/`print_options_reinput` to `%-15s\t%s`; `option_get` reads default for unimplemented |
| `src/expand.rs` | `glob_expand_fields` honors `nullglob`/`dotglob`/`nocaseglob`/`failglob`; failglob signals command-abort to its caller |
| `src/executor.rs` | thread glob flags into the `glob_expand_fields` call site(s); handle the failglob abort (status 1, skip command); `nocasematch` case-insensitive matching in `eval_binary` (`[[ ==`/`!=`/`=~ ]]`) and `case_item_matches` (`case`) |
| `tests/shopt_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/shopt_diff_check.sh` | NEW — huck's 13th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | new `[fixed v86]` M-08d entry; update M-08 "Still deferred" (drop shopt); changelog; summary stamp/count; README v86 row |

## Testing

1. **Unit tests** (`src/builtins.rs`): `SHOPT_TABLE` length 57 + a spot-check of
   order/defaults; `SETO_TABLE` length 27; each of `-s`/`-u`/`-q`/`-p`/`-o`/`-po`
   flag-parse; query exit status (0 iff all set); invalid-name → rc 1 + valid
   names still applied; the five live flags flip the right `ShoptOptions` field;
   `option_get` returns defaults for unimplemented set-o names; enabling an
   unimplemented set-o name errors.
2. **Integration tests** (`tests/shopt_integration.rs`): `shopt -oq posix`→1;
   `shopt -s nullglob; echo no*match`→empty line; `shopt -s dotglob; echo *`
   includes a dotfile; `shopt -s nocaseglob; echo A*` matches a lowercase file;
   `shopt -s nocasematch; [[ ABC == abc ]]`→match and `case ABC in abc)`→match;
   `shopt -s failglob; echo no*match`→rc 1 + **empty stdout** (assert rc + stdout,
   NOT stderr text); round-trip `shopt -s extglob; shopt -q extglob`→0 (inert but
   tracked); `shopt -s bogus`→rc 1.
3. **bash-diff harness** `tests/scripts/shopt_diff_check.sh` (huck's 13th),
   byte-identical to bash 5.2: bare `shopt`; `shopt -p`; `shopt -s`/`shopt -u`
   no-name lists; `shopt -o`; `shopt -po`; `set -o`; `set +o`; multi-name query
   `shopt dotglob nullglob` (+ rc); `shopt -q` queries; `shopt -oq posix`;
   nullglob/dotglob/nocaseglob/nocasematch effects (run in a fixture temp dir
   with known files). **Excluded from the byte harness** (documented in a NOTE
   comment, covered by integration on rc/stdout instead): `failglob`'s error
   line, whose stderr text uses huck's `huck:` prefix vs bash's `bash: line N:`.

## Edge cases & notes

- The five behavioral flags default off ⇒ **zero** behavior change for all
  existing glob/`case`/`[[` tests.
- `shopt -s` with a mix of valid and invalid names applies the valid ones and
  returns 1 (bash-faithful).
- Flag clustering on the `-o` query (`-oq`, `-qo`) is order-independent.
- `set -o`/`set +o` enable/disable semantics and `$-` are **unchanged** from
  v69; only the *listing* is reformatted and expanded.
- `nullglob`/`failglob`/`dotglob`/`nocaseglob` interact only with fields that
  already contain unquoted glob metacharacters; quoted/plain words are untouched.
