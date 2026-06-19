# Honest Default Keymap for `bind -p`/`-P` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `bind -p`/`-P` emit huck's real default emacs keymap (rustyline's defaults for huck's 33 honored functions) layered with the user's bindings, instead of only user-added binds (nothing in `-c` mode).

**Architecture:** Add a static `DEFAULT_EMACS_BINDS` table; compute an effective keymap = defaults ⊕ user binds (active + pending) ⊖ unbinds; rewrite the two `bind -p`/`-P` renderers. The harness enforces that every binding huck emits is also in bash's `bind -p` (huck ⊆ bash — no fabrication).

**Tech Stack:** Rust. Files: `src/readline_bind.rs` (the table), `src/shell_state.rs` (`unbound` field, effective keymap, renderers). New harness `tests/scripts/bind_keymap_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-19-bind-p-default-keymap-design.md`

**Background the implementer needs:**
- `bind -p`/`-P` dispatch (`src/builtins.rs:7647-7648`) calls `shell.active_bind_lines()` / `active_bind_lines_verbose()` — unchanged; only those two methods change.
- `ReadlineSettings` (`src/shell_state.rs:~330`) has `active_binds: BTreeMap<String,String>` (keyseq→func, populated by the interactive loop), `pending_binds: Vec<(String,String)>` (user binds not yet applied — the ONLY place a `-c`-mode bind lives), `pending_unbinds: Vec<String>`. Add a persistent `unbound: BTreeSet<String>`.
- `quote_keyseq(k)` (`src/shell_state.rs:2406`, module-private free fn) wraps a keyseq in `"…"` if not already quoted — use it to NORMALIZE keys (defaults are `\C-a`; user binds may be stored as `"\C-a"`).
- `readline_bind::readline_function_names()` returns the 33 honored function names, ALREADY alphabetically sorted (matches bash's `bind -p` function ordering). `readline_bind::function_to_cmd`/`is_known_function` define the honored set.
- bash format: `-p` → `"keyseq": func` (one line per pair, sorted by func) + `# func (not bound)`; `-P` → `func can be found on "k1", "k2".` or `func is not bound to any keys`.
- Bash-diff harnesses: `tests/scripts/*_diff_check.sh`, run via `bash tests/scripts/<name>.sh`. `bash -c 'bind -p'` prints the full default keymap (494 lines) even non-interactively.

---

## Task 1: `DEFAULT_EMACS_BINDS` table + `unbound` field

**Files:**
- Modify: `src/readline_bind.rs` — add `DEFAULT_EMACS_BINDS`.
- Modify: `src/shell_state.rs` — add `unbound` to `ReadlineSettings` (+ `Default`), record it in `add_unbind`.
- Test: `src/readline_bind.rs` `mod tests`.

- [ ] **Step 1: Write the failing honesty test** in `src/readline_bind.rs` `mod tests`:

```rust
    #[test]
    fn default_emacs_binds_only_reference_honored_functions() {
        assert!(!DEFAULT_EMACS_BINDS.is_empty());
        for (seq, func) in DEFAULT_EMACS_BINDS {
            assert!(is_known_function(func), "default binds a function huck can't honor: {func}");
            assert!(!seq.is_empty());
        }
    }
```

- [ ] **Step 2: Run to confirm it FAILS.**

Run: `cargo test --lib default_emacs_binds_only_reference_honored_functions 2>&1 | tail -6`
Expected: FAIL — `DEFAULT_EMACS_BINDS` does not exist (compile error).

- [ ] **Step 3: Add the table** in `src/readline_bind.rs` (near `function_to_cmd`):

```rust
/// huck's default emacs key bindings — the standard emacs keys rustyline honors
/// for huck's supported functions, in bash's `bind -p` keyseq spelling. Each
/// entry is verified to appear in bash's own default `bind -p` (the harness
/// enforces this subset relation), so huck never reports a binding bash lacks.
/// Functions in the honored set with no entry here render as `# … (not bound)`.
pub const DEFAULT_EMACS_BINDS: &[(&str, &str)] = &[
    ("\\C-a", "beginning-of-line"), ("\\C-e", "end-of-line"),
    ("\\C-f", "forward-char"), ("\\C-b", "backward-char"),
    ("\\ef", "forward-word"), ("\\eb", "backward-word"),
    ("\\C-k", "kill-line"), ("\\C-u", "unix-line-discard"),
    ("\\C-w", "unix-word-rubout"), ("\\ed", "kill-word"),
    ("\\e\\C-?", "backward-kill-word"),
    ("\\C-l", "clear-screen"), ("\\C-g", "abort"),
    ("\\C-j", "accept-line"), ("\\C-m", "accept-line"),
    ("\\C-p", "previous-history"), ("\\C-n", "next-history"),
    ("\\e<", "beginning-of-history"), ("\\e>", "end-of-history"),
    ("\\C-r", "reverse-search-history"), ("\\C-s", "forward-search-history"),
    ("\\C-i", "complete"),
    ("\\eu", "upcase-word"), ("\\el", "downcase-word"),
    ("\\ec", "capitalize-word"), ("\\C-t", "transpose-chars"),
    ("\\et", "transpose-words"), ("\\C-_", "undo"),
    ("\\C-y", "yank"), ("\\C-d", "delete-char"),
    ("\\C-?", "backward-delete-char"),
];
```

- [ ] **Step 4: Add the `unbound` field** in `src/shell_state.rs`. In `ReadlineSettings`:
```rust
    /// Keyseqs the user removed via `bind -r` — subtracted from the effective
    /// keymap so unbinding a DEFAULT keyseq is reflected in `bind -p`/`-P`.
    pub unbound: std::collections::BTreeSet<String>,
```
In `ReadlineSettings`'s `Default` impl, initialize `unbound: std::collections::BTreeSet::new(),`. In `add_unbind`, after the existing `pending_unbinds.push(...)`, add:
```rust
        self.readline_settings.unbound.insert(keyseq.to_string());
```
(Leave the existing `pending_unbinds`/`dirty` lines.)

- [ ] **Step 5: Run to confirm PASS + build.**

Run: `cargo test --lib default_emacs_binds_only_reference_honored_functions 2>&1 | tail -6 && cargo build 2>&1 | tail -2`
Expected: PASS; clean build.

- [ ] **Step 6: Commit.**

```bash
git add src/readline_bind.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
v191: DEFAULT_EMACS_BINDS table + ReadlineSettings.unbound

huck's real emacs default keymap for its 33 honored functions (in bash bind -p
spelling), plus an `unbound` set so `bind -r` of a default keyseq is reflected.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: effective keymap + rewrite `bind -p`/`-P` renderers

