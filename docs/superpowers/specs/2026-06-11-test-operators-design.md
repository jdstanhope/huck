# huck v135 — finish `test`/`[[` operators (M-27 + M-14b; M-26 already done) Design

**Status:** approved design, ready for implementation plan.
**Implements:** the remaining `test`/`[`/`[[ ]]` unary operators — M-27 (the file-
type/mode/ownership/terminal tests `-p -S -b -c -O -G -N -k -u -g -t`) and M-14b
(`[[ -v arr[i] ]]` / `test -v 'arr[i]'` array-element form). M-26 (`test -v VAR`)
is ALREADY implemented — its divergence entry is stale and is removed.
**Branch (impl):** `v135-test-operators`.

## Background — current state (verified)

huck has two test engines that already SHARE one file-test implementation:
- `src/test_builtin.rs` — `evaluate_with(args, var_is_set)` drives the
  `test`/`[`/`]` builtin (`builtin_test`, builtins.rs:6200). `apply_unary`
  (test_builtin.rs:~134) holds the unary operators.
- `[[ ]]` — `TestUnaryOp` (command.rs:389) is parsed (via `try_unary_op`,
  command.rs:1995) and evaluated in `eval_unary` (executor.rs:1395), which
  DELEGATES the file ops to `test_builtin::evaluate(&["-X", s])`.

So adding an operator to `test_builtin` makes it available to BOTH engines.

Verified:
- **M-26**: `test -v x` → set, `test -v NOPE` → unset; `[[ -v x ]]` works. ALREADY
  DONE (test_builtin.rs:138 `var_is_set(operand)`). Stale entry → delete + add a
  regression test.
- **M-14b**: `[[ -v arr[1] ]]` → huck false (treats `arr[1]` as a plain name),
  bash true. bash ALSO does it for the builtin (`[ -v "arr[1]" ]`). Both fixed.
- **M-27**: `-p -S -b -c -O -G -N -k -u -g -t` absent. In `[[ ]]` they're a PARSE
  ERROR (`try_unary_op` returns None → the lexer/parser mis-handles them).

## Architecture — extend the shared engine + the `[[ ]]` enum

### Component 1 — M-27 operators in `src/test_builtin.rs` (`apply_unary`)
Add the 11 operators. File operators use `std::fs::metadata(operand)` (follows
symlinks, matching `-f`/`-d`) and `std::os::unix::fs::MetadataExt`; a missing file
→ `false` (like the existing file ops):
```rust
use std::os::unix::fs::MetadataExt;
// helper:
fn mode_bits(p: &str) -> Option<u32> { std::fs::metadata(p).ok().map(|m| m.mode()) }
// arms (Ok(...)):
"-p" => Ok(mode_bits(operand).map_or(false, |m| m & libc::S_IFMT as u32 == libc::S_IFIFO as u32)),
"-S" => Ok(mode_bits(operand).map_or(false, |m| m & libc::S_IFMT as u32 == libc::S_IFSOCK as u32)),
"-b" => Ok(mode_bits(operand).map_or(false, |m| m & libc::S_IFMT as u32 == libc::S_IFBLK as u32)),
"-c" => Ok(mode_bits(operand).map_or(false, |m| m & libc::S_IFMT as u32 == libc::S_IFCHR as u32)),
"-k" => Ok(mode_bits(operand).map_or(false, |m| m & libc::S_ISVTX as u32 != 0)),
"-u" => Ok(mode_bits(operand).map_or(false, |m| m & libc::S_ISUID as u32 != 0)),
"-g" => Ok(mode_bits(operand).map_or(false, |m| m & libc::S_ISGID as u32 != 0)),
"-O" => Ok(std::fs::metadata(operand).map_or(false, |m| m.uid() == unsafe { libc::geteuid() })),
"-G" => Ok(std::fs::metadata(operand).map_or(false, |m| m.gid() == unsafe { libc::getegid() })),
"-N" => Ok(std::fs::metadata(operand).map_or(false, |m| m.mtime() > m.atime())),
"-t" => Ok(operand.parse::<i32>().map_or(false, |fd| unsafe { libc::isatty(fd) } == 1)),
```
(Exact casts/forms are the implementer's to make compile cleanly; the SEMANTICS
above are the contract. `S_IFMT`/`S_IFIFO`/… are `libc` constants; their types
vary by platform — cast consistently.)

Also extend `is_unary_op` (test_builtin.rs:~92, the `match` listing
`-a -e -f -d …`) to include the 11 new flags so the parser/short-form recognizes
them as unary operators.

### Component 2 — `src/command.rs` (`TestUnaryOp` + `try_unary_op`)
Add 11 variants to `enum TestUnaryOp` (e.g. `IsFifo, IsSocket, IsBlockDev,
IsCharDev, IsSticky, IsSetuid, IsSetgid, OwnedByEuid, OwnedByEgid, NewerThanRead,
IsTerminal`) and their `try_unary_op` arms (`"-p" => Some(TestUnaryOp::IsFifo)`,
etc.). This is what makes `[[ -p f ]]` PARSE.

### Component 3 — `src/executor.rs` (`eval_unary`)
Route the 11 new variants to `test_builtin::evaluate(&["-X".to_string(),
s.to_string()])` — the existing delegation pattern (one impl). `-t` included
(operand is the fd string).

