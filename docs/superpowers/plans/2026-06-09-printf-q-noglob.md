# v120 — `printf %q` + `set -f`/`noglob` (M-73 / M-08) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `printf %q` (shell-quote conversion) and `set -f`/`set -o noglob` (disable pathname expansion), clearing the two errors mise's `_mise` completion handler hits.

**Architecture:** `%q` adds `ConvChar::Q` + a `printf_q` helper (backslash-escape style; `$'…'` for control chars; `''` for empty), reusing a refactored `ansi_c_quote` extracted from `@Q`'s `shell_quote`. `noglob` wires a real `ShellOptions.noglob` toggle (`set -f`/`+f`/`-o noglob`/`+o noglob`, reflected in `$-`) and short-circuits the single pathname-glob site (`glob_expand_fields_opts`) to literal — pathname-only (case/`[[`/`${//}` unaffected).

**Tech Stack:** Rust. `src/param_expansion.rs`, `src/builtins.rs`, `src/shell_state.rs`, `src/expand.rs`. Tests: `cargo test`, a new integration test, a new `tests/scripts/*_diff_check.sh` harness.

**Spec:** `docs/superpowers/specs/2026-06-09-printf-q-noglob-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `shell_quote` (`src/param_expansion.rs:270`) — the `@Q` impl; its `is_control()` branch (`:271-292`) is the `$'…'` encoder to extract.
- `enum ConvChar` (`src/builtins.rs:2254`); the conv match (`:2424-2433`, `b'q'` goes here); `format_one` (`:2518`, `ConvChar::S` arm at `:2589` is the model); `has_consuming_conv` (`:2732`, `!matches!(c.conv, ConvChar::Percent)` — `Q` counts automatically, no change).
- `ShellOptions` (`src/shell_state.rs:107`); `dollar_dash_value` (`:418`); `glob_opts()` (`:~430`).
- `option_get` (`builtins.rs:4277`); `option_set` (`:4289`); `set -` short-flag loop (`other => not supported`, `~:4419`); `set +` short-flag loop (`:4432`).
- `GlobOpts` (`src/expand.rs:10`); `glob_expand_fields_opts` per-field loop (`:1407`).

**Verified bash contract (probed):** `%q`: `plain`→`plain`, `a b`→`a\ b`, `c'd`→`c\'d`, `a$b`→`a\$b`, `*`→`\*`, ``→`''`, tab→`$'tab\tx'`, `ünï`→`ünï`, `%6q` of `a b`→`  a\ b`, `%q one two three` cycles. noglob: `set -f; echo *.txt`→`*.txt` literal (even with a match), `set +f` restores, `set -euf; echo $-`→`efhuB`, `set -f` leaves case/`${//}`/`[[` matching active.

---

## Task 1: `printf %q`

**Files:**
- Modify: `src/param_expansion.rs` (extract `ansi_c_quote`)
- Modify: `src/builtins.rs` (`ConvChar::Q` + `printf_q` + `format_one` arm)
- Create: `tests/printf_q_integration.rs`

- [ ] **Step 1: Write failing unit + integration tests**

In `src/builtins.rs`'s test module, add:
```rust
    #[test]
    fn printf_q_quoting() {
        assert_eq!(printf_q("plain"), "plain");
        assert_eq!(printf_q("a b"), "a\\ b");
        assert_eq!(printf_q("c'd"), "c\\'d");
        assert_eq!(printf_q("a$b"), "a\\$b");
        assert_eq!(printf_q("x\"y"), "x\\\"y");
        assert_eq!(printf_q("*"), "\\*");
        assert_eq!(printf_q(""), "''");
        assert_eq!(printf_q("p/q-r.s"), "p/q-r.s"); // /,-,. not escaped
        assert_eq!(printf_q("a\tb"), "$'a\\tb'");    // control -> $'...'
        assert_eq!(printf_q("ünï"), "ünï");          // UTF-8 as-is
    }
```
Create `tests/printf_q_integration.rs` (file-arg `run` helper, pid+atomic-counter temp path; verify EACH expected value against real bash first):
```rust
//! v120: printf %q (M-73).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v120q_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test]
fn q_simple() {
    assert_eq!(run("printf '%q\\n' 'a b'\n"), "a\\ b\n");
    assert_eq!(run("printf '%q\\n' plain\n"), "plain\n");
    assert_eq!(run("printf '%q\\n' \"c'd\"\n"), "c\\'d\n");
    assert_eq!(run("printf '[%q]\\n' ''\n"), "['']\n");
}
#[test]
fn q_control_and_cycle() {
    assert_eq!(run("printf '%q\\n' \"$(printf 'a\\tb')\"\n"), "$'a\\tb'\n");
    assert_eq!(run("printf '%q\\n' one two three\n"), "one\ntwo\nthree\n");
}
#[test]
fn q_width_and_capture() {
    assert_eq!(run("printf '[%6q]\\n' 'a b'\n"), "[  a\\ b]\n");
    assert_eq!(run("printf -v x '%q' 'a b'\necho \"$x\"\n"), "a\\ b\n");
}
```

