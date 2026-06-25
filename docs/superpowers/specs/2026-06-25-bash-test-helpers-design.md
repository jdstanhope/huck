# v217 — bash test-suite helper binaries (recho/zecho/printenv)

## Status

Design approved 2026-06-25 (direction validated empirically before writing).

## Background

The v214 bash test-suite harness (`tests/bash-test-suite/runner.sh`) runs
upstream bash 5.2.21's own `.tests` files through huck and diffs the output
against bash's committed `.right` reference files. Of the 73 FAIL categories
in the current baseline, ~21 fail for a reason that has **nothing to do with
huck**: the `.tests` invoke three helper programs — `recho`, `zecho`,
`printenv` — that bash's own build compiles from `support/*.c`, but which are
absent when the harness runs. huck (correctly) reports `command not found:
recho`, producing a massive diff against a `.right` file full of `recho`
output.

These are **not bash builtins**. They are standalone C helpers in the bash
source tree:

- `support/recho.c` — "really echo": prints each argument bracketed as
  `argv[N] = <arg>` with invisible characters made visible (e.g. tab → `^I`).
  Used to verify *exact* word-splitting / quoting / IFS results.
- `support/zecho.c` — a bare-bones `echo`.
- `support/printenv.c` — prints a named environment variable.

### Verified impact

Compiling the three helpers from `$BASH_SOURCE_DIR/support/*.c` and putting
them on `PATH` for the category runs was tested directly. The
`command not found: recho/zecho/printenv` wall disappeared from **every**
affected category, and:

| Category | diff lines after helpers | Category | diff lines |
|---|---|---|---|
| dollars | **0 → PASS** | nquote | 43 |
| nquote5 | **0 → PASS** | comsub | 89 |
| iquote | 6 | more-exp | 132 |
| nquote1 | 6 | exp-tests | 165 |
| nquote3 | 17 | glob-test | 247 |
| array2 | 22 | braces | 34 |
| nquote2 | 22 | nquote4 | 38 |

Two categories flip straight to PASS; several collapse to small *real*
divergences (now measurable for the first time). The rest no longer hide
behind command-not-found, so the baseline can finally describe what actually
differs.

## Goals

- The harness builds `recho`, `zecho`, `printenv` from
  `$BASH_SOURCE_DIR/support/*.c` during preflight and makes them available on
  `PATH` for every category run.
- An override env var (`HUCK_BASH_TEST_HELPERS`) lets an operator point at a
  pre-built helper directory instead of compiling.
- Graceful degradation: if no C compiler is available or compilation fails,
  the harness warns and continues (categories that need the helpers stay FAIL,
  exactly as today — the runner never aborts over this).
