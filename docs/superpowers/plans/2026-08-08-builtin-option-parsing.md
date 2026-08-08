# v359 — one option parser for the builtins — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every builtin that parses options routes through one `internal_getopt`-shaped scanner that owns bundling, `--`, lone `-`, attached values and stop-at-first-non-option, plus a usage table keyed on the invoked name — closing [#496](https://github.com/jdstanhope/huck/issues/496).

**Architecture:** A new `builtin_opts` module yields options one at a time; each builtin keeps its own `match` on the option character and loses only its hand-rolled scanner. The failure path emits both bash lines and reuses v358's `SpecialBuiltinUsage` classifier, so posix fatality is inherited rather than re-derived.

**Tech Stack:** Rust 2024, `huck-engine` crate. Verified by a new bash-diff harness plus the existing 274-harness sweep.

**Spec:** `docs/superpowers/specs/2026-08-08-builtin-option-parsing-design.md`

## Global Constraints

- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`
- **Formatting:** `cargo fmt --all` before every commit; CI enforces `--check`.
- **Linting:** `cargo +1.97.1 clippy --workspace --all-targets -- -D warnings`. Use the **pinned** toolchain — a newer local stable MISSES warnings CI raises (#497).
- **Tests, per-crate only** — `cargo test --workspace` OOM-kills this 1-core/1.9GB box. Use `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1`.
- **Sweep on an IDLE box.** Two job-control harnesses fail non-deterministically under concurrent build load (#476); a red sweep run beside a running `cargo` proves nothing.
- **Branch:** `v359-builtin-opts`, off `main`. The PR is handed to the user — do NOT self-merge a `vNN` iteration.
- **Behaviour changes only where bash disagrees today.** No existing harness may need an expected-value edit.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/huck-engine/src/builtin_opts.rs` (create) | The scanner (`Getopt`, `Opt`, `ArgView`) and the usage table (`usage_for`). Both live here because the usage text has no caller but the scanner's failure path. |
| `crates/huck-engine/src/builtin_opts/tests.rs` (create) | Unit tests for the contract rows and the table's completeness. |
| `crates/huck-engine/src/lib.rs` (modify) | `pub(crate) mod builtin_opts;` |
| `crates/huck-engine/src/builtins.rs` (modify) | ~23 builtins lose their scanners. No split — see spec §5. |
| `tests/scripts/builtin_options_diff_check.sh` (create) | Differential matrix vs bash 5.2.21. |
| `docs/architecture.md` (modify) | Cheatsheet entry: how to add a builtin option. |

---

### Task 1: The scanner

**Files:**
- Create: `crates/huck-engine/src/builtin_opts.rs`
- Create: `crates/huck-engine/src/builtin_opts/tests.rs`
- Modify: `crates/huck-engine/src/lib.rs`

**Interfaces:**
- Produces: `Getopt::new(name: &str, args: ArgView<'a>, spec: &str)`, `Getopt::next_opt(&mut self, shell: &mut Shell, err: &mut dyn Write) -> Result<Option<Opt>, i32>`, `Getopt::rest_index(&self) -> usize`, `pub(crate) struct Opt { pub ch: char, pub value: Option<String> }`, `pub(crate) enum ArgView<'a> { Plain(&'a [String]), Decl(&'a [DeclArg]) }`.
- Consumes: `crate::error_fatality::ErrorKind::SpecialBuiltinUsage` (v358), `crate::sh_error_to!`.

`ArgView` exists because the four declaration builtins receive `&[DeclArg]`, not `&[String]`. A `DeclArg::Assign` can never be an option, so it terminates scanning exactly like a non-option string.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/huck-engine/src/builtin_opts/tests.rs
use super::*;
use crate::shell_state::Shell;

fn scan(spec: &str, argv: &[&str]) -> (Vec<(char, Option<String>)>, usize, Option<i32>) {
    let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut sh = Shell::new();
    let mut err: Vec<u8> = Vec::new();
    let mut g = Getopt::new("t", ArgView::Plain(&args), spec);
    let mut out = Vec::new();
    loop {
        match g.next_opt(&mut sh, &mut err) {
            Ok(Some(o)) => out.push((o.ch, o.value.clone())),
            Ok(None) => return (out, g.rest_index(), None),
            Err(code) => return (out, g.rest_index(), Some(code)),
        }
    }
}