- [ ] **Step 2: Run — confirm fail**

Run: `cargo build --bin huck 2>&1 | tail -5` (will fail: `printf_q` undefined). Then after a stub, `cargo test --test printf_q_integration 2>&1 | tail` shows `%q: invalid directive`.

- [ ] **Step 3: Extract `ansi_c_quote` from `shell_quote`**

In `src/param_expansion.rs`, replace `shell_quote` (`:270-296`) with:
```rust
/// bash `${v@Q}`: shell-quote `v` so the result re-reads as the same value.
/// Control chars use the `$'…'` ANSI-C form; empty/ordinary strings use single
/// quotes with `'` rewritten as `'\''`.
fn shell_quote(v: &str) -> String {
    if v.chars().any(|c| c.is_control()) {
        ansi_c_quote(v)
    } else {
        format!("'{}'", crate::builtins::escape_alias_value(v))
    }
}

/// ANSI-C `$'…'` quoting of `v` (escaping `\`, `'`, and control chars). Shared
/// by `${v@Q}` (control-char branch) and `printf %q`.
pub(crate) fn ansi_c_quote(v: &str) -> String {
    let mut out = String::from("$'");
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\x0B' => out.push_str("\\v"),
            '\x0C' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\x1B' => out.push_str("\\E"),
            c if (c as u32) < 0x20 || c == '\x7F' => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}
