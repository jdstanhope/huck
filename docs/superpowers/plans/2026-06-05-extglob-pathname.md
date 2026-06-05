# extglob Pathname Globbing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support extended-glob patterns (`?(…)`/`*(…)`/`+(…)`/`@(…)`/`!(…)`, `shopt -s extglob`) in pathname/filesystem globbing — `echo +(a|b)`, `dir*/+(foo|bar).txt`, etc. (the piece v90 deferred as M-84a).

**Architecture:** A custom recursive directory walker in `src/glob_match.rs` (`extglob_pathname_expand`) reuses v90's `extglob_match` per path component, since the `glob` crate can't do extglob. It is invoked from `glob_expand_fields_opts` ONLY when extglob is on AND the field contains an unquoted extglob operator; every other pathname glob keeps the `glob` crate byte-for-byte.

**Tech Stack:** Rust; `std::fs::read_dir`; huck's `glob_match::{extglob_match, has_extglob}`, `GlobOpts`, `Field`; `tempfile` (dev-dep, already present).

**Spec:** `docs/superpowers/specs/2026-06-05-extglob-pathname-design.md`

**Verified bash 5.2 contract** (dir with `a b ab aab abc cd xy .hidden .ab` + `dir1/{foo.txt,bar.log}` `dir2/foo.txt`, `shopt -s extglob`): `+(a|b)`→`a aab ab b`; `@(a|cd)`→`a cd`; `!(a|ab)`→`aab abc b cd dir1 dir2 xy` (dotfiles excluded); `.+(ab)`→`.ab`; `+([a-c])`→`a aab ab abc b`; `dir*/+(foo|bar).txt`→`dir1/foo.txt dir2/foo.txt`; nocaseglob `@(A|AB)`→`a ab`; output sorted; no-match→literal (empty under nullglob).

**Conventions:**
- Binary crate: unit `cargo test --bin huck <filter>`; integration `cargo test --test <name>`; full `cargo test`.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Baseline: **2494** tests pass, clippy clean. Each task keeps clippy clean + suite green.
- **Test cwd safety**: the walker reads `.` for *relative* patterns. `set_current_dir` is process-global and RACY under parallel unit tests — so **unit tests use ABSOLUTE patterns** (`format!("{}/+(a|b)", dir.path().display())`), which make the walker start at `/` and never touch cwd. **Integration tests** set `current_dir(fixture)` on the spawned `huck` process (per-process cwd, safe) and use relative patterns.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/glob_match.rs` | NEW `extglob_pathname_expand` + `walk_components`/`join_path`/`component_needs_match` + unit tests | 1 |
| `src/expand.rs` | `GlobOpts.extglob`; dispatch in `glob_expand_fields_opts`; `build_glob_pattern` quoted-escape extension | 2 |
| `src/shell_state.rs` | `Shell::glob_opts()` sets `extglob` | 2 |
| `tests/extglob_pathname_integration.rs` | NEW — integration tests (fixture dir) | 2 |
| `tests/scripts/extglob_pathname_diff_check.sh` | NEW — huck's 18th bash-diff harness | 3 |
| `docs/bash-divergences.md`, `README.md` | M-84a `[deferred]`→`[fixed v91]`; M-84 note; changelog; README v91 row | 3 |

---

### Task 1: The custom directory walker

**Files:**
- Modify: `src/glob_match.rs` (add `extglob_pathname_expand` + helpers + unit tests)
- Test: `src/glob_match.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing walker unit tests (absolute patterns — cwd-safe)**

Add to `src/glob_match.rs` `#[cfg(test)]`:

