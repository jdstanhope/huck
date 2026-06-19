# Bare `declare` No-Args Format Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make bare `declare`/`typeset` (no args) print variables as `name=value` (bash's minimal-quote form) and then list functions, instead of huck's current `declare -- x="1"` form — while leaving `declare -p` unchanged.

**Architecture:** A builtins-only formatting change in `src/builtins.rs`. Add a minimal-quote scalar quoter and a bare line-formatter, thread a `bare` flag from the two `declare_list_all_vars` callers, and list functions in bare mode. No AST/parser/expand change.

**Tech Stack:** Rust. File: `src/builtins.rs` (+ make one helper `pub(crate)` in `src/param_expansion.rs`). New harness `tests/scripts/declare_no_args_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-19-declare-no-args-format-design.md`

**Background the implementer needs:**
- Bare `declare` (no args) routes to `declare_list_all_vars(out, shell)` (`src/builtins.rs`), called from `builtin_declare` (~line 1241) AND `builtin_declare_decl` (~line 2177). Both functions parse a `print_mode` bool (the `-p` flag). Currently `declare_list_all_vars` always emits `format_declare_line` (the `declare -- name="value"` form) and never lists functions — so bare `declare` and `declare -p` produce identical (wrong-for-bare) output.
- `format_declare_line(name, var) -> String` (~line 932) builds the `declare -X name=<value_part>` line. Its `value_part` match renders `="v"` for scalars and `=([k]="v" …)` for arrays.
- bash bare-declare scalar quoting is MINIMAL (bare `hello`, `'a b'` only when needed, `$'…'` for control chars, **empty → bare** `name=`), NOT `${v@Q}` (which always quotes). It equals bash's `set -x` quoting except for the empty case.
- Reusable quoting primitives: `crate::param_expansion::ansi_c_quote(v)` (already `pub(crate)`, the `$'…'` quoter), `crate::builtins::escape_alias_value(v)` (already `pub(crate)`, rewrites `'`→`'\''`), and `crate::param_expansion::contains_shell_metas(v)` (`src/param_expansion.rs:296`, currently private — make `pub(crate)`). This is bash's `sh_contains_shell_metas`.
- Indexed arrays: huck's `declare -p` already matches bash byte-for-byte, so the bare form is the `-p` value reused minus the `declare -a ` prefix. Associative arrays have a separate pre-existing `-p` divergence (huck quotes the key `["k"]`, bash uses bare `[k]` + a trailing space) — OUT OF SCOPE, inherited as-is.
- `emit_function(name, names_only, out, shell: &Shell)` (~line 1106) writes one function in the `f () {…}` form (huck's normalized body, M-121).
- Bash-diff harnesses live in `tests/scripts/*_diff_check.sh`; run via `bash tests/scripts/<name>.sh` after building huck.

---

## Task 1: minimal-quote scalar quoter `declare_scalar_quote`

**Files:**
- Modify: `src/param_expansion.rs` — make `contains_shell_metas` `pub(crate)`.
- Modify: `src/builtins.rs` — add `declare_scalar_quote`.
- Test: `src/builtins.rs` `mod tests`.

- [ ] **Step 1: Expose the primitive.** In `src/param_expansion.rs:296`, change `fn contains_shell_metas(` to `pub(crate) fn contains_shell_metas(`.

- [ ] **Step 2: Write the failing unit test** in `src/builtins.rs` `mod tests`:

```rust
    #[test]
    fn declare_scalar_quote_matches_bash_listing() {
        // bash bare-declare / set -x style minimal quoting (verified vs bash 5.x)
        assert_eq!(declare_scalar_quote("hello"), "hello");
        assert_eq!(declare_scalar_quote(""), "");            // empty -> bare (name=)
        assert_eq!(declare_scalar_quote("a b"), "'a b'");
        assert_eq!(declare_scalar_quote("x;y"), "'x;y'");
        assert_eq!(declare_scalar_quote("gl*ob"), "'gl*ob'");
        assert_eq!(declare_scalar_quote("d$ollar"), "'d$ollar'");
        assert_eq!(declare_scalar_quote("bang!x"), "'bang!x'");
        assert_eq!(declare_scalar_quote("lt<gt>"), "'lt<gt>'");
        assert_eq!(declare_scalar_quote("br[ack]"), "'br[ack]'");
        assert_eq!(declare_scalar_quote("qu'ote"), "'qu'\\''ote'");
        // not metacharacters in this context -> stay bare
        assert_eq!(declare_scalar_quote("ti~lde"), "ti~lde");
        assert_eq!(declare_scalar_quote("eq=ual"), "eq=ual");
        assert_eq!(declare_scalar_quote("hash#x"), "hash#x");
        // control char -> ANSI-C
        assert_eq!(declare_scalar_quote("ta\tb"), "$'ta\\tb'");
    }
```

- [ ] **Step 3: Run to confirm it FAILS.**

Run: `cargo test --lib declare_scalar_quote_matches_bash_listing 2>&1 | tail -8`
Expected: FAIL — `declare_scalar_quote` does not exist (compile error).

- [ ] **Step 4: Implement** in `src/builtins.rs` (near `format_declare_line`):

```rust
/// bash's variable-listing quoting (the bare `declare` / `set` / `set -x`
/// style): bare unless the value needs quoting; a value with a shell
/// metacharacter is single-quoted (with `'` rewritten `'\''`); a value with a
/// control char uses ANSI-C `$'…'`; the EMPTY value is bare (`name=`). This is
/// NOT `${v@Q}` (which always quotes); it mirrors bash's `sh_contains_shell_metas`
/// + `sh_single_quote`.
fn declare_scalar_quote(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }
    if v.chars().any(|c| c.is_control()) {
        return crate::param_expansion::ansi_c_quote(v);
    }
    if crate::param_expansion::contains_shell_metas(v) {
        return format!("'{}'", escape_alias_value(v));
    }
    v.to_string()
}
```

- [ ] **Step 5: Run to confirm PASS.**

Run: `cargo test --lib declare_scalar_quote_matches_bash_listing 2>&1 | tail -6`
Expected: PASS. If a specific row fails (e.g. `bang!x` stays bare), `contains_shell_metas`'s metacharacter set differs from bash's bare-declare set for that char — STOP and report (the harness in Task 4 is the byte-identical authority; do not silently change the assertion).

- [ ] **Step 6: Commit.**

```bash
git add src/builtins.rs src/param_expansion.rs
git commit -m "$(cat <<'EOF'
v190: declare_scalar_quote — bash variable-listing minimal quoting

Bare `declare` quotes values the set -x way (bare unless a shell metachar; '…'
or $'…' when needed; empty -> bare), not the always-quoting ${v@Q}. Reuses
contains_shell_metas (now pub(crate)) + ansi_c_quote + escape_alias_value.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `format_declare_bare_line` + factor the array renderer

**Files:**
- Modify: `src/builtins.rs` — factor `render_declare_value_part`; add `format_declare_bare_line`.
- Test: `src/builtins.rs` `mod tests`.

- [ ] **Step 1: Factor the value renderer.** In `format_declare_line` (~line 932), extract the `let value_part = match &var.value { … };` block into a helper and call it:

```rust
/// Renders the `=<value>` suffix of a declare line: `="v"` for a scalar,
/// `=([k]="v" …)` for arrays. Shared by `format_declare_line` (the `-p` form)
/// and `format_declare_bare_line` (arrays only).
fn render_declare_value_part(var: &crate::shell_state::Variable) -> String {
    use crate::shell_state::VarValue;
    match &var.value {
        VarValue::Scalar(s) => {
            if var.nameref && s.is_empty() {
                String::new()
            } else {
                format!("=\"{}\"", escape_double_quote_value(s))
            }
        }
        VarValue::Indexed(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("[{k}]=\"{}\"", escape_double_quote_value(v)))
                .collect();
            format!("=({})", parts.join(" "))
        }
        VarValue::Associative(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| {
                    format!(
                        "[\"{}\"]=\"{}\"",
                        escape_double_quote_value(k),
                        escape_double_quote_value(v)
                    )
                })
                .collect();
            format!("=({})", parts.join(" "))
        }
    }
}
```

In `format_declare_line`, replace the inline `value_part` block with
`let value_part = render_declare_value_part(var);` (keep the surrounding
`format!("declare {flag_str} {name}{value_part}")`).

- [ ] **Step 2: Write the failing test** for the bare formatter:

```rust
    #[test]
    fn format_declare_bare_line_scalar_and_array() {
        use crate::shell_state::Shell;
        let mut sh = Shell::new();
        sh.set_var("zs", "a b");                 // scalar needing quotes
        sh.set_var("zp", "plain");               // bare scalar
        let zs = sh.get_var_struct("zs").unwrap();
        let zp = sh.get_var_struct("zp").unwrap();
        assert_eq!(format_declare_bare_line("zs", zs), "zs='a b'");
        assert_eq!(format_declare_bare_line("zp", zp), "zp=plain");
    }