- The baseline doc (`docs/bash-test-suite-baseline.md`) is re-triaged so the
  ~21 previously helper-blocked categories show their true status (PASS, or a
  Note describing the *real* remaining divergence rather than "recho not
  found").

## Non-goals / Out of scope

- **Fixing the real divergences** the unblock reveals (the 6–247-line diffs in
  nquote/comsub/exp-tests/etc.). Those become future targeted iterations,
  prioritized using the now-accurate baseline. This iteration is harness
  infrastructure only — it changes **no huck source code**.
- A full bash build. The harness diffs against the committed `.right` files
  (the 5.2.21 reference is already in the source tree); it only needs the
  three helpers *at runtime*. No bash binary is required.
- `xcase` (named in `support/Makefile.in` alongside the others) — its source
  is not present in 5.2.21's `support/`, and no in-scope category references
  it. Skipped.

## Design

All changes are in `tests/bash-test-suite/runner.sh` plus the README and the
baseline doc. No crate code changes.

### 1. Helper provisioning (new preflight section)

Insert after the huck build (after the `HUCK` binary check, ~line 53) and
before the scratch-dir setup. Pseudocode:

```sh
# ---- Provision test helpers (recho / zecho / printenv) -------------
# bash's .tests invoke these standalone helper programs (NOT builtins);
# bash builds them from support/*.c. We compile them from the
# operator-supplied $BASH_SOURCE_DIR (nothing vendored; GPL posture) into
# an ephemeral dir and add it to PATH for the category runs. An operator
# may instead point HUCK_BASH_TEST_HELPERS at a pre-built dir.
HELPER_DIR=""
HELPERS="recho zecho printenv"

if [ -n "${HUCK_BASH_TEST_HELPERS:-}" ]; then
    # Override: use a pre-built directory if it has all three executables.
    # (subshell so the `exit 1` only aborts the check, not the runner)
    if ( for h in $HELPERS; do [ -x "$HUCK_BASH_TEST_HELPERS/$h" ] || exit 1; done ); then
        HELPER_DIR="$HUCK_BASH_TEST_HELPERS"
    else
        echo "warning: HUCK_BASH_TEST_HELPERS=$HUCK_BASH_TEST_HELPERS is missing one of: $HELPERS; falling back to compiling from source." >&2
    fi
fi

if [ -z "$HELPER_DIR" ]; then
    CC="${CC:-cc}"
    if ! command -v "$CC" >/dev/null 2>&1; then
        echo "warning: no C compiler ('$CC') found; test helpers (recho/zecho/printenv) will be unavailable. Categories that need them will FAIL. Set HUCK_BASH_TEST_HELPERS to a pre-built dir, or install a compiler." >&2
    elif [ ! -f "$BASH_SOURCE_DIR/support/recho.c" ]; then
        echo "warning: $BASH_SOURCE_DIR/support/recho.c not found; cannot build test helpers. Categories that need them will FAIL." >&2
    else
        built_dir=$(mktemp -d -t "huck-bash-helpers.XXXXXX")
        inc="-I$BASH_SOURCE_DIR -I$BASH_SOURCE_DIR/include -I$BASH_SOURCE_DIR/builtins"
        all_ok=1
        for h in $HELPERS; do
            # -include string.h: printenv.c uses strlen without including it,
            # which modern gcc treats as a hard error.
            if ! "$CC" $inc -include string.h -o "$built_dir/$h" "$BASH_SOURCE_DIR/support/$h.c" 2>"$built_dir/$h.log"; then
                echo "warning: failed to compile test helper '$h' (see $built_dir/$h.log); categories needing it will FAIL." >&2
                all_ok=0
            fi
        done
        [ "$all_ok" -eq 1 ] && HELPER_DIR="$built_dir"
        # A partial build still helps: keep whatever compiled.
        [ -z "$HELPER_DIR" ] && [ -n "$(ls -A "$built_dir" 2>/dev/null)" ] && HELPER_DIR="$built_dir"
    fi
fi

if [ -n "$HELPER_DIR" ]; then
    PATH="$HELPER_DIR:$PATH"
    export PATH
fi
```

Notes:
- `CC` defaults to `cc`; honors an operator-set `$CC`.
- Include flags `-I$BASH_SOURCE_DIR -I$BASH_SOURCE_DIR/include
  -I$BASH_SOURCE_DIR/builtins` are required because `recho.c`/`zecho.c`
  `#include "bashansi.h"` (bash source root) and related headers.
- `-include string.h` is required for `printenv.c` (it calls `strlen`
  without including the header; gcc ≥14 makes implicit declarations a hard
  error). It is harmless for `recho`/`zecho`.
- Verified working with `cc (Ubuntu 13.3.0)`; all three compile and run.

### 2. PATH wiring for category runs

Exporting `PATH` once in preflight (above) is sufficient: the per-category
subshell (`runner.sh` lines 122–126) inherits it, so a bare `recho` inside a
`.tests` resolves to the built binary. No change to the subshell invocation is
needed beyond the inherited `PATH`. (The `.tests` invoke the helpers by bare
name, matching how bash's own `make test` resolves them.)

### 3. Lifetime / cleanup

The helper binaries are built into an ephemeral `mktemp -d` directory (or the
operator-provided dir). Like the existing per-run `$SCRATCH`, the runner does
not delete it — it is left in `/tmp` for inspection and reaped by the OS. The
compiled binaries are NOT committed.

### 4. Licensing posture

`recho.c`/`zecho.c`/`printenv.c` are GPL'd bash source. We do **not** vendor
them. They are compiled at runtime from the operator-supplied
`$BASH_SOURCE_DIR` — the same posture the harness already uses for reading the
`.tests` and `.right` files. The resulting binaries live only in an ephemeral
scratch dir. No GPL'd source or binary enters this repository.

### 5. Baseline re-triage

After the change, run the full sweep (with the helpers now built) and update
`docs/bash-test-suite-baseline.md`:
- Move newly-passing categories (at minimum `dollars`, `nquote5`; re-run will
  reveal any others) to PASS, and update the Summary counts.
- For categories that remain FAIL but are now unblocked, replace the
  "recho/zecho not found" Note with a huck-authored description of the *real*
  remaining divergence (do NOT paste verbatim bash `.right`/helper output —
  GPL posture; describe in prose).
- Update the "huck commit" / "Sweep date" header.

### 6. README update

`tests/bash-test-suite/README.md`: document that the runner now compiles the
three helpers from `$BASH_SOURCE_DIR/support/*.c` (requires a C compiler —
already needed to build huck), the `HUCK_BASH_TEST_HELPERS` override, and the
graceful-degradation behavior.

## Testing / Verification

- **Smoke check** (manual, recorded in the implementation): with
  `$BASH_SOURCE_DIR` set, run `HUCK_BASH_TEST_CATEGORY=dollars bash
  tests/bash-test-suite/runner.sh` and confirm `dollars` is now PASS, and that
  `recho`/`zecho`/`printenv` exist in the built helper dir and are on `PATH`.
- **Unblock check**: re-run the ~21 affected categories and confirm zero
  `command not found: recho/zecho/printenv` lines remain in their diffs (the
  verification table above is the expected shape).
- **Override check**: set `HUCK_BASH_TEST_HELPERS` to a pre-built dir and
  confirm the runner uses it (skips compilation).
- **Degrade check**: simulate a missing compiler (`CC=/bin/false` or an
  unset/bogus `$CC`) and confirm the runner warns and still completes the
  sweep (does not abort).
- The existing v214 smoke harness (`tests/scripts/*` for the runner) must
  still pass.

The success criterion is the baseline re-triage: the helper-blocked categories
no longer fail on command-not-found, and the PASS count rises by at least the
two confirmed flips.

## Risks

- **No C compiler / compile failure** — handled by graceful degradation
  (warn + continue; affected categories stay FAIL as today). The override env
  var is the escape hatch.
- **gcc-version implicit-declaration error** (printenv `strlen`) — addressed by
  `-include string.h`.
- **A category that still references a helper we didn't build** — none found in
  the in-scope set; if one surfaces it degrades to the same command-not-found
  FAIL as today (no regression).

## Divergence-doc bookkeeping

No `bash-divergences.md` entry: the helper gap was a harness limitation, not a
huck-vs-bash divergence. The real divergences the unblock reveals are tracked
implicitly by the re-triaged baseline and become future iteration targets.
