# v359 — one option parser for the builtins — design

**Issue:** [#496](https://github.com/jdstanhope/huck/issues/496) — *Builtins:
bundled short options rejected (`readonly -pa`, `wait -fn`, `history -cd`,
`unset -vf`).*

**Follows:** v358 ([#198](https://github.com/jdstanhope/huck/issues/198)), whose
`error_fatality` classifier this reuses rather than re-derives — a builtin usage
error is already a kind it knows (`SpecialBuiltinUsage { status }`), so posix
fatality comes for free.

**Found by:** the technical-debt review that produced #497/#498/#499. #496 was
the one finding with user-visible impact, and the only one sized as an iteration.

## Problem

Every builtin parses its own options. `builtins.rs` has **41 `invalid option`
emit sites across ~23 builtins**, written in two incompatible styles:

- a **bundled-character loop** (`for c in s[1..].chars()`) — `export`,
  `declare`, `local`, `jobs`, `type`, …
- a **whole-string match** (`"-p" => …, "-a" => …`) — `readonly`, `wait`,
  `history`, `unset`, …

The second style cannot parse a bundled flag. Nobody decided that `readonly`
should behave differently from `export`; it happened because the same small job
was solved twenty-three times.

```
# bash 5.2.21                    # huck
$ readonly -pa   -> ok           $ readonly -pa   -> readonly: -pa: invalid option  (rc 2)
$ history -cd 1  -> ok           $ history -cd 1  -> history: -cd: invalid option   (rc 2)
$ wait -fn       -> ok           $ wait -fn       -> wait: -fn: invalid option      (rc 2)
$ unset -vf x    -> "cannot simultaneously unset a function and a variable"
                                 $ unset -vf x    -> unset: -vf: invalid option     (rc 2)
```

`wait` rejects the exact bundling its own usage string documents
(`wait [-fn] [-p var] [id ...]`).

### Measured: the contract is smaller than it looks

Probed against bash 5.2.21. **huck already matches on most of it** — the
speculation that the subtle rows would also be divergent was wrong, and measuring
first is what kept them out of scope:

| Behaviour | bash | huck today |
|---|---|---|
| bundled shorts, order-independent (`-pa` ≡ `-ap`) | yes | **only the char-loop builtins** |
| `--` terminates options | yes | matches |
| lone `-` is an OPERAND, not an option | yes | matches |
| value attached or separate (`-n3` ≡ `-n 3`) | yes | matches |
| scanning STOPS at the first non-option (no permutation) | yes | matches |
| unknown option → `<name>: -X: invalid option` + `<name>: usage: …`, rc 2 | yes | **usage line missing for ~half** |
| that usage error is FATAL under `--posix`, not otherwise | yes | matches (v358) |

So the live bug surface is two rows: **bundling**, and **the missing usage
line**.

### Measured: the divergence inventory

`<builtin> -Q` through both shells, explicit `$0` so prefixes match. **29 of 48
differ, 19 already match.** The 29 are six distinct classes, not one bug:

| # | Class | Builtins | n |
|---|---|---|---|
| 1 | Usage line missing entirely | unset, readonly, read, type, hash, declare, printf, command, mapfile, help, complete, compgen, compopt | 13 |
| 2 | Usage line present, wrong shape/text | jobs, trap | 2 |
| 3 | Wrong builtin NAME in the message | readarray→"mapfile", typeset→"declare" | 2 |
| 4 | `-Q` treated as an operand, not an option | alias, unalias, builtin | 3 |
| 5 | Check ORDERING differs (state reported before parsing) | fg, bg, bind | 3 |
| 6 | Not a getopt path at all (`+N`, numeric arg, no options) | pushd, popd, dirs, return, times, caller | 6 |

**Classes 1–4 are in scope** (20 names, 18 distinct implementations —
`readarray`/`mapfile` and `typeset`/`declare` share code). Classes 5 and 6 are
not: class 5 is about *when* a state check runs relative to parsing, which lives
in each builtin's prologue; class 6 genuinely is not getopt, and forcing `+N`
parsing through a getopt contract would make those wrong in a new way.

## Design

### 1. One scanner owns the contract

New `crates/huck-engine/src/builtin_opts.rs`, modelled on bash's
`internal_getopt` rather than re-derived. It yields options one at a time; each
builtin keeps its own `match` on the option character and loses only its
scanner:

```rust
let mut opts = Getopt::new(args, "aAfp");   // 'd:' marks a value-taking option
while let Some(opt) = opts.next_opt(shell, err)? {  // Err => usage emitted, rc 2
    match opt.ch {
        'a' => want_indexed = true,
        'p' => want_list = true,
        _ => unreachable!("spec and match must agree"),
    }
}
let operands = opts.rest();
```

Keeping the per-builtin `match` is deliberate: it is what makes a twenty-file
diff reviewable, and what lets each builtin's semantics stay where a reader
expects them.

### 2. The usage table is keyed on the INVOKED name

`usage_for(invoked_name) -> &'static str`, one entry per builtin, transcribed
from bash 5.2.21. Keying on the invoked name — not the implementation's name —
is the whole of the class-3 fix: `readarray` and `typeset` currently announce
themselves as `mapfile` and `declare` because the message is built from whichever
function handles them.

The usage text has no other caller, so it belongs beside the parser that emits
it. In bash the same coupling exists: the getopt failure path calls
`builtin_usage()`, which is why the two lines always appear together.

### 3. The error path reuses v358

On an unknown option the scanner emits both lines and calls
`report_error(ErrorKind::SpecialBuiltinUsage { status: 2 })`. That is the
existing classifier: it already knows a special builtin's usage error aborts
under `--posix` and continues outside it. **This iteration derives no fatality
rule of its own** — measured and confirmed identical to bash before adoption.

### 4. Adoption is broader than the bug list

Adoption covers **every builtin that parses options today** — the ~23 carrying an
`invalid option` emit site — whether or not it is currently divergent. Not "the
19 that match": that set also contains builtins which match merely because they
take no options, and giving those a parser would be inventing a surface bash does
not have.

Converting the already-correct ones matters because leaving them hand-rolled
preserves exactly the drift that created this issue — `export` is correct today
only because someone happened to write a character loop, and nothing stops the
next builtin from copying `readonly` instead. Converting a correct builtin is a
no-op the harness proves.

### 5. What is deliberately NOT done

- `set`, `let`, `test`/`[`, `echo` keep their hand-written parsers — bash does
  not use getopt for them either (`echo -neee`, `set -o` follow their own rules).
- Classes 5 and 6 are filed, not fixed (see Follow-ups).
- `builtins.rs` is not split. The file is 10.5k lines and this work touches
  twenty of its functions, so splitting it is tempting — and out of scope. A
  behaviour change and a file move in one diff is a diff nobody can review.

## Verification

**New harness** `tests/scripts/builtin_options_diff_check.sh`. For every
in-scope builtin, a matrix of: invalid option, bundled pair, `--` terminator,
lone `-`, operand-then-option. Both shells are invoked with an **explicit `$0`**
(`bash -c '…' huck5` / `huck -c '…' huck5`) so the program-name prefix is
identical and the comparison is a byte comparison with no normalisation —
verified working during the brainstorm. It uses the shared
`tests/scripts/lib/harness.sh` scaffolding from #498 and keeps its own driver
lines.

**Commit the harness RED first**, with the failing rows visible, so the fix is
demonstrated rather than asserted.

Also required green before the PR: the full 274-harness sweep, both lib suites
(at or above their 483 / 1994 baselines — this adds tests, so the counts rise),
the integration binaries, and clippy under the **pinned** toolchain
(`cargo +1.97.1 clippy`, #497 — a newer local stable misses warnings CI raises).

Run the sweep on an **idle** box. Two of its job-control harnesses fail
non-deterministically under concurrent build load (#476), and a red sweep run
beside a running `cargo` proves nothing.

## Success criteria

1. `readonly -pa`, `wait -fn`, `history -cd 1`, `unset -vf x` match bash.
2. All 20 class-1–4 builtins produce byte-identical invalid-option output,
   including the usage line and the invoked name.
3. Every getopt-using builtin routes through `builtin_opts`; no `invalid option`
   string is emitted from a hand-rolled scanner.
4. The 19 already-matching builtins still match — proven, not assumed.
5. Sweep, lib suites and clippy green; no expected-value edits to existing
   harnesses (this changes error output only where it was already wrong).

## Follow-ups to file

- Class 5: `fg`/`bg`/`bind` report job-control / line-editing state before
  parsing options; huck parses first.
- Class 6: `pushd`/`popd`/`dirs` parse `+N`/`-N` as numbers ("invalid number",
  not "invalid option"); `return`/`caller` take a numeric argument; `times`
  takes none.
- `pushd -Q` misroutes entirely, emitting `cd: -Q: invalid option` with `cd`'s
  usage string — a dispatch bug, not a parsing one.
