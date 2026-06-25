# function-def reconstruction fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `declare -f` / `type` reconstruction byte-faithful to bash 5.2.21 for two cases: nested function-defs render with a leading `function ` keyword, and a redirected brace-group body hoists its redirect onto the function's closing brace.

**Architecture:** Both fixes live entirely in `crates/huck-syntax/src/generate.rs`. Extract one shared helper `render_function_def(name, body, indent, with_keyword)` holding the body-unwrap + redirect-hoist logic; the outer entry point calls it without the keyword, the nested `Command::FunctionDef` arm calls it with the keyword. No parser/AST changes.

**Tech Stack:** Rust (`huck-syntax`); bash diff harnesses under `tests/scripts/`.

## Global Constraints

- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run the FULL suite with `cargo test --workspace` (~3692 baseline) — plain `cargo test` skips most crates.
- Byte-faithfulness oracle is system `bash` (5.2.21).
- `cargo build --release --bin huck` is slow — long timeout (~480000ms).
- bash rules (verified): EVERY nested function-def renders as `function NAME () ` (keyword + `()` always added, for all source forms `function f3()` / `function f3` / `f3()`); the OUTER named function renders as `NAME () ` (no keyword); `type` matches `declare -f`. A brace-group body with a redirect (`{ …; } 1>&2`) renders unwrapped with the redirect hoisted to the close brace (`} 1>&2`); a subshell body with a redirect (`( … ) 2>&1`) is NOT hoisted (stays `{ ( … ) 2>&1 }`).
- Do NOT change parser/AST, the `other`-body path, or any non-function reconstruction.

---

### Task 1: Outer-vs-nested keyword split + redirect hoist in `generate.rs`

**Files:**
- Modify: `crates/huck-syntax/src/generate.rs` (`function_to_source` ~line 18; the `Command::FunctionDef` arm ~lines 59-75; add the helper)
- Test: `crates/huck-syntax/src/generate.rs` tests module (mirror the existing `declf(src)` helper)
- Create: `tests/scripts/func_reconstruct_diff_check.sh`

**Interfaces (existing, in `generate.rs`/`command.rs`):**
- `fn pad(indent: usize) -> String`, `fn group_body(seq: &Sequence, indent: usize) -> String`.
- `fn redirect_to_source(r: &Redirect, which: RedirDefault) -> String`; `enum RedirDefault { Stdin, Stdout, Stderr }`.
- `crate::command::slots_for_simple_path(redirs: &[Redirection]) -> (Option<Redirect>, Option<Redirect>, Option<Redirect>)`.
- `Command::Redirected { inner: Box<Command>, redirects: Vec<Redirection> }`; `Command::BraceGroup(Sequence)`.
- Test helper `declf(src: &str) -> String` (parses `src`, expects a top-level `FunctionDef`, returns `function_to_source(&name, &body)`). A nested def is tested by calling `declf` on an OUTER function whose body contains it.

- [ ] **Step 1: Write the failing unit tests**

Add to the `generate.rs` tests module (the `declf` helper already exists there):

```rust
    #[test]
    fn declf_outer_no_function_keyword_all_forms() {
        for src in ["f(){ echo a; }", "function f { echo a; }", "function f() { echo a; }"] {
            let s = declf(src);
            assert!(s.starts_with("f () \n"), "outer must omit keyword: {s:?}");
            assert!(!s.starts_with("function "), "outer must not start with `function `: {s:?}");
        }
    }

    #[test]
    fn declf_nested_def_gets_function_keyword_all_forms() {
        // All three nested forms render identically as `function f3 () `.
        for inner in ["function f3() { echo b; }", "function f3 { echo b; }", "f3() { echo b; }"] {
            let s = declf(&format!("outer(){{ echo a; {inner}; }}"));
            assert!(s.contains("function f3 () \n"), "nested def needs keyword (inner={inner:?}): {s:?}");
            assert!(s.starts_with("outer () \n"), "outer still keyword-free: {s:?}");
        }
    }

    #[test]
    fn declf_outer_redirected_brace_body_hoists() {
        // `{ …; } 1>&2` body → unwrapped, redirect on the function close brace.
        assert_eq!(declf("f(){ echo a; echo b; } 1>&2"),
            "f () \n{ \n    echo a;\n    echo b\n} 1>&2");
    }

    #[test]
    fn declf_nested_redirected_brace_body_hoists() {
        let s = declf("outer(){ f3() { echo b; } 1>&2; }");
        assert!(s.contains("function f3 () \n    { \n        echo b\n    } 1>&2"),
            "nested redirected brace body must hoist: {s:?}");
    }

    #[test]
    fn declf_subshell_body_with_redirect_not_hoisted() {
        // A subshell body keeps its redirect INSIDE the function braces.
        assert_eq!(declf("funcc() ( echo c ) 2>&1"),
            "funcc () \n{ \n    ( echo c ) 2>&1\n}");
    }
```

