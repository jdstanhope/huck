# v76 — Programmable Completion (M-36) Design Spec

**Date**: 2026-06-02
**Iteration**: v76
**Divergence closed**: M-36 (`complete` builtin / programmable completion) — the
last remaining high-priority Tier-2 deferral.

## Goal

Add bash-style programmable completion so that real-world completion scripts
(`/etc/bash_completion`, `git-completion.bash`, `kubectl completion bash`,
etc.) can be sourced into huck and fire correctly at the Tab key.

Concretely: ship the `complete`, `compgen`, and `compopt` builtins; the
`COMP_WORDS` / `COMP_CWORD` / `COMP_LINE` / `COMP_POINT` / `COMPREPLY`
variables; and the tab-completion dispatcher that runs a registered
`complete -F func cmd` function when the user presses Tab on `cmd <args>`.

## Non-goals

These are explicitly deferred (see § Deferrals at the end) and become
`[deferred]` follow-on entries in `docs/bash-divergences.md`:

- `complete -C "shell-cmd"` (run subshell, parse stdout as completions)
- `complete -I` (initial-word completion, bash 5.2+)
- `complete -b` (load builtin completion shortcut)
- 16 of bash's 25 `-A` actions (e.g. `arrayvar`, `binding`, `shopt`, `signal`)
- `-o` options `nosort`, `noquote`, `plusdirs`
- `compopt -D` / `-E` (mutate default/empty specs from inside a function)
- `COMP_TYPE`, `COMP_KEY` variables
- Bash's quirky `COMP_WORDBREAKS` default (we default to whitespace-only;
  it remains settable)
- Timeout on `-F` execution (a runaway completion function hangs the prompt)

## Architecture overview

Three new/grown modules, each with one clear responsibility:

| File | Purpose | LOC est |
|---|---|---|
| `src/completion.rs` | rustyline `HuckHelper` + `analyze()` cursor classifier + `dispatch::resolve()` orchestrator | 757 → ~1100 |
| `src/completion_spec.rs` | `CompletionSpec` data + `resolve_spec()` generator (turns a spec into candidates; owns `-F` invocation) | new, ~500 |
| `src/completion_builtins.rs` | `builtin_complete` / `builtin_compgen` / `builtin_compopt` | new, ~700 |

Data flow:

```
                 +-------- complete/compgen/compopt -------+
                 |   (parses flags, builds CompletionSpec) |
                 +--------------------+--------------------+
                                      |
                                      v
              +---------------- Shell.completion_specs -----+
              |   HashMap<String, CompletionSpec>           |
              |   + default_spec   (-D)                     |
              |   + empty_spec     (-E)                     |
              +---------------------+-----------------------+
                                    |
                                    v  read
   rustyline complete()  ->  dispatch::resolve(line, pos, &mut Shell)
                                    |
                          +---------+---------+
                          |                   |
                          v                   v
                  static (-W/-A/-G/-X)  function (-F)
                          |                   |
                          v                   v
                       Vec<Candidate>      Vec<Candidate> (from COMPREPLY)
```

`compgen` is a thin wrapper around the same `resolve_spec()` so that
"what tab completion produces" and "what `compgen` produces" stay
identical — which is what completion scripts assume.

## Foundation: `Rc<RefCell<Shell>>`

The single biggest architectural change. Currently `Shell` is constructed
in `src/shell.rs::run()` as `let mut shell = Shell::new();` and passed by
`&mut Shell` throughout. The rustyline `Completer::complete` callback is
`&self`, so it cannot mutate shell state — meaning a `-F` function called
from inside completion cannot use the executor as-is.

The fix: wrap `Shell` in `Rc<RefCell<Shell>>` at construction. Internal
APIs **keep their `&mut Shell` signatures**; only the readline boundary
acquires/releases the borrow.

### Changes by file

**`src/shell.rs::run()`** — ~40 lines edited:
```rust
let shell_cell = Rc::new(RefCell::new(Shell::new()));
// every prior `&mut shell` becomes a scoped borrow:
{
    let mut shell = shell_cell.borrow_mut();
    install_sigint_handler(Arc::clone(&shell.sigint_flag));
    install_sigchld_handler(Arc::clone(&shell.sigchld_flag));
    shell.history.load();
}
// ... helper wiring with a clone of the Rc:
editor.set_helper(Some(HuckHelper::new(Rc::clone(&shell_cell))));
```