```rust
#[cfg(test)]
mod pathname_tests {
    use super::*;
    use std::fs;

    /// Builds a tempdir fixture and returns (TempDir, its absolute path string).
    fn fixture() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        for f in ["a", "b", "ab", "aab", "abc", "cd", "xy", ".hidden", ".ab"] {
            fs::write(d.path().join(f), b"").unwrap();
        }
        fs::create_dir(d.path().join("dir1")).unwrap();
        fs::create_dir(d.path().join("dir2")).unwrap();
        fs::write(d.path().join("dir1/foo.txt"), b"").unwrap();
        fs::write(d.path().join("dir1/bar.log"), b"").unwrap();
        fs::write(d.path().join("dir2/foo.txt"), b"").unwrap();
        let base = d.path().to_str().unwrap().to_string();
        (d, base)
    }

    /// Maps file names to absolute paths under `base`, sorted.
    fn abs(base: &str, names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|n| format!("{base}/{n}")).collect();
        v.sort();
        v
    }

    #[test]
    fn plus_one_or_more_excludes_dotfiles() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/+(a|b)"), false, false);
        assert_eq!(got, abs(&base, &["a", "aab", "ab", "b"]));
    }

    #[test]
    fn at_exactly_one() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/@(a|cd)"), false, false);
        assert_eq!(got, abs(&base, &["a", "cd"]));
    }

    #[test]
    fn negation_excludes_listed_and_dotfiles() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/!(a|ab)"), false, false);
        assert_eq!(got, abs(&base, &["aab", "abc", "b", "cd", "dir1", "dir2", "xy"]));
    }

    #[test]
    fn class_inside_extglob() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/+([a-c])"), false, false);
        assert_eq!(got, abs(&base, &["a", "aab", "ab", "abc", "b"]));
    }

    #[test]
    fn explicit_dot_matches_dotfile() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/.+(ab)"), false, false);
        assert_eq!(got, abs(&base, &[".ab"]));
    }

    #[test]
    fn nocaseglob_folds_case() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/@(A|AB)"), true, false);
        assert_eq!(got, abs(&base, &["a", "ab"]));
    }

    #[test]
    fn multi_component() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/dir*/+(foo|bar).txt"), false, false);
        assert_eq!(got, abs(&base, &["dir1/foo.txt", "dir2/foo.txt"]));
    }

    #[test]
    fn no_match_is_empty() {
        let (_d, base) = fixture();
        assert!(extglob_pathname_expand(&format!("{base}/+(zzz)"), false, false).is_empty());
    }
}
```

- [ ] **Step 2: Run, verify they fail to compile**

Run: `cargo test --bin huck pathname_tests 2>&1 | tail` → `extglob_pathname_expand` undefined.

- [ ] **Step 3: Implement the walker**

Add to `src/glob_match.rs` (above the test module):

```rust
/// Filesystem pathname expansion for an extglob `pattern` (the `glob` crate
/// can't do extglob). Returns matched paths sorted lexicographically; empty if
/// nothing matches. Honors the dotfile rule, `nocaseglob`, and `dotglob`.
/// Per-component matching delegates to `extglob_match` (which also implements
/// `*`/`?`/`[…]`), so mixed patterns like `dir*/+(foo|bar).txt` work.
pub fn extglob_pathname_expand(pattern: &str, nocaseglob: bool, dotglob: bool) -> Vec<String> {
    let absolute = pattern.starts_with('/');
    let comps: Vec<String> = pattern
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if comps.is_empty() {
        return Vec::new();
    }
    let start = if absolute { "/".to_string() } else { String::new() };
    let mut out = Vec::new();
    walk_components(&start, &comps, 0, nocaseglob, dotglob, &mut out);
    out.sort();
    out
}

/// True if a path component needs directory matching (vs literal descent):
/// it has a glob wildcard or an extglob operator.
fn component_needs_match(comp: &str) -> bool {
    comp.contains('*') || comp.contains('?') || comp.contains('[') || has_extglob(comp)
}

/// Joins `prefix` + `name` into a path: empty prefix → bare name (relative,
/// no `./`); root prefix → `/name`; else `prefix/name`.
fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else if prefix == "/" {
        format!("/{name}")
    } else {
        format!("{prefix}/{name}")
    }
}

fn walk_components(
    prefix: &str,
    comps: &[String],
    idx: usize,
    nocaseglob: bool,
    dotglob: bool,
    out: &mut Vec<String>,
) {
    if idx == comps.len() {
        out.push(prefix.to_string());
        return;
    }
    let comp = &comps[idx];
    let is_last = idx + 1 == comps.len();

    // Literal component: descend (or include) only if the path exists on disk.
    if !component_needs_match(comp) {
        let next = join_path(prefix, comp);
        if std::path::Path::new(&next).exists() {
            walk_components(&next, comps, idx + 1, nocaseglob, dotglob, out);
        }
        return;
    }

    // Pattern component: list the directory and keep matching entries.
    let dir = if prefix.is_empty() { "." } else { prefix };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    // Dotfile rule: a leading-dot entry is matched only if `dotglob` is on or
    // the component's first char is a literal `.` (the pattern is dot-anchored).
    let dot_anchored = comp.starts_with('.');
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue, // skip non-UTF8 names
        };
        if name == "." || name == ".." {
            continue;
        }
        if name.starts_with('.') && !dotglob && !dot_anchored {
            continue;
        }
        if extglob_match(comp, &name, nocaseglob) {
            let next = join_path(prefix, &name);
            if is_last {
                out.push(next);
            } else if std::path::Path::new(&next).is_dir() {
                walk_components(&next, comps, idx + 1, nocaseglob, dotglob, out);
            }
        }
    }
}
```

