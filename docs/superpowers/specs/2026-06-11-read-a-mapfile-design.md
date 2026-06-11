# huck v140 — `read -a` + `mapfile`/`readarray` Design

**Status:** approved design, ready for implementation plan.
**Implements:** `read -a NAME` (read one line, IFS-split into an indexed array) and
the `mapfile`/`readarray` builtin (read lines of input into an array). Both
populate arrays from a stream — the input side of array support, which huck
lacked. Resolves the "array/stream input builtins" gap (read currently handles
only `-r`/`-s`/`-p`/`-d`; `mapfile`/`readarray` did not exist).
**Branch (impl):** `v140-read-a-mapfile`.

## Background — current state

Core array support already works (`declare -a`/`-A`, `${arr[@]}`, `${#arr[@]}`,
`${arr[-1]}`, `${!arr[@]}`, append). What was missing is reading INTO arrays:
- `read -a arr` → `read: -a: invalid option`.
- `mapfile`/`readarray` → `command not found`.

Reusable primitives discovered:
- `Shell::replace_array(name, BTreeMap<usize,String>) -> Result<(),AssignErr>` —
  clears + sets an indexed array, honors readonly (used by PIPESTATUS). Perfect
  for the clear-then-assign case.
- `Shell::set_array_element(name, idx, value) -> Result<(),AssignErr>` — sets one
  element without clearing (promotes scalar→indexed). For `mapfile -O origin`.
- `read_one_line<R>(r, raw, delim) -> io::Result<Option<String>>` — reads one
  record, STRIPS the delimiter, does backslash processing (unless `raw`). Reused
  by `read -a`.
- `split_into_names(line, names, ifs)` — the read-specific IFS field splitter
  (multi-name: N-1 fields then remainder). `read -a` needs the UNBOUNDED variant.
- `RawStdinReader` — reads `STDIN_FILENO` directly (bypasses the shared BufReader),
  the correct stdin source for `read`/`mapfile`.
- Builtin dispatch: `run_builtin` match in `src/builtins.rs:67` + `BUILTIN_NAMES`.

Verified against bash 5 (all to be reproduced byte-for-byte by the harness):
```
read -a arr <<< "a b c"            -> ${arr[*]}="a b c" ${#arr[@]}=3
IFS=: read -a arr <<< "a:b:c"      -> "a b c" 3
arr=(old x y z); read -a arr <<<"a b" -> "a b" 2          (array CLEARED first)
mapfile -t arr <<< $'x\ny\nz'      -> 3 ; arr[1]=y
mapfile arr <<< $'a\nb'            -> arr[0]=$'a\n' arr[1]=$'b\n'  (keeps newline)
mapfile -n 2 -t arr <<< $'a\nb\nc\nd' -> "a b" 2
mapfile -s 1 -t arr <<< $'a\nb\nc' -> "b c"
mapfile -d : -t arr <<< "a:b:c"    -> 3 ; arr[1]=b   (last elem "c\n", no strip)
mapfile -O 2 -t arr <<< $'x\ny'    -> indices "2 3" values "x y"  (no clear)
readarray -t arr <<< $'p\nq'       -> "p q"           (synonym)
mapfile -t <<< $'a\nb'             -> ${MAPFILE[*]}="a b"  (default name)
```

## Architecture

### 1. `read -a NAME`

In `builtin_read`'s flag loop (`src/builtins.rs:~2096`), add an `b'a'` arm
mirroring `-p`/`-d` (value is rest-of-arg, else the next arg):
```rust
                b'a' => {
                    let v: String = if j + 1 < bytes.len() {
                        String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
                    } else {
                        i += 1;
                        if i >= args.len() {
                            eprintln!("huck: read: -a: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        args[i].clone()
                    };
                    array_name = Some(v);
                    break;
                }
```
Add `let mut array_name: Option<String> = None;` near the other flag locals.
Validate the array name with `is_valid_name` in the existing pre-read validation
block (POSIX ordering — fail before reading).

After the single-line read (the existing `read_one_line` path, `raw`/`delim`
honored), branch on `array_name`:
```rust
    if let Some(arr) = array_name {
        let fields = split_read_fields(&line, &ifs);
        let map: BTreeMap<usize, String> =
            fields.into_iter().enumerate().collect();
        if shell.replace_array(&arr, map).is_err() {
            return ExecOutcome::Continue(1); // replace_array printed the readonly msg
        }
        // bash clears any extra scalar NAME targets given alongside -a.
        for name in &names {
            let _ = shell.try_set(name, String::new());
        }
        return ExecOutcome::Continue(0);
    }
```
(rc 0 on a successful read — mirrors the existing scalar path, whose `exit` is 0
unless a readonly assignment failed. The EOF-with-nothing case already returns
`Continue(1)` earlier, before this assignment branch is reached.)

