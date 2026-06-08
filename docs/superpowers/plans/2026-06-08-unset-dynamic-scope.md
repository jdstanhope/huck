# v118 — `unset -v` dynamic-scope reveal/pop (M-115) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `unset -v NAME` honor bash's dynamic scope — when NAME is `local` to an *enclosing* function, pop that one scope level and reveal the next-enclosing binding — so the bash "upvar" idiom (`unset -v NAME; eval NAME=val` writing into a caller's variable across an intervening `local`) works, finally making `mise<TAB>` functional.

**Architecture:** Add a scope-aware `Shell::unset_var(name)` and route ONLY the `unset` builtin's variable path through it. huck's scope model is a flat `vars` map plus a stack of restore-snapshots (`local_scopes`); `unset_var` finds the nearest frame holding a snapshot for `name`: top frame (or none) → plain `vars.remove` (current behavior); an enclosing frame → pop that frame's snapshot (no restore-clobber on its return) and reveal its shadowed value. Plain `Shell::unset` and its many internal callers are untouched.

**Tech Stack:** Rust. `src/shell_state.rs` (`unset_var`), `src/builtins.rs` (`builtin_unset`). Tests: `cargo test`, a new integration test, a new `tests/scripts/*_diff_check.sh` harness.

**Spec:** `docs/superpowers/specs/2026-06-08-unset-dynamic-scope-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `Shell::unset` (`src/shell_state.rs:580`): `pub fn unset(&mut self, name: &str) { self.vars.remove(name); }` — leave UNCHANGED; add `unset_var` next to it.
- `local_scopes` (`src/shell_state.rs:327`): `pub local_scopes: Vec<std::collections::HashMap<String, Option<Variable>>>` — stack, top = last; `local NAME` records the pre-`local` value (`Option<Variable>`: `Some(var)` shadowed a binding, `None` shadowed nothing).
- `builtin_unset` variable path (`src/builtins.rs:616`): `shell.unset(arg);` after the readonly guard (`:611-615`).

**Verified bash contract (probed; `outer` has `x=orig`):**
A `inner(){ unset -v "$1"; eval $1=VAL; }; mid(){ local x=midval; inner x; echo "mid:[$x]"; }; outer(){ local x=orig; mid x; echo "out:[$x]"; }` → `mid:[VAL]` `out:[VAL]`.
B `inner(){ unset -v "$1"; }; mid(){ local x=midval; inner x; echo "mid:[${x-U}]"; }; outer{…}` → `mid:[orig]` `out:[orig]`.
C `inner(){ local x=il; unset -v "$1"; eval $1=VAL; }; outer(){ local x=orig; inner x; echo "[$x]"; }` → `[orig]`.
D 3 intervening locals → `[orig]`.
E global only → `[VAL]`.
F `inner(){ local x=iv; unset -v x; echo "[${x-U}]"; }; outer{ local x=orig; inner; echo "[$x]"; }` → `[U]` `[orig]`.
G `leaf(){ unset -v "$1"; eval $1=VAL; }; pass(){ leaf "$1"; }; mid(){ local x=mv; pass x; echo "mid:[$x]"; }; outer{ local x=orig; mid x; echo "out:[$x]"; }` → `mid:[VAL]` `out:[VAL]`.
H `inner(){ unset -v "$1"; }; mid(){ local x=mv; inner x; x=re; echo "mid:[$x]"; }; outer{ local x=orig; mid x; echo "out:[$x]"; }` → `mid:[re]` `out:[re]`.

---

## Task 1: `Shell::unset_var` + wire the builtin + tests

**Files:**
- Modify: `src/shell_state.rs` (add `unset_var` after `unset`/`restore_var` ~`:580-601`; unit tests)
- Modify: `src/builtins.rs` (`:616`)
- Create: `tests/unset_dynamic_scope_integration.rs`

- [ ] **Step 1: Write the failing integration tests (cases A–H)**

Create `tests/unset_dynamic_scope_integration.rs`. The `run` helper executes a script as a FILE ARG (not piped stdin) with a pid+atomic-counter temp path (avoids the L-27 piped-stdin history-expansion path and multithreaded-test races):
```rust
//! v118: `unset -v` dynamic-scope reveal/pop (M-115).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `script` as a file-arg (true non-interactive path). Returns stdout.
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v118_{}_{}.sh", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(script.as_bytes()).unwrap();
    }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().expect("spawn huck");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_unset_eval_promotes_across_intervening_local() {
    let s = "inner(){ unset -v \"$1\"; eval $1=VAL; }\n\
             mid(){ local x=midval; inner x; echo \"mid:[$x]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[VAL]\nout:[VAL]\n");
}
#[test]
fn b_bare_unset_reveals_enclosing() {
    let s = "inner(){ unset -v \"$1\"; }\n\
             mid(){ local x=midval; inner x; echo \"mid:[${x-U}]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[${x-U}]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[orig]\nout:[orig]\n");
}
#[test]
fn c_current_fn_local_stays_local() {
    let s = "inner(){ local x=il; unset -v \"$1\"; eval $1=VAL; }\n\
             outer(){ local x=orig; inner x; echo \"[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "[orig]\n");
}
#[test]
fn d_three_intervening_locals() {
    let s = "leaf(){ unset -v \"$1\"; eval $1=VAL; }\n\
             a(){ local x=av; leaf x; }\n\
             b(){ local x=bv; a x; }\n\
             outer(){ local x=orig; b x; echo \"[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "[orig]\n");
}
#[test]
fn e_global_only() {
    let s = "inner(){ unset -v \"$1\"; eval $1=VAL; }\nx=global\ninner x\necho \"[$x]\"\n";
    assert_eq!(run(s), "[VAL]\n");
}
#[test]
fn f_current_fn_local_reads_unset_after() {
    let s = "inner(){ local x=iv; unset -v x; echo \"in:[${x-U}]\"; }\n\
             outer(){ local x=orig; inner; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "in:[U]\nout:[orig]\n");
}
#[test]
fn g_unset_skips_intervening_nonlocal_frame() {
    let s = "leaf(){ unset -v \"$1\"; eval $1=VAL; }\n\
             pass(){ leaf \"$1\"; }\n\
             mid(){ local x=mv; pass x; echo \"mid:[$x]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[VAL]\nout:[VAL]\n");
}
#[test]
fn h_caller_reassigns_after_callee_unset() {
    let s = "inner(){ unset -v \"$1\"; }\n\
             mid(){ local x=mv; inner x; x=re; echo \"mid:[$x]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[re]\nout:[re]\n");
}
```

- [ ] **Step 2: Run the tests — confirm they fail**

Run: `cargo build --bin huck && cargo test --test unset_dynamic_scope_integration 2>&1 | tail -20`
Expected: A, B, G, H FAIL (clobber / no-reveal); C, D, E, F PASS (already correct).

- [ ] **Step 3: Add `Shell::unset_var`**

In `src/shell_state.rs`, immediately AFTER `pub fn unset` (`:580-582`), add:
```rust
    /// Scope-aware variable unset for the `unset` builtin's `-v`/default path
    /// (M-115). Implements bash's dynamic scope: `unset NAME` acts on the
    /// nearest dynamically-visible binding.
    ///
    /// - NAME local to the CURRENT function (snapshot in the TOP `local_scopes`
    ///   frame), or not local anywhere: plain `vars.remove` — the local
    ///   attribute persists via the kept snapshot, a read shows unset, and the
    ///   enclosing binding is restored on return (cases C/F/E).
    /// - NAME local to an ENCLOSING function (snapshot in a lower frame): pop
    ///   that frame's snapshot (so it will NOT restore-clobber on return) and
    ///   reveal the value it was shadowing, so a subsequent assignment promotes
    ///   upward (cases A/B/D/G/H).
    ///
    /// `Shell::unset` (plain `vars.remove`) is left for internal callers.
    pub fn unset_var(&mut self, name: &str) {
        // Nearest frame holding a snapshot for `name`, innermost-first
        // (the stack's top is the last element).
        let nearest = self
            .local_scopes
            .iter()
            .rposition(|frame| frame.contains_key(name));
        match nearest {
            // An ENCLOSING frame (not the top) localized `name`: pop it + reveal.
            Some(i) if i + 1 < self.local_scopes.len() => {
                match self.local_scopes[i].remove(name) {
                    Some(Some(var)) => {
                        self.vars.insert(name.to_string(), var);
                    }
                    Some(None) => {
                        self.vars.remove(name);
                    }
                    // rposition just found the key, so the entry is present.
                    None => {}
                }
            }
            // Top-frame local, or not local anywhere: plain unset.
            _ => {
                self.vars.remove(name);
            }
        }
    }