The main loop's iteration body becomes a series of scoped borrows around
the `read_logical_command` call.

**`src/shell.rs::read_logical_command`** — signature changes from
`shell: &mut Shell` to `cell: &RefCell<Shell>`. Every block that needs
shell does a scoped `let mut shell = cell.borrow_mut();`. Crucially, **no
borrow is held across the `editor.readline()` call** — that's the
invariant rustyline's helper relies on.

**`src/completion.rs::HuckHelper`** — struct shape changes:
```rust
pub struct HuckHelper {
    shell: Rc<RefCell<Shell>>,
}

impl HuckHelper {
    pub fn new(shell: Rc<RefCell<Shell>>) -> Self { Self { shell } }
}

impl rustyline::completion::Completer for HuckHelper {
    type Candidate = rustyline::completion::Pair;
    fn complete(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>)
        -> rustyline::Result<(usize, Vec<Self::Candidate>)>
    {
        let mut shell = self.shell.borrow_mut();
        let (start, pairs) = dispatch::resolve(line, pos, &mut shell);
        Ok((start, pairs))
    }
}
```

The prior `var_names`/`path`/`home` snapshot fields go away — the helper
reads live state. `HuckHelper::refresh` is removed; its call site in the
main loop is deleted.

**`src/main.rs`** — adds two `mod completion_spec;` / `mod completion_builtins;`
lines.

**Internal modules** (`executor.rs`, `expand.rs`, `param_expansion.rs`,
`builtins.rs`, `arith.rs`, `traps.rs`, `test_builtin.rs`, `shell_state.rs`)
— **zero signature changes**. All 137 existing `&mut Shell` signatures
remain.

### Soundness note

The current `read_logical_command` holds `shell: &mut Shell` across the
`editor.readline()` call (line 252 → 272). Under stacked-borrows rules
that's UB if rustyline ever calls back into shell via the helper. The
refactor fixes this latent issue as a side effect.

## Data: `CompletionSpec` and the registry

```rust
// in src/shell_state.rs
pub struct Shell {
    // ... existing fields ...
    pub completion_specs: CompletionSpecs,
    pub current_completion_spec: Option<CompletionSpec>,
}

// in src/completion_spec.rs
#[derive(Debug, Default, Clone)]
pub struct CompletionSpecs {
    pub by_command: HashMap<String, CompletionSpec>,
    pub default_spec: Option<CompletionSpec>,  // -D
    pub empty_spec: Option<CompletionSpec>,    // -E
}

#[derive(Debug, Clone, Default)]
pub struct CompletionSpec {
    // Content generators — multiple may be set; results concatenated.
    pub function: Option<String>,     // -F func
    pub wordlist: Option<String>,     // -W "a b c"  (raw; IFS-split at use)
    pub glob: Option<String>,         // -G "*.txt"
    pub actions: Vec<Action>,         // -A directory, -A function, ...

    // Decoration (applied after generation)
    pub prefix: Option<String>,       // -P
    pub suffix: Option<String>,       // -S
    pub filter: Option<String>,       // -X pattern (! prefix = remove matches)

    // Behavior flags
    pub options: CompOptions,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CompOptions {
    pub default: bool,        // -o default
    pub nospace: bool,        // -o nospace
    pub filenames: bool,      // -o filenames
    pub bashdefault: bool,    // -o bashdefault
    pub dirnames: bool,       // -o dirnames
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    File, Directory, Command, Function, Variable, Alias, Builtin, Keyword,
}
```

Notes:
- Specs are **cloneable** because the dispatch path clones the spec out
  of the registry borrow before invoking `-F` (which may itself mutate
  the registry via `compopt`).
- `wordlist` is stored raw, NOT pre-split. `-W` is documented as
  expanded at completion time — `complete -W '$opts' mycmd` re-reads
  `$opts` on every Tab.
- `current_completion_spec` is the ephemeral slot used by `compopt`
  inside a `-F` function. Set right before invoking `-F`, taken back
  out after the function returns, then used for the rest of
  `resolve_spec`'s decoration / fallback / filename-rendering pass.
  No interior mutability needed — `compopt` receives `&mut Shell` like
  any builtin and mutates it directly.

## Dispatch ladder

When `dispatch::resolve(line, pos, &mut Shell)` runs:

1. Call existing `analyze(line, pos)` for cursor context and replacement
   offset. Three contexts: `Command { prefix }`, `Variable { prefix }`,
   `File { dir, prefix }`.