**Files:**
- Modify: `src/shell_state.rs` — add `effective_binds`; rewrite `active_bind_lines` + `active_bind_lines_verbose`.
- Test: `src/shell_state.rs` `mod tests`.

- [ ] **Step 1: Write the failing tests** in `src/shell_state.rs` `mod tests`:

```rust
    #[test]
    fn bind_p_shows_defaults_user_override_and_unbind() {
        let mut sh = Shell::new();
        // default present
        let p = sh.active_bind_lines();
        assert!(p.iter().any(|l| l == "\"\\C-a\": beginning-of-line"), "missing default C-a: {p:?}");
        assert!(p.iter().any(|l| l == "# backward-kill-line (not bound)"), "missing not-bound line: {p:?}");
        // -P format
        let pv = sh.active_bind_lines_verbose();
        assert!(pv.iter().any(|l| l == "beginning-of-line can be found on \"\\C-a\"."), "{pv:?}");
        assert!(pv.iter().any(|l| l == "backward-kill-line is not bound to any keys"), "{pv:?}");
        // user override via pending_binds (the -c-mode path)
        sh.add_bind("\"\\C-a\"", "kill-line");
        let p2 = sh.active_bind_lines();
        assert!(p2.iter().any(|l| l == "\"\\C-a\": kill-line"), "override not applied: {p2:?}");
        assert!(!p2.iter().any(|l| l == "\"\\C-a\": beginning-of-line"), "default not overridden: {p2:?}");
        // unbind a default
        let mut sh2 = Shell::new();
        sh2.add_unbind("\\C-e");
        let p3 = sh2.active_bind_lines();
        assert!(!p3.iter().any(|l| l.contains("\\C-e")), "C-e still shown after unbind: {p3:?}");
    }
```

