# huck v154 — shell-maintained builtin variables + completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement bash's shell-maintained builtin variables (RANDOM, SECONDS, UID, EUID, PPID, BASH_VERSION, …) and make computed/special variables tab-complete and appear in `compgen -v`.

**Architecture:** Static vars set in `Shell::new` (stored, readonly flags matching bash); dynamic vars (RANDOM/SECONDS/EPOCHSECONDS/BASHPID) computed in `lookup_var` (RANDOM via an interior-mutable `Cell` LCG; RANDOM/SECONDS reseedable via a `Shell::set` hook). A `DYNAMIC_SPECIAL_VARS` registry + `completion_var_names()` feeds the completion / `compgen -v` sites.

**Tech Stack:** Rust; `libc` (getuid/geteuid/getppid/getpid/getgroups/gethostname); `std::time`.

**Reference:** spec at `docs/superpowers/specs/2026-06-13-builtin-variables-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>`. Stay on `v154-builtin-vars`. Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Verified facts:**
- `libc` is a direct dep. `nix` is NOT — use raw `libc`.
- `Shell::new` builds `vars` (env-load loop) then a `Self { … }` struct literal, then does post-construction mutation (e.g. the BASH_FUNC import loop) before returning `shell`. Add new fields to the struct + literal; add static vars via post-construction mutation (so libc/helpers are usable).
- `lookup_var(&self, name)` special-param block (shell_state.rs ~595-605) already handles `?`/`$`/`!`/`0`/`-`/`LINENO`. `lookup_var` is `&self` → RANDOM needs interior mutability (`Cell`).
- `Shell::set(&mut self, name, value)` (shell_state.rs:639) is the scalar-assign chokepoint. `apply_one_assignment` (executor.rs:4738) is the higher-level path for `NAME=value` — confirm its scalar branch routes through `Shell::set` (or add the hook there too).
- Array helper `set_indexed_var(&mut self, name, BTreeMap<usize,String>)` (shell_state.rs:1152) sets an indexed array (readonly:false). `Variable { value, exported, readonly, integer }` and `VarValue::Indexed` are the building blocks; readonly arrays need a direct `vars.insert` with `readonly:true`.
- `var_names()` (shell_state.rs:1605) = `vars.keys()`, used by completion (`completion.rs` Variable handler ~:404/:570; `completion_spec.rs` `Action::Variable` ~:419) and `compgen -v` (`builtins.rs` ~:4708). `declare -p`/`set` listing must stay on the raw vars table (don't route through the registry).
- `$$` is `shell_pid` (cached at Shell::new); a forked subshell keeps it, so `BASHPID = getpid()` correctly differs from `$$` in subshells.

---

### Task 1: Dynamic vars (RANDOM/SECONDS/EPOCHSECONDS/BASHPID) + assignability

**Files:** `src/shell_state.rs`, `tests/builtin_vars.rs` (new).

- [ ] **Step 1: Add state fields.** In `struct Shell`, add `pub random_state: std::cell::Cell<u64>` and `pub seconds_base: std::time::Instant`. In `Shell::new`'s struct literal, init:
```rust
    random_state: std::cell::Cell::new({
        // seed from pid + wall-clock nanos (reproducible only once reseeded via RANDOM=n)
        let pid = shell_pid as u64;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64).unwrap_or(0);
        pid.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(nanos) | 1
    }),
    seconds_base: std::time::Instant::now(),
```
(Fix any other `Shell` constructor the compiler flags — give them `Cell::new(1)` / `Instant::now()`.)

- [ ] **Step 2: Write failing tests** — create `tests/builtin_vars.rs`:
```rust
use std::process::Command;
fn huck(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).args(["-c", s]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}
#[test] fn random_in_range() {
    for _ in 0..20 {
        let n: i64 = huck("echo $RANDOM").trim().parse().unwrap();
        assert!((0..=32767).contains(&n), "RANDOM out of range: {n}");
    }
}
#[test] fn random_reseed_is_deterministic() {
    let a = huck("RANDOM=42; echo $RANDOM $RANDOM $RANDOM");
    let b = huck("RANDOM=42; echo $RANDOM $RANDOM $RANDOM");
    assert_eq!(a, b, "same seed must give same sequence");
    assert_ne!(a, huck("RANDOM=99; echo $RANDOM $RANDOM $RANDOM"), "different seed differs");
}
#[test] fn seconds_starts_zero_and_resets() {
    assert_eq!(huck("echo $SECONDS").trim(), "0");
    let n: i64 = huck("SECONDS=5; echo $SECONDS").trim().parse().unwrap();
    assert!(n >= 5, "SECONDS=5 should read >= 5, got {n}");
}
#[test] fn epochseconds_is_recent() {
    let n: i64 = huck("echo $EPOCHSECONDS").trim().parse().unwrap();
    assert!(n > 1_700_000_000, "EPOCHSECONDS too small: {n}");
}
#[test] fn bashpid_is_a_pid_and_differs_from_dollar_in_subshell() {
    let n: i64 = huck("echo $BASHPID").trim().parse().unwrap();
    assert!(n > 1);
    // in a subshell, BASHPID != $$ (the parent shell pid)
    assert_eq!(huck("echo $(( $BASHPID == $$ ))").trim(), "1"); // top level: equal
    assert_eq!(huck("( [ \"$BASHPID\" != \"$$\" ] && echo diff || echo same )").trim(), "diff");
}
```
(VERIFY the subshell BASHPID behavior against bash first; adjust if huck's top-level BASHPID==$$ semantics differ — bash: top-level BASHPID == $$, subshell differs.)

- [ ] **Step 3: Run — verify failure** (`$RANDOM` etc. expand empty → parse panics): `cargo build 2>&1 | tail -3 && cargo test --test builtin_vars 2>&1 | tail -15` → FAIL. Record.

- [ ] **Step 4: Implement lookup_var arms + a RANDOM helper.** Add a free fn near `lookup_var`:
```rust
/// Advance the LCG and return a value in 0..=32767 (15 bits), like bash's RANDOM range.
fn random_next(state: &std::cell::Cell<u64>) -> u32 {
    let s = state.get().wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    state.set(s);
    ((s >> 33) as u32) & 0x7fff
}
```
In `lookup_var`'s special-param `match name { … }` block, add:
```rust
    "RANDOM" => return Some(random_next(&self.random_state).to_string()),
    "SECONDS" => return Some(self.seconds_base.elapsed().as_secs().to_string()),
    "EPOCHSECONDS" => return Some(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0).to_string()),
    "BASHPID" => return Some((unsafe { libc::getpid() }).to_string()),
```

- [ ] **Step 5: Reseed/reset interception in `Shell::set`.** At the top of `Shell::set` (shell_state.rs:639), before the existing `match self.vars.get_mut(name)`:
```rust
    match name {
        "RANDOM" => {
            if let Ok(n) = value.parse::<u64>() {
                // reseed deterministically: same seed -> same sequence
                self.random_state.set(n.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(0x1234) | 1);
            }
            return;
        }
        "SECONDS" => {
            if let Ok(n) = value.parse::<u64>() {
                self.seconds_base = std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(n))
                    .unwrap_or_else(std::time::Instant::now);
            }
            return;
        }
        _ => {}
    }
```
Then confirm `apply_one_assignment` (executor.rs:4738) scalar path routes `RANDOM=`/`SECONDS=` through `Shell::set` — add a quick check; if it stores directly, route it through `set` for these names. (The `random_reseed_is_deterministic` and `seconds_*` tests cover both `RANDOM=42; …` standalone forms.)

- [ ] **Step 6: Run — verify pass + regression.**
`cargo test --test builtin_vars 2>&1 | tail -15` → all pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE.
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 7: Commit.**
```bash
git add src/shell_state.rs tests/builtin_vars.rs src/executor.rs
git commit -m "$(printf 'feat: dynamic builtin vars RANDOM/SECONDS/EPOCHSECONDS/BASHPID (assignable)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Static builtin vars in `Shell::new`

**Files:** `src/shell_state.rs`, `tests/builtin_vars.rs`.

- [ ] **Step 1: Write failing tests** — add to `tests/builtin_vars.rs`:
```rust
#[test] fn ids_match_bash() {
    for v in ["UID", "EUID", "PPID"] {
        let frag = format!("echo ${}", v);
        let b = std::process::Command::new("bash").args(["-c", &frag]).output().unwrap();
        let h = huck(&frag);
        assert_eq!(h.trim(), String::from_utf8_lossy(&b.stdout).trim(), "{v} mismatch");
    }
}
#[test] fn bash_version_present_and_versinfo() {
    assert_eq!(huck("[ -n \"$BASH_VERSION\" ] && echo yes").trim(), "yes");
    assert_eq!(huck("echo ${BASH_VERSINFO[0]}").trim(), "5");
    assert_eq!(huck("echo $HUCK_VERSION").trim(), env!("CARGO_PKG_VERSION"));
}
#[test] fn platform_and_host_present() {
    assert!(!huck("echo $HOSTNAME").trim().is_empty());
    assert!(!huck("echo $HOSTTYPE").trim().is_empty());
    assert!(!huck("echo $OSTYPE").trim().is_empty());
    assert!(!huck("echo $MACHTYPE").trim().is_empty());
    assert!(!huck("echo ${GROUPS[0]}").trim().is_empty());
    assert!(!huck("echo $BASH").trim().is_empty());
}
#[test] fn uid_is_readonly() {
    // bash: UID is readonly -> assignment errors (rc != 0), value unchanged
    let out = huck("UID=99999 2>/dev/null; echo $UID");
    let real: i64 = huck("echo $UID").trim().parse().unwrap();
    assert_eq!(out.trim().parse::<i64>().unwrap(), real);
}
#[test] fn shlvl_increments_from_env() {
    // huck increments an inherited SHLVL by 1
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", "echo $SHLVL"]).env("SHLVL", "5").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "6");
}
```
(For `ids_match_bash`, `bash` must be on PATH — it is in CI/dev. VERIFY `BASH_VERSINFO[0]` expected `5` matches the version string you choose.)

- [ ] **Step 2: Run — verify failure.** Record.

- [ ] **Step 3: Implement static-var installation.** Add helpers + an installer, called from `Shell::new` AFTER the struct is constructed (post-construction, like the BASH_FUNC loop). Helpers:
```rust
impl Shell {
    fn install_var(&mut self, name: &str, value: String, readonly: bool) {
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Scalar(value), exported: false, readonly, integer: false });
    }
    fn install_indexed(&mut self, name: &str, elems: Vec<String>, readonly: bool) {
        let map: BTreeMap<usize, String> = elems.into_iter().enumerate().collect();
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Indexed(map), exported: false, readonly, integer: false });
    }
    fn install_builtin_vars(&mut self) {
        unsafe {
            self.install_var("UID", libc::getuid().to_string(), true);
            self.install_var("EUID", libc::geteuid().to_string(), true);
            self.install_var("PPID", libc::getppid().to_string(), true);
        }
        // GROUPS via getgroups(2)
        let groups = current_groups();
        self.install_indexed("GROUPS", groups.iter().map(|g| g.to_string()).collect(), true);
        self.install_var("HOSTNAME", hostname(), false);
        self.install_var("HOSTTYPE", std::env::consts::ARCH.to_string(), false);
        self.install_var("OSTYPE", OSTYPE.to_string(), false);
        self.install_var("MACHTYPE", machtype(), false);
        self.install_var("BASH", std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "huck".into()), false);
        self.install_var("BASH_VERSION", "5.2.0(1)-release".to_string(), false);
        self.install_indexed("BASH_VERSINFO",
            vec!["5".into(),"2".into(),"0".into(),"1".into(),"release".into(), machtype()], true);
        self.install_var("HUCK_VERSION", env!("CARGO_PKG_VERSION").to_string(), false);
        // SHLVL: inherited numeric + 1, exported
        let lvl = self.vars.get("SHLVL").and_then(|v| v.value.scalar_view().parse::<i64>().ok()).unwrap_or(0);
        let next = (lvl + 1).max(1);
        self.vars.insert("SHLVL".to_string(), Variable {
            value: VarValue::Scalar(next.to_string()), exported: true, readonly: false, integer: false });
    }
}
```
Free fns + platform constants:
```rust
#[cfg(target_os = "linux")] const OSTYPE: &str = "linux-gnu";
#[cfg(target_os = "macos")] const OSTYPE: &str = "darwin";
#[cfg(not(any(target_os = "linux", target_os = "macos")))] const OSTYPE: &str = "unknown";