2. **Variable context** → existing `complete_variable()`. Done.
3. **Command context** with empty line and `-E` spec → run `-E` spec. Done.
4. **Command context** with non-empty prefix → existing `complete_command()`
   (builtins + PATH). Done. (User-registered `-F` specs do NOT apply
   to word 0; matches bash.)
5. **File context** (cursor is on an argument):
   1. Extract the **command word** of the current simple command (word 0,
      after the most recent `;` / `|` / `&&` / `||` / compound keyword).
      Reuses the same scanning logic `analyze()` already does.
   2. Tokenize the simple command into `COMP_WORDS` per `COMP_WORDBREAKS`.
   3. Look up `shell.completion_specs.by_command[cmd]`. If missing,
      look up `default_spec` (`-D`). If still missing, fall back to
      existing `complete_file()`. Done.
   4. With a spec found, clone it out of the registry borrow, then run
      `resolve_spec(&spec, ctx, &mut Shell)`:
      - `-F func` results (sets COMP_*, runs function, reads COMPREPLY) ++
      - `-W wordlist` results (IFS-split at use, filtered by current word prefix) ++
      - `-G glob` results (pathname expansion against CWD) ++
      - `-A action` results (Directory → dirs; Function → fn names; etc.)
   5. **Filter** through `-X pattern` (POSIX glob match; `!pat` keeps
      only non-matches, plain `pat` removes matches).
   6. **Decorate** with `-P prefix` and `-S suffix`.
   7. **Empty-fallback**:
      - empty + `options.default` → `complete_file()`.
      - empty + `options.bashdefault` → re-run the file/variable/command
        ladder (steps 2–4 above).
      - else → empty result.
   8. **Filename rendering**: if `options.filenames`, treat each result
      as a filename (escape metachars via existing `escape_filename()`;
      append `/` to directories via existing `std::fs::metadata` check).
   9. Sort + dedupe.

## `-F` function invocation

When a spec has `function: Some("_git")`:

**Setup** (before function call):
1. Tokenize the current simple command into `comp_words` per
   `COMP_WORDBREAKS`.
2. Compute `comp_cword` (zero-based index of the cursor word in
   `comp_words`).
3. Compute `cur_word` (the cursor word — possibly empty) and `prev_word`
   (word at `comp_cword - 1`, or empty).
4. Set shell variables (saving prior values for restore):
   - `COMP_LINE` ← entire line
   - `COMP_POINT` ← byte offset of cursor as decimal string
   - `COMP_WORDS` ← indexed array of `comp_words`
   - `COMP_CWORD` ← `comp_cword` as decimal string
   - `COMPREPLY` ← unset (so we can detect empty result)
5. Stash the spec into `shell.current_completion_spec` so `compopt`
   inside the function can mutate the live spec.
6. Snapshot `shell.positional_params` and `shell.last_status`.
7. Replace positional params with `[cmd_name, cur_word, prev_word]`.

**Invocation**:
- Look up the function in `shell.functions`. If missing → empty result,
  cleanup, return.
- Call the function body via the existing executor function-call path.
  We factor a small `pub(crate) fn call_function_body(shell, name, args)
  -> ExecOutcome` helper out of `src/executor.rs` if one isn't already
  there.
- An `ExecOutcome::Exit(n)` propagates — the shell exits. Matches bash.
- `FunctionReturn(_)` / `LoopBreak` / `LoopContinue` all coerce to "done,
  read COMPREPLY."

**Teardown** (after function returns):
- Restore positional params and `last_status` (completion functions do
  NOT pollute `$?`).
- Take the (possibly mutated) spec back out of
  `shell.current_completion_spec` and use it for the rest of
  `resolve_spec`'s decoration / fallback / filename-rendering.
- Read `COMPREPLY` as an indexed array via
  `shell.get_array("COMPREPLY")`; values in index order.
- Leave `COMP_LINE` / `COMP_POINT` / `COMP_WORDS` / `COMP_CWORD` set
  (matches bash — they remain readable until next completion).
- If a pending fatal PE error was set during the function (e.g.,
  `set -u` fired on an unbound var), drain it so the next prompt is
  clean.

**SIGINT during function**: existing handler fires; we catch and
return empty results so the prompt redraws.

## Tokenization: `COMP_WORDBREAKS`

- Default value: `' \t\n'` (whitespace only). Documented divergence
  from bash's default of `' \t\n"\'><=;|&(:'`.
