# `declare -p` ANSI-C value quoting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `declare -p` render values containing control characters as ANSI-C `$'…'` (matching bash 5.2.21), flipping the `herestr` bash-test-suite category to PASS.

**Architecture:** huck already has a byte-faithful `param_expansion::ansi_c_quote`; the `declare -p` value renderer (`render_declare_value_part`) just never consults it. Add a small `declare_p_value_quote` helper that picks `$'…'` for control-bearing values (else the current `"…"`) and wire it into the three `VarValue` arms.

**Tech Stack:** Rust (`huck-engine` builtins); bash diff harnesses under `tests/scripts/*_diff_check.sh`.

## Global Constraints

- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run the FULL suite with `cargo test --workspace` (~3684 baseline) before finishing.
- Byte-faithfulness oracle is bash **5.2.21** (system `bash`); capture expected strings with `bash -c '…; declare -p v' | cat -A`.
- Control-free values must render EXACTLY as today (`"…"`) — no regression. The bare-`declare`/`set` path (`declare_scalar_quote`) is already ANSI-C-aware and must NOT change.
- GPL posture: read bash behavior from system bash; never paste bash `.right`/output into committed files (describe divergences in prose).
- Do NOT push to main or merge without explicit user confirmation.

---

### Task 1: `declare_p_value_quote` helper + wire into `render_declare_value_part`

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (add `declare_p_value_quote`; change `render_declare_value_part` at ~875-913)
- Create: `tests/scripts/declare_p_ansic_diff_check.sh`
- Test: `crates/huck-engine/src/builtins.rs` (tests module)

**Interfaces:**
- Consumes: `crate::param_expansion::ansi_c_quote(&str) -> String` (existing — produces `$'…'` with bash's named escapes `\a \b \t \n \v \f \r \E`, 3-digit octal for other `<0x20`/`0x7f`, `\\`/`\'`, printable passthrough) and `crate::escape_double_quote_value(&str) -> String` (existing — escapes `" \ $ \``).
- Produces: `fn declare_p_value_quote(s: &str) -> String` returning the FULL quoted token (`"…"` or `$'…'`), no leading `=`.

**Background (current RED behavior, measured):**
`declare -p` of a control-bearing value renders a literal control char in `"…"`:
- scalar `i⏎` → huck `declare -- v="i⏎"`, bash `declare -- v=$'i\n'`
- indexed elem `i⏎` → huck `[1]="i⏎"`, bash `[1]=$'i\n'`
- assoc elem `a⇥b` → huck `[k]="a⇥b"`, bash `[k]=$'a\tb'`
- plain `hello` → both `"hello"` (must stay identical)

- [ ] **Step 1: Write the failing unit tests**

Add to the `builtins.rs` tests module (mirrors the existing `assoc_key_bareword_for_identifier` test that calls `render_declare_value_part(&var)`):

```rust
#[test]
fn declare_p_scalar_control_uses_ansi_c() {
    use crate::shell_state::Variable;
    // value is `i` + a real newline; bash renders it as $'i\n'
    let v = Variable::scalar("i\n".to_string());
    assert_eq!(render_declare_value_part(&v), "=$'i\\n'");
    let t = Variable::scalar("a\tb".to_string());
    assert_eq!(render_declare_value_part(&t), "=$'a\\tb'");
    // 0x01 SOH -> 3-digit octal
    let c = Variable::scalar("a\u{01}b".to_string());
    assert_eq!(render_declare_value_part(&c), "=$'a\\001b'");
}

#[test]
fn declare_p_scalar_plain_unchanged() {
    use crate::shell_state::Variable;
    assert_eq!(render_declare_value_part(&Variable::scalar("hello".to_string())), "=\"hello\"");
    // `$` and `"` stay in the double-quoted form (no control char -> no $'…')
    assert_eq!(
        render_declare_value_part(&Variable::scalar("a$b\"c".to_string())),
        "=\"a\\$b\\\"c\""
    );
}

#[test]
fn declare_p_indexed_control_uses_ansi_c() {
    use crate::shell_state::{VarValue, Variable};
    let mut m = std::collections::BTreeMap::new();
    m.insert(0usize, "x".to_string());
    m.insert(1usize, "i\n".to_string());
    let a = Variable {
        value: VarValue::Indexed(m),
        exported: false, readonly: false, integer: false, case_fold: None, nameref: false,
    };
    assert_eq!(render_declare_value_part(&a), "=([0]=\"x\" [1]=$'i\\n')");
}

#[test]
fn declare_p_assoc_control_uses_ansi_c() {
    use crate::shell_state::{VarValue, Variable};
    let var = Variable {
        value: VarValue::Associative(vec![("k".into(), "a\tb".into())]),
        exported: false, readonly: false, integer: false, case_fold: None, nameref: false,
    };
    assert_eq!(render_declare_value_part(&var), "=([k]=$'a\\tb' )");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-engine declare_p_scalar_control_uses_ansi_c declare_p_indexed_control_uses_ansi_c declare_p_assoc_control_uses_ansi_c`
Expected: the three control-char tests FAIL (huck emits a literal control char in `"…"`). `declare_p_scalar_plain_unchanged` PASSES already.

- [ ] **Step 3: Add the helper**

Add immediately above `render_declare_value_part` in `builtins.rs`:

```rust
/// Quote a value for `declare -p` display. bash double-quotes normally but
/// switches the whole value to ANSI-C `$'…'` when it contains a control
/// character (newline, tab, etc.) — the same `is_control()` trigger as
/// `declare_scalar_quote`, so the `-p` and bare forms agree. Returns the full
/// quoted token (`"…"` or `$'…'`), with no leading `=`.
fn declare_p_value_quote(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        crate::param_expansion::ansi_c_quote(s)
    } else {
        format!("\"{}\"", crate::escape_double_quote_value(s))
    }
}
```

- [ ] **Step 4: Wire it into the three arms**

In `render_declare_value_part` (`builtins.rs:875-913`):

Scalar arm — replace `format!("=\"{}\"", crate::escape_double_quote_value(s))` with:
```rust
                format!("={}", declare_p_value_quote(s))