#[test]
fn bundled_shorts_are_order_independent() {
    assert_eq!(scan("ap", &["-pa"]).0, vec![('p', None), ('a', None)]);
    assert_eq!(scan("ap", &["-ap"]).0, vec![('a', None), ('p', None)]);
}

#[test]
fn double_dash_terminates_and_is_consumed() {
    let (opts, rest, err) = scan("p", &["-p", "--", "-a"]);
    assert_eq!(opts, vec![('p', None)]);
    assert_eq!(rest, 2, "`--` is consumed; operands start after it");
    assert!(err.is_none());
}

#[test]
fn lone_dash_is_an_operand_not_an_option() {
    let (opts, rest, err) = scan("p", &["-"]);
    assert!(opts.is_empty());
    assert_eq!(rest, 0, "a lone `-` is left for the builtin as an operand");
    assert!(err.is_none());
}

#[test]
fn value_attaches_or_separates() {
    assert_eq!(scan("d:", &["-d", "x"]).0, vec![('d', Some("x".into()))]);
    assert_eq!(scan("d:", &["-dx"]).0, vec![('d', Some("x".into()))]);
}

#[test]
fn scanning_stops_at_the_first_non_option() {
    let (opts, rest, err) = scan("p", &["-p", "name", "-a"]);
    assert_eq!(opts, vec![('p', None)]);
    assert_eq!(rest, 1, "`-a` after an operand is an OPERAND, not an option");
    assert!(err.is_none());
}

#[test]
fn unknown_option_reports_two_and_stops() {
    let (_, _, err) = scan("p", &["-Q"]);
    assert_eq!(err, Some(2));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p huck-engine --lib --jobs 1 builtin_opts -- --test-threads 1`
Expected: FAIL — module `builtin_opts` does not exist.

- [ ] **Step 3: Implement the scanner**

```rust
// crates/huck-engine/src/builtin_opts.rs
//! One option scanner for the builtins, modelled on bash's `internal_getopt`.
//!
//! Owns the whole contract, measured against bash 5.2.21: bundled shorts
//! (order-independent), `--` as terminator, a lone `-` as an OPERAND, values
//! attached (`-n3`) or separate (`-n 3`), and scanning that STOPS at the first
//! non-option (POSIX — no permutation).
//!
//! Each builtin keeps its own `match` on the option character. What it loses is
//! the scanning, which is where 23 hand-rolled copies drifted apart (#496).

use crate::command::DeclArg;
use crate::shell_state::Shell;
use std::io::Write;

pub(crate) struct Opt {
    pub ch: char,
    pub value: Option<String>,
}

/// Declaration builtins receive `&[DeclArg]`; everything else `&[String]`. A
/// `DeclArg::Assign` can never be an option, so it terminates scanning exactly
/// like a non-option string does.
pub(crate) enum ArgView<'a> {
    Plain(&'a [String]),
    Decl(&'a [DeclArg]),
}

impl ArgView<'_> {
    fn len(&self) -> usize {
        match self {
            ArgView::Plain(v) => v.len(),
            ArgView::Decl(v) => v.len(),
        }
    }
    /// `None` when the slot cannot be an option (a compound assignment).
    fn at(&self, i: usize) -> Option<&str> {
        match self {
            ArgView::Plain(v) => v.get(i).map(|s| s.as_str()),
            ArgView::Decl(v) => match v.get(i) {
                Some(DeclArg::Plain(s)) => Some(s.as_str()),
                _ => None,
            },
        }
    }
}

pub(crate) struct Getopt<'a> {
    name: &'a str,
    args: ArgView<'a>,
    spec: &'a str,
    idx: usize,
    /// Byte offset within the current bundled cluster, 0 when not inside one.
    ch: usize,
    done: bool,
}

impl<'a> Getopt<'a> {
    pub fn new(name: &'a str, args: ArgView<'a>, spec: &'a str) -> Self {
        Self { name, args, spec, idx: 0, ch: 0, done: false }
    }

    /// Index of the first operand. Valid once `next_opt` has returned
    /// `Ok(None)` or `Err`.
    pub fn rest_index(&self) -> usize {
        self.idx
    }