- Readable and writable as a normal shell variable.
- The tokenizer for `COMP_WORDS`:
  - Whitespace chars in the wordbreaks set act as **separators only**
    (consumed, not emitted as words).
  - Non-whitespace chars in the wordbreaks set act as
    **single-character separator-words** — they break the surrounding
    word and themselves become a one-char entry in `COMP_WORDS`. So
    if a user sets `COMP_WORDBREAKS=' \t\n:'`, then `user:pass` →
    `["user", ":", "pass"]`.
  - Quotes are NOT respected during tokenization (matches bash).
  - The cursor word includes the partial text before the cursor only —
    text after the cursor is ignored, matching huck's existing
    `analyze()` behavior.

## Builtin: `complete`

Lives in `src/completion_builtins.rs`. Entry point:
```rust
pub fn builtin_complete(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome
```
Wired into `run_builtin` in `src/builtins.rs`; added to `BUILTIN_NAMES`.
Not a POSIX special builtin.

**Flag grammar**:
```
complete [-DE] [-F func] [-W wordlist] [-G glob] [-A action]...
         [-P prefix] [-S suffix] [-X filter]
         [-o option]... [--] [name ...]
complete -p [name ...]
complete -r [name ...]
```

**Flag handling**:
- Short flags only; may cluster (`-rF` etc., though combinations that
  mix `-r` with content flags error).
- `--` ends flags.
- `-F`, `-W`, `-G`, `-A`, `-P`, `-S`, `-X`, `-o` each consume an arg.
  Both inline (`-Ffunc`) and split (`-F func`) accepted, matching the
  convention used by huck's `read`, `printf`, `hash`.
- `-A` accepts: `file`, `directory`, `command`, `function`, `variable`,
  `alias`, `builtin`, `keyword`. Unknown action → exit 2 with
  `huck: complete: ACTION: invalid action name`.
- `-o` accepts: `default`, `nospace`, `filenames`, `bashdefault`,
  `dirnames`. Unknown option → exit 2 with
  `huck: complete: OPTION: invalid completion option`.
- Multiple `-A`s and multiple `-o`s accumulate.
- `-D` and `-E` set the default/empty slot instead of per-name; mutually
  exclusive with each other but may combine with content flags. With
  names AND `-D`/`-E` → error
  `huck: complete: cannot use -D or -E with command names`, exit 2.

**Behavior modes**:
- **Print** (`-p` or bare `complete`): iterate registry sorted by
  name; print each spec in re-input form:
  `complete -F _git -o default -- git`. With names, prints only
  those (exit 1 if any missing). Bare `complete` ≡ `complete -p`.
- **Remove** (`-r`): with names, remove each (exit 1 if any missing);
  no names → clear `by_command` (does NOT clear `default_spec` /
  `empty_spec`; use `complete -rD` / `complete -rE` for those).
- **Register**: build a `CompletionSpec` from flags. With one or more
  names, install into `by_command[name]` (overwriting any prior spec).
  With `-D` flag, set `default_spec`. With `-E`, set `empty_spec`.
  With names AND no content flags → exit 1 with
  `huck: complete: nothing to complete` (matches bash).

**Re-input formatting** (for `-p`):
- Literal `complete`, then flags in canonical deterministic order:
  `-F`, `-W`, `-G`, then each `-A`, `-P`, `-S`, `-X`, then each `-o`,
  then `--`, then names.
- Arg values single-quote-escaped via the existing
  `escape_alias_value()` helper (so `complete -W 'a b c' -- mycmd`).
- **Bash divergence**: bash's flag-ordering varies; we pick a
  deterministic order and document this as a low-impact L-* entry.

**Exit status**: 0 success; 1 missing-name (with `-p` / `-r`); 2 flag-parse
or invalid action/option.

## Builtin: `compgen`

Lives in `src/completion_builtins.rs`. Wired into `run_builtin`; added
to `BUILTIN_NAMES`. Not POSIX-special. Shares the flag-parser helper
with `complete`.

```
compgen [-F func] [-W wordlist] [-G glob] [-A action]...
        [-P prefix] [-S suffix] [-X filter] [-o option]... [--] [word]
```

- No `-D` / `-E` / `-p` / `-r`. Takes an optional final positional
  `word` (the prefix to match against).
