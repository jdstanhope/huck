# v230 — umask / ulimit / enable builtins — Design

**Status:** approved (brainstorm 2026-06-26)
**Iteration goal:** Implement the three currently-missing builtins `umask`,
`ulimit`, and `enable` (plus the `times` special builtin that `enable -ps`
must list), so the bash-test categories that use them stop emitting
`command not found` and instead produce bash-correct output/errors.

## Background & measurement (measure-first)

`umask`, `ulimit`, `enable` are entirely unimplemented today — not in
`BUILTIN_NAMES`, no dispatch arm, so a call falls through to PATH lookup and
prints `<src>: line N: <name>: command not found` (rc 127).

The complete residuals were measured (bash 5.2.21, release huck):

- **`builtins`** (FAIL): the three `command not found` clusters PLUS independent
  blockers — `set +p` unsupported, `source4.sub`/`source6.sub`/`source7.sub`
  failures (missing file / cannot source `/dev/null`,`/dev/fd/3`,`/dev/stdin` /
  sourced-function visibility), a `declare` prologue mismatch
  (`huck: declare: FOO: not found` vs `./builtins.tests: line 228: declare: …`),
  and a `kill 4096` signal-spec divergence.
- **`errors`** (FAIL): dominated by the error-**prologue** rollout (~30 lines —
  alias/unalias/readonly/unset/declare/exec/hash/… all `huck:` vs
  `./errors.tests: line N:`), exact usage-string differences, a comsub
  syntax-error rewrite, AND the 7 `umask` `command not found` lines.
- **`redir`** (TIMEOUT): `read -ru3` (read -u FD, unimplemented = L-34) makes
  `redir10.sub`'s FD loop misbehave; `ulimit -n` is incidental.
- **`vredir`** (FAIL): gated by `exec {v}> file` (named-fd-variable redirections),
  not ulimit.
- **`procsub`** (FAIL): needs ulimit PLUS `read -u FD` PLUS a `/dev/fd/3:
  Permission denied` procsub-fd fix.

**Conclusion: NO category flips.** This is a broad-shrink iteration (real
missing features used across builtins/errors/procsub/redir/vredir), same
character as v228/v229 — user-confirmed (scope: all three). ulimit's
redir/procsub payoff is **latent** (those categories stay FAIL/TIMEOUT until
`read -u` and `{var}>` also land); its immediate test value is the `builtins`
setup + the `ulimit -c` round-trip line.

## Part 1 — `umask`

**Forms:**
| invocation | behavior |
|---|---|
| `umask` | print current mask octal, 4-digit zero-padded: `0022` |
| `umask -S` | print symbolic of the ALLOWED perms: `u=rwx,g=rx,o=rx` |
| `umask -p` | print reusable: `umask 0022` |
| `umask -p -S` | print reusable symbolic: `umask -S u=rwx,g=rx,o=rx` |
| `umask <octal>` | set mask from 3–4 octal digits |
| `umask <symbolic>` | set mask from symbolic clauses |

**Reading the current mask without disturbing it:** umask(2) both sets and
returns the previous mask, so read via `let cur = libc::umask(0); libc::umask(cur);`.

**Octal set:** parse digits `0`–`7` only; a digit `8`/`9` (e.g. `umask 09`) →
error `<arg>: octal number out of range`. Accept 1–4 octal digits; apply
`libc::umask(parsed & 0o777)`.

**Symbolic print (`-S`):** for each class `u`,`g`,`o` in that order, emit
`<class>=<perms>` where `<perms>` is the subset of `rwx` (in that order) whose
corresponding mask bit is **clear** (allowed). Classes joined by `,`. Example:
mask `0022` → `u=rwx,g=rx,o=rx`.

**Symbolic set:** comma-separated clauses, each
`<classes><op><perms>` where classes ⊆ `{u,g,o,a}` (`a` = all three; empty
classes ⇒ `a`), op ∈ `{=,+,-}`, perms ⊆ `{r,w,x}` (may be empty). Semantics on
the *mask* (mask bit set = perm denied):
- `=` : within each named class, set the mask to deny exactly the perms NOT
  listed (allowed = listed).