New helper `split_read_fields(line: &str, ifs: &str) -> Vec<String>`: the
unbounded version of `split_into_names`' multi-name walk —
- empty IFS → one field (the whole line);
- otherwise: skip leading IFS-whitespace, then repeatedly consume a field +
  one separator-run (a non-ws IFS char = exactly one delimiter then optional
  ws-IFS; a ws-IFS run = collapse, then optionally one non-ws IFS), until the
  string is exhausted. Trailing IFS-whitespace yields no empty field. This is the
  same byte-classification logic already in `split_into_names` (factor a shared
  inner walk if clean, or write the parallel unbounded loop — implementer's call,
  keep `split_into_names` behavior identical).

### 2. `mapfile` / `readarray`

New `pub(crate) fn builtin_mapfile(args: &[String], shell: &mut Shell) -> ExecOutcome`.
Register both names in `BUILTIN_NAMES` and add to the `run_builtin` match:
`"mapfile" | "readarray" => builtin_mapfile(args, shell),`.

**Options (core set):**
| flag | meaning |
|---|---|
| `-t` | strip the trailing delimiter from each element |
| `-d DELIM` | line delimiter; default `\n`; empty → NUL; first byte used (like `read -d`) |
| `-n COUNT` | read at most COUNT lines (0 = unlimited) |
| `-O ORIGIN` | assign starting at index ORIGIN; do NOT clear the array |
| `-s SKIP` | discard the first SKIP lines before assigning |

`-n`/`-O`/`-s` take a numeric arg (rest-of-arg or next arg; parse as `usize`,
reject non-numeric with `mapfile: <val>: invalid <opt> specification`-style rc 2).
`-d` takes a string arg. `-O` presence is tracked (`origin: Option<usize>`).
Array name = the first non-flag operand; default `"MAPFILE"`; validate with
`is_valid_name` (rc 1 `not a valid identifier` on failure). Unknown flag → rc 2.

**Reading** — new helper:
```rust
/// Reads one record up to (not including) `delim`. Returns
/// `(content, had_delim)`; `had_delim` is false at EOF for a final unterminated
/// record. `None` only when nothing remains (immediate EOF). Raw bytes — no
/// backslash processing (mapfile reads raw lines).
fn read_one_record<R: std::io::Read>(r: &mut R, delim: u8)
    -> std::io::Result<Option<(String, bool)>>
```
Loop with `RawStdinReader`:
1. Discard the first `SKIP` records (read + drop; stop early on EOF).
2. Read records, collecting into `Vec<String>`; for each `(content, had_delim)`
   the element value is `content` plus `delim as char` appended ONLY when
   `had_delim && !strip_t`. Stop after `COUNT` elements when `COUNT > 0`, or at EOF.

**Assign:**
- `origin == None` → `shell.replace_array(arr, BTreeMap{i: val})` (clear + set
  from index 0).
- `origin == Some(o)` → for each element `i`, `shell.set_array_element(arr, o+i, val)`
  (no clear; preserves/overwrites existing elements ≥ o).
Return rc 0 on success; rc 1 on a readonly/invalid-name failure.

**Help registry:** add `mapfile`/`readarray` synopsis+description entries
alongside the existing builtin help table (so `help mapfile` / `type mapfile`
work) — nice-to-have, low effort.

### Not in scope (documented)
- **`mapfile -u FD`** (read from fd N), **`-C callback`** + **`-c quantum`**
  (run a callback every quantum lines). Rare; `-C/-c` need callback eval. → new
  `[deferred]`/`[low]` divergence entry.
- **`read -n`/`-N`/`-t`/`-u`** (nchars / timeout / fd) — a separate follow-on; not
  part of v140. (read still rejects them as today.)