    fn takes_value(&self, c: char) -> bool {
        let mut it = self.spec.chars().peekable();
        while let Some(sc) = it.next() {
            if sc == c {
                return it.peek() == Some(&':');
            }
        }
        false
    }

    fn accepts(&self, c: char) -> bool {
        self.spec.chars().any(|sc| sc == c)
    }

    pub fn next_opt(
        &mut self,
        shell: &mut Shell,
        err: &mut dyn Write,
    ) -> Result<Option<Opt>, i32> {
        if self.done {
            return Ok(None);
        }
        if self.ch == 0 {
            // Positioned at a fresh argument: decide whether it opens options.
            let Some(cur) = self.args.at(self.idx) else {
                self.done = true;
                return Ok(None);
            };
            if cur == "--" {
                self.idx += 1; // consumed
                self.done = true;
                return Ok(None);
            }
            // A lone "-" is an operand, and so is anything not starting with '-'.
            if !cur.starts_with('-') || cur == "-" {
                self.done = true;
                return Ok(None);
            }
            self.ch = 1;
        }

        let cur = self.args.at(self.idx).expect("cluster arg present");
        let bytes = cur.as_bytes();
        if self.ch >= bytes.len() {
            self.idx += 1;
            self.ch = 0;
            return self.next_opt(shell, err);
        }
        let c = bytes[self.ch] as char;
        self.ch += 1;

        if !self.accepts(c) {
            self.fail_invalid(c, shell, err);
            return Err(2);
        }

        if !self.takes_value(c) {
            if self.ch >= bytes.len() {
                self.idx += 1;
                self.ch = 0;
            }
            return Ok(Some(Opt { ch: c, value: None }));
        }

        // Value: the rest of this cluster, else the next argument.
        let rest = &cur[self.ch..];
        if !rest.is_empty() {
            let v = rest.to_string();
            self.idx += 1;
            self.ch = 0;
            return Ok(Some(Opt { ch: c, value: Some(v) }));
        }
        self.idx += 1;
        self.ch = 0;
        match self.args.at(self.idx) {
            Some(v) => {
                let v = v.to_string();
                self.idx += 1;
                Ok(Some(Opt { ch: c, value: Some(v) }))
            }
            None => {
                self.fail_missing_value(c, shell, err);
                Err(2)
            }
        }
    }

    fn fail_invalid(&self, c: char, shell: &mut Shell, err: &mut dyn Write) {
        crate::sh_error_to!(shell, err, None, "{}: -{c}: invalid option", self.name);
        let _ = writeln!(err, "{}: usage: {}", self.name, usage_for(self.name));
        shell.builtin_usage_error = Some(2);
        shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage { status: 2 });
    }

    fn fail_missing_value(&self, c: char, shell: &mut Shell, err: &mut dyn Write) {
        crate::sh_error_to!(shell, err, None, "{}: option requires an argument -- {c}", self.name);
        let _ = writeln!(err, "{}: usage: {}", self.name, usage_for(self.name));
        shell.builtin_usage_error = Some(2);
        shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage { status: 2 });
    }
}

#[cfg(test)]
#[path = "builtin_opts/tests.rs"]
mod tests;
```

Add to `crates/huck-engine/src/lib.rs`, alphabetically among the other module declarations:

```rust
pub(crate) mod builtin_opts;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p huck-engine --lib --jobs 1 builtin_opts -- --test-threads 1`
Expected: PASS, 6 tests. `usage_for` does not exist yet — Task 2 adds it; until then, stub it at the bottom of `builtin_opts.rs` as `fn usage_for(_: &str) -> &'static str { "" }` so this task compiles standalone, and delete the stub in Task 2.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/builtin_opts.rs crates/huck-engine/src/builtin_opts/tests.rs crates/huck-engine/src/lib.rs
git commit -m "v359: the builtin option scanner (#496)"
```

---

### Task 2: The usage table

**Files:**
- Modify: `crates/huck-engine/src/builtin_opts.rs`
- Modify: `crates/huck-engine/src/builtin_opts/tests.rs`

**Interfaces:**
- Produces: `pub(crate) fn usage_for(name: &str) -> &'static str`.

