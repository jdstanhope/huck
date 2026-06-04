# huck v88 — `complete`/`compgen` action expansion Design

**Status:** approved design, ready for implementation plan.
**Implements:** the full bash `complete`/`compgen` action surface — all **24** `-A`
action names and the **12** single-letter action shortcuts (`-a`/`-b`/`-c`/`-d`/
`-e`/`-f`/`-g`/`-j`/`-k`/`-s`/`-u`/`-v`). huck recognizes only 8 actions and no
short-flag forms today, so a real bashrc/bash-completion's `complete -A stopped`,
`complete -u`, `complete -A setopt`, etc. fail with "invalid action name" /
"invalid option". Beyond *recognition*, v88 also *generates* completions for the
actions whose data huck already holds.
**Discovered:** loading a stock Debian `~/.bashrc` (its bash-completion setup
registers completions with `-u`/`-j`/`-v`/`-a`/`-c`/`-b` short flags and
`-A stopped`/`setopt`/`shopt`/`helptopic`).
**Divergence tracker:** extend **M-36** (programmable completion) with a new
sub-entry **M-36a** `[fixed v88]` + **M-36b** `[deferred]` (system-data actions).
**Branch (impl):** `v88-complete-actions` (created from `main` at plan time).

## Scope

Decided during brainstorming:

1. **Recognize the full action set + short flags** so `complete`/`compgen` never
   error on a real bashrc.
2. **Generate completions for the cheap shell-data subset**; the system-data
   actions are recognized-but-empty (deferred).

**Out of scope** (recognized-but-empty, documented): `hostname` (would need
`/etc/hosts`/`$HOSTFILE`), `user` (`/etc/passwd`/libc), `group` (`/etc/group`),
`service` (`/etc/services`) — deferred as **M-36b**; plus `binding` (huck exposes
no readline key bindings) and `disabled` (huck has no `enable -n` / disabled-builtin
concept) — these are permanently empty, not deferred work. Also out of scope: any
non-action `complete` flag not already supported (e.g. `-C`); only actions + their
short flags are added.

## Verified bash 5.2 facts (the implementation targets these)

- **All 12 short flags and all 24 action names are *accepted*** (no "invalid"
  error). An empty result is rc 1 with no message; an unknown action/flag is rc 2
  with a message. v88 must make all 36 accepted.
- **`compgen` emits each action's natural *source order* — it does NOT sort.**
  `compgen -A shopt` is in bash's shopt-table order (`autocd`,
  `assoc_expand_once`, `cdable_vars`, …, NOT alphabetical); `compgen -A setopt`
  is alphabetical only because bash's `set -o` table is. This dictates the
  byte-diff harness (below): the `setopt`/`shopt` generators must emit
  **table order**, not `.sort()`ed order.
- `signal` names are **`SIG`-prefixed** (`compgen -A signal SIGIN` → `SIGINT`).
- `compgen` prints one candidate per line, rc 0 if any matched else rc 1.

### The 12 short-flag → action map

`-a`→Alias, `-b`→Builtin, `-c`→Command, `-d`→Directory, `-e`→Export, `-f`→File,
`-g`→Group, `-j`→Job, `-k`→Keyword, `-s`→Service, `-u`→User, `-v`→Variable.

### The 24 `-A` action names

Existing (8): `file directory command function variable alias builtin keyword`.
New (16): `arrayvar binding disabled enabled export group helptopic hostname job
running service setopt shopt signal stopped user`.

## Part 1 — Action set + short-flag forms

**`src/completion_spec.rs`** — the `Action` enum grows from 8 to 24 variants
(add `Arrayvar, Binding, Disabled, Enabled, Export, Group, Helptopic, Hostname,
Job, Running, Service, Setopt, Shopt, Signal, Stopped, User`). `Action::parse`
maps all 24 names → variants; `Action::name` round-trips all 24 (used by
`complete -p` reconstruction).

**`src/completion_builtins.rs`** — in the flag-parse loop, the 12 letters that
currently fall into the `other => "-{other}: invalid option"` arm become explicit
cases that push the mapped `Action` onto `out.spec.actions` (semantically
identical to `-A name`). Like `-A`, the short flags reject a leading `+`
(consistent with the existing `F/W/G/A/P/S/X` handling). They take no argument.

