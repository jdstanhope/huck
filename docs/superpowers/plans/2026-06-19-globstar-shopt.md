# `**` Globstar Respects `shopt globstar` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `**` recursive only when `shopt -s globstar` (bash's gate); when off (the default), `**` behaves like `*` — instead of huck's current unconditional recursion.

**Architecture:** Thread the `globstar` shopt into `GlobOpts`; in the non-extglob glob path, collapse `**`→`*` in the pattern when globstar is off (so the `glob` crate sees single-level `*`), leaving `**` when on.

**Tech Stack:** Rust, the `glob` crate. Files: `src/expand.rs` (GlobOpts + collapse + gate + tests), `src/shell_state.rs` (`glob_opts()`). New harness `tests/scripts/globstar_diff_check.sh`. Doc `docs/bash-divergences.md`.

**Spec:** `docs/superpowers/specs/2026-06-19-globstar-shopt-design.md`

**Background the implementer needs:**
- `GlobOpts` (`src/expand.rs:10`, `#[derive(Clone, Copy, Default, Debug)]`) holds glob shopt flags; `Shell::glob_opts()` (`src/shell_state.rs:815`) builds it from `self.shopt_options.get("…")`. `globstar` is ALREADY a tracked shopt (`shell_state.rs:256`, default `false`) but is never read here.
- The regular glob path (`glob_expand_fields_opts`, `src/expand.rs`, the `else` branch ~line 1510): builds `let npat = translate_bracket_negation(&pattern);` (line 1530) then `match glob_with(&npat, match_opts)` (line 1531). The `glob` crate with `require_literal_separator: true` makes `*` single-level but treats `**` as recursive UNCONDITIONALLY — that's the bug.
- The extglob path (`extglob_pathname_expand`/`walk_components`) already treats a `**` component like a single-level `*` (no recursion), so it's already globstar-OFF-correct; leave it.
- `glob` is OFF by default → after this fix, huck's DEFAULT changes from "recursive `**`" to "`**`≡`*`" (matching bash). An up-front grep found NO test asserting the old default-recursive behavior (only a stale comment in `extglob_pathname_diff_check.sh`).
- Bash-diff harnesses: `tests/scripts/*_diff_check.sh`, run via `bash tests/scripts/<name>.sh`.

---

## Task 1: thread `globstar` + `collapse_globstar` + gate

**Files:**
- Modify: `src/expand.rs` — `GlobOpts.globstar`; `collapse_globstar`; gate the glob path.
- Modify: `src/shell_state.rs` — `glob_opts()` reads the shopt.
- Test: `src/expand.rs` `mod tests`; a `glob_opts` plumbing test in `src/shell_state.rs` `mod tests`.

- [ ] **Step 1: Write the failing unit tests.**

In `src/expand.rs` `mod tests` (near the other glob tests):

```rust
    #[test]
    fn collapse_globstar_reduces_double_star_to_single() {
        assert_eq!(collapse_globstar("**"), "*");
        assert_eq!(collapse_globstar("***"), "*");
        assert_eq!(collapse_globstar("**/*.txt"), "*/*.txt");
        assert_eq!(collapse_globstar("a/**/b"), "a/*/b");
        assert_eq!(collapse_globstar("a*b"), "a*b");          // single star unchanged
        assert_eq!(collapse_globstar("[**]"), "[**]");        // inside bracket class: untouched
        assert_eq!(collapse_globstar("\\*\\*"), "\\*\\*");    // escaped stars: untouched
    }
```

In `src/shell_state.rs` `mod tests`:

```rust
    #[test]
    fn glob_opts_reads_globstar_shopt() {
        let mut sh = Shell::new();
        assert!(!sh.glob_opts().globstar, "globstar off by default");
        crate::shell::process_line("shopt -s globstar", &mut sh, false);
        assert!(sh.glob_opts().globstar, "globstar on after shopt -s");
    }
```

(ADAPT the `process_line` call to its real signature — check an existing
`shell_state`/`shell` test for how a `shopt`/command is run against a `Shell`;
if `process_line` differs, use the actual API or set the shopt directly via the
real setter, e.g. `sh.shopt_options.set("globstar", true)` if that exists.)

- [ ] **Step 2: Run to confirm they FAIL.**

Run: `cargo test --lib collapse_globstar_reduces_double_star_to_single glob_opts_reads_globstar_shopt 2>&1 | tail -15`
Expected: FAIL — `collapse_globstar` and the `GlobOpts.globstar` field don't exist (compile errors). Run each separately if the multi-name filter errors.

- [ ] **Step 3: Add the `globstar` field + plumbing.**

In `src/expand.rs`, add to `GlobOpts` (after `noglob`):
```rust
    pub globstar: bool,
```
In `src/shell_state.rs` `glob_opts()`, add to the struct literal:
```rust
            globstar: self.shopt_options.get("globstar").unwrap_or(false),
```