```

(NOTE: adapt `set_var` / `get_var_struct` to the actual `Shell` test API — check how other `format_declare_line` tests construct a `Variable`; if there's a direct `Variable` constructor for tests, build a `VarValue::Scalar` directly instead of going through `Shell`.)

- [ ] **Step 3: Run to confirm it FAILS.**

Run: `cargo test --lib format_declare_bare_line_scalar_and_array 2>&1 | tail -8`
Expected: FAIL — `format_declare_bare_line` does not exist.

- [ ] **Step 4: Implement** `format_declare_bare_line`:

```rust
/// Formats one variable in bash's bare-`declare` (no-args) form: `name=value`
/// with NO `declare -X` prefix and NO attribute flags. Scalars use the minimal
/// `declare_scalar_quote`; arrays reuse the `-p` value renderer (their element
/// format is identical to `declare -p` minus the `declare -a/-A ` prefix).
fn format_declare_bare_line(name: &str, var: &crate::shell_state::Variable) -> String {
    use crate::shell_state::VarValue;
    match &var.value {
        VarValue::Scalar(s) => {
            if var.nameref && s.is_empty() {
                name.to_string()
            } else {
                format!("{name}={}", declare_scalar_quote(s))
            }
        }
        VarValue::Indexed(_) | VarValue::Associative(_) => {
            format!("{name}{}", render_declare_value_part(var))
        }
    }
}
```

- [ ] **Step 5: Run to confirm PASS.**

Run: `cargo test --lib format_declare_bare_line_scalar_and_array 2>&1 | tail -6 && cargo test --lib format_declare 2>&1 | grep "test result:"`
Expected: PASS; existing `format_declare_line` tests still green (the factor was behavior-preserving).

- [ ] **Step 6: Commit.**

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
v190: format_declare_bare_line + factor render_declare_value_part

Bare-declare line formatter: name=value (no declare -X prefix); scalars via
declare_scalar_quote, arrays reuse the shared -p value renderer.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: thread `bare` + list functions

**Files:**
- Modify: `src/builtins.rs` — `declare_list_all_vars(out, shell, bare)`; both call sites; function listing in bare mode.
- Test: integration via `huck -c` (covered by the Task 4 harness; add a focused integration assertion here).

- [ ] **Step 1: Write the failing integration-style test** in `src/builtins.rs` `mod tests` (use the existing pattern that runs a builtin into a buffer — mirror `kill_l_no_args_lists_all_standard_signals`):

```rust
    #[test]
    fn bare_declare_lists_name_value_and_functions() {
        let mut shell = crate::shell_state::Shell::new();
        let mut buf = Vec::new();
        // define a var and a function, then run bare `declare`
        run_builtin_or_decl("zsv", "hello", &mut shell);   // helper: set a scalar
        shell.define_function_for_test("zf");              // helper: define `zf`
        let _ = run_declaration_builtin("declare", &[], &mut buf, &mut shell);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("\nzsv=hello\n") || s.starts_with("zsv=hello\n") || s.contains("zsv=hello"),
            "bare declare should list zsv=hello: {s}");
        assert!(!s.contains("declare -- zsv"), "bare declare must not use the -p form: {s}");
        assert!(s.contains("zf ()"), "bare declare should list function zf: {s}");
    }
