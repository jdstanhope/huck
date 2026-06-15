# v162: `cd -P/-L` + `pwd -P/-L` (logical vs physical PWD) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck track a LOGICAL `PWD` by default (symlinks preserved, like bash), with `-P`/`-L` flags on `cd`/`pwd` and the `set -o physical` option selecting physical mode.

**Architecture:** Add a pure `normalize_logical` lexical path normalizer and a `physical` field on `ShellOptions`. Rewrite `builtin_cd` to compute the new `PWD` logically (join onto `$PWD` + lexical-normalize, no symlink resolution) unless physical mode (flag or option), and `builtin_pwd` to print the logical `$PWD` or the resolved path per mode. The actual `chdir` always uses the real path; only what's stored in/printed from `PWD` changes. Reverses the intentional I-01.

**Tech Stack:** Rust, std `env`/`fs`/`Path`. Spec: `docs/superpowers/specs/2026-06-15-cd-pwd-physical-logical-design.md` (read its "Verified bash behavior" list — it's the contract).

**Build/test (huck is a BIN crate — NO `--lib`):** `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | tail -5`; `cargo clippy --bins --quiet 2>&1 | tail -3`; harness loop `for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 || echo "FAIL $s"; done; echo done`.

**Reference (current code, all in `src/builtins.rs` unless noted):**
- `builtin_cd` (~294): parses `-` (OLDPWD), no `-P`/`-L`; sets `PWD = env::current_dir()` (physical) after `set_current_dir`. Uses `shell.get("PWD")`, `shell.export_set("PWD"/"OLDPWD", …)`. `print_new_pwd` (the `cd -` case) prints `env::current_dir()`.
- `builtin_pwd(out: &mut dyn Write)` (~348): prints `env::current_dir()`, ignores args. Dispatched at `run_builtin` line 88 `"pwd" => builtin_pwd(out)`; a unit test calls it at ~7782.
- `option_get(shell, name) -> Option<bool>` (~5203) and `option_set(shell, name, value)` (~5219): `match name { "errexit" => …, …, other => … }`. `physical` is in `SETO_TABLE` (~5182, `OptionInfo { name: "physical", default: false }`) but has NO `ShellOptions` field yet — so `option_get("physical")` returns the table default (false) and `option_set("physical", true)` hits the `other` arm (doesn't store).
- `ShellOptions` (src/shell_state.rs ~the struct): fields `errexit`/`nounset`/`pipefail`/`verbose`/`xtrace`/`noglob`/`noclobber`/`noexec` (all `bool`).
- bash `cd -x` → `bash: cd: -x: invalid option` + `cd: usage: cd [-L|[-P [-e]] [-@]] [dir]`, rc 2. `cd -- /tmp` works (`--` ends flags).

---

## Task 1: `normalize_logical` helper + wire the `physical` set-option

**Files:** `src/builtins.rs`, `src/shell_state.rs`.

- [ ] **Step 1: Write failing unit tests for `normalize_logical`**

In the `#[cfg(test)] mod tests` of `src/builtins.rs`:

```rust
    #[test]
    fn normalize_logical_collapses_lexically() {
        assert_eq!(normalize_logical("/a/b/../c"), "/a/c");
        assert_eq!(normalize_logical("/a/./b"), "/a/b");
        assert_eq!(normalize_logical("/a//b"), "/a/b");
        assert_eq!(normalize_logical("/a/b/.."), "/a");
        assert_eq!(normalize_logical("/.."), "/");
        assert_eq!(normalize_logical("/a/../.."), "/");
        assert_eq!(normalize_logical("/"), "/");
        assert_eq!(normalize_logical("/tmp/m/link/.."), "/tmp/m");
    }
```

- [ ] **Step 2: Implement `normalize_logical`**

Add (a free `fn` near `builtin_cd`):

```rust
/// Lexically normalizes an ABSOLUTE path for logical `cd`: collapses `.`,
/// empty components (from `//`), and `..` (removing the preceding component
/// WITHOUT resolving symlinks). A `..` at the root is dropped (bash behavior).
/// Always returns an absolute path; `/` for an empty result.
fn normalize_logical(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if matches!(components.last(), Some(&c) if c != "..") {
                    components.pop();
                }
            }
            other => components.push(other),
        }
    }
    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}