```

- [ ] **Step 4: Wire the `unset` builtin**

In `src/builtins.rs:616`, change:
```rust
        shell.unset(arg);
```
to:
```rust
        shell.unset_var(arg);
```
(The readonly guard at `:611-615` and the `-f`/subscripted-element paths above are unchanged.)

- [ ] **Step 5: Run the integration tests — confirm green**

Run: `cargo build --bin huck && cargo test --test unset_dynamic_scope_integration 2>&1 | tail -15`
Expected: all 8 (A–H) PASS.

- [ ] **Step 6: Add unit tests for `unset_var` in `src/shell_state.rs`**

In the existing `#[cfg(test)] mod` in `src/shell_state.rs` (where `unset_removes_variable` lives ~`:1332`), add:
```rust
    #[test]
    fn unset_var_enclosing_local_pops_and_reveals() {
        let mut s = Shell::new();
        // outer frame: shadowed nothing (None); enclosing "mid" frame: shadowed orig.
        s.set("x", "midval".into());
        let mut outer = std::collections::HashMap::new();
        outer.insert("x".to_string(), None); // outer's `local x` shadowed an unset global
        let mut mid = std::collections::HashMap::new();
        mid.insert("x".to_string(), Some(crate::shell_state::Variable::scalar("orig".into())));
        s.local_scopes.push(outer);
        s.local_scopes.push(mid);
        s.local_scopes.push(std::collections::HashMap::new()); // top frame (inner): no local x
        s.unset_var("x");
        // mid's snapshot popped; revealed its shadowed value "orig".
        assert_eq!(s.get("x"), Some("orig"));
        assert!(!s.local_scopes[1].contains_key("x"));
    }

    #[test]
    fn unset_var_top_frame_local_plain_removes() {
        let mut s = Shell::new();
        s.set("x", "v".into());
        let mut top = std::collections::HashMap::new();
        top.insert("x".to_string(), Some(crate::shell_state::Variable::scalar("orig".into())));
        s.local_scopes.push(top);
        s.unset_var("x");
        assert_eq!(s.get("x"), None);                 // value removed
        assert!(s.local_scopes[0].contains_key("x")); // snapshot KEPT (restores on return)
    }

    #[test]
    fn unset_var_no_frames_plain_removes() {
        let mut s = Shell::new();
        s.set("x", "v".into());
        s.unset_var("x");
        assert_eq!(s.get("x"), None);
    }
```
(Verify the `Variable::scalar` constructor and `Variable` import path match the file — `unset_removes_variable` and nearby tests show the in-module conventions; adjust the path/constructor to match, e.g. `Variable::scalar(...)` is used elsewhere in this module.)