fn machtype() -> String {
    let arch = std::env::consts::ARCH;
    #[cfg(target_os = "linux")] { format!("{arch}-pc-linux-gnu") }
    #[cfg(target_os = "macos")] { format!("{arch}-apple-darwin") }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))] { format!("{arch}-unknown") }
}
fn current_groups() -> Vec<u32> {
    unsafe {
        let n = libc::getgroups(0, std::ptr::null_mut());
        if n <= 0 { return Vec::new(); }
        let mut buf = vec![0 as libc::gid_t; n as usize];
        let m = libc::getgroups(n, buf.as_mut_ptr());
        if m < 0 { return Vec::new(); }
        buf.truncate(m as usize);
        buf.into_iter().map(|g| g as u32).collect()
    }
}
fn hostname() -> String {
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 { return String::new(); }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}
```
Call `shell.install_builtin_vars();` in `Shell::new` after the struct is built and after the env-load loop / BASH_FUNC import (so it overwrites any inherited values and the SHLVL read sees the inherited value first).

- [ ] **Step 4: Run — verify pass.**
`cargo test --test builtin_vars 2>&1 | tail -20` → all pass. (If `GROUPS` byte-compare to bash is flaky — order/egid differences — change `ids_match_bash`-style GROUPS assertions to a behavior check, NON-empty + numeric, and note it; the harness in Task 4 handles GROUPS carefully.)
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE. (Watch for existing tests that assumed UID/PPID/etc. were UNSET — update them.)
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 5: Commit.**
```bash
git add src/shell_state.rs tests/builtin_vars.rs
git commit -m "$(printf 'feat: static builtin vars (UID/EUID/PPID/GROUPS/HOSTNAME/BASH_VERSION/...) in Shell::new\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Completion registry