After Part 1, every `complete`/`compgen` action invocation from a real bashrc is
accepted; registration (`complete …`) succeeds and stores the actions.

## Part 2 — Generators (`enumerate_action`, `src/completion_spec.rs`)

`enumerate_action(action, prefix, &Shell) -> Vec<String>` gets 16 new arms. All
filter by `prefix` (a candidate is kept iff `candidate.starts_with(prefix)`).

**Generate from shell data (10):**

| Action | Source | Order |
|--------|--------|-------|
| `Setopt` | the 27 `set -o` names (new `pub` accessor in `builtins.rs`) | **table order** (no sort) |
| `Shopt` | the 57 shopt names (`SHOPT_TABLE`, already `pub`) | **table order** (no sort) |
| `Helptopic` | the 60 `HELP_ENTRIES` names (new `pub` accessor) | `HELP_ENTRIES` order |
| `Signal` | `SIG`-prefixed signal names (new `pub` accessor over huck's trap/kill signal table) | table order |
| `Export` | variable names where `is_exported` is true | sorted |
| `Arrayvar` | variable names whose value is `VarValue::Indexed`/`Associative` | sorted |
| `Enabled` | all `BUILTIN_NAMES` (huck has no disabled builtins) | sorted |
| `Job` | every job's display token from the job table | table order |
| `Running` | jobs with `JobState::Running` | table order |
| `Stopped` | jobs with `JobState::Stopped` | table order |

- `Setopt`/`Shopt` emit their **table order** (matching bash's non-sorting
  `compgen`) — these are the harness's byte-diff fragments. The other generators'
  order is not byte-diff-checked (their content differs from bash or is volatile),
  so `sorted` vs table is a readability choice; sorting matches the existing
  `Variable`/`Alias`/`Builtin` arms.
- `Job`/`Running`/`Stopped` read the job table (typically empty non-interactively;
  wired correctly regardless). The "display token" is the job's command string
  (bash completes job specs; matching bash's exact jobspec text is out of scope —
  membership/empty is what's tested).

**Recognize-but-empty (6):** `Disabled`, `Binding`, `Hostname`, `User`, `Group`,
`Service` → `Vec::new()`. These succeed with empty output (rc 1 from `compgen`,
exactly like bash when the underlying source is empty). `Hostname`/`User`/`Group`/
`Service` are deferred work (**M-36b**); `Disabled`/`Binding` are permanently
empty in huck.

### New `pub` accessors

- `src/builtins.rs`: `pub fn seto_option_names() -> impl Iterator<Item = &'static str>`
  (the `SETO_TABLE` names, table order); `pub fn help_topic_names() -> impl
  Iterator<Item = &'static str>` (the `HELP_ENTRIES` names); `pub fn signal_names()
  -> Vec<String>` (`SIG`-prefixed names from huck's existing signal table —
  reuse whatever backs `kill -l`/`trap -l`).
- `src/shell_state.rs`: `SHOPT_TABLE` is already `pub`. Add (if not present) a way
  to read the job table immutably (e.g. `JobTable::jobs(&self) -> &[Job]` in
  `src/jobs.rs`) and to test a variable's array-ness (iterate `var_names` +
  inspect the stored `VarValue`; add a small `pub fn array_var_names(&self) ->
  Vec<String>` on `Shell` if cleaner than exposing internals).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/completion_spec.rs` | `Action` enum +16 variants; `parse`/`name` cover all 24; `enumerate_action` +16 arms (10 generate, 6 empty) |
| `src/completion_builtins.rs` | 12 short-flag cases in the flag-parse loop → push mapped `Action` |
| `src/builtins.rs` | `pub` name accessors: `seto_option_names`, `help_topic_names`, `signal_names` |
| `src/shell_state.rs` | `pub fn array_var_names`; (job-table read accessor if needed) |
| `src/jobs.rs` | `JobTable::jobs(&self) -> &[Job]` read accessor (if not already present) |
| `tests/complete_actions_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/complete_actions_diff_check.sh` | NEW — huck's 15th bash-diff harness (`setopt`/`shopt` only) |
| `docs/bash-divergences.md`, `README.md` | M-36 update + M-36a `[fixed v88]` + M-36b `[deferred]`; changelog; README v88 row |

## Testing

1. **Unit tests** (`completion_spec.rs` / `completion_builtins.rs`):
   - `Action::parse` accepts all 24 names and round-trips through `name()`;
     unknown name → `None`.
   - Each of the 12 short flags pushes the correct `Action` onto the spec
     (e.g. `complete -u -- foo` → `spec.actions == [User]`; `complete -ev -- foo`
     clustered → `[Export, Variable]`).
   - `enumerate_action` membership: `Setopt` contains `errexit`; `Shopt` contains
     `nullglob`; `Helptopic` non-empty; `Signal` contains `SIGINT`; `Export` lists
     an exported var but not an unexported one; `Arrayvar` lists an indexed array
     but not a scalar; `Enabled` contains a known builtin; the 6 empty actions
     return `vec![]`.
   - `Setopt`/`Shopt` arms emit **table order** (assert first two `shopt` names are
     `autocd`, `assoc_expand_once` — i.e. NOT sorted).
2. **Integration tests** (`tests/complete_actions_integration.rs`):
   - Registration never errors: `complete -u cmd; echo $?` → 0;
     `complete -A stopped cmd; echo $?` → 0; `complete -A setopt -A shopt cmd;
     echo $?` → 0; `complete -ev cmd; echo $?` → 0.
   - Generation: `compgen -A setopt e` → `emacs\nerrexit\nerrtrace`;
     `compgen -A shopt null` → `nullglob`; `compgen -A signal SIGIN` → `SIGINT`;
     `export FOO=1; compgen -A export FO` → `FOO`; `compgen -v PA` includes `PATH`;
     `compgen -b ec` → `echo`; `arr=(x y); compgen -A arrayvar ar` → `arr`.
   - Recognize-but-empty: `compgen -A hostname x; echo $?` → 1 (no error text);
     `compgen -A binding; echo $?` → 1.
3. **bash-diff harness** `tests/scripts/complete_actions_diff_check.sh`
   (huck's 15th), byte-identical to bash 5.2 — **only the deterministic,
   content-and-order-identical generators**: `compgen -A setopt` (+ a prefix),
   `compgen -A shopt` (+ a prefix), and the registration-rc fragments
   (`complete -u cmd; echo rc=$?`, `complete -A stopped cmd; echo rc=$?`). A NOTE
   comment documents why `builtin`/`keyword`/`helptopic`/`command`/`file`/
   `variable`/`signal`/`job` are NOT byte-diffed (their candidate *sets* differ
   between huck and bash — different builtin tables, env, PATH, platform signal
   set — or are volatile; they are membership-tested in integration instead, not
   weakened in the harness).

## Edge cases & notes

- Short flags cluster with each other and with existing flags
  (`complete -bc cmd` → `[Builtin, Command]`); each is argument-less so clustering
  is straightforward.
- `compgen` prints source order (no global sort); `Setopt`/`Shopt` must therefore
  preserve table order to byte-match bash. Re-sorting them would silently break
  the harness.
- `compgen -A <empty-source-action>` returns rc 1 with no stderr (matches bash's
  empty-result behavior) — NOT rc 2 (which is reserved for genuinely invalid
  actions/flags, of which there are now none in the bash set).
- The 8 existing actions and all existing `complete`/`compgen` behavior are
  unchanged; this is purely additive.
- **`complete -p` output is unchanged by v88 and not byte-diffed.** huck
  reconstructs actions in its existing `complete -A name -- cmd` style. bash
  instead prints the *short* flag where one exists (`complete -u myc`, and it even
  normalizes a registered `-A user` back to `-u`) and omits the `--`. That
  `complete -p`-format mismatch is a **pre-existing M-36 divergence** (already true
  for the 8 current actions, e.g. huck `complete -A function -- f` vs bash
  `complete -A function f`); v88 neither widens nor fixes it, and no harness
  fragment compares `complete -p` output. (`Action::name` is used only for huck's
  own internal reconstruction, not for bash parity.)
