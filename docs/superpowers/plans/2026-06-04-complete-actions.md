# `complete`/`compgen` Action Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recognize bash's full `complete`/`compgen` action surface (all 24 `-A` action names + the 12 single-letter shortcuts) so a real bashrc stops erroring, and generate completions for the actions whose data huck already holds.

**Architecture:** Expand the `Action` enum (8→24) with `parse`/`as_str` covering every name; add 16 arms to the private `enumerate_action(action, prefix, &Shell)` generator (10 generate from existing shell data via small new `pub` accessors, 6 return empty); and map the 12 short-flag letters in the `complete`/`compgen` flag-parse loop to the corresponding `Action`. Purely additive — the 8 existing actions and all current behavior are unchanged.

**Tech Stack:** Rust; huck's `Action`/`CompletionSpec`/`Shell`/`VarValue`/`JobTable` types; existing tables `SETO_TABLE`/`SHOPT_TABLE`/`HELP_ENTRIES` and `traps::name_table()`.

**Spec:** `docs/superpowers/specs/2026-06-04-complete-actions-design.md`

**Key facts (verified against bash 5.2):**
- All 12 short flags + 24 action names are *accepted* (empty result = rc 1, no message; invalid = rc 2 + message). After v88 there are no invalid actions in the bash set.
- `compgen` emits each action's **source order — it does NOT sort**. So `Setopt`/`Shopt` generators must emit table order (no `.sort()`) to byte-match bash; `compgen -A shopt` is `autocd, assoc_expand_once, …`, NOT alphabetical.
- `signal` names are `SIG`-prefixed (`SIGINT`). huck's `traps::name_table()` stores un-prefixed names (`"INT"`).

**Conventions:**
- Binary crate: unit tests `cargo test --bin huck <filter>`; integration `cargo test --test <name>`; full suite `cargo test`.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Baseline: **2449** tests pass, clippy clean. Each task keeps clippy clean + suite green.
- The action method is `Action::as_str` (NOT `name`). `enumerate_action` is a private `fn` in `src/completion_spec.rs`; its tests live in that file's `#[cfg(test)]` module.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/completion_spec.rs` | `Action` enum +16 variants; `parse`/`as_str` all 24; `enumerate_action` +16 arms | 1 |
| `src/builtins.rs` | `pub fn seto_option_names` / `help_topic_names` / `signal_names` | 1 |
| `src/shell_state.rs` | `pub fn array_var_names` | 1 |
| `src/jobs.rs` | `JobTable::jobs(&self) -> &[Job]` read accessor | 1 |
| `src/completion_builtins.rs` | 12 short-flag cases in the flag-parse loop | 2 |
| `tests/complete_actions_integration.rs` | NEW — integration tests | 2 |
| `tests/scripts/complete_actions_diff_check.sh` | NEW — huck's 15th harness | 3 |
| `docs/bash-divergences.md`, `README.md` | M-36 update + M-36a/M-36b; changelog; README row | 3 |

---

### Task 1: Action enum (8→24) + generators + data accessors

**Files:**
- Modify: `src/builtins.rs` (add 3 `pub` accessors), `src/shell_state.rs` (add `array_var_names`), `src/jobs.rs` (add `JobTable::jobs`)
- Modify: `src/completion_spec.rs` (`Action` enum, `parse`, `as_str`, `enumerate_action`)
- Test: `src/completion_spec.rs` `#[cfg(test)]`

The enum change makes `enumerate_action`'s `match` non-exhaustive, so the arms and the accessors they call must land in the same task.

- [ ] **Step 1: Add the data accessors (with unit tests)**

In `src/builtins.rs`, add (near `SETO_TABLE` / `HELP_ENTRIES` / the signal code):