```

Run `cargo test normalize_logical_collapses_lexically 2>&1 | tail -4` → PASS.

- [ ] **Step 3: Add the `physical` field to `ShellOptions`**

In `src/shell_state.rs`, add `pub physical: bool,` to the `ShellOptions` struct (after `noexec`). If `ShellOptions` derives `Default`, no constructor change is needed (bool → false). If it's built with an explicit struct literal anywhere, `cargo build 2>&1 | grep 'missing field'` will list it — add `physical: false,` there.

- [ ] **Step 4: Wire `physical` into `option_get`/`option_set`**

In `src/builtins.rs` `option_get` (~5203), add the arm alongside the others:
```rust
        "physical" => Some(shell.shell_options.physical),
```
In `option_set` (~5219), add:
```rust
        "physical" => { shell.shell_options.physical = value; Ok(()) }
```
(Place each before the `other =>` fallthrough.)

- [ ] **Step 5: Test the option round-trips + commit**

```bash
cargo build 2>&1 | tail -1
H=./target/debug/huck
$H -c 'set -o physical; set -o | grep physical'   # physical  on
$H -c 'set -o | grep physical'                     # physical  off
```
(bash: `set -o | grep physical` → `physical        off` by default, `on` after `set -o physical` — confirm huck matches the on/off, column spacing may differ; the key is the on/off toggles.) Full `cargo test 2>&1 | tail -5` green, clippy clean.

```bash
git add src/builtins.rs src/shell_state.rs
git commit -m "v162 task 1: normalize_logical helper + wire the physical set-option

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Rewrite `builtin_cd` (flags + logical/physical PWD)

**Files:** `src/builtins.rs`.

- [ ] **Step 1: Rewrite the flag/mode/target handling**

Replace the body of `builtin_cd` (keep the signature `pub(crate) fn builtin_cd(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome`). New body:

```rust
pub(crate) fn builtin_cd(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // 1. Parse leading -L/-P flags (last wins) and `--`. `-` is NOT a flag (it
    //    is the OLDPWD shortcut / target).
    let mut physical_flag: Option<bool> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "-L" => { physical_flag = Some(false); idx += 1; }
            "-P" => { physical_flag = Some(true); idx += 1; }
            "--" => { idx += 1; break; }
            "-" => break, // OLDPWD shortcut, handled as the target below
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: cd: {s}: invalid option");
                eprintln!("huck: cd: usage: cd [-L|[-P [-e]] [-@]] [dir]");
                return ExecOutcome::Continue(2);
            }
            _ => break, // a target
        }
    }
    let rest = &args[idx..];
    if rest.len() > 1 {
        eprintln!("huck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }

    // 2. Effective mode: explicit flag, else the `physical` set-option.
    let physical = physical_flag.unwrap_or_else(|| option_get(shell, "physical").unwrap_or(false));

    // 3. Compute the target directory.
    let mut print_new_pwd = false;
    let target = match rest.first() {
        Some(dir) if dir == "-" => match shell.get("OLDPWD") {
            Some(oldpwd) if !oldpwd.is_empty() => { print_new_pwd = true; oldpwd.to_string() }
            _ => { eprintln!("huck: cd: OLDPWD not set"); return ExecOutcome::Continue(1); }
        },
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => { eprintln!("huck: cd: HOME not set"); return ExecOutcome::Continue(1); }
        },
    };

    let prev_pwd = shell.get("PWD").map(str::to_string);

    let new_pwd: String = if physical {
        // Physical: chdir to the target, store the canonical cwd.
        if let Err(e) = env::set_current_dir(Path::new(&target)) {
            eprintln!("huck: cd: {target}: {e}");
            return ExecOutcome::Continue(1);
        }
        match env::current_dir() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                eprintln!("huck: cd: warning: could not read current dir: {e}");
                // Fall back to the prior PWD-relative guess; keep going.
                prev_pwd.clone().unwrap_or_default()
            }
        }
    } else {
        // Logical: build curpath from $PWD (for relative targets), lexically
        // normalize, chdir to the normalized path, store it.
        let curpath = if target.starts_with('/') {
            target.clone()
        } else {
            let base = prev_pwd.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| {
                env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
            });
            format!("{base}/{target}")
        };
        let normalized = normalize_logical(&curpath);
        if let Err(e) = env::set_current_dir(Path::new(&normalized)) {
            eprintln!("huck: cd: {target}: {e}");
            return ExecOutcome::Continue(1);
        }
        normalized
    };

    // 4. Maintain OLDPWD / PWD.
    if let Some(prev) = prev_pwd {
        shell.export_set("OLDPWD", prev);
    }
    shell.export_set("PWD", new_pwd.clone());

    // 5. `cd -` prints the new directory.
    if print_new_pwd
        && let Err(e) = writeln!(out, "{new_pwd}")
    {
        eprintln!("huck: cd: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}
```