- [ ] **Step 2: Run to confirm it FAILS.**

Run: `cargo test --lib bind_p_shows_defaults_user_override_and_unbind 2>&1 | tail -12`
Expected: FAIL — current `active_bind_lines` returns only `active_binds` (empty for a fresh `Shell`).

- [ ] **Step 3: Implement.** In `src/shell_state.rs`, add the effective-keymap method (in `impl Shell`, near `active_bind_lines`):

```rust
    /// The effective key bindings (keyseq → function): the default emacs keymap,
    /// overlaid with the user's bindings (already-applied `active_binds` AND
    /// not-yet-applied `pending_binds`, so `-c`-mode binds show too), minus any
    /// keyseq the user unbound. Keyseqs are normalized to bash's quoted form.
    fn effective_binds(&self) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        for (k, f) in crate::readline_bind::DEFAULT_EMACS_BINDS {
            m.insert(quote_keyseq(k), (*f).to_string());
        }
        for (k, f) in &self.readline_settings.active_binds {
            m.insert(quote_keyseq(k), f.clone());
        }
        for (k, f) in &self.readline_settings.pending_binds {
            m.insert(quote_keyseq(k), f.clone());
        }
        for k in &self.readline_settings.unbound {
            m.remove(&quote_keyseq(k));
        }
        m
    }
```

Replace `active_bind_lines` and `active_bind_lines_verbose` with:

```rust
    /// `bind -p` lines: `"KEYSEQ": FUNCTION` for each effective binding, grouped
    /// and sorted by function name (matching bash); `# FUNCTION (not bound)` for
    /// honored functions with no binding.
    pub fn active_bind_lines(&self) -> Vec<String> {
        let eff = self.effective_binds();
        let mut by_func: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for (k, f) in &eff {
            by_func.entry(f.as_str()).or_default().push(k.as_str());
        }
        let mut out = Vec::new();
        for func in crate::readline_bind::readline_function_names() {
            match by_func.get(func) {
                Some(keys) => {
                    let mut keys = keys.clone();
                    keys.sort_unstable();
                    for k in keys {
                        out.push(format!("{k}: {func}"));
                    }
                }
                None => out.push(format!("# {func} (not bound)")),
            }
        }
        out
    }

    /// `bind -P` lines: `FUNCTION can be found on "K1", "K2".` (all keyseqs) or
    /// `FUNCTION is not bound to any keys`, per honored function sorted by name.
    pub fn active_bind_lines_verbose(&self) -> Vec<String> {
        let eff = self.effective_binds();
        let mut by_func: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for (k, f) in &eff {
            by_func.entry(f.as_str()).or_default().push(k.as_str());
        }
        let mut out = Vec::new();
        for func in crate::readline_bind::readline_function_names() {
            match by_func.get(func) {
                Some(keys) => {
                    let mut keys = keys.clone();
                    keys.sort_unstable();
                    out.push(format!("{func} can be found on {}.", keys.join(", ")));
                }
                None => out.push(format!("{func} is not bound to any keys")),
            }
        }
        out
    }
```

(Delete the old one-line `.map(...)` bodies of both methods.)

- [ ] **Step 4: Run to confirm PASS + no regressions.**

Run: `cargo test --lib bind_p_shows_defaults_user_override_and_unbind 2>&1 | tail -6 && cargo test --lib 2>&1 | grep "test result:" | grep -v "0 failed" || echo OK`
Expected: PASS; `OK`. If an existing `active_bind_lines`/`bind` test asserted the old empty/user-only output, update it to the new default-keymap output (verify the expected lines against `bash -c 'bind -p'`).

- [ ] **Step 5: Manual cross-check vs bash.**

Run:
```bash
cargo build 2>&1 | tail -1
echo "=== huck binding lines NOT in bash bind -p (must be EMPTY = honesty) ==="
comm -23 <(./target/debug/huck -c 'bind -p' 2>/dev/null | grep '^"' | sort -u) \
         <(bash -c 'bind -p' 2>/dev/null | grep '^"' | sort -u)
