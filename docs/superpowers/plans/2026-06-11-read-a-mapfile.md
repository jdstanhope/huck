# huck v140 — `read -a` + `mapfile`/`readarray` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `read -a NAME` (read one line, IFS-split into an indexed array) and a new `mapfile`/`readarray` builtin (read lines of input into an array), populating arrays from a stream.

**Architecture:** `read -a` reuses the existing single-line `read_one_line` path + a new unbounded IFS splitter `split_read_fields`, assigning via `Shell::replace_array`. `mapfile` is a new builtin with a raw record reader `read_one_record` (keeps the delimiter unless `-t`), assigning via `replace_array` (default) or `set_array_element` (`-O origin`, no clear). Both read `STDIN_FILENO` via `RawStdinReader`.

**Tech Stack:** Rust; huck `builtins.rs` (`builtin_read`, new `builtin_mapfile`, `run_builtin` dispatch, `BUILTIN_NAMES`, help table); `Shell::replace_array`/`set_array_element` (shell_state.rs); `std::collections::BTreeMap` (fully-qualified — not imported at the top of builtins.rs).

**Reference:** spec at `docs/superpowers/specs/2026-06-11-read-a-mapfile-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits in a prior iteration). Stay on `v140-read-a-mapfile`. Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** BINARY crate — `cargo test --bin huck <filter>` for unit tests, `cargo test --test <name>` for integration, `cargo clippy --all-targets` (NOT `--lib`). Builds/tests take minutes.

---

### Task 1: `split_read_fields` — unbounded IFS splitter

**Files:**
- Modify: `src/builtins.rs` (add the helper near `split_into_names` ~1901; add unit tests in the test module)

- [ ] **Step 1: Write the failing tests** — add to the `#[cfg(test)] mod tests` in `src/builtins.rs`:

```rust
#[test]
fn split_read_fields_default_ws() {
    assert_eq!(split_read_fields("a b c", " \t\n"), vec!["a", "b", "c"]);
    assert_eq!(split_read_fields("  x   y  ", " \t\n"), vec!["x", "y"]); // trim + collapse
    assert_eq!(split_read_fields("", " \t\n"), Vec::<String>::new());   // empty -> none
}

#[test]
fn split_read_fields_nonws_ifs() {
    assert_eq!(split_read_fields("a:b:c", ":"), vec!["a", "b", "c"]);
    assert_eq!(split_read_fields("x:y:", ":"), vec!["x", "y"]);       // trailing delim: NO empty
    assert_eq!(split_read_fields(":x", ":"), vec!["", "x"]);          // leading delim: empty first
    assert_eq!(split_read_fields("x::y", ":"), vec!["x", "", "y"]);   // adjacent: empty between
}

#[test]
fn split_read_fields_mixed_and_empty_ifs() {
    assert_eq!(split_read_fields("x : y", " :"), vec!["x", "y"]);     // ws around nonws collapses
    assert_eq!(split_read_fields("a b c", ""), vec!["a b c"]);        // empty IFS -> one field
    assert_eq!(split_read_fields("", ""), Vec::<String>::new());      // empty IFS + empty -> none
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck split_read_fields 2>&1 | tail -15`
Expected: FAILS to compile (`split_read_fields` undefined). Record.

- [ ] **Step 3: Implement the helper** — add to `src/builtins.rs` (near `split_into_names`):

```rust
/// Splits `line` into ALL IFS fields (the unbounded form used by `read -a` /
/// mapfile element splitting). Mirrors bash word-splitting: leading IFS-ws is
/// stripped; a non-ws IFS char delimits (a leading one yields a leading empty
/// field, an adjacent pair yields an empty field between, but a TRAILING one
/// yields no trailing empty field); ws-IFS runs collapse. Empty IFS -> the whole
/// line as one field (none for an empty line).
fn split_read_fields(line: &str, ifs: &str) -> Vec<String> {
    let ifs_bytes: Vec<u8> = ifs.bytes().collect();
    if ifs_bytes.is_empty() {
        return if line.is_empty() { Vec::new() } else { vec![line.to_string()] };
    }
    let is_ws = |b: u8| ifs_bytes.contains(&b) && matches!(b, b' ' | b'\t' | b'\n');
    let is_nonws = |b: u8| ifs_bytes.contains(&b) && !matches!(b, b' ' | b'\t' | b'\n');
    let is_any = |b: u8| ifs_bytes.contains(&b);
    let bytes = line.as_bytes();
    let mut fields: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && !is_any(bytes[i]) {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
        if i >= bytes.len() {
            break;
        }
        // Consume one separator. Non-ws IFS: exactly one + trailing ws-IFS.
        // ws-IFS: collapse the run, then optionally one non-ws IFS + trailing ws.
        if is_nonws(bytes[i]) {
            i += 1;
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
        } else {
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() && is_nonws(bytes[i]) {
                i += 1;
                while i < bytes.len() && is_ws(bytes[i]) {
                    i += 1;
                }
            }
        }
    }
    fields
}
```