- [ ] **Step 2: Run the new tests to confirm they FAIL**

Run: `cargo test -p huck-syntax declf_outer_no_function_keyword_all_forms declf_nested_def_gets_function_keyword_all_forms declf_outer_redirected_brace_body_hoists declf_nested_redirected_brace_body_hoists declf_subshell_body_with_redirect_not_hoisted`
Expected: the nested-keyword and redirect-hoist tests FAIL (huck drops the keyword and double-wraps); `declf_outer_no_function_keyword_all_forms` and `declf_subshell_body_with_redirect_not_hoisted` likely already PASS.

- [ ] **Step 3: Add the helper functions**

Add to `generate.rs` (near `function_to_source`):

```rust
/// Render the trailing redirects of a hoisted brace-group body, reusing the
/// 0/1/2 slot fast-path (mirrors the `Command::Redirected` arm). Each slot is
/// prefixed with a space, e.g. ` 1>&2`.
fn render_hoisted_redirects(redirects: &[crate::command::Redirection]) -> String {
    let (stdin, stdout, stderr) = crate::command::slots_for_simple_path(redirects);
    let mut s = String::new();
    if let Some(r) = &stdin { s.push(' '); s.push_str(&redirect_to_source(r, RedirDefault::Stdin)); }
    if let Some(r) = &stdout { s.push(' '); s.push_str(&redirect_to_source(r, RedirDefault::Stdout)); }
    if let Some(r) = &stderr { s.push(' '); s.push_str(&redirect_to_source(r, RedirDefault::Stderr)); }
    s
}

/// Render a function definition. `with_keyword` adds the leading `function `
/// that bash emits for NESTED defs; the outer named function (declare -f / type
/// entry point) passes `false`. A brace-group body — bare or carrying a redirect
/// — becomes the function's own braces, with any redirect hoisted to the close
/// brace (`} 1>&2`). Any other body is wrapped in fresh `{ }`.
fn render_function_def(name: &str, body: &Command, indent: usize, with_keyword: bool) -> String {
    let kw = if with_keyword { "function " } else { "" };
    let (group_seq, hoisted): (Sequence, String) = match body {
        Command::BraceGroup(seq) => ((**seq).clone(), String::new()),
        Command::Redirected { inner, redirects }
            if matches!(inner.as_ref(), Command::BraceGroup(_)) =>
        {
            let Command::BraceGroup(seq) = inner.as_ref() else { unreachable!() };
            ((**seq).clone(), render_hoisted_redirects(redirects))
        }
        other => (
            Sequence { first: other.clone(), rest: Vec::new(), background: false },
            String::new(),
        ),
    };
    format!(
        "{kw}{name} () \n{p}{{ \n{}{p}}}{hoisted}",
        group_body(&group_seq, indent + 1),
        p = pad(indent),
    )
}
```

Note: `Command::BraceGroup(Box<Sequence>)` is boxed, so the match binds `seq: &Box<Sequence>` and `(**seq).clone()` yields the `Sequence` (do NOT use `seq.clone()`, which would be a `Box<Sequence>` and fail to typecheck).

- [ ] **Step 4: Wire the two call sites**

Replace `function_to_source` (~line 18):