- Builds a `CompletionSpec` from flags (using the same parser), then
  runs `resolve_spec()` against a synthetic context:
  - `cur_word` ← `word_arg.unwrap_or("")`
  - `prev_word` ← `""`
  - `comp_words` ← `[word]`
  - `comp_cword` ← `0`
  - `comp_line` ← `word`
  - `comp_point` ← `word.len()`
  - For `-F`: the `cmd_name` argument is literally `"compgen"` (matches
    bash).
- Print each result on its own line to stdout.
- Exit 0 if at least one match, 1 if none, 2 on flag error.

## Builtin: `compopt`

Lives in `src/completion_builtins.rs`. Wired into `run_builtin`. Not
POSIX-special.

```
compopt [+o option]... [-o option]... [name ...]
```

**Inside a `-F` function** (no names): mutate the **live spec** for the
current completion via `shell.current_completion_spec` (set by the
dispatch path before `-F` invocation).

**Outside a `-F` function** (with names): mutate `by_command[name]`
directly.

**No name + not in a function** → exit 1 with
`huck: compopt: not currently executing completion function`.

- `-o foo` sets the option to true; `+o foo` sets to false.
- Options accepted: `default`, `nospace`, `filenames`, `bashdefault`,
  `dirnames` (same five as `complete -o`).
- Unknown option → exit 2.
- Bash's `compopt -D` / `-E` (mutate default/empty specs) deferred.

## Testing strategy

Three layers, mirroring v71 / v72 / v74 / v75:

### Unit tests (in-file `#[cfg(test)] mod tests`)

**`src/completion_spec.rs`** (~25 tests):
- `CompletionSpec` building from flag combinations.
- `resolve_spec` with each generator in isolation
  (`-W` only, `-G` only, `-A directory` only, etc.).
- `-X filter` semantics (`pat` removes, `!pat` keeps-only).
- `-P`/`-S` decoration; filter applies BEFORE prefix/suffix.
- `-W` IFS-splitting at use-time (changing `$IFS` changes the split).
- `Action::Function` enumerates `shell.functions`; `Action::Variable`
  enumerates set vars; `Action::Alias` enumerates aliases; `Action::Builtin`
  enumerates `BUILTIN_NAMES`.

**`src/completion_builtins.rs`** (~30 tests):
- Flag parser: each flag's arg consumption (inline and split).
- `--` terminator.
- Mutual-exclusivity errors (`-D` + named, `-r` + content flags).
- `-r name` missing-name → exit 1.
- `-r` (no name) clears `by_command` but not `default_spec` / `empty_spec`.
- `-p` re-input form is round-trippable (parse output, get same spec).
- `compgen -W "a b c" -- a` returns `["a"]` only.
- `compopt` outside `-F` → exit 1 with diagnostic.
- Invalid action / invalid option → exit 2.

**`src/completion.rs`** (~15 new on top of existing 30):
- Dispatch ladder: variable context bypasses `-F`; command-pos word 0
  bypasses `-F`.
- Tokenization: default `COMP_WORDBREAKS=' \t\n'` splits whitespace only.
- Tokenization with custom `COMP_WORDBREAKS=' \t\n:'` splits on `:`.
- Empty-fallback: `-o default` falls back to file completion when `-F`
  returns empty.
- `-o filenames` decoration: directories get trailing `/`; special chars
  escaped.

### Integration tests (`tests/completion_integration.rs`, new)

~15 tests via `assert_cmd`-driven `huck` invocations. All use `compgen`
since tab completion proper requires interactive tty:
- `compgen -W "alpha alpine beta" -- al` → `alpha\nalpine\n`.
- `compgen -A directory -- src` enumerates directories.
- `compgen -A function` after defining a function returns it.
- `complete -F _foo foo; complete -p foo` round-trips.
- `compgen -F _foo -- partial` invokes `_foo`, returns COMPREPLY.
- Inside `_foo`: `COMP_WORDS`, `COMP_CWORD`, `$1`, `$2`, `$3` correctly
  populated.
- `compopt -o nospace` inside `_foo` mutates current spec (visible after
  via `complete -p foo`).
- `complete -r foo` removes; subsequent `complete -p foo` → exit 1.
- `complete -D -F _default` registers; applies when no specific spec
  found.
- `-X '!*.txt'` filters to only `.txt` matches.
- `-P 'pre_' -S '_suf'` decorates each result.
- Quoted `-W '$opts'` re-expands `$opts` at completion time.

### Bash-diff harness (`tests/scripts/completion_diff_check.sh`, new)