```
Expected: EMPTY (every huck binding line is also a bash binding line). If a line prints, that table entry's keyseq/func disagrees with bash — STOP and report it (fix the table entry; do NOT keep a binding bash doesn't have).

- [ ] **Step 6: Commit.**

```bash
git add src/shell_state.rs
git commit -m "$(cat <<'EOF'
v191: bind -p/-P emit the effective default+user keymap

effective_binds = DEFAULT_EMACS_BINDS overlaid with user binds (active +
pending) minus unbinds; active_bind_lines/_verbose render it in bash's format
(sorted by function, `# (not bound)` / `is not bound to any keys`).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: bash-diff harness

**Files:**
- Create: `tests/scripts/bind_keymap_diff_check.sh`

- [ ] **Step 1: Create the harness:**

```bash
#!/usr/bin/env bash
# bash<->huck harness for v191: bind -p/-P default-keymap honesty + format.
# huck uses rustyline (fewer functions than GNU readline), so we do NOT expect
# byte-identical full output; we assert (a) huck's keymap is a SUBSET of bash's
# (no fabricated bindings), (b) core bindings match bash's exact line, (c) user
# override/unbind behave like bash.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf 'PASS: %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL: %s\n' "$1"; shift; printf '%s\n' "$@" | sed 's/^/    /'; }

# (a) honesty: every huck `"...": func` line must be a whole line in bash bind -p
extra=$(comm -23 \
  <("$HUCK_BIN" -c 'bind -p' 2>/dev/null | grep '^"' | sort -u) \
  <(bash -c 'bind -p' 2>/dev/null | grep '^"' | sort -u))
if [[ -z "$extra" ]]; then ok "huck bind -p subset of bash"; else bad "huck has bindings bash lacks" "$extra"; fi

# (b) core bindings: huck's exact line == bash's exact line
core_check() {
    local label="$1" pat="$2" b h
    b=$(bash -c 'bind -p' 2>/dev/null | grep -F "$pat")
    h=$("$HUCK_BIN" -c 'bind -p' 2>/dev/null | grep -F "$pat")
    if [[ "$b" == "$h" && -n "$h" ]]; then ok "$label"; else bad "$label" "bash: $b" "huck: $h"; fi
}
core_check "C-a beginning-of-line" '"\C-a": beginning-of-line'
core_check "C-e end-of-line"       '"\C-e": end-of-line'
core_check "C-k kill-line"         '"\C-k": kill-line'
core_check "C-y yank"              '"\C-y": yank'
core_check "C-i complete"          '"\C-i": complete'

# (c) -P line for beginning-of-line: huck's exact line, and bash's line starts the same
hP=$("$HUCK_BIN" -c 'bind -P' 2>/dev/null | grep '^beginning-of-line can be found on')
bP=$(bash -c 'bind -P' 2>/dev/null | grep '^beginning-of-line can be found on')
if [[ "$hP" == 'beginning-of-line can be found on "\C-a".' && "$bP" == 'beginning-of-line can be found on "\C-a"'* ]]; then
  ok "-P beginning-of-line format"; else bad "-P beginning-of-line format" "bash: $bP" "huck: $hP"; fi

# (d) user override: rebinding C-a to kill-line (no space after colon — see L-note)
ov_b=$(bash -c 'bind "\"\C-a\":kill-line"; bind -p' 2>/dev/null | grep -F '"\C-a"')
ov_h=$("$HUCK_BIN" -c 'bind "\"\C-a\":kill-line"; bind -p' 2>/dev/null | grep -F '"\C-a"')
if [[ "$ov_b" == "$ov_h" && "$ov_h" == '"\C-a": kill-line' ]]; then ok "user override C-a"; else bad "user override C-a" "bash: $ov_b" "huck: $ov_h"; fi

# (e) unbind a default: C-a gone from both
ub_b=$(bash -c 'bind -r "\C-a"; bind -p' 2>/dev/null | grep -c '"\\C-a"')
ub_h=$("$HUCK_BIN" -c 'bind -r "\C-a"; bind -p' 2>/dev/null | grep -c '"\\C-a"')
if [[ "$ub_b" == "$ub_h" && "$ub_h" == 0 ]]; then ok "unbind default C-a"; else bad "unbind default C-a" "bash:$ub_b huck:$ub_h"; fi

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable and run.**

Run:
```bash
chmod +x tests/scripts/bind_keymap_diff_check.sh
cargo build 2>&1 | tail -1
bash tests/scripts/bind_keymap_diff_check.sh
```
Expected: all `PASS`, `Fail: 0`, exit 0. If the subset check (a) fails, a `DEFAULT_EMACS_BINDS` entry isn't in bash's keymap — FIX the table entry (correct or drop it). If a `core_check` fails because bash spells the keyseq differently (e.g. `\C-i` vs something), report it and adjust. If the override (d) or unbind (e) case behaves unexpectedly, report the exact output (do NOT weaken).

- [ ] **Step 3: Prove non-tautological** (fails pre-fix):

```bash
BASE=$(git merge-base HEAD main)
git worktree add -d /tmp/huck-prefix "$BASE" 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/bind_keymap_diff_check.sh | tail -4
git worktree remove --force /tmp/huck-prefix 2>&1 | tail -1
```
Expected: the core/`-P`/override/unbind cases FAIL pre-fix (old huck emits nothing). The subset check (a) PASSES pre-fix vacuously (empty ⊆ bash). Report the counts.

- [ ] **Step 4: Commit.**

```bash
git add tests/scripts/bind_keymap_diff_check.sh
git commit -m "$(cat <<'EOF'
v191: bash-diff harness for bind -p/-P default keymap