```

(ADAPT to the real test helpers: find how existing tests set a scalar var and define a function on a `Shell`, and how they invoke `declare` — e.g. via `run_declaration_builtin("declare", &[...], &mut buf, &mut shell)`. If there is no `define_function_for_test`, insert into `shell.functions` directly with a parsed body, or run `run_declaration_builtin` after parsing `zf(){ :; }` through the normal path. Keep the three assertions.)

- [ ] **Step 2: Run to confirm it FAILS.**

Run: `cargo test --lib bare_declare_lists_name_value_and_functions 2>&1 | tail -12`
Expected: FAIL — bare `declare` currently emits `declare -- zsv="hello"` and no function.

- [ ] **Step 3: Implement.** Change `declare_list_all_vars` to take a `bare` flag and branch:

```rust
fn declare_list_all_vars(
    out: &mut dyn std::io::Write,
    shell: &Shell,
    bare: bool,
) -> ExecOutcome {
    let mut entries: Vec<(&String, &crate::shell_state::Variable)> =
        shell.iter_vars().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, var) in entries {
        let line = if bare {
            format_declare_bare_line(name, var)
        } else {
            format_declare_line(name, var)
        };
        let _ = writeln!(out, "{line}");
    }
    // bare `declare` also lists all functions (sorted), in the `f () {…}` form.
    if bare {
        let mut fnames: Vec<String> = shell.functions.keys().cloned().collect();
        fnames.sort();
        for n in &fnames {
            emit_function(n, false, out, shell);
        }
    }
    ExecOutcome::Continue(0)
}
```

Update the two call sites to pass `!print_mode`:
- `builtin_declare` (~line 1241): `return declare_list_all_vars(out, shell, !print_mode);`
- `builtin_declare_decl` (~line 2177): `return declare_list_all_vars(out, shell, !print_mode);`

(Confirm `print_mode` is the in-scope `-p` flag at each site — both functions declare `let mut print_mode = false;`. `emit_function` takes `&Shell`, so `declare_list_all_vars` stays `&Shell`.)

- [ ] **Step 4: Run to confirm PASS + no regressions.**

Run: `cargo test --lib bare_declare_lists_name_value_and_functions 2>&1 | tail -6 && cargo test --lib 2>&1 | grep "test result:" | grep -v "0 failed" || echo OK`
Expected: PASS; `OK`. If an existing test asserted bare `declare` produced the `declare --` form, update it to the new `name=value` form (verify vs bash). The `declare -p` tests must stay green.

- [ ] **Step 5: Manual byte-check vs bash.**

Run:
```bash
cargo build 2>&1 | tail -1
diff <(bash -c 'zq="a b"; zi=42; za=(p "q r"); declare 2>/dev/null | grep "^z"') \
     <(./target/debug/huck -c 'zq="a b"; zi=42; za=(p "q r"); declare 2>/dev/null | grep "^z"')
