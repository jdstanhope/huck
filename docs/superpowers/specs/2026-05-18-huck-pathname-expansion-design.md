# huck v10: Pathname Expansion (Globbing)

**Date:** 2026-05-18
**Status:** Design

## Goal

Add POSIX basic pathname expansion to huck: `*`, `?`, and bracket
expressions (`[abc]`, `[a-z]`, `[!abc]`). Patterns appear in command
arguments and are expanded against the filesystem after parameter,
command, and tilde expansion. Quoted metacharacters are treated as
literals. Behavior matches interactive bash defaults.

## Scope

**In scope:**
- `*` matches any run of characters not crossing `/` and not matching a
  leading `.`
- `?` matches exactly one character with the same restrictions
- `[abc]`, `[a-z]`, `[!abc]` bracket expressions
- Per-character quoting tracking so `"*"`, `'*'`, and `\*` stay literal
- No-match: pattern is returned literally (bash default)
- Dotfile exclusion: leading `.` requires explicit `.` in pattern
- Path separator: `*`/`?`/brackets do not cross `/`
- Integration with existing tilde expansion: `~/foo*` expands tilde
  first, then globs
- Sorted output (the `glob` crate handles this)

**Out of scope (deferred):**
- Glob expansion on redirect targets (`> *.log`). Redirects keep
  plain string expansion in v10; ambiguous-redirect semantics arrive
  with a later iteration.
- `**` (`globstar`)
- Extended glob patterns (`?(...)`, `*(...)`, `+(...)`, `@(...)`, `!(...)`)
- Case-insensitive matching (`nocaseglob`)
- `failglob` / `nullglob` shopt options
- Brace expansion (`{a,b,c}`) — its own future iteration
- Non-UTF8 filenames are skipped with a stderr warning rather than
  passed through as raw bytes

## Architecture

Pathname expansion runs as a **post-expansion step**. The pipeline
becomes:

1. **Lex** the input into `Word`s, each composed of `WordPart`s.
2. **Expand** each `Word` into `Vec<Field>` (current expand returns
   `Vec<String>`). A `Field` carries both the resolved characters and
   a parallel per-character `quoted` vector.
3. **Glob-expand** each `Field` into `Vec<String>` final argv strings.
4. **Execute** the resulting argv via the builtin dispatch or
   `fork`/`exec`.

The new representation lets globbing distinguish quoted glob characters
(literal) from unquoted ones (active metacharacters). Without
per-character quoting, we would lose the information after expansion
that says "this `*` came from inside double quotes."

### Field type

```rust
pub struct Field {
    pub chars: String,
    pub quoted: Vec<bool>, // len == chars.chars().count()
}
```