Asserts huck's bind -p is a subset of bash's (no fabricated bindings), core
bindings match bash's exact lines, and user override/unbind behave like bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: full regression + docs + memory

**Files:**
- Modify (memory): `project_huck_iterations.md`, `MEMORY.md`.
- Maybe modify: `docs/bash-divergences.md`.

- [ ] **Step 1: Full test suite (0 failures).**

Run: `cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"`
Expected: `ALL GREEN`. Update any test that encoded the old user-only `bind -p` output (verify vs bash). Report changes.

- [ ] **Step 2: Harnesses + clippy.**

Run:
```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -E "Fail: [1-9]" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: `ALL HARNESSES GREEN`; clippy clean.

- [ ] **Step 3: Divergence doc.** `grep -ni "bind" docs/bash-divergences.md`. If a `bind -p` entry exists, update/remove it (v191 narrows it). Add a `[deferred]` line for the follow-ons: (i) vi-mode default keymap; (ii) `bind -l` limited to huck's 33 honored functions (vs bash 173); (iii) `bind '"\C-a": kill-line'` with a SPACE after the colon errors ("unknown function name" — the func isn't trimmed). Place in the right tier; adjust counts. Report what changed.

- [ ] **Step 4: Record the iteration in memory.**

Prepend a v191 entry to `project_huck_iterations.md` (newest-first): last of the 3 coverage-sweep divergences; huck uses rustyline so `bind -p` emits an HONEST default emacs keymap (`DEFAULT_EMACS_BINDS` for the 33 honored functions, verified ⊆ bash by the harness) merged with user binds (active + pending, so `-c` mode works) minus unbinds; rewrote `active_bind_lines`/`_verbose`; emacs-only (vi deferred); merge SHA (fill after merge). Update the `MEMORY.md` index line + the coverage-divergence note (**all 3 sweep divergences — kill -l v189, declare v190, bind -p v191 — now DONE**). Note deferred: vi keymap, `bind -l` expansion, the bind space-after-colon parse bug.

- [ ] **Step 5: Commit memory (+ any divergence-doc change).**

```bash
git add /home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md docs/bash-divergences.md
git commit -m "v191: record bind -p default-keymap iteration in memory

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Report-back (Task 4)

Report: STATUS, all commit SHAs, the Step 1 result (+ any test updated), full `cargo test` summary, harness results (incl. the new `bind_keymap_diff_check.sh` + its pre-fix FAIL count), clippy status, the subset-honesty `comm` result (empty?), and what changed in `bash-divergences.md`.