```
Expected: EMPTY (byte-identical for the `z*` vars). Also confirm `declare -p` unchanged: `diff <(bash -c 'zq=x; declare -p zq') <(./target/debug/huck -c 'zq=x; declare -p zq')` empty.

- [ ] **Step 6: Commit.**

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
v190: bare declare lists name=value + functions; declare -p unchanged

declare_list_all_vars takes a `bare` flag (= !print_mode) from both callers;
bare mode emits format_declare_bare_line then all functions, while declare -p
keeps the format_declare_line (-p) form.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: bash-diff harness

**Files:**
- Create: `tests/scripts/declare_no_args_diff_check.sh`

- [ ] **Step 1: Create the harness** (mirrors `tests/scripts/process_sub_diff_check.sh`; greps to `^z` to filter the inherited environment):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v190: bare `declare`/`typeset` (no args)
# variable listing format. Each case sets z* vars and greps `declare` to ^z to
# filter out the inherited environment. `declare -p` is included as a guard.
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

# scalar quoting battery (bare declare)
check "scalar bare"   'zq=hello; declare 2>/dev/null | grep "^zq="'
check "scalar empty"  'zq=; declare 2>/dev/null | grep "^zq="'
check "scalar space"  'zq="a b"; declare 2>/dev/null | grep "^zq="'
check "scalar semi"   'zq="x;y"; declare 2>/dev/null | grep "^zq="'
check "scalar glob"   'zq="gl*ob"; declare 2>/dev/null | grep "^zq="'
check "scalar dollar" 'zq="d\$ollar"; declare 2>/dev/null | grep "^zq="'
check "scalar bang"   'zq="bang!x"; declare 2>/dev/null | grep "^zq="'
check "scalar angle"  'zq="lt<gt>"; declare 2>/dev/null | grep "^zq="'
check "scalar quote"  "zq=\"qu'ote\"; declare 2>/dev/null | grep '^zq='"
check "scalar tilde"  'zq="ti~lde"; declare 2>/dev/null | grep "^zq="'
check "scalar eq"     'zq="eq=ual"; declare 2>/dev/null | grep "^zq="'
check "scalar tab"    'zq=$'"'"'ta\tb'"'"'; declare 2>/dev/null | grep "^zq="'
# integer / exported / readonly (bare: no attribute flag)
check "integer"       'declare -i zi=42; declare 2>/dev/null | grep "^zi="'
check "exported"      'export ze=world; declare 2>/dev/null | grep "^ze="'
check "readonly"      'readonly zr=const; declare 2>/dev/null | grep "^zr="'
# indexed array
check "array"         'za=(p "q r" ""); declare 2>/dev/null | grep "^za="'
# typeset parity
check "typeset"       'zt=hi; typeset 2>/dev/null | grep "^zt="'
# function listing (structural: huck normalizes bodies (M-121), so just confirm
# the function is listed at all — count is byte-identical "1")
check "lists fn"      'zf(){ echo hi; }; declare 2>/dev/null | grep -c "^zf"'
# REGRESSION GUARD: declare -p must stay byte-identical (the -p path is unchanged)
check "declare -p"    'zq="a b"; zi=42; declare -i zi; za=(p "q r"); declare -p zq zi za 2>/dev/null'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable and run.**

Run:
```bash
chmod +x tests/scripts/declare_no_args_diff_check.sh
cargo build 2>&1 | tail -1
bash tests/scripts/declare_no_args_diff_check.sh
```
Expected: all `PASS`, `Fail: 0`, exit 0. If a scalar-quoting case diverges, the quoter's metacharacter set is off — STOP and report the exact diff (do NOT delete the failing case). If the `array` case diverges, report it (indexed arrays were expected to match). Associative arrays are intentionally NOT tested (known deferred `-p` divergence).

- [ ] **Step 3: Prove non-tautological** (fails pre-fix):

```bash
BASE=$(git merge-base HEAD main)
git worktree add -d /tmp/huck-prefix "$BASE" 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/declare_no_args_diff_check.sh | tail -4
git worktree remove --force /tmp/huck-prefix 2>&1 | tail -1
```
Expected: many FAILs pre-fix (every bare-declare scalar/integer/array/typeset/fn case; only `declare -p` passes). Report how many.

- [ ] **Step 4: Commit.**

```bash
git add tests/scripts/declare_no_args_diff_check.sh
git commit -m "$(cat <<'EOF'
v190: bash-diff harness for bare declare no-args format