```
(Behavior of `@Q` is unchanged — the encoder is byte-identical, just relocated.)

- [ ] **Step 4: Add `ConvChar::Q`, the conv match arm, `printf_q`, and the `format_one` arm**

In `src/builtins.rs`:
- `enum ConvChar` (`:2254`): add `Q,` (e.g. after `B`).
- conv match (`:2433`, before `b'%' => ConvChar::Percent`): add `b'q' => ConvChar::Q,`.
- Add the helper (near `format_one`):
```rust
/// bash `printf %q`: quote `arg` so it re-reads as the same word. Empty → `''`;
/// a control char → the `$'…'` ANSI-C form; otherwise backslash-escape each
/// shell-special char (SPACE plus `!"#$&'()*,;<>?[\]^`{|}~`). Letters, digits,
/// and `%+-./:=@_` and printable UTF-8 are emitted as-is.
fn printf_q(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg.chars().any(|c| c.is_control()) {
        return crate::param_expansion::ansi_c_quote(arg);
    }
    const SPECIAL: &str = " !\"#$&'()*,;<>?[\\]^`{|}~";
    let mut out = String::with_capacity(arg.len());
    for c in arg.chars() {
        if SPECIAL.contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
```
- In `format_one` (`:2589` region), add an arm (alongside `ConvChar::S`):
```rust
        ConvChar::Q => {
            out.extend_from_slice(&pad_string(printf_q(arg).as_bytes(), spec));
            Ok(true)
        }
```
(`has_consuming_conv` already counts `Q` since it's `!= Percent` — no change there.)

- [ ] **Step 5: Run — confirm green**

Run: `cargo build --bin huck && cargo test --bin huck printf_q 2>&1 | tail -5` (unit passes) and `cargo test --test printf_q_integration 2>&1 | tail -8` (all pass). Run the existing `@Q` tests to confirm the refactor: `cargo test param 2>&1 | grep -E "test result|FAILED" | tail -3` (no regression).

- [ ] **Step 6: Spot check vs bash + clippy**

```bash
cargo build --bin huck
for f in "printf '%q\n' 'a b'" "printf '%q\n' \"c'd\"" "printf '%q\n' 'a\$b'" "printf '[%q]\n' ''" "printf '%q\n' one two" "printf '%q\n' p/q-r.s"; do
  printf '%s\n' "$f" > /tmp/t.sh
  b=$(bash --norc --noprofile /tmp/t.sh 2>&1); h=$(./target/debug/huck /tmp/t.sh 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b] h=[$h]"; }
done
cargo clippy --bin huck 2>&1 | tail -3
```
Expected: all MATCH; clippy clean.

- [ ] **Step 7: Commit**

```bash
git add src/param_expansion.rs src/builtins.rs tests/printf_q_integration.rs
git commit -m "$(cat <<'EOF'
feat: printf %q shell-quote conversion (M-73)

%q quotes its arg so it re-reads as the same word: empty -> '', control char ->
$'...' (ANSI-C, via a refactored shared ansi_c_quote extracted from @Q),
otherwise backslash-escape the shell-special set (SPACE + !"#$&'()*,;<>?[\]^`{|}~).
Backslash style (a\ b) matching bash %q (distinct from @Q's single-quote style).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, `printf_q` + the `ansi_c_quote` extraction, the unit + integration pass lines, the `@Q` no-regression line, the MATCH spot-check, clippy status.

---

## Task 2: `set -f` / `set -o noglob`

**Files:**
- Modify: `src/shell_state.rs` (`ShellOptions.noglob`, `dollar_dash_value`, `glob_opts`)
- Modify: `src/builtins.rs` (`option_get`/`option_set`/short-flag loops)
- Modify: `src/expand.rs` (`GlobOpts.noglob` + the gate)
- Create: `tests/noglob_integration.rs`

- [ ] **Step 1: Write failing integration tests**

Create `tests/noglob_integration.rs` (file-arg `run` helper as in Task 1; verify vs bash first):
```rust
//! v120: set -f / set -o noglob (M-08).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v120g_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test]
fn noglob_makes_star_literal() {
    let s = "d=$(mktemp -d); touch \"$d\"/x.txt\ncd \"$d\"\nset -f; echo *.txt\nset +f; echo *.txt\nrm -rf \"$d\"\n";
    assert_eq!(run(s), "*.txt\nx.txt\n");
}
#[test]
fn noglob_via_long_form() {
    let s = "d=$(mktemp -d); touch \"$d\"/x.txt\ncd \"$d\"\nset -o noglob; echo *.txt\nset +o noglob; echo *.txt\nrm -rf \"$d\"\n";
    assert_eq!(run(s), "*.txt\nx.txt\n");
}
#[test]
fn noglob_in_dollar_dash_and_minus_o() {
    assert_eq!(run("set -f\n[[ -o noglob ]] && echo ON || echo OFF\ncase \"$-\" in *f*) echo HASF;; *) echo no;; esac\n"), "ON\nHASF\n");
}
#[test]
fn noglob_is_pathname_only() {
    // case / ${//} / [[ == ]] still match under set -f.
    assert_eq!(run("set -f\ncase abc in a*) echo CY;; esac\ns=a1b; echo \"${s//[0-9]/_}\"\n[[ x == ? ]] && echo BY\n"), "CY\na_b\nBY\n");
}
```

- [ ] **Step 2: Run — confirm fail**

Run: `cargo build --bin huck && cargo test --test noglob_integration 2>&1 | tail -12`
Expected: failures (`set: noglob: not yet supported` / `set: -f: not yet supported`).

- [ ] **Step 3: Add `ShellOptions.noglob`**

In `src/shell_state.rs:107`, add the field to `ShellOptions`:
```rust
pub struct ShellOptions {
    pub errexit: bool,
    pub nounset: bool,
    pub pipefail: bool,
    pub verbose: bool,
    pub xtrace: bool,
    pub noglob: bool,
}
```
If `ShellOptions` is constructed by a struct literal anywhere (not `#[derive(Default)]`), add `noglob: false` there — `cargo build` will flag any missing-field literal.

- [ ] **Step 4: `dollar_dash_value` + `glob_opts`**

In `dollar_dash_value` (`:418`), add `f` after `e` (bash order `efhuB`):
```rust
        if self.shell_options.errexit { out.push('e'); }
        if self.shell_options.noglob { out.push('f'); }
        if self.is_interactive { out.push('i'); }
        if self.shell_options.nounset { out.push('u'); }
        if self.shell_options.verbose { out.push('v'); }
        if self.shell_options.xtrace { out.push('x'); }
```
In `glob_opts()` (`:~430`), add `noglob`:
```rust
    pub fn glob_opts(&self) -> crate::expand::GlobOpts {
        crate::expand::GlobOpts {
            nullglob: self.shopt_options.get("nullglob").unwrap_or(false),
            dotglob: self.shopt_options.get("dotglob").unwrap_or(false),
            nocaseglob: self.shopt_options.get("nocaseglob").unwrap_or(false),
            failglob: self.shopt_options.get("failglob").unwrap_or(false),
            extglob: self.shopt_options.get("extglob").unwrap_or(false),
            noglob: self.shell_options.noglob,
        }
    }
```

- [ ] **Step 5: `option_get` / `option_set` / short flags**

In `src/builtins.rs`:
- `option_get` (`:4277`): add `"noglob" => Some(shell.shell_options.noglob),` (before the `other =>` arm).
- `option_set` (`:4289`): add `"noglob" => { shell.shell_options.noglob = value; Ok(()) }` (before the `other =>` arm).
- `set -` short-flag loop: the `other => not supported` arm (`~:4419`) — add `b'f' => shell.shell_options.noglob = true,` before it.
- `set +` short-flag loop (`:4432`): add `b'f' => shell.shell_options.noglob = false,` before its `other =>` arm.

- [ ] **Step 6: `GlobOpts.noglob` + the gate**

In `src/expand.rs:10`, add to `GlobOpts`:
```rust
pub struct GlobOpts {
    pub nullglob: bool,
    pub dotglob: bool,
    pub nocaseglob: bool,
    pub failglob: bool,
    pub extglob: bool,
    pub noglob: bool,
}
```
In `glob_expand_fields_opts` (`:1407`), at the TOP of the `for field in fields` loop, before `build_glob_pattern`, short-circuit when noglob:
```rust
    for field in fields {
        if opts.noglob {
            words.push(field.chars);
            continue;
        }
        let pattern = build_glob_pattern(&field);
        ...
```
(This makes `*`/`?`/`[`/extglob/POSIX-class all literal under `set -f`. Check for OTHER constructors of `GlobOpts` — e.g. `GlobOpts::default()` or test literals — and add `noglob: false`; `cargo build` flags any missing-field literal.)

- [ ] **Step 7: Run — confirm green + regression + clippy**

```bash
cargo build --bin huck && cargo test --test noglob_integration 2>&1 | tail -8
cargo test 2>&1 | grep -E "test result: FAILED" || echo "no failures"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: all noglob tests pass; no FAILED; clippy clean. Watch `glob`/`expand`/`set`/`dbracket`/`case` suites.

- [ ] **Step 8: Spot check vs bash**

```bash
cargo build --bin huck
for f in 'set -euf; echo "$-"' \
         'set -f; echo *.nomatch_xyz' \
         'set -o noglob; [[ -o noglob ]] && echo ON; set +o noglob; [[ -o noglob ]] && echo ON || echo OFF' \
         'set -f; case abc in a*) echo CY;; esac; s=a1b; echo "${s//[0-9]/_}"'; do
  printf '%s\n' "$f" > /tmp/t.sh
  b=$(bash --norc --noprofile /tmp/t.sh 2>&1); h=$(./target/debug/huck /tmp/t.sh 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b] h=[$h]"; }
done
```
Expected: all MATCH (note `set -euf; echo $-` → `efhuB` in bash; huck's order is `e f i u v x` so a non-interactive `set -euf` → `efu` — VERIFY huck matches bash for the flags huck tracks; the `h`/`B` bash adds are inert-default flags huck may not list. If huck's `$-` omits `h`/`B` that's a PRE-EXISTING divergence, not v120's — confirm `set -euf` gives huck `efu` and bash `efhuB`, and if so the `echo $-` fragment will DIFF on `h`/`B`; in that case drop that fragment from the spot-check and rely on the `case *f*` membership test instead, which is what the integration test uses).

- [ ] **Step 9: Commit**

```bash
git add src/shell_state.rs src/builtins.rs src/expand.rs tests/noglob_integration.rs
git commit -m "$(cat <<'EOF'
feat: set -f / set -o noglob disables pathname expansion (M-08)

New ShellOptions.noglob, toggled by set -f/+f and set -o/+o noglob, reflected
in $- (f, after e) and [[ -o noglob ]]. glob_expand_fields_opts short-circuits
to the literal field when noglob, so */?/[ (and extglob/POSIX classes) stay
literal. Pathname-only: case / [[ == ]] / ${//} matching are unaffected.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the wiring edits (ShellOptions/dollar_dash/glob_opts/option_set/option_get/short-flags/GlobOpts/gate), the noglob-test pass line, the MATCH spot-check (and the `$-` `h`/`B` note if relevant), full-suite green, clippy status.

---

## Task 3: 44th harness + payoff + docs

**Files:**
- Create: `tests/scripts/printf_q_noglob_diff_check.sh`
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/printf_q_noglob_diff_check.sh` (file-arg execution per L-27):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v120: printf %q (M-73) + set -f/noglob
# (M-08). File-arg execution (L-27: huck history-expands piped stdin).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "q plain"      'printf "%q\n" plain'
check "q space"      'printf "%q\n" "a b"'
check "q squote"     'printf "%q\n" "c'"'"'d"'
check "q dollar"     'printf "%q\n" '"'"'a$b'"'"''
check "q glob"       'printf "%q\n" "*" "?" "[a]"'
check "q empty"      'printf "[%q]\n" ""'
check "q safe"       'printf "%q\n" p/q-r.s_t=u@v'
check "q control"    'printf "%q\n" "$(printf '"'"'a\tb'"'"')"'
check "q cycle"      'printf "%q\n" one two three'
check "q width"      'printf "[%6q]\n" "a b"'
check "q capture"    'printf -v x "%q" "a b"; echo "$x"'
check "noglob -f"    'd=$(mktemp -d); touch "$d"/x.txt; cd "$d"; set -f; echo *.txt; set +f; echo *.txt; rm -rf "$d"'
check "noglob -o"    'd=$(mktemp -d); touch "$d"/y.md; cd "$d"; set -o noglob; echo *.md; set +o noglob; echo *.md; rm -rf "$d"'
check "noglob -o opt" 'set -f; [[ -o noglob ]] && echo ON || echo OFF; set +f; [[ -o noglob ]] && echo ON || echo OFF'
check "noglob hasf"  'set -f; case "$-" in *f*) echo HASF;; *) echo no;; esac'
check "noglob pathonly" 'set -f; case abc in a*) echo CY;; esac; s=a1b; echo "${s//[0-9]/_}"; [[ x == ? ]] && echo BY'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
(If a fragment's quoting is awkward through the harness, the implementer may simplify it but MUST keep `%q` coverage [plain/space/quote/`$`/glob/empty/safe/control/cycle/width/capture] and noglob coverage [`-f`/`-o`/`[[ -o ]]`/`$-`/pathname-only]. Verify each is byte-identical.)

- [ ] **Step 2: Make executable, run it, run ALL harnesses**

```bash
chmod +x tests/scripts/printf_q_noglob_diff_check.sh && cargo build --bin huck
bash tests/scripts/printf_q_noglob_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo all-harnesses-done
```
Expected: `Total: 16, Pass: 16, Fail: 0`; `count: 44`; no `FAIL` lines. If a fragment FAILs, report the diff (do NOT change source).

- [ ] **Step 3: Payoff check (the two `_mise` errors cleared)**

```bash
cargo build --bin huck
printf 'printf "%%q\\n" "a task" plain\nset -o noglob; echo "noglob-ok"; set +o noglob\nset -f; echo "f-ok"; set +f\n' > /tmp/v120_smoke.sh
echo "--- bash ---"; bash --norc --noprofile /tmp/v120_smoke.sh 2>&1
echo "--- huck ---"; ./target/debug/huck /tmp/v120_smoke.sh 2>&1
```
Expected: NO `printf: \`%q': invalid directive` and NO `set: noglob: not yet supported` / `set: -f: not yet supported` in huck's output; both shells byte-identical. Report both blocks.

- [ ] **Step 4: Docs — drop %q + noglob from deferred**

In `docs/bash-divergences.md`:
- M-73 entry: in the "Deferred: floating point (`%f`/…), `%q` (shell-quote), `%(...)T`, …" sentence, remove `%q` (shell-quote) and add a clause `\`%q\` (shell-quote) fixed v120.`
- M-08 entry: in the "Still deferred: `-n` (noexec), `-f` (noglob), `-a` (allexport), …" sentence, remove `-f` (noglob) from the deferred list and add `\`-f\`/\`noglob\` (disable pathname expansion) shipped v120.`
- "Last updated" line → v120 (printf `%q` + `set -f`/noglob — the two mise `_mise`-handler gaps).
- Change-log: append a `2026-06-09` v120 entry covering both features + the payoff (the two `_mise` errors cleared; full `mise<TAB>` still needs the `mise` binary).
- README: add a v120 row after v119. Use the real test count: `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.

- [ ] **Step 5: Verify + commit**

```bash
grep -n 'v120\|%q.*v120\|noglob.*v120' docs/bash-divergences.md README.md | head
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v120 — printf %q + set -f/noglob (M-73 / M-08 sub-features)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the `Total: 16, Pass: 16` + `count: 44` lines, the payoff output (both errors gone), the docs greps, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 44 files).
- [ ] **Payoff**: the two `_mise`-handler errors (`%q` invalid directive, `set: noglob` unsupported) are gone (Task 3 Step 3).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`; MEMORY.md is near its cap — compress older entries while updating). **Tell the user to re-test `mise<TAB>` live (these clear two `_mise` errors; the next gap, if any, surfaces there).**