- [ ] **Step 4: Run the tests + clippy**

Run: `cargo test --bin huck split_read_fields 2>&1 | tail -15` → 3 tests PASS.
Run: `cargo clippy --bin huck 2>&1 | tail -8` → if `split_read_fields` is flagged dead_code (no caller until Task 2), add `#[allow(dead_code)]` with a comment "Wired into `read -a` in Task 2" above it (Task 2 removes it).

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs
git commit -m "$(printf 'feat: split_read_fields — unbounded IFS field splitter for read -a/mapfile\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: `read -a NAME`

**Files:**
- Modify: `src/builtins.rs` (`builtin_read` ~2075-2229; help table entry ~5023)
- Create: `tests/read_array_integration.rs`

- [ ] **Step 1: Write the failing integration tests** — create `tests/read_array_integration.rs`:

```rust
//! v140: `read -a` reads a line, IFS-splits into an indexed array. Run via the
//! huck binary with the script in `-c` (here-strings keep `read` in the main shell).
use std::process::Command;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn huck_c(script: &str) -> (String, i32) {
    let o = Command::new(huck_bin()).arg("-c").arg(script).output().expect("spawn");
    (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
}

#[test]
fn read_a_basic() {
    let (out, code) = huck_c(r#"read -a arr <<< "a b c"; echo "${arr[*]}|${#arr[@]}""#);
    assert_eq!(out, "a b c|3\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn read_a_custom_ifs() {
    let (out, _c) = huck_c(r#"IFS=: read -a arr <<< "a:b:c"; echo "${arr[*]}|${#arr[@]}""#);
    assert_eq!(out, "a b c|3\n", "out={out:?}");
}

#[test]
fn read_a_clears_existing_array() {
    let (out, _c) = huck_c(r#"arr=(old x y z); read -a arr <<< "a b"; echo "${arr[*]}|${#arr[@]}""#);
    assert_eq!(out, "a b|2\n", "out={out:?}");
}

#[test]
fn read_ra_raw_backslash() {
    // -r: backslash is literal. Input "x\ty" (literal backslash-t) -> with default
    // IFS no split on the literal chars; one field "x\ty".
    let (out, _c) = huck_c(r#"read -ra arr <<< 'x\ty'; echo "${#arr[@]}|${arr[0]}""#);
    assert_eq!(out, "1|x\\ty\n", "out={out:?}");
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --test read_array_integration 2>&1 | tail -20`
Expected: all FAIL — pre-wiring, `read -a` errors (`read: -a: invalid option`), so stdout is empty / wrong. Record.

- [ ] **Step 3: Add the `-a` flag + the array-assign branch** — `src/builtins.rs` `builtin_read`.

(a) Add the local near the other flag vars (~2080): `let mut array_name: Option<String> = None;`

(b) In the flag `match bytes[j]` loop, add an arm (alongside `b'd'`):
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

(c) In the pre-read name-validation block (~2143), also validate the array name:
```rust
    if let Some(arr) = &array_name
        && !is_valid_name(arr)
    {
        eprintln!("huck: read: `{arr}': not a valid identifier");
        return ExecOutcome::Continue(1);
    }