**Files:** `src/shell_state.rs`, `src/completion.rs`, `src/completion_spec.rs`, `src/builtins.rs`.

- [ ] **Step 1: Write failing test** — add to `tests/builtin_vars.rs`:
```rust
#[test] fn compgen_v_lists_dynamic_specials() {
    let out = huck("compgen -v");
    for v in ["RANDOM", "SECONDS", "LINENO", "BASHPID", "UID", "BASH_VERSION", "BASH_SOURCE"] {
        assert!(out.lines().any(|l| l == v), "compgen -v should list {v}; got:\n{out}");
    }
}
```
(`compgen -v` lists variable names one per line. UID/BASH_VERSION come from the stored vars; RANDOM/SECONDS/LINENO/BASHPID/BASH_SOURCE from the registry.)

- [ ] **Step 2: Run — verify failure** (RANDOM/LINENO/etc. missing from compgen -v). Record.

- [ ] **Step 3: Add the registry + `completion_var_names()`.** In `src/shell_state.rs`:
```rust
/// Special variables that are valid/known but not always present in the vars table
/// (computed dynamics + the sometimes-unset call-stack arrays). Surfaced in variable-name
/// completion and `compgen -v` so they complete like bash even when unset.
pub const DYNAMIC_SPECIAL_VARS: &[&str] =
    &["RANDOM", "SECONDS", "EPOCHSECONDS", "BASHPID", "LINENO", "BASH_SOURCE", "BASH_LINENO"];

impl Shell {
    /// Variable names for completion / `compgen -v`: the vars table plus the known
    /// dynamic/special names not always stored. Deduped; caller sorts.
    pub fn completion_var_names(&self) -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> =
            self.vars.keys().cloned().collect();
        for &n in DYNAMIC_SPECIAL_VARS { set.insert(n.to_string()); }
        set.into_iter().collect()
    }
}
```