- [ ] **Step 7: Run unit + full regression + clippy**

Run: `cargo test --bin huck unset_var 2>&1 | tail -6` (3 pass) ; `cargo test 2>&1 | grep -E "test result: FAILED" || echo "no failures"` ; `cargo clippy --all-targets 2>&1 | tail -3` (clean). Watch `local`/`declare`/`unset`/`getopts`/`cd`/completion suites.

- [ ] **Step 8: Byte-identical spot check vs bash**

```bash
cargo build --bin huck
for f in 'inner(){ unset -v "$1"; eval $1=VAL; }; mid(){ local x=midval; inner x; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer' \
         'inner(){ unset -v "$1"; }; mid(){ local x=mv; inner x; x=re; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer' \
         'leaf(){ unset -v "$1"; eval $1=VAL; }; pass(){ leaf "$1"; }; mid(){ local x=mv; pass x; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer' \
         'inner(){ local x=il; unset -v "$1"; eval $1=VAL; }; outer(){ local x=orig; inner x; echo "$x"; }; outer'; do
  printf '%s\n' "$f" > /tmp/t.sh
  b=$(bash --norc --noprofile /tmp/t.sh 2>&1); h=$(./target/debug/huck /tmp/t.sh 2>&1)
  [ "$b" = "$h" ] && echo "MATCH" || { echo "DIFF: $f"; echo " b=[$b] h=[$h]"; }
done
```
Expected: 4 MATCH.