- [ ] **Step 4: Add `collapse_globstar`** in `src/expand.rs` (near `build_glob_pattern` or the other glob helpers):

```rust
/// Collapses a run of consecutive `*` to a single `*` (`**`→`*`, `***`→`*`),
/// matching bash when `shopt globstar` is OFF (two `*` are just one). Skips `*`
/// inside a `[…]` bracket class and honors `\`-escapes, so `[**]` and `\*\*`
/// are untouched.
fn collapse_globstar(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len());
    let mut chars = pat.chars().peekable();
    let mut in_bracket = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '[' if !in_bracket => {
                in_bracket = true;
                out.push('[');
            }
            ']' if in_bracket => {
                in_bracket = false;
                out.push(']');
            }
            '*' if !in_bracket => {
                out.push('*');
                while chars.peek() == Some(&'*') {
                    chars.next();
                }
            }
            other => out.push(other),
        }
    }
    out
}
```

- [ ] **Step 5: Gate the glob path.** In `glob_expand_fields_opts`, between the
`let npat = translate_bracket_negation(&pattern);` line (1530) and `match glob_with`:

```rust
            let npat = crate::glob_match::translate_bracket_negation(&pattern);
            // `**` is recursive only with `shopt -s globstar`; otherwise it is
            // two ordinary `*` (≡ `*`). The `glob` crate always treats `**` as
            // recursive, so collapse it to `*` when globstar is off.
            let npat = if opts.globstar { npat } else { collapse_globstar(&npat) };
            match glob_with(&npat, match_opts) {
```

- [ ] **Step 6: Run to confirm PASS + no regression.**

Run: `cargo test --lib collapse_globstar glob_opts_reads_globstar 2>&1 | tail -10 && cargo test --lib 2>&1 | grep "test result:" | grep -v "0 failed" || echo OK`
Expected: PASS; `OK`. If an existing glob test now fails because it relied on `**` recursing by default, update it to the gated behavior (verify vs `bash -c 'shopt -u globstar; …'`). `cargo clippy --lib` clean.

- [ ] **Step 7: Manual byte-check vs bash (default off + on).**