```

(d) Replace the final assignment block (currently builds `assignments` and loops `try_set`, ~2213-2228) so the `-a` case is handled first:
```rust
    // Assignment.
    let ifs = shell.ifs();
    if let Some(arr) = array_name {
        let fields = split_read_fields(&line, &ifs);
        let map: std::collections::BTreeMap<usize, String> =
            fields.into_iter().enumerate().collect();
        if shell.replace_array(&arr, map).is_err() {
            return ExecOutcome::Continue(1); // replace_array printed the readonly message
        }
        // bash clears any extra scalar NAME targets given alongside -a.
        for name in &names {
            let _ = shell.try_set(name, String::new());
        }
        return ExecOutcome::Continue(0);
    }
    let assignments: Vec<(String, String)> = if names.is_empty() {
        vec![("REPLY".to_string(), line)]
    } else {
        split_into_names(&line, &names, &ifs)
    };

    let mut exit = 0;
    for (name, value) in assignments {
        if shell.try_set(&name, value).is_err() {
            eprintln!("huck: read: {name}: readonly variable");
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
```
(If Task 1 added `#[allow(dead_code)]` to `split_read_fields`, REMOVE it now — it has a caller.)

(e) Update the `read` help entry (~5024): change the synopsis to
`"read [-r] [-p PROMPT] [-s] [-d DELIM] [-a ARRAY] [NAME ...]"` and append to the
description: `\n-a ARRAY assigns the IFS-split words to the indexed array ARRAY.`

- [ ] **Step 4: Build, run tests, clippy**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --test read_array_integration 2>&1 | tail -15` → all 4 PASS.
Run: `cargo test --bin huck split_read_fields read 2>&1 | tail -10` → split + existing read tests green.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → no new warnings.

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs tests/read_array_integration.rs
git commit -m "$(printf 'feat: read -a ARRAY — IFS-split a line into an indexed array\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: `read_one_record` — raw record reader for mapfile

**Files:**
- Modify: `src/builtins.rs` (add the helper near `read_one_line` ~1851; add unit tests)

- [ ] **Step 1: Write the failing tests** — add to the test module in `src/builtins.rs`:

```rust
#[test]
fn read_one_record_newline_delim() {
    let mut r = std::io::Cursor::new(b"a\nb\n".to_vec());
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("a".to_string(), true)));
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("b".to_string(), true)));
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), None);
}

#[test]
fn read_one_record_unterminated_last() {
    let mut r = std::io::Cursor::new(b"a\nb".to_vec());
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("a".to_string(), true)));
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("b".to_string(), false)));
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), None);
}