- [ ] **Step 9: Commit**

```bash
git add src/shell_state.rs src/builtins.rs tests/unset_dynamic_scope_integration.rs
git commit -m "$(cat <<'EOF'
fix: unset -v honors dynamic scope (pop enclosing local + reveal) (M-115)

New Shell::unset_var (used by the unset builtin's -v/default path): when NAME
is local to an ENCLOSING function it pops that frame's restore-snapshot and
reveals the shadowed binding, so bash's upvar idiom (unset -v NAME; eval
NAME=val across an intervening local) promotes the value upward instead of
being clobbered on the intervening frame's return. Current-fn-local and global
unsets keep the plain vars.remove behavior; internal Shell::unset callers and
unset -f / unset arr[i] / readonly handling are untouched.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the `unset_var` method as written, the 8-case integration pass line, the 3 unit-test pass line, the 4 MATCH spot-check, full-suite green (no FAILED), clippy status.

---

## Task 2: 42nd bash-diff harness + payoff smoke

**Files:**
- Create: `tests/scripts/unset_dynamic_scope_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/unset_dynamic_scope_diff_check.sh` (file-arg execution per L-27):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v118: `unset -v` dynamic-scope
# reveal/pop (M-115). Cases A-H + readonly / unset -f / unset arr[i] guards.
# File-arg execution (L-27: huck history-expands piped stdin).
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

check "A promote across local"  'inner(){ unset -v "$1"; eval $1=VAL; }; mid(){ local x=mv; inner x; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer'
check "B bare unset reveals"    'inner(){ unset -v "$1"; }; mid(){ local x=mv; inner x; echo "m:${x-U}"; }; outer(){ local x=orig; mid x; echo "o:${x-U}"; }; outer'
check "C current-fn-local"      'inner(){ local x=il; unset -v "$1"; eval $1=VAL; }; outer(){ local x=orig; inner x; echo "$x"; }; outer'
check "D three locals"          'leaf(){ unset -v "$1"; eval $1=VAL; }; a(){ local x=av; leaf x; }; b(){ local x=bv; a x; }; outer(){ local x=orig; b x; echo "$x"; }; outer'
check "E global only"           'inner(){ unset -v "$1"; eval $1=VAL; }; x=g; inner x; echo "$x"'
check "F read unset after"      'inner(){ local x=iv; unset -v x; echo "i:${x-U}"; }; outer(){ local x=orig; inner; echo "o:$x"; }; outer'
check "G skip nonlocal frame"   'leaf(){ unset -v "$1"; eval $1=VAL; }; pass(){ leaf "$1"; }; mid(){ local x=mv; pass x; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer'
check "H caller reassigns"      'inner(){ unset -v "$1"; }; mid(){ local x=mv; inner x; x=re; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer'
check "readonly unset rc"       'readonly r=x; unset r 2>/dev/null; echo "rc=$? r=$r"'
check "unset -f function"       'f(){ echo hi; }; unset -f f; type f 2>/dev/null; echo "rc=$?"'
check "unset array element"     'a=(p q r); unset "a[1]"; echo "${a[*]} n=${#a[@]}"'
check "global unset noreveal"   'g=top; f(){ unset -v g; echo "in:${g-U}"; }; f; echo "out:${g-U}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, run it, run ALL harnesses**

```bash
chmod +x tests/scripts/unset_dynamic_scope_diff_check.sh && cargo build --bin huck
bash tests/scripts/unset_dynamic_scope_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo all-harnesses-done
```
Expected: `Total: 12, Pass: 12, Fail: 0`; `count: 42`; no `FAIL` lines. If a fragment FAILs, report the diff (do NOT alter source; the controller decides). NOTE the `unset -f`/`readonly`/`unset arr[i]` fragments may surface a pre-existing message-prefix divergence — if so, report it; if the prefix differs but rc/behavior matches, the controller may swap to a behavior-only assertion.

- [ ] **Step 3: Payoff smoke (the full bash_completion chain)**

```bash
cargo build --bin huck
BC=/usr/share/bash-completion/bash_completion
{
  echo 'shopt -s extglob'
  sed -n '/^_upvars()/,/^}/p' "$BC"
  sed -n '/^__reassemble_comp_words_by_ref()/,/^}/p' "$BC"
  sed -n '/^__get_cword_at_cursor_by_ref()/,/^}/p' "$BC"
  sed -n '/^_get_comp_words_by_ref()/,/^}/p' "$BC"
  sed -n '/^_variables()/,/^}/p' "$BC"
  sed -n '/^_init_completion()/,/^}/p' "$BC"
  cat <<'DRIVE'
COMP_WORDBREAKS=$' \t\n"'"'"'><=;|&(:'
COMP_LINE='mise '; COMP_POINT=5; COMP_WORDS=(mise ""); COMP_CWORD=1
f() { local cur prev words cword split; _init_completion -n : || { echo "init rc=$?"; return; }
      echo "SMOKE cur=[$cur] prev=[$prev] cword=$cword nwords=${#words[@]} w0=[${words[0]}]"; }
f
DRIVE
} > /tmp/v118_smoke.sh
echo "--- bash ---"; bash --norc --noprofile /tmp/v118_smoke.sh 2>&1
echo "--- huck ---"; ./target/debug/huck /tmp/v118_smoke.sh 2>&1
```
Expected: huck prints `SMOKE cur=[] prev=[mise] cword=1 nwords=2 w0=[mise]` (matching bash), `_init_completion` rc 0 — **`mise<TAB>` functional**. Report the EXACT bash and huck blocks. If a residual remains (e.g. `_variables`/`__ltrim_colon_completions` command-not-found is a separate known gap), report honestly whether the SMOKE line now matches and rc is 0 — do NOT over-claim.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/unset_dynamic_scope_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 42nd bash-diff harness for unset -v dynamic scope (M-115)

12 byte-identical fragments (cases A-H + readonly / unset -f / unset arr[i] /
global-no-reveal guards). Payoff: the _init_completion -n : chain now yields
cword=1 nwords=2 prev=mise with rc 0.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the `Total: 12, Pass: 12` line (or the failing fragment + diff), the `count: 42` + no-FAIL line, and the EXACT payoff-smoke output. State clearly whether `mise<TAB>` is now functional (SMOKE line matches bash, rc 0) or what residual remains.

---

## Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|Bugs (Tier 1) |\|^## Change log\|### M-115:' docs/bash-divergences.md | head
grep -n '| v117 ' README.md
cargo test 2>&1 | awk '/test result:/{s+=$4} END{print "TESTCOUNT="s}'
```
Use the real TESTCOUNT for `<N>` below.