~12–15 fragments run byte-identically against bash 5.2 and huck:
- `compgen -W 'alpha alpine beta' -- al`
- `compgen -A directory -- src` (in a known tree)
- `compgen -A function` (after defining a function)
- `complete -F _f cmd; _f() { COMPREPLY=(hi there); }; compgen -F _f -- ""`
- `compgen -W "aa ab" -P 'x:' -S ':y' -- a`
- `compgen -W "alpha apple banana" -X '!a*' --`
- A function reading `${COMP_WORDS[@]}` and `$COMP_CWORD` produces
  expected splits (with a fixed `COMP_WORDBREAKS=' \t\n'` for both
  bash and huck so the test is meaningful).

Fragments that intentionally diverge (e.g., `complete -p` re-input
ordering) are excluded with a comment.

### Test budget

~70 new unit tests + ~15 integration tests + ~12–15 diff-check
fragments. Same ballpark as v72.

## Deferrals (become entries in `bash-divergences.md`)

After v76 ships, M-36's status becomes `[fixed v76 partial]` and the
following deferrals are recorded as a single follow-on list:

**`complete` flags not implemented** (parsed → "not yet supported", exit 1):
- `-C "shell-cmd"` — runs the command in a subshell, parses stdout
- `-I` — initial-word completion (bash 5.2+)
- `-b` — load builtin completion shortcut

**`-A` actions not implemented** (rejected with `invalid action name`):
- `arrayvar`, `binding`, `disabled`, `enabled`, `export`, `group`,
  `helptopic`, `hostname`, `job`, `running`, `service`, `setopt`,
  `shopt`, `signal`, `stopped`, `user` (16 of 25 bash actions)

**`-o` options not implemented**:
- `nosort` — would require disabling existing sort in `complete_file`
- `noquote` — affects post-completion behavior
- `plusdirs` — additional directory completions alongside main results

**`compopt`**:
- `-D` / `-E` (mutate default/empty specs from within a function)

**`COMP_*` variables not implemented**:
- `COMP_TYPE` (completion type indicator)
- `COMP_KEY` (the key that fired completion)
- `BASH_COMPLETION_COMPAT_DIR` and related env-var hooks

**Behavioral**:
- No menu-completion (cycle through repeated Tab)
- `COMP_WORDBREAKS` defaults to whitespace-only, not bash's
  `' \t\n"\'><=;|&(:'`. Settable.
- No timeout on `-F` function execution

**Low-impact divergence** (gets its own L-* entry):
- `complete -p` re-input form uses deterministic flag ordering;
  bash's ordering varies.

## Migration / change-log entry

The change-log block to add to `docs/bash-divergences.md` after merge:

> **2026-06-02** (placeholder — implementer updates to merge date):
> v76 ships M-36 partial — programmable completion.
> New `src/completion_spec.rs` (~500 LOC) holding the `CompletionSpec`
> data and the `resolve_spec()` generator. New `src/completion_builtins.rs`
> (~700 LOC) for `complete` / `compgen` / `compopt`. `src/completion.rs`
> grows from 757 → ~1100 with the `dispatch::resolve()` orchestrator and
> the `Rc<RefCell<Shell>>` `HuckHelper`. The main loop in `src/shell.rs`
> refactors to wrap Shell in `Rc<RefCell<Shell>>` and scope all borrows
> around `editor.readline()` — internal `&mut Shell` signatures
> elsewhere unchanged. New `Shell.completion_specs` and ephemeral
> `Shell.current_completion_spec` fields. Three new builtins added to
> `BUILTIN_NAMES` (none POSIX-special). Standard surface: `-F`/`-W`/
> `-G`/`-A` (8 actions: `file`, `directory`, `command`, `function`,
> `variable`, `alias`, `builtin`, `keyword`)/`-P`/`-S`/`-X`/`-o` (5
> options: `default`, `nospace`, `filenames`, `bashdefault`, `dirnames`)/
> `-D`/`-E`/`-p`/`-r`/`--`. `COMP_WORDBREAKS` defaults to whitespace-only
> (documented divergence from bash's default). ~70 unit tests + ~15
> integration tests + 12–15 bash-diff fragments (huck's 4th harness).
> Deferred from this iteration: `-C`/`-I`/`-b`, 16 obscure `-A` actions,
> `-o {nosort,noquote,plusdirs}`, `compopt -D`/`-E`, `COMP_TYPE`/
> `COMP_KEY`.

## Open questions

None. All architectural decisions resolved during brainstorm.