Run:
```bash
cargo build 2>&1 | tail -1
d=$(mktemp -d); mkdir -p "$d/a/b/c"; touch "$d/r.txt" "$d/a/x.txt" "$d/a/b/y.txt" "$d/a/b/c/z.txt"
echo "off **/*.txt: bash=[$(cd "$d"; bash -c 'printf "%s " **/*.txt')] huck=[$(cd "$d"; "$PWD/../target/debug/huck" -c 'printf "%s " **/*.txt' 2>/dev/null)]"
echo "on  **/*.txt: bash=[$(cd "$d"; bash -c 'shopt -s globstar; printf "%s " **/*.txt')] huck=[$(cd "$d"; "$(git rev-parse --show-toplevel)/target/debug/huck" -c 'shopt -s globstar; printf "%s " **/*.txt' 2>/dev/null)]"
rm -rf "$d"
```
Expected: `off` → both `a/x.txt`; `on` → both `r.txt a/x.txt a/b/y.txt a/b/c/z.txt` (order may differ — that's fine, the harness sorts). If `off` huck still recurses, the gate isn't wired; STOP and report.

- [ ] **Step 8: Commit.**

```bash
git add src/expand.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
v193: gate ** recursion on shopt globstar

Thread the globstar shopt into GlobOpts; in the glob path, collapse **->* when
globstar is off (the glob crate always treats ** as recursive). Default now
matches bash (** ≡ *); shopt -s globstar keeps recursion. Reworks M-53.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: bash-diff harness

**Files:**
- Create: `tests/scripts/globstar_diff_check.sh`

- [ ] **Step 1: Create the harness:**

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v193: `**` globstar gated on
# `shopt globstar`. Builds a private temp tree and compares sorted glob output.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# build a fixed tree; both shells cd into it and run the SAME fragment.
TREE=$(mktemp -d)
mkdir -p "$TREE/a/b/c"
touch "$TREE/r.txt" "$TREE/a/x.txt" "$TREE/a/b/y.txt" "$TREE/a/b/c/z.txt" "$TREE/a/b/note.md"
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$TREE"; bash -c "$frag" 2>&1 | sort; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$TREE"; "$HUCK_BIN" -c "$frag" 2>&1 | sort; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# globstar OFF (default): ** ≡ * (single level)
check "off **/*.txt"   'printf "%s\n" **/*.txt'
check "off bare **"    'printf "%s\n" **'
check "off a/**"       'printf "%s\n" a/**'
check "off **/*"       'printf "%s\n" **/*'
# globstar ON: recursive
check "on **/*.txt"    'shopt -s globstar; printf "%s\n" **/*.txt'
check "on **/y.txt"    'shopt -s globstar; printf "%s\n" **/y.txt'
check "on a/**/*.txt"  'shopt -s globstar; printf "%s\n" a/**/*.txt'
check "on **/*.md"     'shopt -s globstar; printf "%s\n" **/*.md'
# control: no ** — unchanged
check "ctrl a/*"       'printf "%s\n" a/*'
check "ctrl *.txt"     'printf "%s\n" *.txt'
# NOTE: bare `**` with globstar ON is a documented residual (the glob crate
# matches dirs-only vs bash's dirs+files) and is intentionally NOT checked.

rm -rf "$TREE"
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable and run.**

Run:
```bash
chmod +x tests/scripts/globstar_diff_check.sh
cargo build 2>&1 | tail -1
bash tests/scripts/globstar_diff_check.sh
```
Expected: all `PASS`, `Fail: 0`, exit 0. If a case diverges, STOP and report the exact diff. If the `on **/*.txt` or `on a/**/*.txt` case diverges, that's a real finding (the glob-crate ON behavior differs from bash for that form) — report it; do NOT weaken. (The bare-`**`-ON residual is deliberately excluded.)

- [ ] **Step 3: Prove non-tautological** (fails pre-fix):

```bash
BASE=$(git merge-base HEAD main)
git worktree add -d /tmp/huck-prefix "$BASE" 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/globstar_diff_check.sh | tail -4
git worktree remove --force /tmp/huck-prefix 2>&1 | tail -1
```
Expected: the `off **/…` cases FAIL pre-fix (old huck recurses where bash doesn't); the `on …` and `ctrl …` cases PASS pre-fix. Report the count.

- [ ] **Step 4: Commit.**

```bash
git add tests/scripts/globstar_diff_check.sh
git commit -m "$(cat <<'EOF'
v193: bash-diff harness for ** globstar gated on shopt

off: ** ≡ * (byte-identical); on: **/* recursive (byte-identical). bare-**-on
residual excluded. Non-tautological (off cases fail pre-fix).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: rework M-53 doc + regression + memory

**Files:**
- Modify: `docs/bash-divergences.md` (reword M-53).
- Modify (memory): `project_huck_iterations.md`, `MEMORY.md`.

- [ ] **Step 1: Full test suite (0 failures).**

Run: `cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"`
Expected: `ALL GREEN`. Update any test that encoded the old unconditional-`**` behavior (verify vs bash) and report.

- [ ] **Step 2: All harnesses + clippy green.**

Run:
```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -E "Fail: [1-9]" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: `ALL HARNESSES GREEN`; clippy clean.

- [ ] **Step 3: Rework the M-53 divergence entry.** Find `M-53` in `docs/bash-divergences.md` (Tier 2, Globbing). Replace its body to reflect v193:

```markdown
- **M-53: bare `**` globstar matches dirs-only (not files)** — `[deferred]` low (narrowed v193). `shopt globstar` now gates `**` correctly: OFF (default) `**` ≡ `*` (single level), ON `**` is recursive — both byte-identical to bash for the common `**/*.ext` form (v193). RESIDUAL: a bare `**` with globstar ON matches directories at all depths but NOT the files bash also yields (huck's regular glob path uses the `glob` crate, whose bare `**` is dir-recursive only). Matching bash's bare-`**` set exactly needs huck's own recursive walker (the extglob `walk_components` path). Also: globstar inside an extglob pattern (`**/+(…)`) does not recurse. Rarely decisive; the dominant `**/<glob>` form is correct.
```

(Keep it in Tier 2; the count is UNCHANGED — reworded, not added/removed. Verify the `M-53` line is the only one and the Tier-2 count is still 13.)

- [ ] **Step 4: Record the iteration in memory.**

Prepend a v193 entry to `project_huck_iterations.md` (newest-first): Tier-2 backlog M-53; huck globstarred `**` UNCONDITIONALLY (regular glob path → `glob` crate, whose `**` is always recursive; `GlobOpts` had no `globstar` field). Fix: thread the `globstar` shopt into `GlobOpts` + `collapse_globstar` (`**`→`*`, bracket/escape-aware) gating the non-extglob path so OFF (default) matches bash. Common `**/*.ext` form byte-identical on+off. RESIDUAL (documented, narrowed M-53): bare-`**`-ON dirs-vs-dirs+files (glob-crate limitation; needs own walker). merge SHA (fill after merge). Update the `MEMORY.md` index line + note the backlog (M-53 narrowed; 12 other Tier-2 deferred items remain).

- [ ] **Step 5: Commit doc + memory.**

```bash
git add docs/bash-divergences.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md
git commit -m "v193: narrow M-53 (globstar gated) + record iteration

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

(If the controller handles memory post-merge, do only the `bash-divergences.md` reword here and report.)

---

## Report-back (Task 3)

Report: STATUS, all commit SHAs, full `cargo test` summary, harness results (incl. the new `globstar_diff_check.sh` + its pre-fix FAIL count), clippy status, the Task-1 manual byte-check (off + on), and the M-53 reword.