- [ ] **Step 2: Flip M-115 to fixed**

In `docs/bash-divergences.md`, in the `### M-115:` entry, change `- **Status**: \`[deferred]\` …` to `- **Status**: \`[fixed v118]\``, and append a fix line:
```
- **Fix (v118)**: new `Shell::unset_var` (`src/shell_state.rs`), used by the `unset` builtin's `-v`/default path: finds the nearest `local_scopes` frame holding a snapshot for the name; an ENCLOSING frame → pop its snapshot (no restore-clobber on return) + reveal its shadowed value; the TOP (current-function) frame or none → plain `vars.remove` (unchanged). Plain `Shell::unset` + internal callers, `unset -f`, `unset arr[i]`, and the readonly guard are untouched. Matches all 8 probed bash cases (A–H). 42nd harness + 8 integration tests.
```
If the smoke confirms (Task 2 Step 3), append to the Driver/Next line: `**PAYOFF: `mise<TAB>` is now functional** — `_init_completion` returns rc 0 with `cword=1 nwords=2 prev=mise`.` If the smoke shows a residual instead, write the honest status (what still fails) and DO NOT claim mise works.

- [ ] **Step 3: Update Tier-1 count + Last-updated**

- `| Bugs (Tier 1) | 23 |` → `23` stays the COUNT of entries (M-115 was already counted), but the "all fixed EXCEPT …" clause must drop M-115 (now fixed) so the EXCEPT set becomes just `M-114`. Append to the notes: `; M-115 unset -v dynamic-scope reveal/pop fixed v118`.
- "Last updated" line → replace with:
```
**Last updated:** 2026-06-08 (after v118: `unset -v` honors bash dynamic scope — unset of an enclosing function's local pops that level and reveals the next binding (M-115), so the bash_completion `_upvars` upvar idiom works <and `mise<TAB>` completes / and one residual remains: …>).
```
(Pick the parenthetical tail per the Task-2 smoke result — functional, or honest residual.)