- `+` : clear the mask bits for the listed perms in each named class (allow them).
- `-` : set the mask bits for the listed perms in each named class (deny them).
Compute the entire new mask first; apply `libc::umask(new)` ONLY if every clause
parsed. On any parse error, do not change the mask (errors test asserts the
process mask is unchanged after a bad assignment) and return rc 1.

**Errors** (each diagnostic prefixed with `shell.error_prefix(Some("umask"))`
so file-mode output is `<src>: line N: umask: …`; the usage line is bare,
program-name-only):
| input | message |
|---|---|
| `umask 09` | `` umask: 09: octal number out of range `` |
| symbolic, char expected but not `r`/`w`/`x` (e.g. `u=rwx:…` at `:`, or `g=u` at `u`) | `` umask: `<ch>': invalid symbolic mode character `` |
| symbolic, op expected but not `=`/`+`/`-` (e.g. `u:rwx` at `:`) | `` umask: `<ch>': invalid symbolic mode operator `` |
| unknown option (e.g. `umask -i`) | `` umask: -i: invalid option `` then bare `umask: usage: umask [-p] [-S] [mode]` |

(Note: the `:`-as-character vs `:`-as-operator distinction is positional —
`u=rwx:g=…` fails at `:` while scanning for the next clause's class/perm chars,
yielding "invalid symbolic mode character"; `u:rwx` fails at `:` where an
operator was expected, yielding "invalid symbolic mode operator".)

Usage-error rc = 2; octal/symbolic parse-error rc = 1 (bash).

## Part 2 — `ulimit`

**Resource table** — a static slice of entries `{ letter, rlimit_resource,
multiplier, label }` covering the standard Linux set:

| letter | resource | multiplier | `-a` label |
|---|---|---|---|
| `-c` | RLIMIT_CORE | 1024 | core file size (blocks) |
| `-d` | RLIMIT_DATA | 1024 | data seg size (kbytes) |
| `-e` | RLIMIT_NICE | 1 | scheduling priority |
| `-f` | RLIMIT_FSIZE | 1024 | file size (blocks) |
| `-i` | RLIMIT_SIGPENDING | 1 | pending signals |
| `-l` | RLIMIT_MEMLOCK | 1024 | max locked memory (kbytes) |
| `-m` | RLIMIT_RSS | 1024 | max memory size (kbytes) |
| `-n` | RLIMIT_NOFILE | 1 | open files |
| `-p` | (pipe, special) | 512-block | pipe size (512 bytes) |
| `-q` | RLIMIT_MSGQUEUE | 1024 | POSIX message queues (bytes) |
| `-r` | RLIMIT_RTPRIO | 1 | real-time priority |
| `-s` | RLIMIT_STACK | 1024 | stack size (kbytes) |
| `-t` | RLIMIT_CPU | 1 | cpu time (seconds) |
| `-u` | RLIMIT_NPROC | 1 | max user processes |
| `-v` | RLIMIT_AS | 1024 | virtual memory (kbytes) |
| `-x` | RLIMIT_LOCKS | 1 | file locks |

`-p` (pipe buffer) is not a POSIX rlimit; match bash by reporting a fixed `8`
(512-byte blocks) and treating a set as a no-op success. (Each table entry's
exact multiplier/label follows bash 5.2.21's `ulimit` man page; the plan pins
the constants. `-a` labels are implemented faithfully but are NOT byte-pinned by
a harness — they are environment-specific and unused by the gating categories.)

**Flags & semantics:**
- `-S` / `-H` select the soft / hard limit. With **neither**: a SET writes both
  soft and hard; a QUERY reports the soft limit.
- `--` ends option parsing.
- A bare letter with no value ⇒ query that resource. Multiple resource letters
  are accepted (e.g. `-c -S`).
- A value of `unlimited` ⇔ `RLIM_INFINITY`; query prints `unlimited` for an
  infinite limit, else the scaled integer.
- `getrlimit`/`setrlimit` are real syscalls — `ulimit -n 128` genuinely
  constrains the process (this is the latent redir/procsub value).