```rust
pub fn function_to_source(name: &str, body: &Command) -> String {
    render_function_def(name, body, 0, false)
}
```

Replace the `Command::FunctionDef { name, body }` arm body (~lines 59-75) with:

```rust
        Command::FunctionDef { name, body } => render_function_def(name, body, indent, true),
```

- [ ] **Step 5: Run the unit tests to confirm they PASS**

Run: `cargo test -p huck-syntax declf_`
Expected: all `declf_*` tests PASS, including the pre-existing ones (e.g. `declf_simple_last_semi_suppressed`, `declf_subshell_inline`) — they assert the OUTER form, which must be unchanged.

- [ ] **Step 6: Run the huck-syntax crate + round-trip tests**

Run: `cargo test -p huck-syntax`
Expected: PASS. If a `rt_*` round-trip or `declf_*` test breaks, check it: a test asserting a nested def WITHOUT `function` encoded the old bug (update it to bash-faithful); an OUTER-form change means the keyword leaked to the entry point (bug — fix the wiring).

- [ ] **Step 7: Add the bash diff harness**

Create `tests/scripts/func_reconstruct_diff_check.sh`, mirroring
`tests/scripts/declare_f_diff_check.sh`'s structure (shebang, `HUCK_BIN` →
`target/release/huck`, bash-absent SKIP, a `fragments` array, PASS/FAIL loop
comparing combined stdout of `bash -c "$frag"` vs `"$HUCK_BIN" -c "$frag"`,
`exit $(( FAIL>0 ? 1 : 0 ))`). Fragments (each ends in a `declare -f` or `type`):

```bash
fragments=(
  'outer(){ echo a; function f3() { echo b; }; }; declare -f outer'
  'outer(){ echo a; f3() { echo b; }; }; declare -f outer'
  'outer(){ function g { echo b; }; }; type outer'
  'f(){ echo a; echo b; } 1>&2; declare -f f'
  'f4(){ echo a; f5() { echo b; } 1>&2; f5; } 2>&1; declare -f f4'
  'funcc() ( echo c ) 2>&1; declare -f funcc'
  'function topkw { echo a; }; declare -f topkw'
)
```

Run:
```bash
cargo build --release --bin huck   # slow, ~480000ms timeout
bash tests/scripts/func_reconstruct_diff_check.sh | tail -2
```
Expected: `Fail: 0` (every fragment byte-identical to bash).

- [ ] **Step 8: Full suite + func re-measure + no-regress guard**

Run: `cargo test --workspace`
Expected: PASS (~3692). Also run the existing `declare_f_diff_check.sh`,
`function_keyword_diff_check.sh`, and `func_redirect_diff_check.sh` harnesses —
all `Fail: 0`.

Re-measure the func category and confirm cprint does not regress:
```bash
for cat in func cprint; do
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$cat bash tests/bash-test-suite/runner.sh 2>&1 | grep -E "\| $cat \|"
done
```
Expected: `cprint` stays PASS (no regression — this is the hard requirement);
`func` stays FAIL but its diff SHRANK (capture the scratch `func.diff`: the
`function f3`/`f5` keyword hunks and the `} 1>&2` / `} 2>&1` brace-nesting hunks
are gone). Record whether any category incidentally flipped (not predicted).

- [ ] **Step 9: Commit**

```bash
git add crates/huck-syntax/src/generate.rs tests/scripts/func_reconstruct_diff_check.sh
git commit -m "$(printf 'v222: function-def reconstruction — nested function keyword + redirect hoist\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer

- The outer-vs-nested split is structural: `function_to_source` is the ONLY
  no-keyword entry; every `FunctionDef` reached via `command_to_source` recursion
  is nested and gets the keyword. Do not add any keyword logic anywhere else.
- The redirect-hoist is restricted to `Redirected { inner: BraceGroup, .. }` —
  the `declf_subshell_body_with_redirect_not_hoisted` test guards that subshell
  (and any other) bodies are untouched.
- `cprint` must stay PASS — it is the regression tripwire for an accidental
  keyword leak onto the outer function.
- Do NOT touch the parser, AST, or non-function reconstruction paths.