- [ ] **Step 4: Change-log entry + README row**

Append to the END of `## Change log`:
```
- **2026-06-08**: M-115 (`unset -v` dynamic-scope reveal/pop) shipped as v118. New `Shell::unset_var` (used by the `unset` builtin's variable path): `unset` of a variable local to an ENCLOSING function pops that frame's restore-snapshot and reveals the shadowed binding, so bash's upvar idiom (`unset -v NAME; eval NAME=val` across an intervening `local`) promotes the value upward instead of being clobbered on the intervening frame's return; current-fn-local and global unsets keep plain `vars.remove`; internal callers / `unset -f` / `unset arr[i]` / readonly untouched. Matches the 8 probed bash cases A–H. <PAYOFF sentence per smoke.> 42nd harness `unset_dynamic_scope_diff_check.sh` (12 fragments) + 8 integration tests; full suite <N> tests pass, clippy clean.
```
Add a README row after the v117 row:
```
| v118      | **`unset -v` dynamic-scope reveal/pop (M-115)** — `unset NAME` ignored `local_scopes`, so unsetting a variable local to an ENCLOSING function let that frame's snapshot clobber any later value on return; bash's upvar idiom (`unset -v NAME; eval NAME=val` writing into a caller across an intervening `local`) broke. Fix: new `Shell::unset_var` (used by the `unset` builtin's variable path) pops the nearest enclosing frame's snapshot + reveals its shadowed binding; current-fn-local/global unsets keep plain `vars.remove`; `unset -f`/`unset arr[i]`/readonly/internal callers untouched. Matches the 8 probed bash cases A–H. <PAYOFF: mise<TAB> functional / honest residual>. 42nd harness `unset_dynamic_scope_diff_check.sh` (12 fragments) + 8 integration tests; full suite <N> tests pass, clippy clean |
```

- [ ] **Step 5: Verify + commit**

```bash
grep -n 'M-115\|fixed v118\|v118' docs/bash-divergences.md README.md | head
grep -n '<N>\|<PAYOFF' docs/bash-divergences.md README.md && echo "PLACEHOLDER LEFT — fix" || echo "no placeholders"
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v118 — unset -v dynamic-scope reveal/pop (M-115 fixed)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the grep proving M-115 `[fixed v118]`, confirmation no `<N>`/`<PAYOFF` placeholder remains, the test count used, and whether the docs state mise<TAB> functional or an honest residual.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 42 files).
- [ ] **Payoff**: the `_init_completion -n :` chain yields `cword=1 nwords=2 prev=mise` with rc 0 (Task 2 Step 3) — or the honest residual.
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`; MEMORY.md is near its size cap — compress older entries while updating). **This is intended to make `mise<TAB>` functional end-to-end — confirm with the user after merge; if a residual remains, report honestly and scope the next iteration.**