- [ ] **Step 4: Run the walker tests, verify pass**

Run: `cargo test --bin huck pathname_tests 2>&1 | tail` → all 8 pass.
Run: `cargo build 2>&1 | tail -3` → clean (a transitional dead-code warning on `extglob_pathname_expand` is expected until Task 2 wires it; it clears in Task 2).
Run: `cargo clippy --all-targets 2>&1 | tail -3` → no errors.

- [ ] **Step 5: Commit**

```bash
git add src/glob_match.rs
git commit -m "v91 task 1: extglob pathname directory walker (extglob_pathname_expand)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Wire the walker into pathname expansion

**Files:**
- Modify: `src/expand.rs` (`GlobOpts` struct ~line 10; `build_glob_pattern` ~1125; `glob_expand_fields_opts` ~1048-1110)
- Modify: `src/shell_state.rs` (`Shell::glob_opts`)
- Create: `tests/extglob_pathname_integration.rs`
- Test: integration

- [ ] **Step 1: Write failing integration tests**

Create `tests/extglob_pathname_integration.rs` (spawn huck with `current_dir` on a temp fixture — per-process cwd is safe):

```rust
//! Integration tests for v91 extglob pathname globbing (M-84a).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Builds a fresh temp fixture dir, runs `script` in it (cwd = fixture),
/// returns (stdout, exit_code).
fn run_in_fixture(script: &str) -> (String, i32) {
    let dir = std::env::temp_dir().join(format!(
        "huck_egpath_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["a", "b", "ab", "aab", "abc", "cd", "xy", ".hidden"] {
        std::fs::write(dir.join(f), b"").unwrap();
    }
    std::fs::create_dir(dir.join("dir1")).unwrap();
    std::fs::write(dir.join("dir1/foo.txt"), b"").unwrap();
    std::fs::write(dir.join("dir1/bar.log"), b"").unwrap();
    std::fs::create_dir(dir.join("dir2")).unwrap();
    std::fs::write(dir.join("dir2/foo.txt"), b"").unwrap();
    let mut child = Command::new(huck_bin())
        .current_dir(&dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn echo_extglob_expands_sorted() {
    assert_eq!(run_in_fixture("shopt -s extglob\necho +(a|b)\n").0, "a aab ab b\n");
    assert_eq!(run_in_fixture("shopt -s extglob\necho @(a|cd)\n").0, "a cd\n");
    assert_eq!(run_in_fixture("shopt -s extglob\necho dir*/+(foo|bar).txt\n").0, "dir1/foo.txt dir2/foo.txt\n");
}

#[test]
fn for_loop_over_extglob() {
    assert_eq!(
        run_in_fixture("shopt -s extglob\nfor f in @(a|cd); do printf '%s|' \"$f\"; done\necho\n").0,
        "a|cd|\n"
    );
}

#[test]
fn extglob_off_is_literal() {
    // extglob off: +(a|b) is not special → no expansion (one literal word).
    assert_eq!(run_in_fixture("echo +(a|b)\n").0, "+(a|b)\n");
}

#[test]
fn quoted_extglob_is_literal() {
    // A quoted group is literal, never filesystem-expanded.
    assert_eq!(run_in_fixture("shopt -s extglob\necho \"+(a|b)\"\n").0, "+(a|b)\n");
}

#[test]
fn nullglob_extglob_no_match_empty() {
    assert_eq!(run_in_fixture("shopt -s extglob nullglob\necho zzz+(q)\n").0, "\n");
}

#[test]
fn no_match_is_literal() {
    assert_eq!(run_in_fixture("shopt -s extglob\necho zzz+(q)\n").0, "zzz+(q)\n");
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test --test extglob_pathname_integration 2>&1 | tail -15` → fails (extglob fields pass through literally; `GlobOpts.extglob` doesn't exist yet).

- [ ] **Step 3: Add `GlobOpts.extglob` + populate it**

In `src/expand.rs`, add the field to `GlobOpts`:

```rust
pub struct GlobOpts {
    pub nullglob: bool,
    pub dotglob: bool,
    pub nocaseglob: bool,
    pub failglob: bool,
    pub extglob: bool,
}
```

In `src/shell_state.rs` `Shell::glob_opts()`, add the `extglob` line:

```rust
    pub fn glob_opts(&self) -> crate::expand::GlobOpts {
        crate::expand::GlobOpts {
            nullglob: self.shopt_options.get("nullglob").unwrap_or(false),
            dotglob: self.shopt_options.get("dotglob").unwrap_or(false),
            nocaseglob: self.shopt_options.get("nocaseglob").unwrap_or(false),
            failglob: self.shopt_options.get("failglob").unwrap_or(false),
            extglob: self.shopt_options.get("extglob").unwrap_or(false),
        }
    }
```

> If any OTHER `GlobOpts { … }` literal exists (grep `GlobOpts {`), add `extglob: false` to it. `GlobOpts::default()` (derived) covers test sites that use `..Default::default()`; explicit literals need the field. `cargo build` will flag any missed one.

- [ ] **Step 4: Extend `build_glob_pattern` to escape quoted `| ( )`**

In `src/expand.rs` `build_glob_pattern`, add `| ( )` to the quoted-escape set so a quoted `"+(a)"` stays literal (not seen as an extglob group):

```rust
fn build_glob_pattern(field: &Field) -> String {
    let mut p = String::new();
    for (c, &q) in field.chars.chars().zip(field.quoted.iter()) {
        if q && matches!(c, '*' | '?' | '[' | ']' | '|' | '(' | ')') {
            p.push('[');
            p.push(c);
            p.push(']');
        } else {
            p.push(c);
        }
    }
    p
}
```

(`[|]`/`[(]`/`[)]` are single-char classes — literal-equivalent in both the `glob` crate and the walker. `glob::Pattern::escape`-style; no regression for non-extglob since a quoted bare `|`/`(`/`)` was already a literal in the glob crate.)

- [ ] **Step 5: Dispatch extglob fields to the walker in `glob_expand_fields_opts`**

Rewrite the per-field loop body of `glob_expand_fields_opts` so an extglob field routes to the walker and shares the existing no-match handling. Replace the loop body (the `if !has_unquoted_metachar … through the `glob_with` match) with:

```rust
    for field in fields {
        let pattern = build_glob_pattern(&field);
        let is_extglob = opts.extglob && crate::glob_match::has_extglob(&pattern);

        // No globbing needed: not a wildcard field AND not an extglob field.
        if !has_unquoted_metachar(&field) && !is_extglob {
            words.push(field.chars);
            continue;
        }

        let matched: Vec<String> = if is_extglob {
            crate::glob_match::extglob_pathname_expand(&pattern, opts.nocaseglob, opts.dotglob)
        } else {
            // Existing `glob` crate path (unchanged behavior for plain globs).
            let literal_leading_dot =
                pattern.starts_with('.') || pattern.starts_with("[.]");
            let match_opts = MatchOptions {
                case_sensitive: !opts.nocaseglob,
                require_literal_separator: true,
                require_literal_leading_dot: !literal_leading_dot && !opts.dotglob,
            };
            match glob_with(&pattern, match_opts) {
                Ok(paths) => {
                    let mut m = Vec::new();
                    for entry in paths {
                        let Ok(path) = entry else { continue };
                        match path.into_os_string().into_string() {
                            Ok(s) => m.push(s),
                            Err(_) => eprintln!("huck: skipping non-UTF8 path"),
                        }
                    }
                    m.retain(|p| {
                        let last = std::path::Path::new(p).file_name().and_then(|s| s.to_str());
                        !matches!(last, Some(".") | Some(".."))
                    });
                    m
                }
                Err(_) => {
                    // Invalid glob pattern → literal fallback (unchanged).
                    words.push(field.chars);
                    continue;
                }
            }
        };

        if matched.is_empty() {
            if opts.failglob {
                failglob_unmatched.push(field.chars);
            } else if opts.nullglob {
                // contribute nothing
            } else {
                words.push(field.chars);
            }
        } else {
            words.extend(matched);
        }
    }
```

> This preserves the `glob`-crate path exactly (same `MatchOptions`, same `.`/`..` retain filter, same invalid-pattern literal fallback) and unifies the no-match handling for both paths. Only the `is_extglob` branch is new.

- [ ] **Step 6: Run integration tests + bash parity**

Run: `cargo test --test extglob_pathname_integration 2>&1 | tail -15` → all pass.
bash parity (fixture dir): `cd $(mktemp -d) && touch a b ab aab abc cd xy && mkdir dir1 dir2 && touch dir1/foo.txt dir2/foo.txt` then for `echo +(a|b)`, `echo @(a|cd)`, `echo dir*/+(foo|bar).txt`, `echo !(a|ab)`: `diff <(printf 'shopt -s extglob\n%s\n' "$f" | bash) <(printf 'shopt -s extglob\n%s\n' "$f" | HUCK)` → empty.
Regression — non-extglob globbing unchanged: `echo *.txt` style, and `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Clippy: `cargo clippy --all-targets 2>&1 | tail -3` → clean (dead-code warning from Task 1 now gone).

- [ ] **Step 7: Commit**

```bash
git add src/expand.rs src/shell_state.rs tests/extglob_pathname_integration.rs
git commit -m "v91 task 2: dispatch extglob fields to the pathname walker

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/extglob_pathname_diff_check.sh` (huck's 18th harness)
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/extglob_pathname_diff_check.sh`, modeled on `tests/scripts/extglob_diff_check.sh`. `chmod +x`. A `mktemp -d` fixture; each fragment `cd "$FIX"; shopt -s extglob; echo <pattern>`.

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v91: extglob pathname globbing (M-84a).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
( cd "$FIX"; touch a b ab aab abc cd xy .hidden .ab; mkdir dir1 dir2
  touch dir1/foo.txt dir1/bar.log dir2/foo.txt )
check() {
    local label="$1" frag="$2" b h
    b=$(printf 'cd %q\nshopt -s extglob\n%s\n' "$FIX" "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf 'cd %q\nshopt -s extglob\n%s\n' "$FIX" "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# NOTE: pathname extglob output is sorted + deterministic, so byte-diffable.
# `**` globstar is a separate (unsupported) shopt and is not exercised.
check "plus"        'echo +(a|b)'
check "at"          'echo @(a|cd)'
check "star"        'echo *(a)'
check "negation"    'echo !(a|ab)'
check "class"       'echo +([a-c])'
check "explicit dot" 'echo .+(ab)'
check "multi-comp"  'echo dir*/+(foo|bar).txt'
check "dirs"        'echo @(dir1|dir2)'
check "no-match lit" 'echo zzz+(q)'
check "compose star" 'echo +(a|b)*'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness, confirm all PASS**

```bash
cd /home/john/projects/shuck
cargo build 2>&1 | tail -1
chmod +x tests/scripts/extglob_pathname_diff_check.sh
bash tests/scripts/extglob_pathname_diff_check.sh; echo "rc=$?"
```
Expected: `Fail: 0`, `rc=0`. If a fragment differs in ORDER, the walker's final `out.sort()` is missing/wrong. If a dotfile leaks/missing, the dotfile rule needs review. Do NOT weaken `check()`; fix the walker. Report any fragment relocated.

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Read the M-84/M-84a entries first.

1. Flip **M-84a** from `[deferred]` to `[fixed v91]`, rewording to: extglob now works in pathname/filesystem globbing via a custom recursive directory walker (`extglob_pathname_expand` in `src/glob_match.rs`) that reuses `extglob_match` per path component (the `glob` crate can't do extglob); dispatched from `glob_expand_fields_opts` only when extglob is on AND the field has an unquoted extglob op (all other globbing keeps the `glob` crate); honors the dotfile rule + `nocaseglob`/`dotglob`/`nullglob`/`failglob`; sorted output. Note `**` globstar remains unsupported (separate shopt).
2. In the **M-84** entry, update the "**Pathname … is OUT**" sentence to "pathname globbing shipped in v91 (M-84a)".
3. Update the Tier-2 count line (~25): append `; M-84a fixed by v91`. (M-84a was already counted when added in v90, so do NOT increment the count — just append the note. Verify against how M-84a is currently listed.)
4. Update the "Last updated" stamp (line 3) to `2026-06-05 (after v91 extglob pathname globbing; M-84a fixed)`.
5. Add a changelog entry at the END (match the v90 entry's format), dated 2026-06-05: the walker (split-on-`/`, per-component `extglob_match`, dotfile/sort/separator rules), `GlobOpts.extglob` + `Shell::glob_opts`, the dispatch in `glob_expand_fields_opts`, the `build_glob_pattern` quoted-`|()` escape, and the 18th harness. Note extglob is now complete (string + pathname).

- [ ] **Step 4: Update `README.md`**

Read the iteration table + v90 row first. Add a v91 row (2-column, escape literal `|` as `\|`):
```markdown
| v91 | extglob pathname globbing (M-84a) | `+(a\|b)` etc. now filesystem-expand via a custom directory walker; completes extglob (string + pathname) |
```

- [ ] **Step 5: Verify whole branch**

```bash
cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'   # FAIL=0
cargo clippy --all-targets 2>&1 | tail -3                                                       # clean
for f in tests/scripts/*_diff_check.sh; do printf '%s: ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo FAIL; done  # all 18 OK
```

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/extglob_pathname_diff_check.sh docs/bash-divergences.md README.md
git commit -m "v91 task 3: extglob pathname bash-diff harness + docs (M-84a)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **The walker is invoked only for extglob fields** (`opts.extglob && has_extglob(pattern)`); every other field keeps the `glob` crate path **byte-for-byte** — confirm existing pathname-glob tests stay green.
- **cwd safety**: unit tests use ABSOLUTE patterns (walker starts at `/`, never touches cwd — safe under parallel tests). Integration tests set `current_dir` on the spawned process. NEVER `std::env::set_current_dir` in a unit test.
- **Sorting**: `extglob_pathname_expand` sorts the final result list — `read_dir` order is arbitrary, so the sort is what makes output match bash. Don't drop it.
- **Dotfile rule**: a leading-`.` entry matches only if `dotglob` is on OR the component starts with a literal `.`. `.`/`..` always excluded.
- **Per-component matching reuses `extglob_match`**, which also implements `*`/`?`/`[…]`, so a mixed component (`+(a|b)*`) or a plain glob component inside an extglob pattern (`dir*/…`) is handled by the same walker.
- **Default-off / non-extglob ⇒ zero change.** Don't alter the `glob`-crate branch's `MatchOptions` or the `.`/`..` retain filter.