```rust
/// Names of the `set -o` options, in table order. Used by `compgen -A setopt`.
pub fn seto_option_names() -> impl Iterator<Item = &'static str> {
    SETO_TABLE.iter().map(|o| o.name)
}

/// Names of all `help` topics (builtins + keywords). Used by `compgen -A helptopic`.
pub fn help_topic_names() -> impl Iterator<Item = &'static str> {
    HELP_ENTRIES.iter().map(|e| e.name)
}

/// `SIG`-prefixed names of the real signals huck knows (excludes the trap
/// pseudo-signals EXIT/ERR/DEBUG/RETURN). Used by `compgen -A signal`.
pub fn signal_names() -> Vec<String> {
    crate::traps::name_table()
        .iter()
        .filter(|(n, _)| !matches!(*n, "EXIT" | "ERR" | "DEBUG" | "RETURN"))
        .map(|(n, _)| format!("SIG{n}"))
        .collect()
}
```

In `src/shell_state.rs`, add to an `impl Shell` block:

```rust
    /// Names of variables whose value is an array (indexed or associative).
    /// Used by `compgen -A arrayvar`.
    pub fn array_var_names(&self) -> Vec<String> {
        self.vars
            .iter()
            .filter(|(_, v)| matches!(v.value, VarValue::Indexed(_) | VarValue::Associative(_)))
            .map(|(k, _)| k.clone())
            .collect()
    }
```

In `src/jobs.rs`, add to the `impl JobTable` block:

```rust
    /// Read-only view of the current jobs. Used by `compgen -A job/running/stopped`.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }
```

Add unit tests (place `signal`/`setopt`/`help` ones in `src/builtins.rs` tests, `array_var_names` in `src/shell_state.rs` tests):

```rust
// src/builtins.rs tests
#[test]
fn seto_option_names_includes_errexit_in_table_order() {
    let names: Vec<&str> = seto_option_names().collect();
    assert!(names.contains(&"errexit"));
    assert_eq!(names.len(), 27);
    assert_eq!(names[0], "allexport"); // table order
}
#[test]
fn signal_names_are_sig_prefixed_and_exclude_pseudo() {
    let names = signal_names();
    assert!(names.contains(&"SIGINT".to_string()));
    assert!(names.iter().all(|n| n.starts_with("SIG")));
    assert!(!names.iter().any(|n| n.contains("EXIT")));
}
#[test]
fn help_topic_names_nonempty() {
    assert!(help_topic_names().count() >= 40);
}

// src/shell_state.rs tests
#[test]
fn array_var_names_lists_arrays_not_scalars() {
    let mut sh = Shell::new();
    sh.set("scal", "x".to_string());
    let mut elements = std::collections::BTreeMap::new();
    elements.insert(0usize, "a".to_string());
    elements.insert(1usize, "b".to_string());
    sh.replace_array("arr", elements).unwrap(); // existing pub Shell method (indexed array)
    let names = sh.array_var_names();
    assert!(names.contains(&"arr".to_string()));
    assert!(!names.contains(&"scal".to_string()));
}
```

- [ ] **Step 2: Run accessor tests — verify they fail to compile/fail**

Run: `cargo test --bin huck seto_option_names 2>&1 | tail` → fails (accessors undefined).

- [ ] **Step 3: Run accessor tests — verify pass**

After Step 1's code compiles: `cargo test --bin huck seto_option_names 2>&1 | tail`, `cargo test --bin huck signal_names 2>&1 | tail`, `cargo test --bin huck array_var_names 2>&1 | tail` → pass. (Accessors are dead-code until Step 5 wires them — clippy may warn `never used`; that clears in Step 5. If the build is `-D warnings`-clean today, this is fine: plain `cargo build` only warns.)

- [ ] **Step 4: Write the failing `Action` parse + enumerate tests**

In `src/completion_spec.rs` `#[cfg(test)]`:

```rust
#[test]
fn action_parse_round_trips_all_24() {
    let names = [
        "file","directory","command","function","variable","alias","builtin","keyword",
        "arrayvar","binding","disabled","enabled","export","group","helptopic","hostname",
        "job","running","service","setopt","shopt","signal","stopped","user",
    ];
    for n in names {
        let a = Action::parse(n).unwrap_or_else(|| panic!("parse failed: {n}"));
        assert_eq!(a.as_str(), n, "round-trip mismatch for {n}");
    }
    assert_eq!(Action::parse("bogus_action"), None);
}

#[test]
fn enumerate_setopt_shopt_table_order_and_membership() {
    let sh = Shell::new();
    let setopt = enumerate_action(Action::Setopt, "", &sh);
    assert!(setopt.contains(&"errexit".to_string()));
    assert_eq!(setopt[0], "allexport"); // table order, NOT sorted
    let shopt = enumerate_action(Action::Shopt, "", &sh);
    assert!(shopt.contains(&"nullglob".to_string()));
    assert_eq!(&shopt[0..2], &["autocd".to_string(), "assoc_expand_once".to_string()]); // table order
    assert_eq!(enumerate_action(Action::Shopt, "null", &sh), vec!["nullglob".to_string()]);
}

#[test]
fn enumerate_signal_helptopic_enabled() {
    let sh = Shell::new();
    assert!(enumerate_action(Action::Signal, "SIGIN", &sh) == vec!["SIGINT".to_string()]);
    assert!(!enumerate_action(Action::Helptopic, "", &sh).is_empty());
    assert!(enumerate_action(Action::Enabled, "ech", &sh).contains(&"echo".to_string()));
}

#[test]
fn enumerate_empty_actions_return_empty() {
    let sh = Shell::new();
    for a in [Action::Disabled, Action::Binding, Action::Hostname, Action::User, Action::Group, Action::Service] {
        assert!(enumerate_action(a, "", &sh).is_empty(), "expected empty for {a:?}");
    }
}
```

- [ ] **Step 5: Implement the enum expansion + generators**

In `src/completion_spec.rs`, extend the `Action` enum to all 24 variants:

```rust
pub enum Action {
    File, Directory, Command, Function, Variable, Alias, Builtin, Keyword,
    Arrayvar, Binding, Disabled, Enabled, Export, Group, Helptopic, Hostname,
    Job, Running, Service, Setopt, Shopt, Signal, Stopped, User,
}
```

Extend `Action::parse` with the 16 new names (each `"<name>" => Some(Action::<Variant>)`), and `Action::as_str` with the 16 new `Action::<Variant> => "<name>"` arms. The 16 names: `arrayvar binding disabled enabled export group helptopic hostname job running service setopt shopt signal stopped user`.

Add the 16 arms to `enumerate_action` (before the closing `}` of the `match`):

```rust
        Action::Setopt => crate::builtins::seto_option_names()
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(), // table order — bash compgen does not sort
        Action::Shopt => crate::shell_state::SHOPT_TABLE
            .iter()
            .map(|o| o.name)
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(), // table order
        Action::Helptopic => crate::builtins::help_topic_names()
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(),
        Action::Signal => crate::builtins::signal_names()
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect(),
        Action::Export => {
            let mut names: Vec<String> = shell
                .var_names()
                .filter(|n| shell.is_exported(n) && n.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            names.sort();
            names.dedup();
            names
        }
        Action::Arrayvar => {
            let mut names: Vec<String> = shell
                .array_var_names()
                .into_iter()
                .filter(|n| n.starts_with(prefix))
                .collect();
            names.sort();
            names
        }
        Action::Enabled => {
            let mut names: Vec<String> = crate::builtins::BUILTIN_NAMES
                .iter()
                .filter(|n| n.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            names.sort();
            names
        }
        Action::Job => shell
            .jobs
            .jobs()
            .iter()
            .map(|j| j.command.clone())
            .filter(|c| c.starts_with(prefix))
            .collect(),
        Action::Running => shell
            .jobs
            .jobs()
            .iter()
            .filter(|j| matches!(j.state, crate::jobs::JobState::Running))
            .map(|j| j.command.clone())
            .filter(|c| c.starts_with(prefix))
            .collect(),
        Action::Stopped => shell
            .jobs
            .jobs()
            .iter()
            .filter(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
            .map(|j| j.command.clone())
            .filter(|c| c.starts_with(prefix))
            .collect(),
        Action::Disabled
        | Action::Binding
        | Action::Hostname
        | Action::User
        | Action::Group
        | Action::Service => Vec::new(),
```

> `shell.jobs` is the `pub jobs: JobTable` field on `Shell`. `shell.is_exported` / `shell.var_names` already exist. `SHOPT_TABLE` is already `pub` in `shell_state`.