- `ulimit -a` (or no resource letter, no value) prints the whole table:
  `<label>\t\t(<unit-hint>, -<letter>) <value>` in bash's column format.
- Default resource when none given: bash uses `-f`. `ulimit` with no args ⇒
  query `-f`.

**Test-pinned behavior:** `ulimit -S -c 0` (no output), `ulimit -c -S -- 1000`
(set soft core to 1000 blocks), `ulimit -c` ⇒ prints `1000`; `ulimit -n <v>`
set then `ulimit -n` query ⇒ `<v>`.

**Errors** (prefixed `error_prefix(Some("ulimit"))`):
unknown option ⇒ `ulimit: -<o>: invalid option` + bare usage
`ulimit: usage: ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]`; non-numeric value ⇒
`ulimit: <val>: invalid number`; a hard-limit raise that the kernel rejects ⇒
`ulimit: <val>: cannot modify limit: <strerror>` (via `crate::bash_io_error`).

## Part 3 — `enable` (and the `times` special builtin)

### `times` builtin (new)

Prints two lines — shell self then children — each `<m>m<s>.<ms>s <m>m<s>.<ms>s`
(bash format `%dm%.3fs %dm%.3fs`), computed from `libc::times()` divided by
`sysconf(_SC_CLK_TCK)` (line 1 = `tms_utime`/`tms_stime`, line 2 =
`tms_cutime`/`tms_cstime`). Added to `BUILTIN_NAMES`, `run_builtin` dispatch, and
`is_special_builtin` (so `enable -s` lists it in bash's sorted position between
`source` and `trap`). `times` is a POSIX special builtin huck was simply
missing; its output is not byte-pinned by any gating category (only its name
appears, in `enable -ps`), so the format is faithful but not harness-verified.

### Disabled-builtin state

Add `disabled_builtins: std::collections::BTreeSet<String>` to the shell
(alongside the other per-shell maps). Introduce the split:
- `is_builtin(name)` — unchanged: is `name` a KNOWN builtin name (in
  `BUILTIN_NAMES`)? Used by `enable`'s validity check, the `builtin` forcing
  builtin, the special-builtin/function-name rules — anywhere the question is
  "does this name denote a builtin at all".
- `builtin_active(name, shell)` = `is_builtin(name) && !shell.disabled_builtins
  .contains(name)` — is the builtin currently RUNNABLE as a builtin? Used by:
  - the **executor** command-dispatch site that decides builtin-vs-external (a
    disabled builtin falls through to PATH lookup, matching bash — `enable -n
    test` then `test` runs the external `test`);
  - **`type` / `command -v`** (`resolve_command_name_with`): a disabled builtin
    is NOT reported as `Builtin` (it resolves to the external file or NotFound),
    so `enable -n test; type -t test` is not `builtin`.

The `builtin NAME …` forcing builtin keeps using `is_builtin` (runs even a
disabled builtin — matches bash).

### `enable` command

Flags: `-a` (act on/list all builtins incl. disabled), `-n` (disable the named
builtins; when listing, restrict to the disabled set), `-p` (print in reusable
format — the default when listing), `-s` (restrict to POSIX *special* builtins).
Unsupported bash flags `-d`/`-f`/filename (dynamic loading) ⇒
`enable: -<f>: invalid option` + usage (loadable builtins are out of scope, see
known-skips `loadable`).

**Listing (no name args):**
- candidate set = special builtins if `-s` else all `BUILTIN_NAMES`;
- if `-n`: print only the DISABLED members; else if `-a`: print all members
  (each as `enable NAME`, or `enable -n NAME` if disabled); else print only the
  ENABLED members;
- sorted ascending by name; each line `enable NAME` (enabled) or
  `enable -n NAME` (disabled). rc 0.

This reproduces `builtins.right`: `enable -ps` → the sorted special list once;
`enable -aps` → same (none disabled) → the list again; `enable -nps` → disabled
specials → empty.