- [ ] **Step 4: Wire the THREE completion sites to `completion_var_names()`** (leave `var_names()` and `declare -p`/`set` listing UNCHANGED):
  - `src/completion.rs` ~:404 and ~:570 — the `CompletionContext::Variable { prefix }` handlers: replace `shell.var_names().map(|s| s.to_string()).collect()` with `shell.completion_var_names()`.
  - `src/completion_spec.rs` ~:419 — `Action::Variable`: replace `shell.var_names().filter(…)` with iterating `shell.completion_var_names()`.
  - `src/builtins.rs` ~:4708 — the compgen `-v`/`-A variable` name gather: replace `shell.var_names()…` with `shell.completion_var_names()`.

- [ ] **Step 5: Run — verify pass.**
`cargo test --test builtin_vars compgen_v 2>&1 | tail -8` → pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE.
Manual: `target/debug/huck -c 'compgen -v | grep -E "^(RANDOM|LINENO|FUNCNAME)$"'` → lists `RANDOM`, `LINENO`, but NOT `FUNCNAME` (not in registry; matches bash top-level). Paste it.
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 6: Commit.**
```bash
git add src/shell_state.rs src/completion.rs src/completion_spec.rs src/builtins.rs tests/builtin_vars.rs
git commit -m "$(printf 'feat: completion registry so computed special vars complete + show in compgen -v\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: bash-diff harness + full regression

**Files:** `tests/scripts/builtin_vars_diff_check.sh`.

- [ ] **Step 1: Write the harness** — byte-compare ONLY the comparable vars (same user/host/platform); behavior-check the rest inline. `check_c` runs the SAME fragment through `bash -c` and `huck -c` from the same parent (so `$PPID` matches).
```bash
#!/usr/bin/env bash
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first" >&2; exit 1; }
PASS=0; FAIL=0
check_c(){ local l="$1" f="$2" b h; b=$(bash -c "$f" 2>&1); h=$("$HUCK_BIN" -c "$f" 2>&1)
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1)); else printf 'FAIL: %s\n  bash=[%s] huck=[%s]\n' "$l" "$b" "$h"; FAIL=$((FAIL+1)); fi; }
check_true(){ local l="$1" f="$2" h; h=$("$HUCK_BIN" -c "$f" 2>&1)
  if [[ "$h" == "OK" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1)); else printf 'FAIL: %s (got [%s])\n' "$l" "$h"; FAIL=$((FAIL+1)); fi; }