Confirm `option_get`, `normalize_logical`, `env`, `Path` are in scope (they're all in `src/builtins.rs` already — `option_get` is a free fn, `env`/`Path` are imported at the top). `shell.export_set` and `shell.get` are the existing accessors.

- [ ] **Step 2: Verify vs bash + commit**

```bash
cd /tmp && rm -rf cdtest && mkdir -p cdtest/real && ln -s real cdtest/link && cd /home/john/projects/shuck
H=./target/debug/huck
for f in 'cd /tmp/cdtest/link; echo "$PWD"' 'cd -P /tmp/cdtest/link; echo "$PWD"' 'cd /tmp/cdtest/link; cd ..; echo "$PWD"' 'cd -L /tmp/cdtest/link; echo "$PWD"' 'cd -L -P /tmp/cdtest/link; echo "$PWD"' 'cd -P -L /tmp/cdtest/link; echo "$PWD"' 'set -o physical; cd /tmp/cdtest/link; echo "$PWD"' 'cd /; echo "$PWD"' 'cd -x 2>/dev/null; echo rc=$?'; do
  b=$(bash --norc --noprofile -c "$f" 2>/dev/null); h=$($H -c "$f" 2>/dev/null)
  printf '%-48s %s\n' "$f" "$([[ "$b" == "$h" ]] && echo "OK [$h]" || echo "DIFF b=[$b] h=[$h]")"
done
rm -rf /tmp/cdtest
```
Each rc/stdout must match bash (stderr prefix differs). Then full `cargo test 2>&1 | tail -5` — NOTE: an existing test may assert the OLD physical-after-cd-symlink behavior; if a cd/PWD test fails because it encoded I-01, that's expected — DEFER fixing it to Task 4 (it documents the I-01 reversal) but RECORD which test failed. All harnesses: most stay green; a `cd`/`pwd` harness asserting physical PWD would now diff — note it for Task 4. clippy clean.

```bash
git add src/builtins.rs
git commit -m "v162 task 2: cd -L/-P + logical-default PWD

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Rewrite `builtin_pwd` (flags + mode + new signature)

**Files:** `src/builtins.rs`.

- [ ] **Step 1: Rewrite `builtin_pwd` with the new signature**

Replace `builtin_pwd`:

```rust
fn builtin_pwd(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // Parse -L/-P (last wins); `--` ends flags; non-flag args are ignored
    // (bash prints pwd anyway). Unknown flag → invalid option, rc 2.
    let mut physical_flag: Option<bool> = None;
    for a in args {
        match a.as_str() {
            "-L" => physical_flag = Some(false),
            "-P" => physical_flag = Some(true),
            "--" => break,
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: pwd: {s}: invalid option");
                eprintln!("huck: pwd: usage: pwd [-LP]");
                return ExecOutcome::Continue(2);
            }
            _ => {} // ignore non-flag args
        }
    }
    let physical = physical_flag.unwrap_or_else(|| option_get(shell, "physical").unwrap_or(false));

    let path: String = if physical {
        // Resolved physical path.
        match env::current_dir() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => shell.get("PWD")
                .and_then(|p| std::fs::canonicalize(p).ok())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    } else {
        // Logical: the stored $PWD; fall back to getcwd.
        shell.get("PWD").filter(|p| !p.is_empty()).map(str::to_string)
            .unwrap_or_else(|| env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
    };

    if let Err(e) = writeln!(out, "{path}") {
        eprintln!("huck: pwd: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 2: Update the dispatch + the unit-test call site**

In `run_builtin` (line ~88), change `"pwd" => builtin_pwd(out),` to `"pwd" => builtin_pwd(args, out, shell),`. Find the unit test call (`~7782`, `builtin_pwd(&mut out)`) and update it to the new signature — it needs a `Shell` and an `args` slice; pass an empty args slice and a test shell (`&[]`, `&mut out`, `&mut shell`), matching how neighboring builtin tests construct a `Shell`. Adjust the test's assertion if it asserted physical cwd output (logical default now prints `$PWD`; for a freshly-constructed test shell `$PWD` may be unset → it falls back to getcwd, so the assertion likely still holds — verify).

- [ ] **Step 3: Verify vs bash + commit**

```bash
cd /tmp && rm -rf pwdtest && mkdir -p pwdtest/real && ln -s real pwdtest/link && cd /home/john/projects/shuck
H=./target/debug/huck
for f in 'cd /tmp/pwdtest/link; pwd' 'cd /tmp/pwdtest/link; pwd -L' 'cd /tmp/pwdtest/link; pwd -P' 'cd /tmp/pwdtest/link; pwd -L -P' 'cd /tmp/pwdtest/link; pwd -P -L' 'set -o physical; cd /tmp/pwdtest/link; pwd' 'pwd -x 2>/dev/null; echo rc=$?' 'cd /tmp/pwdtest/link; pwd foo'; do
  b=$(bash --norc --noprofile -c "$f" 2>/dev/null); h=$($H -c "$f" 2>/dev/null)
  printf '%-46s %s\n' "$f" "$([[ "$b" == "$h" ]] && echo "OK [$h]" || echo "DIFF b=[$b] h=[$h]")"
done
rm -rf /tmp/pwdtest
```
Each matches bash. Full `cargo test 2>&1 | tail -5` green (modulo the noted I-01 test → Task 4), clippy clean.

```bash
git add src/builtins.rs
git commit -m "v162 task 3: pwd -L/-P + logical/physical print

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Harness + update I-01-encoding tests + final

**Files:** Create `tests/scripts/cd_pwd_physical_diff_check.sh`; update any existing test that asserted old physical-PWD behavior.

- [ ] **Step 1: Update existing I-01-encoding tests**

From Tasks 2/3 you noted any test/harness that failed because it asserted the OLD physical-after-`cd`-symlink behavior. Find them now: `cargo test 2>&1 | grep -iE 'FAILED|cd|pwd|PWD'` and search for tests asserting a resolved PWD after `cd` through a symlink (grep tests for `current_dir`/`canonicalize`/`PWD` near `cd`). UPDATE each to the new LOGICAL expectation (e.g. a test that did `cd symlink` and expected the resolved path now expects the symlink path). Do NOT weaken a test — change the expected value to bash's logical behavior (verify the new expected value against `bash --norc --noprofile -c '…'`). If NO existing test encoded I-01 (the behavior may only have been covered by the I-01 doc), there's nothing to update — confirm the full suite is green.

- [ ] **Step 2: Write the bash-diff harness**

Create `tests/scripts/cd_pwd_physical_diff_check.sh` (model the `chk` helper on `tests/scripts/local_case_attrs_diff_check.sh`; this one builds its own `mktemp -d` fixture with a symlink so paths are machine-independent):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for logical/physical PWD (cd -P/-L, pwd -P/-L,
# set -o physical). Uses a mktemp fixture with a symlink; compares stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
T=$(mktemp -d)
mkdir -p "$T/real"; ln -s real "$T/link"
trap 'rm -rf "$T"' EXIT
PASS=0; FAIL=0
chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "logical default"   "cd $T/link; echo \"\$PWD\"; pwd; pwd -L"
chk "pwd -P resolves"   "cd $T/link; pwd -P"
chk "cd -P physical"    "cd -P $T/link; echo \"\$PWD\""
chk "cd -L logical"     "cd -L $T/link; echo \"\$PWD\""
chk "cd last-wins LP"   "cd -L -P $T/link; echo \"\$PWD\""
chk "cd last-wins PL"   "cd -P -L $T/link; echo \"\$PWD\""
chk "pwd last-wins LP"  "cd $T/link; pwd -L -P"
chk "pwd last-wins PL"  "cd $T/link; pwd -P -L"
chk "cd .. lexical"     "cd $T/link; cd ..; echo \"\$PWD\""
chk "set -o physical cd" "set -o physical; cd $T/link; echo \"\$PWD\""
chk "set -o physical pwd" "set -o physical; cd $T/link; pwd"
chk "cd - logical"      "cd $T/link; cd /tmp; cd - >/dev/null; echo \"\$PWD\""
chk "cd root"           'cd /; echo "$PWD"'
chk "pwd -x rc"         'pwd -x >/dev/null 2>&1; echo rc=$?'
chk "pwd extra arg"     "cd $T/link; pwd foo"
chk "cd -x rc"          'cd -x >/dev/null 2>&1; echo rc=$?'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
```

`chmod +x` it; `cargo build`; `bash tests/scripts/cd_pwd_physical_diff_check.sh` → all PASS. If a case FAILs, fix the implementation (Tasks 1-3) — the harness is the bash-parity source of truth; do NOT weaken a case.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/cd_pwd_physical_diff_check.sh
git commit -m "v162 task 4: cd/pwd physical-logical bash-diff harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` + `cargo clippy --bins --quiet` clean.
- [ ] `cargo test` FULLY green (incl. `normalize_logical` tests + the updated I-01 test if any).
- [ ] `bash tests/scripts/cd_pwd_physical_diff_check.sh` all PASS; ALL `tests/scripts/*_diff_check.sh` byte-identical.
- [ ] Manual sanity in a real symlink dir: `cd link; echo $PWD` shows the symlink; `pwd -P` resolves; `cd -P link` resolves.

## Notes for the implementer

- **The chdir always uses the real path** — logical mode `chdir`s to the lexically-normalized path (which the OS still resolves to move); only the STORED `PWD` differs. Never store the physical path in logical mode or vice-versa.
- **`option_get(shell, "physical")`** is the single source for the default mode; an explicit `-P`/`-L` flag overrides it per-invocation.
- **Don't touch startup PWD** — huck already inherits a valid `$PWD` from the env; this iteration only changes `cd`/`pwd`.
- **The doc bookkeeping (delete I-01/M-32/M-33, update tier counts) is a POST-MERGE step**, not part of this branch — the branch only updates any TEST that encoded I-01.
- **Deferred edge** (don't implement): a logical `cd ..` whose lexical parent doesn't physically exist errors (rc 1) rather than bash's fallback-retry; rare, log post-merge only if the harness surfaces it.