Scalar-quoting battery + integer/exported/readonly/indexed-array/typeset/function
listing, byte-identical to bash; declare -p regression guard. Non-tautological.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: full regression + docs + memory

**Files:**
- Modify (memory): `project_huck_iterations.md`, `MEMORY.md` (under `/home/john/.claude/projects/-home-john-projects-shuck/memory/`).
- Maybe modify: `docs/bash-divergences.md` (only to add the deferred assoc-array note).

- [ ] **Step 1: Full test suite (0 failures).**

Run: `cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"`
Expected: `ALL GREEN`. A failing test that encoded the old bare-declare `declare --` form must be updated to `name=value` (verify vs bash); a failing `declare -p` test means the `-p` path was changed — investigate, do not "fix" by weakening.

- [ ] **Step 2: All harnesses + clippy green.**

Run:
```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -E "Fail: [1-9]" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: `ALL HARNESSES GREEN`; clippy clean.

- [ ] **Step 3: Divergence doc.** `grep -n "declare" docs/bash-divergences.md`. If no entry covers bare-declare format, add ONE `[deferred]` line for the associative-array `declare -p`/bare key-quoting + trailing-space divergence (huck `["k"]="1"` vs bash `[k]="1" )`), placed in the appropriate tier; increment that tier's count. If an entry already covers declare-no-args, delete/adjust it. Report what you did.

- [ ] **Step 4: Record the iteration in memory.**

Prepend a v190 entry to `project_huck_iterations.md` (newest-first): coverage-found; bare `declare`/`typeset` now lists `name=value` (minimal `declare_scalar_quote` — bash set-x style, NOT @Q; empty→bare) + functions, via a `bare` flag threaded into `declare_list_all_vars`; `declare -p` unchanged; reused `contains_shell_metas`/`ansi_c_quote`/`escape_alias_value`; indexed arrays byte-identical, associative-array key-quoting/trailing-space DEFERRED (shared `-p` bug); M-121 function-body normalization unchanged; merge SHA (fill after merge). Update the `MEMORY.md` index line + the coverage-divergence note (declare-no-args RESOLVED; `bind -p` still pending).

- [ ] **Step 5: Commit memory (+ any divergence-doc change).**

```bash
git add /home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md docs/bash-divergences.md
git commit -m "v190: record bare-declare-format iteration in memory

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Report-back (Task 5)

Report: STATUS, all commit SHAs, the Step 1 grep classification (Task 3/5 — any old-behavior tests updated), full `cargo test` summary, harness results (incl. the new `declare_no_args_diff_check.sh` + its pre-fix FAIL count), clippy status, the byte-diff result for bare `declare` and the `declare -p` guard, and what (if anything) changed in `bash-divergences.md`.