# byte-identical (same user/host/platform; both -c share this shell as parent -> same PPID)
check_c "UID"       'echo $UID'
check_c "EUID"      'echo $EUID'
check_c "PPID"      'echo $PPID'
check_c "HOSTNAME"  'echo $HOSTNAME'
check_c "HOSTTYPE"  'echo $HOSTTYPE'
check_c "OSTYPE"    'echo $OSTYPE'
check_c "MACHTYPE"  'echo $MACHTYPE'
check_c "GROUPS"    'echo "${GROUPS[@]}"'
# behavior checks (not byte-comparable to bash)
check_true "RANDOM range"  'r=$RANDOM; [ "$r" -ge 0 ] && [ "$r" -le 32767 ] && echo OK'
check_true "RANDOM reseed" 'RANDOM=7; a=$RANDOM; RANDOM=7; b=$RANDOM; [ "$a" = "$b" ] && echo OK'
check_true "SECONDS zero"  '[ "$SECONDS" = "0" ] && echo OK'
check_true "SECONDS reset" 'SECONDS=5; [ "$SECONDS" -ge 5 ] && echo OK'
check_true "BASH_VERSION"  '[ -n "$BASH_VERSION" ] && echo OK'
check_true "EPOCHSECONDS"  '[ "$EPOCHSECONDS" -gt 1700000000 ] && echo OK'
check_true "BASHPID"       '[ "$BASHPID" -gt 1 ] && echo OK'
# completion parity: both shells list these in compgen -v
check_c "compgen RANDOM"   'compgen -v | grep -c "^RANDOM$"'
check_c "compgen LINENO"   'compgen -v | grep -c "^LINENO$"'
check_c "compgen UID"      'compgen -v | grep -c "^UID$"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL>0 ? 1:0 ))
```
(If `MACHTYPE`/`OSTYPE`/`GROUPS` diverge from bash on this box — bash's `MACHTYPE` vendor string can differ, GROUPS order/egid can differ — move that specific check to `check_true` with a sensible non-empty/format assertion and note it in a comment. On Linux x86_64, `OSTYPE=linux-gnu`, `HOSTTYPE=x86_64`, `MACHTYPE=x86_64-pc-linux-gnu` should match bash; verify.)

- [ ] **Step 2: Run the harness.** `chmod +x tests/scripts/builtin_vars_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/builtin_vars_diff_check.sh` — paste FULL output, all PASS. If a byte-compare check reveals a REAL divergence that isn't a documented edge (MACHTYPE/OSTYPE/GROUPS nuance), STOP and report BLOCKED with the diff.

- [ ] **Step 3: Full regression + clippy.**
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE.
`for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done; echo done` → only `done`; report `ls tests/scripts/*_diff_check.sh | wc -l`.
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 4: Commit.**
```bash
git add tests/scripts/builtin_vars_diff_check.sh
git commit -m "$(printf 'test: builtin-vars bash-diff harness (comparable) + behavior + completion checks\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer
- **RANDOM is `&self`-read** — the `Cell<u64>` interior mutability is essential (don't make `lookup_var` `&mut`). The LCG is huck's own; only range + reseed-determinism matter (never byte-compare RANDOM to bash).
- **Static vars overwrite inherited env values** — install them AFTER the env-load loop in `Shell::new`, and read `SHLVL` from the inherited value BEFORE overwriting (the `install_builtin_vars` code does this).
- **Readonly** UID/EUID/PPID/GROUPS/BASH_VERSINFO — relies on the existing readonly-enforcement in the assignment path; verify `UID=5` errors.
- **Don't touch `var_names()` / `declare -p`** — only the three completion/`compgen -v` sites use the registry, so `declare` never tries to format a computed var.
- **Watch for existing tests** that assumed UID/PPID/HOSTNAME/etc. were unset, or that counted variables — Task 2 will surface them; update to reflect the new always-present vars.
- **macOS:** all libc calls are POSIX; `OSTYPE`/`MACHTYPE` are `#[cfg]`-gated (approximate on macOS — the harness's byte-compare for those is Linux-targeted).
- **Git safety:** stay on `v154-builtin-vars`; do NOT `git checkout <sha>`.