- [ ] **Step 6: Run tests, verify pass**

Run: `cargo test --bin huck action_parse_round_trips 2>&1 | tail`, `cargo test --bin huck enumerate_ 2>&1 | tail` → pass.
Run: `cargo build 2>&1 | tail -3` → clean (accessor dead-code warnings gone now they're used).
Run full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Run: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 7: Commit**

```bash
git add src/completion_spec.rs src/builtins.rs src/shell_state.rs src/jobs.rs
git commit -m "v88 task 1: full 24-action set + generators for the cheap shell-data subset

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: short-flag action forms + integration tests

**Files:**
- Modify: `src/completion_builtins.rs` (flag-parse loop)
- Create: `tests/complete_actions_integration.rs`
- Test: `src/completion_builtins.rs` `#[cfg(test)]`

- [ ] **Step 1: Write failing unit tests for short flags**

In `src/completion_builtins.rs` `#[cfg(test)]` (use the existing `run_complete` helper + the `sh.completion_specs.by_command[...]` inspection pattern from `complete_multiple_actions_accumulate`):

```rust
#[test]
fn complete_short_flag_actions_map_to_actions() {
    use crate::completion_spec::Action;
    let cases = [
        ("-a", Action::Alias), ("-b", Action::Builtin), ("-c", Action::Command),
        ("-d", Action::Directory), ("-e", Action::Export), ("-f", Action::File),
        ("-g", Action::Group), ("-j", Action::Job), ("-k", Action::Keyword),
        ("-s", Action::Service), ("-u", Action::User), ("-v", Action::Variable),
    ];
    for (flag, want) in cases {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&[flag, "--", "foo"], &mut sh);
        assert_eq!(code, 0, "flag {flag} should be accepted");
        assert_eq!(sh.completion_specs.by_command["foo"].actions, vec![want],
            "flag {flag} → wrong action");
    }
}

#[test]
fn complete_clustered_short_flags_accumulate() {
    let mut sh = Shell::new();
    let (_, code) = run_complete(&["-ev", "--", "foo"], &mut sh);
    assert_eq!(code, 0);
    assert_eq!(sh.completion_specs.by_command["foo"].actions,
        vec![crate::completion_spec::Action::Export, crate::completion_spec::Action::Variable]);
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test --bin huck complete_short_flag 2>&1 | tail` → fails (`-a` etc. → "invalid option", code 2).

- [ ] **Step 3: Add the short-flag cases to the flag-parse loop**

In `src/completion_builtins.rs`, in the `match c { … }` block, add a new arm immediately BEFORE the `other => { return Err(FlagError::Usage(...)); }` arm:

```rust
                'a' | 'b' | 'c' | 'd' | 'e' | 'f' | 'g' | 'j' | 'k' | 's' | 'u' | 'v' => {
                    if leading == '+' {
                        return Err(FlagError::Usage(format!("+{c}: not supported")));
                    }
                    let action = match c {
                        'a' => Action::Alias,
                        'b' => Action::Builtin,
                        'c' => Action::Command,
                        'd' => Action::Directory,
                        'e' => Action::Export,
                        'f' => Action::File,
                        'g' => Action::Group,
                        'j' => Action::Job,
                        'k' => Action::Keyword,
                        's' => Action::Service,
                        'u' => Action::User,
                        'v' => Action::Variable,
                        _ => unreachable!(),
                    };
                    out.spec.actions.push(action);
                }
```

> Ensure `Action` is in scope in this file (it already references `Action::parse` for the `-A` arm, so the import exists). These flags take no argument and apply to BOTH `complete` and `compgen` (not gated on `allow_d_e`).

- [ ] **Step 4: Write integration tests**

Create `tests/complete_actions_integration.rs` (model on `tests/complete_*`/`bang_negation` integration harnesses):

```rust
//! Integration tests for v88 complete/compgen action expansion (M-36a).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn registration_never_errors() {
    assert_eq!(run("complete -u cmd; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("complete -A stopped cmd; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("complete -A setopt -A shopt cmd; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("complete -ev cmd; echo rc=$?\n").0, "rc=0\n");
}

#[test]
fn compgen_setopt_shopt_signal() {
    assert_eq!(run("compgen -A setopt e\n").0, "emacs\nerrexit\nerrtrace\n");
    assert_eq!(run("compgen -A shopt null\n").0, "nullglob\n");
    assert_eq!(run("compgen -A signal SIGIN\n").0, "SIGINT\n");
}

#[test]
fn compgen_export_arrayvar_builtin() {
    assert_eq!(run("export FOO=1\ncompgen -A export FO\n").0, "FOO\n");
    assert_eq!(run("arr=(x y)\ncompgen -A arrayvar ar\n").0, "arr\n");
    assert_eq!(run("compgen -b ec\n").0, "echo\n");
    assert!(run("compgen -v PA\n").0.lines().any(|l| l == "PATH"));
}

#[test]
fn compgen_empty_actions_rc_one_no_error() {
    // recognized-but-empty: rc 1, no stdout, no stderr-driven failure
    let (out, rc) = run("compgen -A hostname x\n");
    assert_eq!(out, "");
    assert_eq!(rc, 1);
    assert_eq!(run("compgen -A binding\n").1, 1);
}
```

- [ ] **Step 5: Run tests + bash parity**

Run: `cargo test --bin huck complete_short_flag 2>&1 | tail`, `cargo test --bin huck complete_clustered 2>&1 | tail` → pass.
Run: `cargo test --test complete_actions_integration 2>&1 | tail -12` → all pass.
bash parity: `diff <(bash -c 'compgen -A setopt e') <(printf 'compgen -A setopt e\n' | ./target/debug/huck)` → empty; same for `compgen -A shopt null`; and `bash -c 'complete -u cmd; echo rc=$?'` vs huck.
Full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Clippy: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/completion_builtins.rs tests/complete_actions_integration.rs
git commit -m "v88 task 2: complete/compgen short-flag action forms (-a/-b/.../-v)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/complete_actions_diff_check.sh` (huck's 15th harness)
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/complete_actions_diff_check.sh`, modeled on `tests/scripts/bang_negation_diff_check.sh`. `chmod +x`. Byte-diff ONLY the deterministic, content-and-order-identical fragments.

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v88: complete/compgen actions (M-36a).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# NOTE: only `setopt`/`shopt` action generation + registration-rc are byte-diffed.
# `builtin`/`keyword`/`helptopic`/`command`/`file`/`variable`/`signal`/`job` are
# NOT byte-diffed: their candidate SETS differ between huck and bash (different
# builtin tables / env / PATH / platform signal set) or are volatile. Those are
# membership-tested in tests/complete_actions_integration.rs instead.
check "compgen setopt (all)"   'compgen -A setopt'
check "compgen setopt e"       'compgen -A setopt e'
check "compgen shopt (all)"    'compgen -A shopt'
check "compgen shopt null"     'compgen -A shopt null'
check "register -u rc"         'complete -u cmd; echo rc=$?'
check "register -A stopped rc" 'complete -A stopped cmd; echo rc=$?'
check "register -ev rc"        'complete -ev cmd; echo rc=$?'
check "register -A setopt rc"  'complete -A setopt cmd; echo rc=$?'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness, confirm all PASS**

```bash
cd /home/john/projects/shuck
cargo build 2>&1 | tail -1
chmod +x tests/scripts/complete_actions_diff_check.sh
bash tests/scripts/complete_actions_diff_check.sh; echo "rc=$?"
```
Expected: `Fail: 0`, `rc=0`. If `compgen -A setopt` / `compgen -A shopt` differ in ORDER, the generator is sorting when it must preserve table order — fix the generator (do NOT weaken `check()`). If a `complete -… rc` fragment differs only on an error-prefix line, relocate it to an rc-only integration test with a NOTE. Report any relocations.

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Read the M-36 entry + the Tier-2 count line (~25) first.

1. Add a new **M-36a** sub-entry after M-36:
```markdown
- **M-36a: full `complete`/`compgen` action set** — `[fixed v88]` medium. Recognizes
  all 24 bash `-A` action names and the 12 single-letter shortcuts
  (`-a`/`-b`/`-c`/`-d`/`-e`/`-f`/`-g`/`-j`/`-k`/`-s`/`-u`/`-v`), so a real bashrc's
  bash-completion setup (`complete -u`, `complete -A stopped`, etc.) no longer
  errors. Generates real completions for the actions huck already has data for:
  `setopt`/`shopt`/`helptopic`/`signal`/`export`/`arrayvar`/`enabled` and
  `job`/`running`/`stopped`. `compgen` emits source order (no sort) to match bash,
  so `setopt`/`shopt` preserve table order. **Recognized-but-empty**: `disabled`
  (huck has no `enable -n`) and `binding` (no exposed readline bindings) are
  permanently empty; `hostname`/`user`/`group`/`service` are deferred (see M-36b).
  huck's 15th bash-diff harness (byte-diffs `setopt`/`shopt` + registration rc; the
  other actions' sets differ from bash and are membership-tested). `complete -p`
  output formatting is unchanged (still huck's `-A name --` style — a pre-existing
  M-36 divergence from bash's short-flag form).
- **M-36b: system-data completion actions** — `[deferred]` low. `compgen -A
  hostname`/`user`/`group`/`service` are recognized but return nothing; bash reads
  `/etc/hosts`(`$HOSTFILE`)/`/etc/passwd`/`/etc/group`/`/etc/services`. Rarely the
  decisive completion source; deferred to avoid new filesystem/libc lookups.
```
2. Update the Tier-2 count line (~25): bump by 1 and append `; M-36a fixed by v88, with M-36b added as a new low-priority deferred follow-on`.
3. Update the "Last updated" stamp (line 3) to `2026-06-04 (after v88 complete/compgen action expansion; M-36a fixed)`.
4. Add a changelog entry at the END (match the v87 entry's format), dated 2026-06-04: the `Action` enum 8→24, `parse`/`as_str`, the 16 `enumerate_action` arms (10 generate / 6 empty), the `pub` accessors (`seto_option_names`/`help_topic_names`/`signal_names`/`array_var_names`/`JobTable::jobs`), the 12 short-flag forms, the source-order (no-sort) requirement for setopt/shopt, and the 15th harness.

- [ ] **Step 4: Update `README.md`**

Read the iteration table + v87 row first. Add a v88 row matching the format (escape literal `|` as `\|`):
```markdown
| v88 | `complete`/`compgen` actions (M-36a) | Full 24-name `-A` action set + 12 short flags (`-u`/`-j`/`-v`/…); generates `setopt`/`shopt`/`signal`/`export`/`arrayvar`/etc.; closes a real bashrc's `complete` errors |
```

- [ ] **Step 5: Verify whole branch**

```bash
cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'   # FAIL=0
cargo clippy --all-targets 2>&1 | tail -3                                                       # clean
for f in tests/scripts/*_diff_check.sh; do printf '%s: ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo FAIL; done  # all 15 OK
```

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/complete_actions_diff_check.sh docs/bash-divergences.md README.md
git commit -m "v88 task 3: complete/compgen actions bash-diff harness + docs (M-36a)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **Binary crate:** `cargo test --bin huck <filter>` for unit, `cargo test --test complete_actions_integration` for integration.
- **`compgen` does not sort** — `Setopt`/`Shopt` generators MUST emit table order (no `.sort()`), or the harness byte-diff breaks. The other generators may sort (matching the existing `Variable`/`Builtin` arms) since they're membership-tested only.
- **Only `setopt`/`shopt` are byte-diffable** vs bash — huck's builtin/keyword/helptopic/signal *sets* differ from bash's by design. Do not add other actions to the harness; test them by membership in integration.
- **Recognized-but-empty ≠ error**: the 6 empty actions return `Vec::new()`, so `compgen` exits rc 1 with no stderr — never rc 2 (which is for genuinely invalid actions, of which there are now none in the bash set).
- **Method name is `as_str`**, not `name` (the spec used `name` loosely).
- **Purely additive**: the 8 existing actions + all current `complete`/`compgen` behavior must stay green.