#[test]
fn read_one_record_custom_delim_keeps_other_bytes() {
    let mut r = std::io::Cursor::new(b"a:b:c\n".to_vec());
    assert_eq!(read_one_record(&mut r, b':').unwrap(), Some(("a".to_string(), true)));
    assert_eq!(read_one_record(&mut r, b':').unwrap(), Some(("b".to_string(), true)));
    assert_eq!(read_one_record(&mut r, b':').unwrap(), Some(("c\n".to_string(), false)));
    assert_eq!(read_one_record(&mut r, b':').unwrap(), None);
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck read_one_record 2>&1 | tail -15`
Expected: FAILS to compile (`read_one_record` undefined). Record.

- [ ] **Step 3: Implement** — add to `src/builtins.rs` (near `read_one_line`):

```rust
/// Reads one record up to (not including) `delim`. Returns `(content, had_delim)`;
/// `had_delim` is false for a final unterminated record at EOF. `None` only when
/// nothing remains. Raw bytes — no backslash processing (mapfile reads raw lines).
fn read_one_record<R: std::io::Read>(
    r: &mut R,
    delim: u8,
) -> std::io::Result<Option<(String, bool)>> {
    let mut out: Vec<u8> = Vec::new();
    let mut any = false;
    loop {
        let mut byte = [0u8; 1];
        let n = r.read(&mut byte)?;
        if n == 0 {
            if !any {
                return Ok(None);
            }
            return Ok(Some((String::from_utf8_lossy(&out).into_owned(), false)));
        }
        any = true;
        if byte[0] == delim {
            return Ok(Some((String::from_utf8_lossy(&out).into_owned(), true)));
        }
        out.push(byte[0]);
    }
}
```

- [ ] **Step 4: Run + clippy**

Run: `cargo test --bin huck read_one_record 2>&1 | tail -15` → 3 PASS.
Run: `cargo clippy --bin huck 2>&1 | tail -8` → if flagged dead_code (no caller until Task 4), add `#[allow(dead_code)]` with a comment "Wired into mapfile in Task 4" (Task 4 removes it).

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs
git commit -m "$(printf 'feat: read_one_record — raw delimiter-record reader for mapfile\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: `mapfile` / `readarray` builtin

**Files:**
- Modify: `src/builtins.rs` (`BUILTIN_NAMES` ~25; `run_builtin` match ~67-123; new `builtin_mapfile`; help table ~5023)
- Modify: `tests/read_array_integration.rs` (add mapfile tests)

- [ ] **Step 1: Add the failing integration tests** — append to `tests/read_array_integration.rs`:

```rust
#[test]
fn mapfile_t_strips_newline() {
    let (out, _c) = huck_c("mapfile -t arr <<< $'x\\ny\\nz'; echo \"${#arr[@]}|${arr[1]}\"");
    assert_eq!(out, "3|y\n", "out={out:?}");
}

#[test]
fn mapfile_keeps_newline_without_t() {
    let (out, _c) = huck_c("mapfile arr <<< $'a\\nb'; printf '%q %q\\n' \"${arr[0]}\" \"${arr[1]}\"");
    assert_eq!(out, "$'a\\n' $'b\\n'\n", "out={out:?}");
}

#[test]
fn mapfile_n_limit() {
    let (out, _c) = huck_c("mapfile -n 2 -t arr <<< $'a\\nb\\nc\\nd'; echo \"${arr[*]}|${#arr[@]}\"");
    assert_eq!(out, "a b|2\n", "out={out:?}");
}

#[test]
fn mapfile_s_skip() {
    let (out, _c) = huck_c("mapfile -s 1 -t arr <<< $'a\\nb\\nc'; echo \"${arr[*]}\"");
    assert_eq!(out, "b c\n", "out={out:?}");
}

#[test]
fn mapfile_d_delim() {
    let (out, _c) = huck_c("mapfile -d : -t arr <<< 'a:b:c'; echo \"${#arr[@]}|${arr[1]}\"");
    assert_eq!(out, "3|b\n", "out={out:?}");
}

#[test]
fn mapfile_o_origin_no_clear() {
    let (out, _c) = huck_c("mapfile -O 2 -t arr <<< $'x\\ny'; echo \"${!arr[*]}|${arr[*]}\"");
    assert_eq!(out, "2 3|x y\n", "out={out:?}");
}

#[test]
fn readarray_synonym_and_default_name() {
    let (out, _c) = huck_c("readarray -t arr <<< $'p\\nq'; echo \"${arr[*]}\"");
    assert_eq!(out, "p q\n", "out={out:?}");
    let (out2, _c2) = huck_c("mapfile -t <<< $'a\\nb'; echo \"${MAPFILE[*]}\"");
    assert_eq!(out2, "a b\n", "out2={out2:?}");
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --test read_array_integration 2>&1 | tail -20`
Expected: the 7 new mapfile tests FAIL (`command not found: mapfile`); the 4 read_a tests still PASS. Record.

- [ ] **Step 3: Register the builtin** — `src/builtins.rs`:
- In `BUILTIN_NAMES` (~25), add `"mapfile", "readarray",` (e.g. on the `"declare", "typeset",` line group).
- In the `run_builtin` match (~67-123), add an arm (near `"read"`):
  `"mapfile" | "readarray" => builtin_mapfile(args, shell),`

- [ ] **Step 4: Implement `builtin_mapfile`** — add to `src/builtins.rs`:

```rust
/// `mapfile [-d DELIM] [-n COUNT] [-O ORIGIN] [-s SKIP] [-t] [ARRAY]`
/// (alias `readarray`). Reads delimiter-separated records from stdin into the
/// indexed array ARRAY (default MAPFILE). Core option set (v140); `-u`/`-C`/`-c`
/// are not implemented. (M-59 follow-on / array input.)
fn builtin_mapfile(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut delim: u8 = b'\n';
    let mut strip_t = false;
    let mut count: usize = 0; // 0 = unlimited
    let mut skip: usize = 0;
    let mut origin: Option<usize> = None;
    let mut i = 0;

    // Parse a numeric option value (rest-of-arg or next arg).
    fn num_val(args: &[String], i: &mut usize, j: usize, bytes: &[u8], opt: char) -> Result<usize, ()> {
        let s = if j + 1 < bytes.len() {
            String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
        } else {
            *i += 1;
            if *i >= args.len() {
                eprintln!("huck: mapfile: -{opt}: option requires an argument");
                return Err(());
            }
            args[*i].clone()
        };
        match s.trim().parse::<usize>() {
            Ok(n) => Ok(n),
            Err(_) => {
                eprintln!("huck: mapfile: {s}: invalid number");
                Err(())
            }
        }
    }

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        let bytes = arg.as_bytes();
        let mut j = 1;
        let mut consumed_rest = false;
        while j < bytes.len() {
            match bytes[j] {
                b't' => strip_t = true,
                b'd' => {
                    let s = if j + 1 < bytes.len() {
                        String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
                    } else {
                        i += 1;
                        if i >= args.len() {
                            eprintln!("huck: mapfile: -d: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        args[i].clone()
                    };
                    delim = s.bytes().next().unwrap_or(0u8); // empty -> NUL
                    consumed_rest = true;
                }
                b'n' => match num_val(args, &mut i, j, bytes, 'n') {
                    Ok(n) => { count = n; consumed_rest = true; }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                b's' => match num_val(args, &mut i, j, bytes, 's') {
                    Ok(n) => { skip = n; consumed_rest = true; }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                b'O' => match num_val(args, &mut i, j, bytes, 'O') {
                    Ok(n) => { origin = Some(n); consumed_rest = true; }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                c => {
                    eprintln!("huck: mapfile: -{}: invalid option", c as char);
                    return ExecOutcome::Continue(2);
                }
            }
            if consumed_rest {
                break;
            }
            j += 1;
        }
        i += 1;
    }

    let array_name = args.get(i).cloned().unwrap_or_else(|| "MAPFILE".to_string());
    if !is_valid_name(&array_name) {
        eprintln!("huck: mapfile: `{array_name}': not a valid array name");
        return ExecOutcome::Continue(1);
    }

    let mut handle = RawStdinReader::new();
    // Skip the first `skip` records.
    for _ in 0..skip {
        match read_one_record(&mut handle, delim) {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                eprintln!("huck: mapfile: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    }
    // Collect up to `count` (0 = unlimited) records.
    let mut elements: Vec<String> = Vec::new();
    loop {
        if count != 0 && elements.len() >= count {
            break;
        }
        match read_one_record(&mut handle, delim) {
            Ok(Some((content, had_delim))) => {
                let mut val = content;
                if had_delim && !strip_t {
                    val.push(delim as char);
                }
                elements.push(val);
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("huck: mapfile: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    }

    match origin {
        None => {
            let map: std::collections::BTreeMap<usize, String> =
                elements.into_iter().enumerate().collect();
            if shell.replace_array(&array_name, map).is_err() {
                return ExecOutcome::Continue(1);
            }
        }
        Some(o) => {
            for (k, val) in elements.into_iter().enumerate() {
                if shell.set_array_element(&array_name, o + k, val).is_err() {
                    return ExecOutcome::Continue(1);
                }
            }
        }
    }
    ExecOutcome::Continue(0)
}
```
(If Task 3 added `#[allow(dead_code)]` to `read_one_record`, REMOVE it now.)

- [ ] **Step 5: Add help entries** — add two `HelpEntry { … }` entries (near `read`'s, ~5023):
```rust
    HelpEntry {
        name: "mapfile",
        synopsis: "mapfile [-d DELIM] [-n COUNT] [-O ORIGIN] [-s SKIP] [-t] [ARRAY]",
        description: "Read lines from standard input into an indexed array (default MAPFILE).\n\
                      -t strips the trailing delimiter; -d sets the delimiter (default newline);\n\
                      -n reads at most COUNT lines (0 = all); -O assigns from index ORIGIN\n\
                      (without clearing); -s discards the first SKIP lines.",
    },
    HelpEntry {
        name: "readarray",
        synopsis: "readarray [-d DELIM] [-n COUNT] [-O ORIGIN] [-s SKIP] [-t] [ARRAY]",
        description: "Synonym for mapfile.",
    },
```

- [ ] **Step 6: Build, run tests, clippy**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --test read_array_integration 2>&1 | tail -20` → all 11 PASS.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → no new warnings (dead_code allows removed).

- [ ] **Step 7: Commit**

```bash
git add src/builtins.rs tests/read_array_integration.rs
git commit -m "$(printf 'feat: mapfile/readarray builtin (-t -d -n -O -s; default MAPFILE)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Bash-diff harness (the 59th)

**Files:**
- Create: `tests/scripts/mapfile_read_array_diff_check.sh`

- [ ] **Step 1: Write the harness** — create `tests/scripts/mapfile_read_array_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v140: read -a + mapfile/readarray.
# Each fragment runs via `-c` with a here-string (so read/mapfile stay in the
# main shell — a pipe would subshell both identically). stdout + rc compared.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "read -a basic"     'read -a arr <<< "a b c"; echo "${arr[*]}|${#arr[@]}"'
check "read -a IFS"       'IFS=: read -a arr <<< "a:b:c"; echo "${arr[*]}|${#arr[@]}"'
check "read -a clears"    'arr=(old x y z); read -a arr <<< "a b"; echo "${arr[*]}|${#arr[@]}"'
check "read -ra raw"      'read -ra arr <<< '"'"'x\ty'"'"'; echo "${#arr[@]}|${arr[0]}"'
check "mapfile -t"        'mapfile -t arr <<< $'"'"'x\ny\nz'"'"'; echo "${#arr[@]}|${arr[1]}"'
check "mapfile keeps nl"  'mapfile arr <<< $'"'"'a\nb'"'"'; printf "%q %q\n" "${arr[0]}" "${arr[1]}"'
check "mapfile -n"        'mapfile -n 2 -t arr <<< $'"'"'a\nb\nc\nd'"'"'; echo "${arr[*]}|${#arr[@]}"'
check "mapfile -s"        'mapfile -s 1 -t arr <<< $'"'"'a\nb\nc'"'"'; echo "${arr[*]}"'
check "mapfile -d"        'mapfile -d : -t arr <<< "a:b:c"; echo "${#arr[@]}|${arr[1]}"'
check "mapfile -O"        'mapfile -O 2 -t arr <<< $'"'"'x\ny'"'"'; echo "${!arr[*]}|${arr[*]}"'
check "readarray synonym" 'readarray -t arr <<< $'"'"'p\nq'"'"'; echo "${arr[*]}"'
check "mapfile default"   'mapfile -t <<< $'"'"'a\nb'"'"'; echo "${MAPFILE[*]}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: chmod + build + run**

Run: `chmod +x tests/scripts/mapfile_read_array_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/mapfile_read_array_diff_check.sh`
Expected: `Total: 12, Pass: 12, Fail: 0`. If any check FAILs, paste the diff and STOP (a real divergence) — do not weaken assertions.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/mapfile_read_array_diff_check.sh
git commit -m "$(printf 'test: 59th bash-diff harness for read -a + mapfile/readarray\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 6: Docs — log the deferred mapfile/read flags

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Add a Tier-4 deferred note**

Append a new `L-` entry (use the next free number — `grep -oE "L-[0-9]+" docs/bash-divergences.md | sort -t- -k2 -n | tail -1` to find the highest, then +1) to the Tier-4 bulleted list, e.g.:
```
- **L-34: `mapfile`/`read` unimplemented flags** — `[deferred]`, low (v140). v140 ships `mapfile`/`readarray` with `-t -d -n -O -s` and `read -a`; NOT YET implemented: `mapfile -u FD`/`-C callback`/`-c quantum`, and `read -n`/`-N`/`-t`/`-u`. `-C`/`-c` need callback eval; `-u` needs reading from an arbitrary fd. Rare in practice; deferred.
```

- [ ] **Step 2: Bump the Tier-4 count**

In the Summary table, increment "Low-impact (Tier 4)" by 1 (verify the current value first with `grep -n "Low-impact (Tier 4)" docs/bash-divergences.md`).

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: log L-34 (deferred mapfile/read flags) after v140\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 7: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL pass (baseline after v139 was 3049 tests; v140 adds 3 split + 3 record unit tests + 11 integration tests). Zero failures. Paste any failure.

- [ ] **Step 2: read / array suites explicitly**

Run: `cargo test --bin huck read split_read_fields read_one_record 2>&1 | tail -12` (existing read tests + the new unit tests).
Run: `cargo test --test read_array_integration 2>&1 | tail -6` (run twice — spawns the binary; confirm stable).

- [ ] **Step 3: All bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" | tail -1; done`
Expected: every harness ends with `Fail: 0` (including the new `mapfile_read_array_diff_check.sh` → `Pass: 12, Fail: 0`).

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -8`
Expected: clean.

- [ ] **Step 5: Commit (only if a verification-driven fix was needed)**

If Steps 1-4 surfaced a real issue, make the SMALLEST fix, re-run, commit with the trailer. Otherwise no commit — verification only.

---

## Notes for the implementer
- **`read -a` clears the array** (via `replace_array`) and clears trailing scalar NAMEs to `""` — bash parity.
- **`mapfile` keeps the delimiter per element unless `-t`**; the last unterminated record (EOF) keeps no delimiter (`had_delim` false). This is the load-bearing detail.
- **`-O origin` does NOT clear** — use `set_array_element` per element; otherwise `replace_array`.
- **Use here-strings (`<<<`) / redirection in tests**, never a pipe — `read`/`mapfile` in a pipe run in a forked subshell and lose the array in BOTH bash and huck.
- **`BTreeMap`** is not imported at the top of `builtins.rs`; use the fully-qualified `std::collections::BTreeMap`.
- **Deferred:** `mapfile -u`/`-C`/`-c` and `read -n`/`-N`/`-t`/`-u` are out of scope (logged as L-34).