Keyed on the **invoked** name. That is the whole of the class-3 fix: `readarray` and `typeset` currently report themselves as `mapfile` and `declare` because the message is built from whichever function handles them.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn usage_is_keyed_on_the_invoked_name_not_the_implementation() {
    // readarray/mapfile and typeset/declare share an implementation but must
    // NOT share a usage string — bash names the builtin the user invoked.
    assert!(usage_for("readarray").starts_with("readarray "));
    assert!(usage_for("mapfile").starts_with("mapfile "));
    assert!(usage_for("typeset").starts_with("typeset "));
    assert!(usage_for("declare").starts_with("declare "));
    assert_ne!(usage_for("readarray"), usage_for("mapfile"));
    assert_ne!(usage_for("typeset"), usage_for("declare"));
}

#[test]
fn every_builtin_with_a_scanner_has_a_usage_string() {
    for name in ["unset", "readonly", "read", "type", "hash", "declare", "typeset",
                 "printf", "command", "mapfile", "readarray", "help", "complete",
                 "compgen", "compopt", "jobs", "trap", "alias", "unalias", "builtin",
                 "export", "cd", "wait", "history", "local", "getopts", "shopt",
                 "disown", "umask", "ulimit", "pwd", "enable"] {
        assert!(!usage_for(name).is_empty(), "no usage string for {name}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p huck-engine --lib --jobs 1 builtin_opts -- --test-threads 1`
Expected: FAIL — the Task 1 stub returns `""`.

- [ ] **Step 3: Implement the table**

Delete the Task 1 stub and add. **Every string is transcribed verbatim from bash 5.2.21** (`bash -c '<name> -Q'`, second line, with the leading `<name>: usage: ` stripped):

```rust
/// Usage text, keyed on the INVOKED name. Transcribed verbatim from bash
/// 5.2.21; the differential harness pins every one byte-for-byte, so a typo
/// here is a red test rather than a silent divergence.
pub(crate) fn usage_for(name: &str) -> &'static str {
    match name {
        "alias" => "alias [-p] [name[=value] ... ]",
        "builtin" => "builtin [shell-builtin [arg ...]]",
        "cd" => "cd [-L|[-P [-e]] [-@]] [dir]",
        "command" => "command [-pVv] command [arg ...]",
        "compgen" => "compgen [-abcdefgjksuv] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [word]",
        "complete" => "complete [-abcdefgjksuv] [-pr] [-DEI] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [name ...]",
        "compopt" => "compopt [-o|+o option] [-DEI] [name ...]",
        "declare" => "declare [-aAfFgiIlnrtux] [name[=value] ...] or declare -p [-aAfFilnrtux] [name ...]",
        "disown" => "disown [-h] [-ar] [jobspec ... | pid ...]",
        "enable" => "enable [-a] [-dnps] [-f filename] [name ...]",
        "export" => "export [-fn] [name[=value] ...] or export -p",
        "getopts" => "getopts optstring name [arg ...]",
        "hash" => "hash [-lr] [-p pathname] [-dt] [name ...]",
        "help" => "help [-dms] [pattern ...]",
        "history" => "history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]",
        "jobs" => "jobs [-lnprs] [jobspec ...] or jobs -x command [args]",
        "local" => "local [option] name[=value] ...",
        "mapfile" => "mapfile [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]",
        "printf" => "printf [-v var] format [arguments]",
        "pwd" => "pwd [-LP]",
        "read" => "read [-ers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] [-p prompt] [-t timeout] [-u fd] [name ...]",
        "readarray" => "readarray [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]",
        "readonly" => "readonly [-aAf] [name[=value] ...] or readonly -p",
        "shopt" => "shopt [-pqsu] [-o] [optname ...]",
        "trap" => "trap [-lp] [[arg] signal_spec ...]",
        "type" => "type [-afptP] name [name ...]",
        "typeset" => "typeset [-aAfFgiIlnrtux] name[=value] ... or typeset -p [-aAfFilnrtux] [name ...]",
        "ulimit" => "ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]",
        "umask" => "umask [-p] [-S] [mode]",
        "unalias" => "unalias [-a] name [name ...]",
        "unset" => "unset [-f] [-v] [-n] [name ...]",
        "wait" => "wait [-fn] [-p var] [id ...]",
        other => {
            debug_assert!(false, "no usage string for builtin {other}");
            ""
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p huck-engine --lib --jobs 1 builtin_opts -- --test-threads 1`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/builtin_opts.rs crates/huck-engine/src/builtin_opts/tests.rs
git commit -m "v359: usage table keyed on the invoked name (#496)"
```

---

### Task 3: The differential harness, committed RED

**Files:**
- Create: `tests/scripts/builtin_options_diff_check.sh`

Both shells are invoked with an **explicit `$0`** so the program-name prefix is identical and the comparison needs no normalisation — verified during the brainstorm. Do NOT add a `sed` normaliser; that would hide real prologue divergences.

- [ ] **Step 1: Write the harness**

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the one builtin option scanner (v359,
# #496). Both shells run with an EXPLICIT $0 ("huck5") so the error prologue
# matches and this is a plain byte comparison — no normalisation, which would
# also hide real prologue bugs.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$("$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# ── the four reported bugs (#496) ──
check "readonly -pa"      'readonly -pa >/dev/null'
check "wait -fn"          'wait -fn'
check "history -cd"       'history -cd 1'
check "unset -vf"         'unset -vf x'

# ── invalid option: message, usage line, status, for every in-scope builtin ──
for b in unset readonly read type hash declare typeset printf command mapfile \
         readarray help complete compgen compopt jobs trap alias unalias builtin \
         export cd wait history getopts shopt disown umask ulimit pwd enable; do
    check "$b -Q invalid option" "$b -Q"
done
check "local -Q invalid option" 'f() { local -Q; }; f'

# ── the contract rows (huck already matches these; they must STAY matching) ──
check "bundle order -ap"      'readonly -ap >/dev/null'
check "-- terminates"         'readonly -- x=1; echo $x'
check "lone - is an operand"  'hash -'
check "stop at non-option"    'v=1; readonly v -p'
check "attached value"        'read -n3 </dev/null; echo rc=$?'
check "separate value"        'read -n 3 </dev/null; echo rc=$?'

# ── posix fatality of a special-builtin usage error (v358) ──
check "posix readonly -Q"     'set -o posix; readonly -Q; echo SURVIVED'
check "non-posix readonly -Q" 'readonly -Q; echo SURVIVED'

harness_summary
```

- [ ] **Step 2: Run it to see it RED**

Run: `chmod +x tests/scripts/builtin_options_diff_check.sh && bash tests/scripts/builtin_options_diff_check.sh`
Expected: FAIL rows for the four reported bugs and for every builtin missing a usage line. **Record the exact pass/fail counts in the commit message** — that number is the baseline the later tasks drive to zero.

- [ ] **Step 3: Commit RED**

```bash
git add tests/scripts/builtin_options_diff_check.sh
git commit -m "v359: builtin-option harness, committed RED (#496)"
```

---

### Task 4: Convert the declaration builtins

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — `builtin_readonly_decl` (~1950), `builtin_export_decl` (~1420), `builtin_declare_decl` (~2177), `builtin_local_decl` (~1650)

**Interfaces:**
- Consumes: `Getopt`, `ArgView::Decl`, `Opt` (Task 1); `usage_for` (Task 2).

These take `&[DeclArg]`, hence `ArgView::Decl`. `builtin_declare_decl` and `builtin_local_decl` also accept `+x`-style options; **the scanner does not handle `+`** — keep those builtins' existing `+` handling ahead of the `Getopt` loop and let the scanner own only the `-` side. Do not extend the scanner for `+`: bash's `internal_getopt` does not handle it either (`declare` special-cases it), and inventing it here would be a divergence.

- [ ] **Step 1: Convert `builtin_readonly_decl`**

Replace the whole-string `while idx < args.len()` flag loop with:

```rust
    let mut want_list = false;
    let mut want_associative = false;
    let mut want_indexed = false;
    let mut g = crate::builtin_opts::Getopt::new(name, crate::builtin_opts::ArgView::Decl(args), "paA");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'p' => want_list = true,
                'a' => want_indexed = true,
                'A' => want_associative = true,
                _ => unreachable!("spec and match must agree"),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let rest = &args[g.rest_index()..];
```

`name` must be the **invoked** name. If `builtin_readonly_decl` does not already receive it, thread it from `run_declaration_builtin`, which does.

- [ ] **Step 2: Run the harness — the readonly rows must go green**

Run: `cargo build -p huck --bin huck --jobs 1 && bash tests/scripts/builtin_options_diff_check.sh 2>&1 | grep -E 'readonly|Total'`
Expected: `readonly -pa`, `readonly -Q`, `readonly -ap`, `-- terminates`, `stop at non-option`, both posix rows PASS.

- [ ] **Step 3: Convert `builtin_export_decl` (spec `"pnfa"`), `builtin_declare_decl`, `builtin_local_decl`**

Same shape. `export`'s existing `-a` no-op stays a recognised option (huck-specific, for `mise`); keep the comment explaining why.

- [ ] **Step 4: Run the full lib suite and the harness**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1`
Expected: PASS at or above 1994.
Run: `bash tests/scripts/builtin_options_diff_check.sh | tail -3`
Expected: fail count strictly lower than the Task 3 baseline.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "v359: declaration builtins use the shared scanner (#496)"
```

---

### Task 5: Convert the name/lookup builtins

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — `unset`, `type`, `hash`, `command`, `builtin`, `alias`, `unalias`

Specs, from the usage strings in Task 2: `unset` `"fvn"`, `type` `"afptP"`, `hash` `"lrp:dt"`, `command` `"pVv"`, `builtin` `""`, `alias` `"p"`, `unalias` `"a"`.

`alias`, `unalias` and `builtin` are class 4: today they treat `-Q` as a *name* and report "not found" / "not a shell builtin". After conversion the scanner rejects it as an invalid option, which is what bash does. `builtin` takes no options at all but still rejects `-Q` — an empty spec gives exactly that.

- [ ] **Step 1: Convert all seven, one at a time, running the harness after each**

Run after each: `cargo build -p huck --bin huck --jobs 1 && bash tests/scripts/builtin_options_diff_check.sh 2>&1 | grep -E '<name>|Total'`

- [ ] **Step 2: Verify `unset -vf x` now matches bash**

Run: `./target/debug/huck -c 'unset -vf x' huck5 2>&1`
Expected: `huck5: line 1: unset: cannot simultaneously unset a function and a variable` — the *semantic* error, reached only because `-vf` now parses.

- [ ] **Step 3: Full lib suite**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1`
Expected: PASS at or above 1994.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "v359: name/lookup builtins use the shared scanner (#496)"
```

---

### Task 6: Convert the I/O and job builtins

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — `read`, `mapfile`/`readarray`, `printf`, `jobs`, `trap`, `help`, `wait`, `history`

Specs: `read` `"ersa:d:i:n:N:p:t:u:"`, `mapfile` `"d:n:O:s:tu:C:c:"`, `printf` `"v:"`, `jobs` `"lnprsx"`, `trap` `"lp"`, `help` `"dms"`, `wait` `"fnp:"`, `history` `"cd:anrwps"`.

`read` is the largest conversion — eight value-taking options. Its existing `take_opt_value` helper (`builtins.rs:~3147`) becomes dead once `read` and `mapfile` are converted; **delete it in this task** and remove the `#[allow(clippy::too_many_arguments)]` added for it in #497.

`mapfile`/`readarray` must pass the **invoked** name so the usage line names the right builtin — this is the class-3 fix and the harness has a row for it.

- [ ] **Step 1: Convert each, running the harness after each**

- [ ] **Step 2: Confirm `take_opt_value` is gone**

Run: `grep -c take_opt_value crates/huck-engine/src/builtins.rs`
Expected: `0`.

- [ ] **Step 3: Full lib suite + the read/mapfile integration binaries**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1`
Run: `cargo test -p huck --test read_integration --jobs 1 -- --test-threads 1`
Run: `cargo test -p huck --test read_array_integration --jobs 1 -- --test-threads 1`
Expected: all PASS.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "v359: I/O and job builtins use the shared scanner (#496)"
```

---

### Task 7: Convert the completion and remaining builtins

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs`, `crates/huck-engine/src/completion_builtins.rs` — `complete`, `compgen`, `compopt`, `cd`, `getopts`, `shopt`, `disown`, `umask`, `ulimit`, `pwd`, `enable`

Specs: `complete` `"abcdefgjksuvprDEIo:A:G:W:F:C:X:P:S:"`, `compgen` same minus `prDEI`, `compopt` `"DEIo:"`, `cd` `"LPe@"`, `getopts` `""`, `shopt` `"pqsuo"`, `disown` `"har"`, `umask` `"pS"`, `ulimit` `"SHabcdefiklmnpqrstuvxPRT"`, `pwd` `"LP"`, `enable` `"adnpsf:"`.

`compopt` uses `+o` as well as `-o`; as in Task 4, leave its `+` handling in place ahead of the scanner.

- [ ] **Step 1: Convert each, running the harness after each**

- [ ] **Step 2: The harness must now be fully GREEN**

Run: `bash tests/scripts/builtin_options_diff_check.sh | tail -3`
Expected: `Fail: 0`. If any row still fails, it is either a missing conversion or a usage-string typo — diff the row and fix; do NOT weaken the harness.

- [ ] **Step 3: No hand-rolled scanner survives**

Run: `grep -c 'invalid option' crates/huck-engine/src/builtins.rs`
Expected: a small number — only `set` (which is not getopt) and the scanner's own site. Every remaining hit must be justified in the commit message.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "v359: completion and remaining builtins use the shared scanner (#496)"
```

---

### Task 8: Full verification, docs, follow-up issues

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: Clippy under the PINNED toolchain**

Run: `cargo +1.97.1 clippy --workspace --all-targets -- -D warnings`
Expected: clean. A newer local stable would MISS warnings CI raises (#497).

- [ ] **Step 2: Both lib suites and the integration binaries**

Run: `cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1` (≥483)
Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1` (≥1994)
Run each `-p huck --test <name>` binary touched by the conversion single-threaded.

- [ ] **Step 3: Full sweep on an IDLE box**

Run: `cargo build --locked --bin huck && cargo build --release --locked --bin huck && tests/scripts/run_diff_checks.sh`
Expected: 275 passed (274 + the new harness), 0 failed. **No existing harness may need an expected-value edit** — if one does, the change altered behaviour bash agrees with today. Investigate rather than update the expectation. If a job-control harness fails, re-run idle before believing it (#476).

- [ ] **Step 4: Update `docs/architecture.md`**

In the "where to add common features" cheatsheet, replace the new-builtin-option guidance with: declare the option character in the builtin's `Getopt` spec string (`:` suffix for a value), add a `match` arm, and add the usage string to `builtin_opts::usage_for` keyed on the invoked name. Note that `+`-style options are NOT the scanner's job.

- [ ] **Step 5: File the follow-ups**

```bash
gh issue create --label divergence --label bug --label sev:low \
  --title 'fg/bg/bind report state before parsing options; huck parses first' --body '...'
gh issue create --label divergence --label bug --label sev:low \
  --title 'pushd/popd/dirs parse +N/-N as numbers ("invalid number", not "invalid option")' --body '...'
gh issue create --label divergence --label bug --label sev:medium \
  --title 'pushd -Q misroutes to cd, emitting cd usage text' --body '...'
```

- [ ] **Step 6: Commit and open the PR**

```bash
cargo fmt --all
git add -A && git commit -m "v359: docs + verification (#496)"
git push -u origin v359-builtin-opts
gh pr create --base main --title 'v359: one option parser for the builtins (#496)' --body '... Closes #496 ...'
```

**Hand the PR to the user.** Do NOT self-merge — this is a `vNN` iteration.

---

## Self-Review

**Spec coverage.** §1 scanner → Task 1. §2 usage table keyed on invoked name → Task 2 (with a test that `readarray` ≠ `mapfile`). §3 v358 error path → Task 1's `fail_invalid`. §4 adoption beyond the bug list → Tasks 4–7 cover all 32 named builtins, not only the 20 divergent. §5 exclusions → `set`/`let`/`test`/`echo` never appear; `builtins.rs` is not split; classes 5–6 filed in Task 8 Step 5. Verification → Task 3 (RED first) and Task 8.

**Placeholders.** None: every spec string, usage string and code block is literal. The three `gh issue create --body '...'` bodies are the one abbreviation, and their titles carry the content.

**Type consistency.** `Getopt::new(name, ArgView, spec)`, `next_opt(shell, err) -> Result<Option<Opt>, i32>`, `rest_index() -> usize`, `Opt { ch, value }`, `usage_for(name) -> &'static str` — used identically in Tasks 1, 2 and 4–7. Task 1's `usage_for` stub is explicitly deleted in Task 2.
