# v162: `cd -P/-L` and `pwd -P/-L` (logical vs physical PWD)

**Status:** design approved 2026-06-15
**Scope:** make huck track a LOGICAL `PWD` by default (symlinks preserved),
matching bash, with `-P`/`-L` flags on `cd`/`pwd` and the `set -o physical`
option selecting physical mode. Resolves divergences **M-32** (`cd -P/-L`) and
**M-33** (`pwd -P/-L`); **reverses the intentional I-01** ("`cd` always sets the
physical PWD"), which is deleted.

## Motivation

bash's `PWD` is LOGICAL by default: `cd symlink` leaves the symlink in `PWD`,
and `cd ..` collapses the path lexically (without resolving symlinks). huck
currently sets `PWD = env::current_dir()` (the OS-resolved PHYSICAL path) on
every `cd` and prints the physical path from `pwd`, ignoring `-P`/`-L` (I-01).
This diverges whenever symlinks are in play. v162 implements bash's
logical-default model: `cd`/`pwd` resolve an effective mode (explicit flag, else
the `physical` option) and store/print the logical or physical path accordingly.

## Verified bash behavior (against bash 5.x)

Using `/tmp/m32test/link` → symlink to `real`:
- `cd /tmp/m32test/link; echo "$PWD"` → `/tmp/m32test/link` (logical, default).
- `pwd` / `pwd -L` → `/tmp/m32test/link`; `pwd -P` → `/tmp/m32test/real`.
- `cd -P /tmp/m32test/link; echo "$PWD"` → `/tmp/m32test/real`.
- `cd /tmp/m32test/link; cd ..; echo "$PWD"` → `/tmp/m32test` (lexical parent of
  `link`, NOT resolved).
- `set -o physical` makes BOTH default physical: `set -o physical; cd link` →
  `PWD=.../real`; `set -o physical; pwd` → `.../real` (resolves `$PWD`).
- `-L`/`-P` are LAST-WINS on both: `cd -L -P link` → physical; `cd -P -L link` →
  logical; `pwd -L -P` → physical; `pwd -P -L` → logical.
- `pwd -x` → `pwd: -x: invalid option` + `pwd: usage: pwd [-LP]`, rc 2.
- `pwd foo` (extra non-flag arg) → ignored, prints pwd, rc 0.
- huck already inherits a valid inherited `$PWD` at startup (no change needed
  there); huck has no `CDPATH` (no interaction to handle).

## Architecture

### Mode resolution (shared by `cd` and `pwd`)
The "effective mode" for a `cd`/`pwd` invocation:
1. If an explicit `-P` or `-L` flag is given, the LAST one wins.
2. Otherwise: the `physical` `set -o` option — `true` → physical, `false`
   (default) → logical.

The `physical` option is ALREADY registered (`src/builtins.rs` ~line 5182,
`OptionInfo { name: "physical", default: false }`) but currently inert. v162
makes `cd`/`pwd` consult it. Read it via huck's existing option getter (the same
mechanism `set -o`/`-o`-options use — grep how another `set -o` option like
`nounset`/`noexec` is read, e.g. `shell.shell_options.<field>` or an
`option_get("physical")`); wire `physical` to a readable bool.

### `normalize_logical` — lexical path normalization (the core new piece)
A pure free function (unit-testable, no filesystem access):

```rust
/// Lexically normalizes an ABSOLUTE path for logical `cd`: collapses `.`,
/// empty components (from `//`), and `..` (removing the preceding component
/// WITHOUT resolving symlinks). A leading `..` at the root is dropped (bash
/// behavior). Always returns an absolute path; `/` for an empty result.
fn normalize_logical(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if matches!(components.last(), Some(&c) if c != "..") {
                    components.pop();
                }
                // at root (components empty) a `..` is dropped; we only push
                // ".." for a relative path, but cd always passes an absolute
                // curpath, so leading-".." never occurs here.
            }
            other => components.push(other),
        }
    }
    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}