The `quoted` vector is parallel to `chars` by `char` count (not byte
index). It's true for any character whose source was inside `"..."` or
`'...'`, or escaped with `\`.

### Lexer change

Today, `WordPart::Literal(String)` does not record whether the literal
came from a quoted region, and the tokenizer accumulates both bare and
quoted text into the same `current` buffer before flushing one
combined `Literal`. v10 changes the variant to:

```rust
WordPart::Literal { text: String, quoted: bool },
```

and adjusts the tokenizer to flush at every quote boundary so each
emitted `Literal` is either fully quoted or fully unquoted. Concretely:

- Before entering `'...'` or `"..."`: if the bare-text `current` buffer
  is non-empty, flush it as `Literal { quoted: false }`.
- While inside quotes: accumulate into a separate buffer that flushes
  as `Literal { quoted: true }` on quote exit (and on `$` / backtick
  expansion mid-quote, mirroring the current `parts.push` calls at
  `src/lexer.rs:97-105`).
- After exiting quotes: resume accumulating into the bare-text buffer.

A double-quoted span like `"abc"` becomes
`Literal { text: "abc", quoted: true }`. Bare text becomes
`Literal { text: "abc", quoted: false }`. Mixed input like
`foo"bar"baz` produces three Literal parts.

### Quoting propagation in `expand`

For each `WordPart`, the resulting characters and their `quoted` flag
are appended to the current `Field` being built:

| WordPart                            | Quoted flag per emitted char    |
|-------------------------------------|---------------------------------|
| `Literal { quoted: true }`          | all `true`                      |
| `Literal { quoted: false }`         | all `false`                     |
| `Tilde(spec)`                       | all `false` (resolved value)    |
| `Var { quoted: true }`              | all `true`                      |
| `Var { quoted: false }`             | all `false`, IFS-split today    |
| `LastStatus { quoted: true }`       | all `true`                      |
| `LastStatus { quoted: false }`      | all `false`                     |
| `CommandSub { quoted: true }`       | all `true`                      |
| `CommandSub { quoted: false }`      | all `false`, IFS-split today    |

IFS splitting on unquoted `$var` and `$(cmd)` is unchanged. Each
post-split fragment becomes its own `Field`.

Tilde-resolved characters are unquoted: `cd ~` should expand `~` to
`/home/john` and not glob-escape any characters in that path. If the
home directory itself happens to contain a literal `*`, that's a
pre-existing concern bash also has.

### Glob expansion step

```rust
pub fn glob_expand_fields(fields: Vec<Field>) -> Vec<String>
```

Per field:

1. Detect unquoted glob metacharacters by walking `chars` paired with
   `quoted`. A char is a metachar if it is one of `*`, `?`, `[` and
   `quoted[i] == false`.
2. **If none:** push `field.chars` as one argv string; done.
3. **If any:** build a glob pattern string:
   - Unquoted glob metachar → emit as-is.
   - Quoted glob metachar (or `]`) → escape as `[c]` (works for `]`
     and `\` too).
   - Other char → emit as-is.
4. Run `glob::glob_with(&pattern, opts)` with:
   ```rust
   MatchOptions {
       case_sensitive: true,
       require_literal_separator: true,
       require_literal_leading_dot: true,
   }
   ```
5. Collect successful matches. Convert each `PathBuf` to `String` via
   `into_os_string().into_string()`; on `Err` (non-UTF8), emit a
   stderr warning `huck: skipping non-UTF8 path` and skip that match.
6. **If pattern parsing failed** (`glob` returned `PatternError`):
   push `field.chars` as a single argv string and continue.
7. **If zero matches:** push `field.chars` as a single argv string
   (bash default).
8. **If one or more matches:** push each matched path string.

Sort order: the `glob` crate returns paths in lexicographic order,
matching bash.

### Executor integration

`executor.rs` currently calls `expand(...)` per word and concatenates
the results into argv. v10 changes that path:

```rust
let fields = expand(word, &shell, ...);
let argv_pieces = glob_expand_fields(fields);
argv.extend(argv_pieces);
```

This applies to both:
- The builtin path, where argv is passed to the dispatch table.
- The external command path, where argv is passed to `Command`.

Assignment-context expansion (`expand_assignment`) keeps returning
`String` and does **not** call `glob_expand_fields`. Assignment RHS is
not subject to pathname expansion in bash.

Redirect targets keep their current plain-string expansion in v10.

## Data flow example

Input: `echo "foo"*bar`

1. Lex → one `Word` with three parts:
   - `Literal { text: "foo", quoted: true }`
   - `Literal { text: "*", quoted: false }`
   - `Literal { text: "bar", quoted: false }`
2. Expand → one `Field`:
   - `chars = "foo*bar"`
   - `quoted = [t,t,t,f,f,f,f]`
3. Glob-expand:
   - Pattern: `foo[*]bar`? No — `foo` is literal (all-quoted, no
     metachars), `*` at index 3 is unquoted metachar, `bar` is
     unquoted literal. Build: `foo*bar`. Match against CWD.
   - One match `food_for_thoughtbar` → argv gets `food_for_thoughtbar`.
   - Zero matches → argv gets `foo*bar` literal.

Input: `echo "*"`

1. Lex → `Word` with one part: `Literal { text: "*", quoted: true }`.
2. Expand → one `Field`: `chars = "*"`, `quoted = [t]`.
3. Glob-expand: no unquoted metachars; push `"*"` as-is. Argv: `*`.

Input: `ls ~/[ab].txt` with `HOME=/home/john`

1. Lex → `Word` parts: `Tilde(Empty)`, `Literal { text: "/[ab].txt", quoted: false }`.
2. Expand → one `Field`:
   - `chars = "/home/john/[ab].txt"`
   - `quoted = [f,f,...,f]` (all false)
3. Glob-expand: `[` is unquoted metachar. Pattern:
   `/home/john/[ab].txt`. Matches `a.txt` and `b.txt` under
   `/home/john/`. Argv: `/home/john/a.txt`, `/home/john/b.txt`.

## Error handling

| Condition                                | Behavior                                                |
|------------------------------------------|---------------------------------------------------------|
| Invalid bracket expression (`[`, `[!]`)  | Pattern parse error → fall back to literal field text   |
| Zero matches                             | Fall back to literal field text                         |
| Permission error during glob walk        | Silently skipped (glob iterator `Err` items ignored)    |
| Non-UTF8 filename                        | Warning to stderr, that match skipped                   |
| Pattern produces extremely many matches  | No limit; relies on OS argv limit (handled by execve)   |

## Edge cases

- `*` does not match `.bashrc` (require_literal_leading_dot)
- `.*` matches `.bashrc` (and possibly `.` and `..` — filter these)
- `*` does not cross `/` (require_literal_separator)
- `[]` is invalid → literal
- `[!abc]` is POSIX negation (the `glob` crate accepts both `!` and `^`)
- `cd` updates CWD via `std::env::set_current_dir`; the `glob` crate
  uses CWD for relative patterns; this should pick up cleanly
- `~user/*` resolves user's home then globs against it

## Testing

**Lexer unit tests:** updated to assert `Literal { quoted }` shape;
new tests cover mixed quoted/unquoted sequences.

**Expand unit tests:** field construction, quoted-vector propagation
per `WordPart` kind, IFS splitting still works.

**`glob_expand_fields` unit tests** (use `tempfile::TempDir`):
- `*.txt` matches `a.txt`, `b.txt` (sorted)
- `*` excludes `.hidden`
- `.*` includes `.hidden`, excludes `.` and `..`
- `[ab].txt` matches `a.txt`, `b.txt`
- `[!a]*.txt` excludes `a.txt`
- No match → literal returned
- Quoted `*` → literal returned without filesystem touch
- Partial quoting: `"foo"*` matches files starting with literal `foo`
- Invalid `[` → literal returned
- `~/[ab].txt` with `HOME` set to TempDir → matches

**Integration tests** (`tests/glob_integration.rs`):
- Spawn shell, populate temp CWD, run `echo *.rs` etc., assert stdout
- `echo "*.rs"` → literal `*.rs`
- `echo ~/*.txt` with controlled `HOME`

**Coverage target:** maintain >85% (cargo llvm-cov).

## File layout impact

- `src/lexer.rs` — `WordPart::Literal` shape change, lexer threads
  quoting flag into emitted parts; existing tests updated
- `src/expand.rs` — new `Field` type, `expand` returns `Vec<Field>`,
  new `glob_expand_fields` function
- `src/executor.rs` — call sites updated
- `src/builtins.rs` — call sites updated (builtins also receive
  globbed argv)
- `Cargo.toml` — add `glob = "0.3"` dependency
- `tests/glob_integration.rs` — new integration suite
- `README.md` — v10 row in iteration table, new features section

## Open questions

None remaining at design time.

## References

- POSIX 2008 Shell Command Language §2.13 Pattern Matching Notation
- bash(1) Pathname Expansion section
- `glob` crate docs: https://docs.rs/glob
