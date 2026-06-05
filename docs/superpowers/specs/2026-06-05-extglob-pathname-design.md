# huck v91 — extglob pathname globbing Design

**Status:** approved design, ready for implementation plan.
**Implements:** extended-glob patterns (`?(…)` `*(…)` `+(…)` `@(…)` `!(…)`, `shopt
-s extglob`) in **pathname/filesystem globbing** — `echo +(a|b)`, `ls
@(dir1|dir2)`, `dir*/+(foo|bar).txt`, etc. This is the piece v90 (M-84) deferred:
v90 added extglob to the three *string* contexts (`[[`/`case`/`${}`), but a
pathname word like `+(a|b)` lexes as one word and then is NOT filesystem-expanded
(the `glob` crate can't do extglob), so it passes through literally.
**Closes:** **M-84a** (currently `[deferred]`).
**Branch (impl):** `v91-extglob-pathname` (created from `main` at plan time).

## Scope

Decided during brainstorming ("Extglob fields only (dispatch)"): add a custom
recursive directory walker that reuses v90's `extglob_match` per path component,
invoked **only** when extglob is on AND the field contains an unquoted extglob
operator. All other pathname globbing (`*.txt`, `a?b`, `[a-z]*`) keeps using the
`glob` crate **byte-for-byte** (zero regression surface for existing behavior).

**Out of scope:** `**` globstar (a separate `globstar` shopt, not extglob —
unsupported, unchanged). The v90 string-context extglob (`[[`/`case`/`${}`) is
already shipped and untouched here.

## Verified bash 5.2 semantics (the walker's contract)

In a dir with `a b ab aab abc cd xy .hidden .ab` + `dir1/{foo.txt,bar.log}`,
`dir2/foo.txt`, all with `shopt -s extglob`:

- `echo +(a|b)` → `a aab ab b` (one-or-more of a/b; sorted; **dotfiles excluded**).
- `echo @(a|cd)` → `a cd` (exactly one). `echo *(a)` → `a`.
- `echo !(a|ab)` → `aab abc b cd dir1 dir2 xy` (everything **except** a/ab and dotfiles).
- `echo .+(ab)` → `.ab` (a literal leading `.` in the pattern matches dotfiles).
- `echo +([a-c])` → `a aab ab abc b` (class inside extglob).
- `echo dir*/+(foo|bar).txt` → `dir1/foo.txt dir2/foo.txt` (multi-component).
- `shopt -s nocaseglob; echo @(A|AB)` → `a ab`.
- `shopt -s dotglob; echo +(a|b)` → `a aab ab b` (dotglob makes dotfiles eligible,
  but the pattern still must match the whole name *including* the leading `.`, so
  `.ab`/`.hidden` still don't match `+(a|b)`).
- No match: `echo +(zzz)` → literal `+(zzz)`; under `nullglob` → empty; under
  `failglob` → error + command aborts. Output is **sorted** lexicographically.

## Section 1 — Detection & dispatch (`src/expand.rs`)

Two changes in `glob_expand_fields_opts`'s per-field loop:

1. **`GlobOpts` gains `pub extglob: bool`** (`src/expand.rs`), and
   `Shell::glob_opts()` (`src/shell_state.rs`) populates it from
   `shopt_options.get("extglob").unwrap_or(false)` (alongside the existing
   nullglob/dotglob/nocaseglob/failglob).
2. Build the pattern first (`build_glob_pattern(&field)`, as today — see §3 for the
   quoted-escape extension), then compute
   `let is_extglob = opts.extglob && crate::glob_match::has_extglob(&pattern);`.
   - Change the early literal pass-through guard from
     `if !has_unquoted_metachar(&field) { … push literal … }` to
     `if !has_unquoted_metachar(&field) && !is_extglob { … }` — so an extglob
     field with no `*`/`?`/`[` (e.g. `+(a|b)`) is no longer skipped.
   - When `is_extglob`: call the new walker
     `crate::glob_match::extglob_pathname_expand(&pattern, opts.nocaseglob, opts.dotglob)`
     and use its `Vec<String>` exactly where the `glob_with` matches are used
     today — i.e. feed the surrounding `if matched.is_empty() { failglob / nullglob
     / literal }` logic unchanged. Otherwise take the existing `glob_with` path.

Because `has_extglob` runs on the **built** pattern (where §3 has turned *quoted*
`|`/`(`/`)` into `[|]`/`[(]`/`[)]`), only genuinely-*unquoted* extglob groups
dispatch to the walker.

## Section 2 — The custom directory walker (`src/glob_match.rs`)

New public fn (pure module; no `Shell`):

```rust
/// Filesystem pathname expansion for an extglob `pattern` (the `glob` crate
/// can't do extglob). Returns matched paths, sorted lexicographically; empty
/// if nothing matches. Honors the dotfile rule, `nocaseglob`, and `dotglob`.
pub fn extglob_pathname_expand(pattern: &str, nocaseglob: bool, dotglob: bool) -> Vec<String>;
```

Algorithm:
- Split `pattern` into components on unescaped top-level `/`. A leading `/` means
  start from the filesystem root (`/`) and the first component is empty; otherwise
  start from the current directory (`.`, rendered without a `./` prefix in
  results, matching bash).
- Recurse component-by-component, carrying the path prefix built so far:
  - **Literal component** (no glob/extglob metacharacter): append it to the prefix
    and descend if that path exists (no `read_dir` needed).
  - **Pattern component**: `read_dir` the current directory; for each entry `name`,
    keep it iff it passes the dotfile rule (below) AND
    `extglob_match(component, name, nocaseglob)` is true. For a non-final
    component, only descend into matches that are directories.
- **Dotfile rule** (replicating the `glob` crate's `require_literal_leading_dot`):
  an entry whose `name` starts with `.` is skipped UNLESS the component's first
  effective literal char is `.` (the pattern is `.`-anchored) OR `dotglob` is on.
  `.` and `..` are always excluded.
- **Separator rule**: matching is per-component against single-level `read_dir`
  names (which never contain `/`), so a `*`/extglob group cannot cross `/` for
  free — no explicit handling needed.
- **Sort**: sort the final result list with the default string ordering (bytewise),
  matching bash/`glob`-crate output.
- **Errors**: a `read_dir`/metadata error on any branch yields no matches for that
  branch (no panic), matching the `glob` crate's lenient behavior.

The walker takes the WHOLE built pattern string; per-component matching delegates
to the already-tested `extglob_match` (which also handles `*`/`?`/`[…]` inside the
component). A component with no extglob op but with `*`/`?`/`[…]` is still matched
by `extglob_match` (it implements those) — but the walker is only *entered* for
patterns where `has_extglob` is true overall, so a mixed pattern like
`dir*/+(foo|bar).txt` (plain first component, extglob second) is fully handled by
the one walker.

## Section 3 — Quoted extglob metachars stay literal (`src/expand.rs build_glob_pattern`)

`build_glob_pattern` currently escapes only quoted `*`/`?`/`[`/`]` (as `[x]`). A
quoted `"+(a)"` would therefore reach `has_extglob` as a bare `+(a)` and be
treated as an extglob group. Extend the quoted-escape to also wrap quoted `|`, `(`,
`)` as `[|]`, `[(]`, `[)]` (single-char classes — literal-equivalent in both the
`glob` crate and the walker). This mirrors v90's `escape_pattern_literal` fix on
the string side. (The v90 lexer only forms an extglob group from *unquoted* `X(`,
so the only way a quoted extglob-looking sequence reaches here is via
quoting/escaping — which this correctly neutralizes.)

## Files & responsibilities

| File | Change |
|------|--------|
| `src/glob_match.rs` | NEW `extglob_pathname_expand(pattern, nocaseglob, dotglob)` + the recursive walker + dotfile rule; unit tests against a `tempfile` fixture |
| `src/expand.rs` | `GlobOpts.extglob`; dispatch in `glob_expand_fields_opts` (recognize extglob fields, route to the walker); `build_glob_pattern` quoted-escape extension for `|`/`(`/`)` |
| `src/shell_state.rs` | `Shell::glob_opts()` sets `extglob` from the shopt flag |
| `tests/extglob_pathname_integration.rs` | NEW — binary-driven integration tests (fixture dir) |
| `tests/scripts/extglob_pathname_diff_check.sh` | NEW — huck's 18th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-84a `[deferred]` → `[fixed v91]`; M-84 note "pathname now done"; changelog; README v91 row |

## Testing

1. **Unit tests** (`src/glob_match.rs`, against a `tempfile::tempdir()` fixture with
   known files + a subdir): `+(a|b)`/`@()`/`!()`/`*()`; class `+([a-c])`;
   multi-component `dir*/+(foo|bar).txt`; dotfile rule (excluded by `+(a|b)`,
   matched by `.+(ab)`, dotglob-on still excludes when the `.` isn't in the
   pattern); nocaseglob; no-match → empty `Vec`; sorted output.
2. **Integration tests** (`tests/extglob_pathname_integration.rs`, fixture dir via
   `current_dir`): `shopt -s extglob; echo +(a|b)`; `for f in @(a|cd); do …`;
   multi-component; `nullglob`+extglob no-match → empty; `failglob`+extglob
   no-match → rc 1; extglob-**off** → literal (unchanged); a quoted `"+(a)"` →
   literal (no expansion).
3. **bash-diff harness** `tests/scripts/extglob_pathname_diff_check.sh` (huck's
   18th): a `mktemp -d` fixture (`touch a b ab aab abc cd xy` + a subdir with
   files), each fragment `cd "$FIX"; shopt -s extglob; echo <pattern>` byte-identical
   to bash 5.2 (sorted, deterministic). Covers all five operators, classes,
   multi-component, dotfile rule, no-match-literal.

## Edge cases & notes

- **Default-off / non-extglob ⇒ zero change**: the dispatch only adds a branch for
  `opts.extglob && has_extglob(pattern)`; every other field takes the unchanged
  `glob_with` path, so all existing pathname-glob tests/behavior are byte-identical.
- **`nullglob`/`failglob`/`dotglob`/`nocaseglob`** are honored: dotglob/nocaseglob
  flow into the walker; nullglob/failglob no-match handling stays in the
  surrounding `glob_expand_fields_opts` (the walker just returns matches or empty).
- **`/` inside an extglob group** (e.g. `+(a/b)`) is a pathological edge; the
  splitter treats top-level `/` as separators (not those inside `(...)` would be
  rare) — documented as a non-goal if it surfaces; bash-completion never does this.
- **Result path form**: relative patterns yield relative paths (no `./` prefix);
  absolute patterns (leading `/`) yield absolute paths — matching bash.
- **`**` globstar** is unrelated (separate shopt) and remains unsupported.