- **`cmd | mapfile arr`** loses `arr` (mapfile runs in a forked pipeline stage) —
  IDENTICAL to bash without `lastpipe`. Not a divergence; tests use `<<<`.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/builtins.rs` | `read -a`: `b'a'` flag arm + `array_name` + the array-assign branch; new `split_read_fields`; new `builtin_mapfile` + `read_one_record`; register `mapfile`/`readarray` in `BUILTIN_NAMES` + `run_builtin` match + help table. |
| `tests/scripts/mapfile_read_array_diff_check.sh` (NEW, 59th) | Bash-diff harness over `read -a` + `mapfile`/`readarray` via `-c` + here-strings. |
| `tests/read_array_integration.rs` (NEW) | Binary-level integration tests (here-string redirection) for the matrix. |
| `docs/bash-divergences.md` | Add a `[deferred]`/`[low]` note for the deferred `mapfile -u`/`-C`/`-c` (and `read -n`/`-t`/`-u`). Tier-4 +1. |

## Testing

1. **Unit tests** (`src/builtins.rs` tests):
   - `split_read_fields` (all verified vs bash): `"a b c"`/default-IFS →
     `["a","b","c"]`; leading/trailing ws trimmed (`"  x   y  "` → `["x","y"]`);
     `"a:b:c"`/IFS=":" → 3; empty IFS → one field (empty line → `[]`); runs of
     ws-IFS collapse. The non-ws-IFS asymmetry: a TRAILING non-ws delimiter does
     NOT add an empty field (`"x:y:"`/`:` → `["x","y"]`), but a LEADING one DOES
     (`":x"`/`:` → `["","x"]`), and an adjacent pair yields an empty between
     (`"x::y"`/`:` → `["x","","y"]`). Mixed IFS (`"x : y"`/`" :"`) → `["x","y"]`.
   - `read_one_record`: `"a\nb\n"`/`\n` → `("a",true),("b",true),None`;
     `"a\nb"`/`\n` → `("a",true),("b",false),None`; `"a:b:c\n"`/`:` →
     `("a",true),("b",true),("c\n",false)`; empty input → `None`.
2. **Bash-diff harness** `tests/scripts/mapfile_read_array_diff_check.sh` (the gold
   standard) — each fragment via `-c`, byte-identical bash↔huck (stdout + rc):
   - `read -a arr <<< "a b c"; echo "${arr[*]}|${#arr[@]}"`
   - `IFS=: read -a arr <<< "a:b:c"; echo "${arr[*]}|${#arr[@]}"`
   - `arr=(old x y z); read -a arr <<< "a b"; echo "${arr[*]}|${#arr[@]}"`
   - `read -ra arr <<< 'x\ty'; echo "${#arr[@]}"` (raw)
   - `mapfile -t arr <<< $'x\ny\nz'; echo "${#arr[@]}|${arr[1]}"`
   - `mapfile arr <<< $'a\nb'; printf '%q %q\n' "${arr[0]}" "${arr[1]}"` (keeps `\n`)
   - `mapfile -n 2 -t arr <<< $'a\nb\nc\nd'; echo "${arr[*]}|${#arr[@]}"`
   - `mapfile -s 1 -t arr <<< $'a\nb\nc'; echo "${arr[*]}"`
   - `mapfile -d : -t arr <<< "a:b:c"; echo "${#arr[@]}|${arr[1]}"`
   - `mapfile -O 2 -t arr <<< $'x\ny'; echo "${!arr[*]}|${arr[*]}"`
   - `readarray -t arr <<< $'p\nq'; echo "${arr[*]}"`
   - `mapfile -t <<< $'a\nb'; echo "${MAPFILE[*]}"` (default name)
   (All use `<<<`/redirection so `read`/`mapfile` run in the MAIN shell — bash does
   the same; a pipe would subshell both shells identically.)
3. **Integration tests** (`tests/read_array_integration.rs`) — spawn the huck
   binary, feed a script via stdin or `-c`, assert exact stdout for the key rows
   (read -a basic + custom IFS + clear; mapfile -t, no-t newline via `%q`, -O, -n,
   -s, -d, default MAPFILE, readarray).
4. **Full regression:** entire suite + all harnesses green; clippy clean.

## Edge cases & notes
- **`read -a` clears the array** (via `replace_array`) before assigning — matches
  bash. Trailing scalar names alongside `-a` are cleared to `""` (bash parity).
- **Delimiter vs content**: with `-d :`, a `\n` in the input is ordinary content;
  `-t` strips only a trailing `delim` byte that was actually present (`had_delim`),
  so an EOF-terminated final record keeps its bytes (e.g. `c\n`). Verified vs bash.
- **`-O` does not clear**: existing elements below `origin` (and any not
  overwritten) are preserved — `set_array_element` per element.
- **Empty input / immediate EOF**: `mapfile` assigns an empty array (clear case) /
  leaves the array as-is under `-O`; `read -a` on EOF returns rc 1 (existing path)
  and does not create the array.
- **`-n 0`** = unlimited (read all). **`-s N`** beyond EOF = empty result.
- **`is_valid_name`** rejects bad array names for both builtins before any read.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the
  controller verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
