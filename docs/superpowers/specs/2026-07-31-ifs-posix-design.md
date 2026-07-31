# v350 — flip `ifs-posix` design

**Issue:** [#2](https://github.com/jdstanhope/huck/issues/2) — `read` with a
non-whitespace IFS keeps a trailing delimiter in the last variable.

## Goal

Flip the bash 5.2.21 test-suite **`ifs-posix`** category to PASS (byte-identical
to `ifs-posix.right`) by fixing the single root that accounts for all of its
failures: `read name1 name2 …` with a mixed-class `IFS` (whitespace +
non-whitespace, e.g. `IFS=": "`) keeps a trailing non-whitespace IFS delimiter
in the **last** variable, where bash strips it.

`ifs-posix.tests` runs **6856** sub-tests; huck fails exactly **528**, and all
528 are this one root (the test file's trailer prints `# tests 6856 passed 6328
failed 528` vs bash's `passed 6856 failed 0`). Fixing the root flips the whole
category.

## Symptom

```
echo "::"    | ( IFS=": " read x y; echo "($x)($y)" )   # bash ()()   huck ()(:)
echo ":a:"   | ( IFS=": " read x y; echo "($x)($y)" )   # bash ()(a)  huck ()(a:)
echo "a: b:" | ( IFS=": " read x y; echo "($x)($y)" )   # bash (a)(b) huck (a)(b:)
printf ':a:b:\n' | { IFS=: read x y z; echo "[$x][$y][$z]"; }  # bash [][a][b]  huck [][a][b:]
```

huck's `split_into_names()` (`crates/huck-engine/src/builtins.rs:2904`) assigns
the last variable "the rest of the line with trailing IFS-**whitespace**
stripped", keeping a trailing non-whitespace delimiter. bash strips a trailing
delimiter under a subtler rule.

## Why a heuristic is wrong (history)

A prior attempt (**B-03**) shipped a "strip the sole trailing delimiter"
heuristic and was **reverted in v276**: across the `ifs-posix` multi-char-IFS
matrix, every simple heuristic fixed some rows and regressed others. The
category's own comment in `read_tests.rs` and issue #2 both defer this to "an
iteration that ports bash's `read.def` last-field splitter faithfully." This is
that iteration.

## bash's exact algorithm (`read.def`, bash 5.2.21, lines ~1009–1037)

After the first `n-1` variables have each consumed a field (leading IFS
whitespace skipped once up front; each field extracted as one word followed by
its separator run), the **last** variable is assigned as follows. Let `rest` be
the unconsumed remainder of the line:

1. If `rest` is empty → last var = `""`.
2. Otherwise extract **one word** `W` from `rest` using the same
   word+separator scan used per field (`get_word_from_string`), which skips
   leading IFS whitespace, accumulates non-IFS bytes, then consumes the
   following separator run and leaves a pointer `p` at what remains:
   - If `p` is at end-of-input (the word consumed the rest) → last var = `W`
     (the trailing delimiter that `W`'s scan consumed is **dropped**).
   - Else (bytes remain after `W`) → last var = the **raw** `rest` with only
     trailing IFS-**whitespace** stripped (interior and multiple trailing
     non-whitespace delimiters are **kept**).

### Worked cases (all verified against real bash 5.2.21)

| IFS   | input    | rest for last var | word W | p at end? | last var |
|-------|----------|-------------------|--------|-----------|----------|
| `:`   | `a:b:`   | `b:`              | `b`    | yes       | `b`      |
| `:`   | `a:b::`  | `b::`             | `b`    | no (`:`)  | `b::`    |
| `:`   | `a:b:c:` | `b:c:`            | `b`    | no (`c:`) | `b:c:`   |
| `:`   | `a::`    | `:`               | `` (empty) | yes   | ``       |
| `:`   | `a:::`   | `::`             | ``     | no (`:`)  | `::`     |
| `: `  | `a:b: `  | `b: `            | `b`    | yes       | `b`      |
| `: `  | `a: :b`  | `:b`             | ``     | no (`b`)  | `:b`     |
| `:`   | `a: :b`  | ` :b`           | ` `    | no (`b`)  | ` :b`    |
| `:a:b:`(x y z) | — | z-rest `b:`     | `b`    | yes       | `b`      |

The distinguishing insight B-03 missed: strip the trailing delimiter **only
when extracting one word exhausts the remainder** — not whenever a trailing
delimiter is present.

## Change

Localized to `split_into_names()`'s final "last field" block only.

- Factor the existing per-field "extract one word + consume its separator run"
  logic (already present in `split_into_names`'s loop and mirrored in
  `split_read_fields`) into a small helper `next_word(bytes, pos, ifs classes)
  -> (word: String, next_pos: usize)` so the last-field path provably uses the
  **same** scan as the per-field path (this is what makes it faithful rather
  than a heuristic).
- Replace the last-field body with the algorithm above:
  - `rest_start = i` (current position after the n-1 loop).
  - if `i >= len` → last = `""`.
  - else `(w, p) = next_word(bytes, i)`; if `p >= len` → last = `w`; else last
    = raw `bytes[i..]` with trailing IFS-whitespace stripped.
- The empty-IFS branch is unchanged (assign the whole line to the first name).
  The `n == 1` single-name branch is **removed** so a single variable flows
  through the same last-field block: bash runs its `read.def` last-field rule
  with zero preceding fields, so `IFS=: read x` on `a:` yields `a` (sole
  trailing non-ws delimiter dropped), while a multi-word remainder (`a:b:`) or a
  double trailing delimiter (`a::`) is kept, and default-whitespace IFS still
  strips leading+trailing whitespace (`  a  b  ` → `a  b`). (An earlier draft of
  this spec wrongly called the old strip-only-whitespace single-name branch
  "already correct" — it kept a trailing non-ws delimiter; corrected here.)

### Blast radius

Bounded to `read name1 name2 …`. `read -a` / `mapfile` element-splitting use the
separate `split_read_fields()` (`builtins.rs:3640`), which already drops a
trailing delimiter correctly and is **not** touched. The only other consumer of
`split_into_names` is the named-variable `read` path (`builtins.rs:3660`).

## Tests

- **Update** `split_last_field_strips_only_ws_ifs` in `read_tests.rs` — it
  currently *asserts the divergent old behavior* (`:a:b:` → `["","a","b:"]`,
  `a:b::` → `["a","b::"]`). Re-point every case to the bash-correct value
  (`:a:b:` → `["","a","b"]`; `a:b::` → `["a","b::"]` stays; add the strip cases).
  Rename to reflect the new faithful behavior.
- **Add** unit cases covering the strip/keep matrix: single trailing delim
  (strip), double trailing delim (keep), trailing delim after interior delim
  (keep), empty last field, leading-ws-in-last-field, and the mixed `IFS=": "`
  rows.
- **Add** `tests/scripts/ifs_posix_diff_check.sh` — a byte-identical bash↔huck
  harness over a representative slice of the `read x y`/`read x y z` matrix
  (single/double/interior trailing delims, mixed IFS, leading ws), plus KEEP
  guards for `read -a` and default-whitespace-IFS `read`.

## Verification

- Official `ifs-posix` runner → PASS, and `diff ifs-posix.right <huck>` == 0.
- Full bash-suite runner: PASS-set == v349 baseline (37) **+ `ifs-posix`** = 38,
  **zero regressions** (spot-check `ifs`, `read`, `nquote*`, field-splitting
  categories — though only PASS categories can regress the count).
- `tests/scripts/run_diff_checks.sh` green (incl. the new harness); engine lib
  green; `read` integration bins green.

## Out of scope

The `read` category's other divergences (`/dev/tty`, `read -e`/`-a` option
handling, `-N`/timeout edge cases) are unrelated and remain FAIL. This
iteration fixes only the last-field IFS root (#2).