```

Note: `cd` always passes an ABSOLUTE `curpath` to this (it joins relative targets
onto `$PWD` first), so the relative-leading-`..` case doesn't arise from `cd`.
The unit tests still cover it defensively per the function's contract.

### `cd` builtin (`src/builtins.rs`, `builtin_cd`)
Rewrite the flag/target handling:

1. **Parse flags**: scan leading args for `-L`/`-P` (last wins → an
   `Option<bool> physical_flag`), and `--` (ends flag scanning). `-` is NOT a
   flag — it's the OLDPWD shortcut and must still be recognized as the target.
   A bare `-` or a non-`-LP-`/`--` arg ends flag scanning and is the target.
   (bash also accepts `-e`/`-@` — OUT OF SCOPE, treat an unknown `-X` as bash
   does: error `cd: -X: invalid option` + usage, rc 2, OR — simpler and matching
   the M-32 surface — only special-case `-L`/`-P`/`--`/`-` and pass anything
   else as a literal target/dir. VERIFY bash: `cd -x` → `bash: cd: -x: invalid
   option`. Match that: unknown `-X` flag → rc 2 + usage.)
2. **Effective mode** = `physical_flag` if set, else the `physical` option.
3. **Compute target** (existing logic): `-` → `$OLDPWD` (and set
   `print_new_pwd`); no arg → `$HOME`; else the arg.
4. **Logical mode**:
   - `curpath` = target if it starts with `/`, else `format!("{}/{}", pwd,
     target)` where `pwd` = `$PWD` (fall back to `env::current_dir()` if `$PWD`
     is empty/unset).
   - `let normalized = normalize_logical(&curpath);`
   - `env::set_current_dir(Path::new(&normalized))` — on error, print
     `huck: cd: {target}: {e}`, rc 1 (use `target` in the message, matching the
     current code's style).
   - On success: `OLDPWD = old $PWD` (via `export_set`), `PWD = normalized`.
5. **Physical mode** (the current behavior, now explicit):
   - `env::set_current_dir(Path::new(&target))`; on error → rc 1.
   - `PWD = env::current_dir()?.to_string_lossy()` (canonical); `OLDPWD = old`.
6. `print_new_pwd` (the `cd -` case): print the resulting `PWD` value (logical or
   physical per mode), not `env::current_dir()`. (Currently it prints
   `new_pwd` from `current_dir()`; change to print the stored `PWD`.)

The `-` shortcut: bash's `cd -` is logical-mode by default too — it cd's to
`$OLDPWD` and applies the same logical/physical computation. So route `-`'s
target (`$OLDPWD`) through the same mode logic.

### `pwd` builtin (`src/builtins.rs`, `builtin_pwd`)
Currently takes no args and prints `env::current_dir()`. Rewrite:

1. **Parse flags**: `-L`/`-P` (last wins → `Option<bool>`), `--` ends flags.
   Non-flag args are IGNORED (bash prints pwd anyway, rc 0). An unknown `-X` →
   `huck: pwd: -X: invalid option` + `huck: pwd: usage: pwd [-LP]`, rc 2.
2. **Effective mode** = flag if set, else the `physical` option.
3. **Logical** → print `$PWD` (the stored logical PWD). If `$PWD` is unset/empty,
   fall back to `env::current_dir()`.
4. **Physical** → print the resolved path: `env::current_dir()` (already
   canonical); if that fails, fall back to `std::fs::canonicalize($PWD)`.
5. `builtin_pwd` needs `args` + `shell` now (it currently takes only `out`) —
   update its signature and the `run_builtin` dispatch call site accordingly.

## I-01 reversal

Delete the **I-01** intentional-divergence entry from `docs/bash-divergences.md`
(its "canonical paths are less surprising" rationale is superseded by bash
parity). Delete **M-32** and **M-33**. Update the tier counts (Intentional 10→9,
Missing 20→18). Any existing test/harness that asserted the old physical-PWD
behavior (e.g. a test expecting `PWD`/`pwd` to be resolved after `cd symlink`)
must be UPDATED to the new logical behavior — those encoded I-01.

## Out of scope (deferred / not handled)

- `cd -e` / `cd -@` (bash extensions) — error as invalid options like any
  unknown flag.
- `CDPATH` — huck has none; unchanged.
- Startup `$PWD` VALIDATION (bash checks the inherited `$PWD` resolves to the
  cwd's inode; huck trusts the inherited value) — pre-existing, not changed here.
- The deep-symlink logical-`cd ..` whose lexical parent doesn't physically exist:
  huck errors (rc 1) rather than bash's fallback-retry. Rare; log as a low
  divergence only if it surfaces in testing.

## Error handling

- `cd`/`pwd` unknown flag → `huck:`-prefixed `invalid option` + usage line, rc 2.
- `cd` chdir failure → `huck: cd: {target}: {error}`, rc 1.
- All stderr uses huck's `huck:` prefix (the established prefix-divergence class);
  harnesses compare stdout + rc.

## Testing strategy

`tests/scripts/cd_pwd_physical_diff_check.sh` — byte-identical bash↔huck
(stdout + rc), built around a `mktemp -d` containing a real dir + a symlink to
it (the harness creates and cleans up its own fixture). Cases:
1. logical default: `cd $T/link; echo "$PWD"; pwd; pwd -L` → all the symlink path.
2. `pwd -P` → resolved real path.
3. `cd -P $T/link; echo "$PWD"` → resolved.
4. `cd -L $T/link; echo "$PWD"` → logical.
5. last-wins: `cd -L -P $T/link` → physical; `cd -P -L $T/link` → logical;
   `pwd -L -P` / `pwd -P -L`.
6. `cd $T/link; cd ..; echo "$PWD"` → lexical parent.
7. `set -o physical`: `cd $T/link; echo "$PWD"` → resolved; bare `pwd` → resolved.
8. `cd -` round-trip (logical): `cd $T/link; cd /tmp; cd -; echo "$PWD"` → logical.
9. `pwd -x` rc 2; `pwd foo` rc 0 + prints pwd.
10. `cd /; echo "$PWD"` → `/`.

Each case captures bash's output first and asserts huck matches (paths use the
`mktemp` dir, so they're machine-independent within the run). Plus Rust unit
tests on `normalize_logical`: `/a/b/../c`→`/a/c`, `/a/./b`→`/a/b`, `/a//b`→`/a/b`,
`/..`→`/`, `/a/../..`→`/`, `/`→`/`.

Full `cargo test` + all existing harnesses stay green (except I-01-encoding tests
that get updated to logical behavior).

## Components touched

- `src/builtins.rs` — `normalize_logical` helper; rewrite `builtin_cd` (flags +
  logical/physical paths); rewrite `builtin_pwd` (flags + mode + new signature);
  wire the `physical` option into both; update the `run_builtin` `pwd` dispatch.
- `docs/bash-divergences.md` — delete I-01, M-32, M-33; update tier counts.
- `tests/scripts/cd_pwd_physical_diff_check.sh` (new) + `normalize_logical` unit
  tests; update any I-01-encoding existing test.