**Toggling (name args):** for each name — if not `is_builtin(name)` ⇒
`enable: NAME: not a shell builtin` (prefixed `error_prefix(Some("enable"))`),
and the overall rc becomes 1; else if `-n` add to `disabled_builtins`, else
remove from it. `errors` test: `enable sh bash` ⇒ two `not a shell builtin`
lines, rc 1.

## Part 4 — Wiring, prologue, testing

**Wiring:** add `umask`, `ulimit`, `enable`, `times` to `BUILTIN_NAMES`; add four
`run_builtin` dispatch arms; add `times` to `is_special_builtin`; add
`disabled_builtins` to the shell state and `builtin_active` + route the executor
dispatch and `resolve_command_name_with` through it.

**Prologue:** the new diagnostic sites use `shell.error_prefix(Some("umask" |
"ulimit" | "enable"))`, extending the v229 staged-prologue list. Usage lines are
bare (`<name>: usage: …`), matching the v227 getopts/declare split.

**Out of scope (independent blockers — record as deferred follow-ons, do NOT
attempt):** `set +p`; source4/6/7.sub (missing-file / device-file /
sourced-function); the `declare` prologue line + `declare -x` output in
builtins; `kill 4096` signal-spec; the full errors-category prologue rollout +
comsub syntax-error rewrite; `read -u FD` (L-34); `{var}>` fd-variable
redirections (vredir); procsub `/dev/fd` permission. No category is expected to
flip.

**Testing:**
- **Unit:** umask symbolic↔octal round-trip + each error class (char/operator/
  octal-range/unchanged-on-error); ulimit resource lookup + multiplier scaling +
  soft/hard selection; enable listing (enabled/disabled/special/all) + toggle +
  `builtin_active` semantics.
- **Integration (file mode):** one test file per builtin exercising the
  category-relevant forms (`run_file` helper with an `AtomicU64` unique-temp
  counter, per the v228/v229 pattern).
- **Diff-check harnesses** (byte-identical bash↔huck, file mode, same temp path):
  - `umask_diff_check.sh` — octal/`-S`/`-p`/`-p -S` round-trips (save+restore the
    real mask so the harness is order-independent) + the four error forms.
  - `enable_diff_check.sh` — `enable -ps`/`-aps`/`-nps` listing + `enable -n
    test` / `enable test` toggle (verified via `type -t test`) + `enable sh bash`
    error.
  - `ulimit_diff_check.sh` — environment-INDEPENDENT round-trips only
    (`ulimit -c -S 1000; ulimit -c` ⇒ `1000`; a soft `-n` set+query in a
    subshell) + the invalid-option / invalid-number error forms. NOT `-a`
    absolute values.
- **Regression:** `cargo test --workspace` (0 failed) + all
  `tests/scripts/*_diff_check.sh` (`funcnest_diff_check.sh` release-only).
- **Category re-measure:** builtins / errors / procsub — the three `command not
  found` clusters (and umask's 7 errors lines, and the `ulimit -c` → `1000`
  line) must drop from the diffs; no category may regress to TIMEOUT/ERROR; no
  flip expected.

## Risks

- **umask symbolic parser** is the intricate piece — the
  character-vs-operator error distinction is positional and must match bash
  byte-for-byte; the `umask_diff_check.sh` error cases pin it. The "don't change
  the mask on a bad assignment" rule requires computing the full new mask before
  any `umask()` call.
- **ulimit multipliers/labels** are fiddly; the plan pins each constant from
  bash's man page. Only the env-independent round-trips are harness-verified, so
  a wrong `-a` label would slip the harness — mitigated by copying bash's table
  verbatim and a manual `ulimit -a` spot-check against bash in the plan.
- **`builtin_active` routing** must cover every dispatch/type site without
  breaking the `builtin` forcing builtin or the special-builtin/function-name
  rules — the plan enumerates each `is_builtin` call site and states whether it
  stays `is_builtin` or becomes `builtin_active`.
- **`disabled_builtins` placement** — adding a field to the shell state struct;
  ensure it is initialized in every constructor and (if the shell snapshots/
  restores state for subshells) carried correctly. The plan names the struct and
  every constructor.
