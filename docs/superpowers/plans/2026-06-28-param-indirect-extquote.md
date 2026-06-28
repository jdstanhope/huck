# `${…}` indirect-with-subscript-modifier + extquote name — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `${!name[sub]<modifier>}` (indirect through a subscripted value, then a modifier) and `${$'…'}` (an ANSI-C quote decoded as the parameter name) PARSE and behave like bash, instead of aborting with `syntax error: unterminated '${...}'`.

**Architecture:** Two independent lexer changes in `crates/huck-syntax/src/lexer.rs::scan_braced_param_expansion`, reusing existing machinery. Feature 1 routes the `${!name[@]/[*]}`-with-trailing-operator case through the existing `dispatch_braced_modifier` (instead of the keys-only/bad-subst handling) and adds an engine through-value join in `expand_indirect`. Feature 2 adds an extquote-aware name scan that decodes `$'…'` via the existing `decode_ansi_c_escapes` and routes `$"…"` / invalid-decoded-names to `recover_bad_subst`.

**Tech Stack:** Rust, huck workspace. Tests: `cargo test --workspace`. Bash-compat: `tests/scripts/*_diff_check.sh`. Parse-only sweep: `huck -n <file>`.

## Global Constraints

- **Commit trailer** (every commit, verbatim): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **No bash source vendoring**: diff harnesses run bash at runtime; never commit bash output. Bash sources for the re-parse gate live at `/tmp/bash-5.2.21` (re-fetch with `curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp` if absent).
- **Genuinely unterminated `${…}` stays a parse error** (`LexError::UnterminatedBrace`). Only lexable content defers to runtime.
- **AVOID CPU PEG / HANGS**: wrap every cargo invocation in `timeout`, e.g. `timeout 300 cargo test -p huck-syntax --lib`. Never run a bare unbounded `cargo test --workspace` during iteration; for the final sweep use `timeout 560 cargo test --workspace > LOG 2>&1` (single run, to a log) — running it twice in one shell command exceeds the 10-min tool wall limit. Any scan loop added must consume ≥1 char per iteration or break.
- **Measured bash 5.2.21 ground truth** (from the spec): `v=arr; arr=(aa bb); ${!v[@]%b}`→`aa`; `${!v[@]@Q}`→`'aa'`; real array `arr=(aa bb cb); ${!arr[@]%b}`→`aa bb cb: invalid variable name` (rc 1); `${!v[@]}`→`0` (keys, unchanged); `x1=not; ${$'x1'}`→`not`; `ab=Z; ${a$'b'}`→`Z`; `${$"x1"}`→`bad substitution`; `${$'x\ty'}`→`bad substitution`; `declare -f` of `${$'x1'}`→`${x1}`.

---

### Task 1: Feature 1 — `${!name[@]/[*]<modifier>}` parses + evaluates as indirect

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — the `Some(SubscriptKind::All) | Some(SubscriptKind::Star)` arm in `scan_braced_param_expansion`'s `${!…}` branch (~3077-3100).
- Modify: `crates/huck-engine/src/expand.rs` — `expand_indirect` `Some(sub)` through-value branch (~618-625) + the `invalid variable name` error site(s).
- Test: `crates/huck-syntax/src/lexer.rs` `mod tests`; new `tests/param_indirect_extquote_integration.rs`.

**Interfaces:**
- Consumes: `dispatch_braced_modifier(name: String, quoted: bool, subscript: Option<SubscriptKind>, chars, parts, indirect: bool, opts, dollar_start) -> Result<(), LexError>` (existing). `expand_array_param(name, &ParamModifier, sub, quoted, shell) -> ExpansionResult` (existing). `ifs_join_sep(&str) -> String` (existing, `expand.rs`). `crate::builtins::is_valid_name(&str) -> bool` (existing).
- Produces: parsing of `${!name[@]<op>}` as `WordPart::ParamExpansion { name, subscript: Some(All|Star), modifier: <op>, indirect: true }`.