### Component 4 — M-14b subscript-aware `-v`
A `Shell` method:
```rust
/// `-v` target: a bare name, a positional/special param, OR an array element
/// `name[subscript]`. For the element form, evaluate the subscript (arithmetic
/// for indexed arrays; literal key for associative) and report whether THAT
/// element is set. Else fall back to `is_set`.
pub fn element_or_var_is_set(&self, target: &str) -> bool { … }
```
Parse `name[sub]` with the existing `split_name_subscript` helper (used by
`printf -v`, v113). For an indexed array, arith-evaluate `sub`; for associative,
use `sub` as the key (after the array's known kind). Check element presence. Wire
it in:
- `builtin_test`'s predicate closure: `&|n| shell.element_or_var_is_set(n)`
  (replacing `&|n| shell.is_set(n)`).
- `[[ ]]` `VarSet` evaluation (executor.rs:~1336): use
  `shell.element_or_var_is_set(operand)` instead of `is_set`.
The no-subscript form returns `is_set(name)` (unchanged behavior).

## Scope & must-not-regress
- **M-26 unchanged behavior** (`test -v`/`[[ -v ]]` for plain names) — locked by a
  regression test; only the array-element form is NEW.
- **Existing file tests** (`-e -f -d -r -w -x -s -L`) untouched; the new ops are
  additive arms.
- **Symlink follow**: the new file ops use `metadata` (follow), matching huck's
  existing `-f`/`-d`/`-e` and bash (bash's `-p/-S/-b/-c/-k/-u/-g/-O/-G` stat,
  following symlinks).
- **`[[ ]]` parse**: adding the operators to `try_unary_op` must not change how any
  EXISTING token parses (the new flags were previously unrecognized → errors).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/test_builtin.rs` | Add the 11 M-27 operator arms to `apply_unary`; extend `is_unary_op`. |
| `src/command.rs` | Add 11 `TestUnaryOp` variants + `try_unary_op` arms. |
| `src/executor.rs` | `eval_unary`: delegate the 11 new variants to `test_builtin`; `VarSet` uses `element_or_var_is_set`. |
| `src/shell_state.rs` | `Shell::element_or_var_is_set` (M-14b). |
| `src/builtins.rs` | `builtin_test` predicate → `element_or_var_is_set`. |
| `tests/test_operators_integration.rs` (NEW) | Per-operator + `-v` array-element + M-26 regression. |
| `tests/scripts/test_operators_diff_check.sh` (NEW) | Bash-diff harness over real artifacts. |
| `docs/bash-divergences.md` | DELETE M-26, M-27, M-14b (Tier-2 24→21). |

## Testing

1. **Bash-diff harness** `tests/scripts/test_operators_diff_check.sh` (gold
   standard — runs each fragment through bash AND huck, asserts byte-identical),
   building real artifacts in a `mktemp -d`:
   - `-c`: `[ -c /dev/null ]` (true), `[ -c regfile ]` (false)
   - `-p`: `mkfifo $D/f; [ -p $D/f ]` (true), `[ -p regfile ]` (false)
   - `-S`: bind a unix socket (`python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1])' $D/s` if python is present; else SKIP this row with a note) → `[ -S $D/s ]`
   - `-k`: `chmod +t $D/dir; [ -k $D/dir ]` (true), false on a plain file
   - `-u`/`-g`: `chmod u+s,g+s $D/x; [ -u $D/x ]`/`[ -g $D/x ]`
   - `-O`: `[ -O $D/x ]` (I own it → true); `[ -O /etc/shadow ]` → matches bash (false unless root)
   - `-G`: `[ -G $D/x ]` (matches bash)
   - `-b`: `[ -b /dev/null ]` (false — char not block); if a block dev is readable, test true, else just the false case
   - `-N`: `[ -N $D/x ]` on a fresh file → matches bash (impl-defined w/ relatime; assert PARITY, not a fixed value)
   - `-t`: `[ -t 0 ] </dev/null` (false), `[ -t 99 ]` (false, bad fd)
   - Both `[ -X … ]` and `[[ -X … ]]` forms for a representative subset.
   - `-v` array: `arr=(a b); [[ -v arr[1] ]]`, `[[ -v arr[9] ]]`, `declare -A m; m[k]=1; [[ -v m[k] ]]`, `[[ -v m[nope] ]]`, and the builtin `[ -v 'arr[1]' ]`.
   Each fragment is compared to bash; `[ -O /etc/shadow ]`/`-N` assert PARITY with
   bash's live result, not a hard-coded boolean.
2. **Integration `#[test]`s** (`tests/test_operators_integration.rs`): exact
   results for the `-v` array-element forms (indexed set/unset, associative
   set/unset, no-subscript regression), `test -v`/`[[ -v ]]` plain-name regression
   (M-26), `-c /dev/null` true, `-p` on an `mkfifo`'d path, `-t 0` false. `-t`
   true via a PTY test (or skip-if-no-tty).
3. **Full regression:** entire suite + all harnesses green; clippy clean. The
   existing `test`/`[[ ]]` tests must stay green (new ops are additive).

## Edge cases & notes
- **`-t` operand**: must be a non-negative integer fd; non-numeric → false (bash
  same). bash's bare `[ -t ]` (no operand) defaults to fd 1 in the SHORT-form
  `test`; that no-operand default is an edge — match it only if trivial, else the
  operand-required form (the common `-t 1`/`-t 0`) is the target.
- **`-N` flakiness**: depends on the filesystem's atime policy (`relatime` is
  common); the harness asserts huck == bash on the SAME file, so both shells see
  the same atime/mtime → parity holds regardless of the policy.
- **`-O`/`-G` as root**: tests run as a normal user; if CI runs as root the `-O`
  on root-owned files flips — the harness compares to bash, so parity holds.
- **Associative `-v m[k]`**: requires knowing the array is associative to treat
  `k` as a literal key (not arith). Reuse the array machinery from M-82/v113; if
  the array is indexed, arith-evaluate the subscript.
- **`-v` with a non-existent base var + subscript** (`[[ -v nope[0] ]]`) → false
  (no array).