```
(keep the `if var.nameref && s.is_empty()` early `String::new()` branch above it.)

Indexed arm — replace the `.map(...)` body:
```rust
                .map(|(k, v)| format!("[{k}]={}", declare_p_value_quote(v)))
```

Associative arm — replace the `format!("[{}]=\"{}\"", quote_subscript_key(k), crate::escape_double_quote_value(v))` with:
```rust
                    format!("[{}]={}", quote_subscript_key(k), declare_p_value_quote(v))
```

- [ ] **Step 5: Run the unit tests + full engine crate**

Run: `cargo test -p huck-engine declare_p_`
Expected: all four PASS.

Run: `cargo test -p huck-engine`
Expected: PASS. If an existing `declare -p` test broke, check it: a value with a control char asserting the old `"…"` form must be updated to `$'…'` (the old assertion encoded the bug); a control-FREE value must be unchanged — if a control-free assertion changed, the implementation is wrong.

- [ ] **Step 6: Add the diff harness**

Create `tests/scripts/declare_p_ansic_diff_check.sh`, mirroring `tests/scripts/declare_no_args_diff_check.sh`'s structure (shebang, `HUCK_BIN` → `target/release/huck`, `bash`-absent SKIP, a `fragments` array, PASS/FAIL loop comparing combined stdout, `exit $(( FAIL>0 ? 1 : 0 ))`). Fragments (each ends with a `declare -p`):

```bash
fragments=(
  $'v=$\'i\\n\'; declare -p v'
  $'v=$\'a\\tb\'; declare -p v'
  $'v=$\'a\\x01b\'; declare -p v'
  $'declare -a a=(x $\'i\\n\'); declare -p a'
  $'declare -A m=([k]=$\'a\\tb\'); declare -p m'
  'v=hello; declare -p v'
  $'v=\'a$b"c\'; declare -p v'
)
```
Compare `bash -c "$frag"` vs `"$HUCK_BIN" -c "$frag"` for each; expect every PASS, `Fail: 0`.

Run:
```bash
cargo build --release --bin huck   # long timeout (~480000ms), it is slow
bash tests/scripts/declare_p_ansic_diff_check.sh | tail -2
```
Expected: `Fail: 0`. (If a fragment's `$'…'` shell-escaping is awkward in the array, write the fragments using literal control bytes via `printf` into a temp script instead — but verify each compares byte-identical to bash.)

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/scripts/declare_p_ansic_diff_check.sh
git commit -m "$(printf 'v220 task 1: declare -p ANSI-C value quoting for control chars\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Verify the herestr flip + divergence/baseline bookkeeping

**Files:**
- Modify: `docs/bash-divergences.md` (resolve herestr's ANSI-C value-quoting blocker)
- Modify: `docs/bash-test-suite-baseline.md` (herestr → PASS; Summary counts)

**Interfaces:** none (verification + docs).

- [ ] **Step 1: Confirm herestr flips**

Run:
```bash
cargo build --release --bin huck   # if not already built
BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
  HUCK_BASH_TEST_CATEGORY=herestr bash tests/bash-test-suite/runner.sh
```
Expected: herestr **PASS** (0 diff). If a residual remains, capture it
(`diff herestr.right <(THIS_SH=$PWD/target/release/huck ./target/release/huck herestr.tests)`
under `/tmp/bash-5.2.21/tests`) and report DONE_WITH_CONCERNS with the exact diff
— the flip is the success criterion.

- [ ] **Step 2: Full workspace suite**

Run: `cargo test --workspace`
Expected: PASS (~3684+).

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Find the herestr-scoped successor entry to L-57 (added in v219, naming herestr's
remaining blockers: `declare -p` ANSI-C value quoting + the runtime command-not-
found bug). The `declare -p` ANSI-C value-quoting blocker is now RESOLVED —
remove it from that entry. If the entry's only remaining content is the
harness-masked runtime command-not-found bug, keep a trimmed `[deferred]` note
for that (it is real but does not gate the category), and add a one-line
low-severity `[deferred]` note that `ansi_c_quote` passes C1 controls
(U+0080–U+009F) through literally inside `$'…'` rather than octal-escaping them
(needs locale-aware printability; not exercised by any current category).

- [ ] **Step 4: Re-triage `docs/bash-test-suite-baseline.md`**

- herestr → **PASS**; update its note and the Summary counts (PASS 7→8, FAIL 70→69 or as measured).
- Update the header (huck commit / sweep date 2026-06-25).

- [ ] **Step 5: Commit**

```bash
git add docs/bash-divergences.md docs/bash-test-suite-baseline.md
git commit -m "$(printf 'v220 task 2: herestr flips to PASS; declare -p ANSI-C blocker resolved\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer

- **Oracle is system bash 5.2.21.** Verify every expected string with
  `bash -c '<frag>; declare -p v' | cat -A` before trusting it.
- **Do not touch `declare_scalar_quote`** (the bare/`set` form) — it already
  ANSI-C-quotes and matches bash; this task is only the `declare -p` path.
- **No-regression is a hard rule:** control-free values must render byte-identical
  to before (`"…"`). The `declare_p_scalar_plain_unchanged` test guards this.
- The C1-control passthrough in `ansi_c_quote` is a known, documented edge — do
  NOT expand scope to fix it here.