- [ ] **Step 1: Write failing lexer unit tests** in `lexer.rs` `mod tests` (near the existing `${!a[@]}` keys regression test ~6394):

```rust
    #[test]
    fn indirect_keys_with_suffix_op_is_indirect_not_keys() {
        // `${!v[@]%b}` — trailing `%b` makes it indirect-through-${v[@]} + RemoveSuffix,
        // NOT the array-keys operator.
        let toks = tokenize("${!v[@]%b}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { indirect, subscript, modifier, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert!(*indirect);
        assert!(matches!(subscript, Some(SubscriptKind::All)));
        assert!(matches!(modifier, ParamModifier::RemoveSuffix { .. }));
    }

    #[test]
    fn indirect_keys_with_transform_op_is_indirect() {
        // `${!v[@]@Q}` — was wrongly BadSubst in v233; now indirect + transform.
        let toks = tokenize("${!v[@]@Q}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { indirect, subscript, modifier, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert!(*indirect);
        assert!(matches!(subscript, Some(SubscriptKind::All)));
        assert!(matches!(modifier, ParamModifier::Transform { .. }));
    }

    #[test]
    fn indirect_keys_bare_still_keys() {
        // Regression: `${!v[@]}` with NOTHING after `]` stays the keys operator.
        let toks = tokenize("${!v[@]}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::IndirectKeys));
    }
```

- [ ] **Step 2: Run, verify they fail**

Run: `timeout 300 cargo test -p huck-syntax --lib indirect_keys_with`
Expected: FAIL (`indirect_keys_with_suffix_op` / `_transform_op` error as `UnterminatedBrace` / `BadSubst`).

- [ ] **Step 3: Implement the lexer change.** Replace the inner `match chars.peek().copied()` block (the `Some('}')` / `Some('@')` / `_ =>` arms, ~3082-3099) inside the `Some(SubscriptKind::All) | Some(SubscriptKind::Star)` arm with:

```rust
            Some(SubscriptKind::All) | Some(SubscriptKind::Star) => {
                // `${!arr[@]}` / `${!arr[*]}` with NOTHING after `]` is the
                // array-KEYS operator. With a trailing operator it is instead
                // INDIRECT expansion through `${arr[@]}`'s value, then the
                // operator (bash) — route that through dispatch_braced_modifier
                // exactly like the scalar-subscript `_` arm below.
                if chars.peek() == Some(&'}') {
                    chars.next(); // consume `}`
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::IndirectKeys,
                        quoted,
                        subscript,
                        indirect: false,
                    });
                    return Ok(());
                }
                return dispatch_braced_modifier(name, quoted, subscript, chars, parts, /* indirect */ true, opts, dollar_start);
            }
```

- [ ] **Step 4: Run the lexer tests**

Run: `timeout 300 cargo test -p huck-syntax --lib indirect_keys`
Expected: PASS (3 new + the existing keys regression).

- [ ] **Step 5: Write failing engine integration tests.** Create `tests/param_indirect_extquote_integration.rs` (copy the `run_file` harness verbatim from `tests/param_expansion_badsubst_integration.rs` — read that file's lines 1-26 for the harness):

```rust
//! v234: ${!name[sub]<modifier>} indirect-with-subscript-modifier (Feature 1)
//! and ${$'…'} extquote name (Feature 2) — parse + behave like bash.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v234_{}_{}_.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn indirect_subscript_suffix_op_scalar_degenerate() {
    // v=arr (scalar) -> v[@] is "arr" -> ${arr}=arr[0]="aa" -> %b -> "aa".
    let (o, _e, c) = run_file("v=arr; arr=(aa bb); echo \"${!v[@]%b}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "aa\n");
}

#[test]
fn indirect_subscript_transform_op() {
    let (o, _e, c) = run_file("v=arr; arr=(aa bb); echo \"${!v[@]@Q}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "'aa'\n");
}

#[test]
fn indirect_subscript_real_array_is_invalid_name() {
    // Real array: arr[@] joins to "aa bb cb", used as a name -> invalid.
    let (_o, e, c) = run_file("arr=(aa bb cb); echo \"${!arr[@]%b}\"\n");
    assert_eq!(c, 1);
    assert!(e.contains("invalid variable name"), "stderr: {e}");
}

#[test]
fn indirect_keys_bare_still_works() {
    let (o, _e, c) = run_file("arr=(aa bb cb); echo \"${!arr[@]}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "0 1 2\n");
}
```

- [ ] **Step 6: Run integration tests, observe which fail.**

Run: `timeout 200 cargo test --test param_indirect_extquote_integration indirect_`
Expected: `indirect_keys_bare_still_works` PASSES; `indirect_subscript_*` likely FAIL — the through-value for `All`/`Star` currently drops to empty (`expand_array_param` returns `WordList`, not `Value`, so the `_ => String::new()` arm fires), giving "invalid indirect expansion" instead of the bash result.

- [ ] **Step 7: Implement the engine through-value join.** In `crates/huck-engine/src/expand.rs::expand_indirect`, replace the `Some(sub)` through-value arm (~618-625):

```rust
        Some(sub) => {
            // Indirect through a subscripted source. For `[@]`/`[*]` bash uses
            // the IFS-JOINED array values as the effective name (a single
            // element -> that value; multiple -> a space-joined string that is
            // an invalid name -> the `invalid variable name` fatal below). For
            // a single-index `[i]` read that element's scalar value.
            match sub {
                crate::lexer::SubscriptKind::All | crate::lexer::SubscriptKind::Star => {
                    match expand_array_param(name, &crate::lexer::ParamModifier::None, sub, /* quoted */ true, shell) {
                        ExpansionResult::WordList(ws) => ws.join(&ifs_join_sep(&shell.ifs())),
                        ExpansionResult::Value(v) => v,
                        _ => String::new(),
                    }
                }
                _ => match expand_array_param(name, &crate::lexer::ParamModifier::None, sub, quoted, shell) {
                    ExpansionResult::Value(v) => v,
                    _ => String::new(),
                },
            }
        }
```

- [ ] **Step 8: Add the invalid-name guard for a multi-word through-value.** Immediately AFTER the `let n: &str = &through;` line (~631) and BEFORE the `if through.is_empty()` block, add:

```rust
    // A non-empty through-value that is not a valid name (e.g. the space-joined
    // values of a real `${!arr[@]<op>}`) is rejected by bash as an invalid
    // variable name, before any modifier is applied.
    if !through.is_empty()
        && !crate::builtins::is_valid_name(n)
        && !n.bytes().all(|b| b.is_ascii_digit())
    {
        with_err(|err| e!(err, "{}{}: invalid variable name", shell.error_prefix(None), n));
        return ExpansionResult::Fatal { status: 1 };
    }
```

(This mirrors the v233 `${!*}`/`${!@}` valid-name check and uses `error_prefix(None)` for the bash `script: line N: …` prologue.)

- [ ] **Step 9: Run integration + lexer + engine lib tests.**

Run: `timeout 200 cargo test --test param_indirect_extquote_integration indirect_ && timeout 300 cargo test -p huck-engine --lib 2>&1 | tail -3`
Expected: all 4 `indirect_*` integration tests PASS; engine lib green (no regression).

- [ ] **Step 10: Diff against bash for the Feature-1 cases.**

Run:
```bash
cargo build -p huck --quiet
for s in 'v=arr; arr=(aa bb); echo "${!v[@]%b}"' 'v=arr; arr=(aa bb); echo "${!v[@]@Q}"' 'arr=(aa bb cb); echo "${!arr[@]%b}"' 'arr=(aa bb cb); echo "${!arr[@]}"'; do
  printf '%s\n' "$s" > /tmp/f1.sh
  diff <(bash /tmp/f1.sh 2>&1) <(./target/debug/huck /tmp/f1.sh 2>&1) >/dev/null && echo "OK: $s" || { echo "DIFF: $s"; diff <(bash /tmp/f1.sh 2>&1) <(./target/debug/huck /tmp/f1.sh 2>&1); }
done
```
Expected: all `OK`. If the `invalid variable name` line differs only on the `line N:` prologue, that is now aligned via `error_prefix(None)`; investigate any other diff as a real bug.

- [ ] **Step 11: Build workspace warning-clean + commit.**

Run: `timeout 300 cargo build --workspace 2>&1 | tail -3`
Expected: clean.

```bash
git add -A
git commit -m "v234 F1: \${!name[@]<modifier>} indirect-with-subscript-modifier

Trailing operator after \${!name[@]}/[*] is indirect expansion through
\${name[@]}'s value then the operator (bash), not the keys operator and
not a bad-subst (retires the v233 \${!arr[@]@OP} BadSubst routing). Lexer
routes it through dispatch_braced_modifier; expand_indirect IFS-joins the
\`[@]\`/\`[*]\` through-value and rejects a multi-word effective name with
\`<value>: invalid variable name\` (error_prefix prologue).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Feature 2 — `$'…'` decoded as the name inside `${…}`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — add `is_valid_param_name` + `scan_braced_name_ext` helpers; hook the `Some('$')` arm (~2914) and the regular-name path (~3111).
- Test: `lexer.rs` `mod tests`; `tests/param_indirect_extquote_integration.rs` (add cases).

**Interfaces:**
- Consumes: `decode_ansi_c_escapes(&str) -> String` (existing, public). `recover_bad_subst(chars, parts, quoted, dollar_start)` (existing).
- Produces: `scan_braced_name_ext(chars) -> Result<NameScan, LexError>` where `enum NameScan { Name { name: String, decoded: bool }, BadSubst }`.

- [ ] **Step 1: Write failing lexer unit tests** in `lexer.rs` `mod tests`:

```rust
    #[test]
    fn extquote_name_decodes_to_identifier() {
        // `${$'x1'}` -> name "x1".
        let toks = tokenize(r#"${$'x1'}"#).unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { name, .. } | WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected name-bearing part, got {:?}", parts[0]) };
        assert_eq!(name, "x1");
    }

    #[test]
    fn extquote_name_concatenates() {
        // `${a$'b'}` -> name "ab".
        let toks = tokenize(r#"${a$'b'}"#).unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { name, .. } | WordPart::Var { name, .. } = &parts[0]
        else { panic!("got {:?}", parts[0]) };
        assert_eq!(name, "ab");
    }

    #[test]
    fn extquote_locale_name_is_bad_subst() {
        // `${$"x1"}` -> bash bad substitution.
        let toks = tokenize(r#"${$"x1"}"#).unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        assert!(matches!(parts[0], WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }));
    }

    #[test]
    fn extquote_decoded_invalid_name_is_bad_subst() {
        // `${$'x\ty'}` decodes to "x<TAB>y" — invalid name -> bad substitution.
        let toks = tokenize("${$'x\\ty'}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        assert!(matches!(parts[0], WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }));
    }
```

- [ ] **Step 2: Run, verify they fail** — `timeout 300 cargo test -p huck-syntax --lib extquote_`. Expected: FAIL (`UnterminatedBrace`).

- [ ] **Step 3: Add helpers** near `scan_braced_name` (~3469):

```rust
/// A valid POSIX parameter name: `[A-Za-z_][A-Za-z0-9_]*`, non-empty.
fn is_valid_param_name(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    cs.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Result of scanning a braced parameter NAME with `extquote` support.
enum NameScan {
    /// The assembled name. `decoded` is true if any `$'…'` run contributed
    /// (so the caller validates it as an identifier).
    Name { name: String, decoded: bool },
    /// A `$"…"` in name position — bash bad-substs it; the caller recovers.
    BadSubst,
}

/// Scans a braced parameter name, decoding any `$'…'` (ANSI-C) runs into the
/// name (bash `extquote`). `${a$'b'}` -> "ab". Stops at the first non-name,
/// non-`$'…'` char (leaving the cursor there for subscript/modifier scanning).
/// A `$"…"` (locale) run in name position returns `NameScan::BadSubst`.
fn scan_braced_name_ext(chars: &mut CharCursor<'_>) -> Result<NameScan, LexError> {
    let mut name = String::new();
    let mut decoded = false;
    loop {
        match chars.peek().copied() {
            Some(c) if c == '_' || c.is_ascii_alphanumeric() => {
                name.push(c);
                chars.next();
            }
            Some('$') => {
                // Look past `$` for `'` (ANSI-C, decode) / `"` (locale, bad-subst).
                let mut look = chars.clone();
                look.next();
                match look.peek().copied() {
                    Some('\'') => {
                        chars.next(); // `$`
                        chars.next(); // `'`
                        // ANSI-C span: `\` escapes the next char; closes on the
                        // first UNescaped `'`. Reuses the M4 span shape.
                        let mut body = String::new();
                        loop {
                            match chars.next() {
                                None => return Err(LexError::UnterminatedBrace),
                                Some('\\') => {
                                    body.push('\\');
                                    if let Some(c) = chars.next() { body.push(c); }
                                }
                                Some('\'') => break,
                                Some(c) => body.push(c),
                            }
                        }
                        name.push_str(&decode_ansi_c_escapes(&body));
                        decoded = true;
                    }
                    Some('"') => return Ok(NameScan::BadSubst),
                    _ => break, // a `$` not starting a quote ends the name run
                }
            }
            _ => break,
        }
    }
    Ok(NameScan::Name { name, decoded })
}
```

- [ ] **Step 4: Hook the regular-name path.** Replace the regular-name block in `scan_braced_param_expansion` (~3111-3115):

```rust
    let name = match scan_braced_name_ext(chars)? {
        NameScan::BadSubst => return recover_bad_subst(chars, parts, quoted, dollar_start),
        NameScan::Name { name, decoded } => {
            // A decoded name must be a valid identifier (e.g. `${$'x\ty'}` is
            // invalid -> bad subst). A non-decoded name keeps the prior
            // behavior exactly (empty -> bad subst below).
            if decoded && !is_valid_param_name(&name) {
                return recover_bad_subst(chars, parts, quoted, dollar_start);
            }
            name
        }
    };
    if name.is_empty() {
        // `${}` (truly empty) or `${+foo}` etc. — bad substitution at runtime.
        return recover_bad_subst(chars, parts, quoted, dollar_start);
    }
```

- [ ] **Step 5: Hook the `Some('$')` arm** so `${$'…'}` / `${$"…"}` fall through to the regular-name path instead of being parsed as the `$` (shell-pid) special param. Replace the `Some('$')` arm (~2914-2917):

```rust
        Some('$') => {
            // `${$'…'}` (extquote name) / `${$"…"}` (bad-subst) must NOT be
            // parsed as the `$` shell-pid special param. If `$` is followed by
            // a quote, fall through to the extquote-aware regular-name path.
            let mut look = chars.clone();
            look.next();
            if matches!(look.peek().copied(), Some('\'') | Some('"')) {
                // fall through (do not consume, do not return)
            } else {
                chars.next();
                return dispatch_braced_modifier("$".to_string(), quoted, None, chars, parts, false, opts, dollar_start);
            }
        }
```

- [ ] **Step 6: Run lexer tests** — `timeout 300 cargo test -p huck-syntax --lib extquote_ && timeout 300 cargo test -p huck-syntax --lib`. Expected: 4 new PASS; full lexer lib green (no regression).

- [ ] **Step 7: Add engine integration tests** to `tests/param_indirect_extquote_integration.rs`:

```rust
#[test]
fn extquote_name_resolves_value() {
    let (o, _e, c) = run_file("x1=not; echo \"${$'x1'}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "not\n");
}

#[test]
fn extquote_nested_pattern_operand() {
    // ${x#${$'x1'%$'t'}} -> ${x1%t}="no" -> strip prefix "no" from "notOK" -> "tOK".
    let (o, _e, c) = run_file("x=notOK; x1=not; echo \"${x#${$'x1'%$'t'}}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "tOK\n");
}

#[test]
fn extquote_declare_f_reconstructs_decoded() {
    // declare -f normalizes ${$'x1'} to ${x1} (bash behavior, free via decoded name).
    let (o, _e, _c) = run_file("f() { x1=not; echo \"${$'x1'}\"; }\ndeclare -f f\n");
    assert!(o.contains("${x1}"), "stdout: {o}");
}
```

- [ ] **Step 8: Run integration + diff against bash.**

Run:
```bash
cargo build -p huck --quiet
timeout 200 cargo test --test param_indirect_extquote_integration
for s in "x1=not; echo \"\${\$'x1'}\"" "ab=Z; echo \"\${a\$'b'}\"" "x=notOK; x1=not; echo \"\${x#\${\$'x1'%\$'t'}}\""; do
  printf '%s\n' "$s" > /tmp/f2.sh
  diff <(bash /tmp/f2.sh 2>&1) <(./target/debug/huck /tmp/f2.sh 2>&1) >/dev/null && echo "OK: $s" || { echo "DIFF: $s"; diff <(bash /tmp/f2.sh 2>&1) <(./target/debug/huck /tmp/f2.sh 2>&1); }
done
```
Expected: all integration tests PASS; all diffs `OK`.

- [ ] **Step 9: Build workspace + commit.**

Run: `timeout 300 cargo build --workspace 2>&1 | tail -3` (clean).

```bash
git add -A
git commit -m "v234 F2: \$'…' decoded as the name inside \${…} (extquote)

scan_braced_name_ext decodes \$'…' runs into the parameter name via
decode_ansi_c_escapes (\${\$'x1'}->x1, \${a\$'b'}->ab); \$\"…\" in name
position and an invalid decoded name route to recover_bad_subst (matching
bash). declare -f reconstruction emits the normalized \${x1} for free.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Diff harness, re-parse gate, full sweep, measurement

**Files:**
- Create: `tests/scripts/param_indirect_extquote_diff_check.sh`

**Interfaces:** uses `$HUCK_BIN` (default `$(pwd)/target/debug/huck`), mirroring `tests/scripts/param_expansion_diff_check.sh`.

- [ ] **Step 1: Write the harness** at `tests/scripts/param_indirect_extquote_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v234: ${!name[sub]<mod>} indirect
# (Feature 1) and ${$'…'} extquote name (Feature 2).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ie.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Feature 1: indirect-with-subscript-modifier
checkf "scalar-degenerate %op" 'v=arr; arr=(aa bb); echo "${!v[@]%b}"'
checkf "transform @Q"          'v=arr; arr=(aa bb); echo "${!v[@]@Q}"'
checkf "real array invalid"    'arr=(aa bb cb); echo "${!arr[@]%b}"'
checkf "bare keys unchanged"   'arr=(aa bb cb); echo "${!arr[@]}"'
checkf "star subscript #op"    'v=arr; arr=(Xa Xb); echo "${!v[*]#X}"'
# Feature 2: extquote name
checkf "extquote name"         "x1=not; echo \"\${\$'x1'}\""
checkf "extquote concat"       "ab=Z; echo \"\${a\$'b'}\""
checkf "extquote nested patt"  "x=notOK; x1=not; echo \"\${x#\${\$'x1'%\$'t'}}\""
checkf "extquote locale bad"   "echo \"\${\$\"x1\"}\"; echo after"
checkf "extquote invalid name" "echo \"\${\$'x\\ty'}\"; echo after"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: chmod + build + run.**

Run: `chmod +x tests/scripts/param_indirect_extquote_diff_check.sh && cargo build -p huck --quiet && ./tests/scripts/param_indirect_extquote_diff_check.sh`
Expected: `Total: 10, Pass: 10, Fail: 0`. If a bad-subst case FAILs ONLY on the error-message prologue (`line N:` / `huck:` vs `bash:`), record it as the known staged-prologue divergence — but the `: bad substitution` / `invalid variable name` tail and the exit/continuation MUST match. Investigate any other FAIL as a real bug (esp. the invalid-decoded-name message-form residual noted in the spec — confirm it matches; if the source-vs-decoded form differs, soften ONLY that case's assertion in the harness to the shared tail and note it, do not weaken Feature behavior).

- [ ] **Step 3: Re-parse gate.** Build release and confirm the two M-148 files now parse past their old failure lines:

```bash
cargo build --release -p huck --quiet
for f in new-exp13.sub posixexp7.sub; do
  e=$(./target/release/huck -n "/tmp/bash-5.2.21/tests/$f" 2>&1 >/dev/null)
  if [ -n "$e" ]; then echo "STILL FAILS: $f -> $(echo "$e" | head -1)"; else echo "PARSES: $f"; fi
done
```
Expected: both `PARSES` (the v233 residual lines 72 / 58 were exactly `${!varname[@]%b}` and `${x#${$'x1'%$'t'}}`). If one still fails on a DIFFERENT later line, record it as a new residual; if it still fails on the M-148 construct, it's a bug in Task 1/2.

- [ ] **Step 4: Full sweep.**

Run: `timeout 560 cargo test --workspace > /tmp/v234-sweep.log 2>&1; echo "EXIT=$?"; grep -nE "FAILED|panicked|[1-9][0-9]* failed" /tmp/v234-sweep.log | head`
Expected: `EXIT=0`, no `FAILED`/`panicked` lines.

- [ ] **Step 5: Commit the harness.**

```bash
git add tests/scripts/param_indirect_extquote_diff_check.sh
git commit -m "v234: param_indirect_extquote_diff_check harness + re-parse gate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Measure (controller does this post-review).** Re-run the affected bash-test categories and record flip/shrink honestly:

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
for cat in new-exp posixexp errors; do
  HUCK_BASH_TEST_CATEGORY=$cat timeout 120 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "PASS:|FAIL:" | head -2
done
```
Expected: no flip (parse-robustness iteration; PASS stays 10). Record the honest result.

---

## Post-implementation (controller, after final review)

- Update `docs/bash-divergences.md`: DELETE the resolved **M-148** entry (both parts closed). Add a new `[deferred]` low entry ONLY for the invalid-decoded-name message-form residual (`${$'x\ty'}` → huck shows raw source, bash shows decoded form) if it indeed diverges. Note that v234 also retired the v233 `${!arr[@]@OP}` BadSubst routing.
- Record v234 in `MEMORY.md` + `project_huck_iterations.md`.

## Self-Review

- **Spec coverage:** Feature 1 (`${!name[sub]<modifier>}`) → Task 1 (lexer route + engine through-value join + invalid-name prologue). Feature 2 (`$'…'` extquote name) → Task 2 (scan_braced_name_ext + two hooks). M3-combo `${!arr[@]@OP}` retirement → Task 1 (now indirect+transform). Reconstruction normalization → Task 2 (free, tested in Step 7). Testing/harness/re-parse-gate/measurement → Task 3. All spec sections covered.
- **Placeholder scan:** every code step carries the actual code; the only "verify and fix if diverges" hedge (Task 1 engine) is backed by concrete code (Steps 7-8) and a failing-test gate (Step 6).
- **Type consistency:** `NameScan { Name { name, decoded }, BadSubst }`, `is_valid_param_name`, and `scan_braced_name_ext` are defined in Task 2 Step 3 and used consistently in Steps 4-5. `dispatch_braced_modifier`'s 8-arg signature matches its existing call sites. `SubscriptKind::{All,Star}`, `ParamModifier::{IndirectKeys,RemoveSuffix,Transform,BadSubst}` are existing variants.
- **Risk note for the executor:** Task 1 Step 7 (the `[@]`/`[*]` through-value join) and Task 2 Step 5 (the `Some('$')` fall-through that must NOT consume) are the most error-prone; rely on the failing-test gates (Step 6 each) and `cargo build --workspace` to confirm.
