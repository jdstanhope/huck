# `${…}` Parameter-Expansion Parse Robustness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four `${…}` parse gaps (M1 prefix-name expansion, M2 special-parameter names, M3 `@`-transform edges, M4 `$'…'` in brace patterns) and adopt bash's "scan-to-`}` then defer to runtime" model so a lexable-but-invalid `${…}` becomes a runtime *bad substitution* instead of a parse abort.

**Architecture:** All parsing lives in `crates/huck-syntax/src/lexer.rs` (`scan_braced_operand`, `scan_braced_param_expansion`, `dispatch_braced_modifier`). Evaluation lives in `crates/huck-engine/src/param_expansion.rs` (scalar modifier evaluator) and `crates/huck-engine/src/expand.rs` (routing/array path). Reconstruction lives in `crates/huck-syntax/src/generate.rs`. The model rides on the existing `ParamModifier` enum (`#[non_exhaustive]`) — **no new field on `WordPart::ParamExpansion`** (it has ~70 references). Two new `ParamModifier` variants are added: `PrefixNames { at: bool }` (M1) and `BadSubst { raw: String }` (defer). The M3 combo `${!arr[@]@OP}` (which bash also errors on at runtime, rc=1) routes to `BadSubst`.

**Tech Stack:** Rust, huck workspace. Tests: `cargo test --workspace`. Bash-compat: `tests/scripts/*_diff_check.sh`. Parse-only sweep: `huck -n <file>`.

## Global Constraints

- **Commit trailer** (every commit, verbatim): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **No bash source vendoring**: diff harnesses run bash at runtime; never commit bash output.
- **Unterminated `${…}` stays a parse error** (`LexError::UnterminatedBrace`). Only lexable-with-matching-`}` content defers to runtime `BadSubst`.
- **`ParamModifier` is `#[non_exhaustive]`**: new variants must be handled in every exhaustive match across `param_expansion.rs`, `expand.rs`, `generate.rs`, `lexer.rs`. After each task, `cargo build --workspace` must be warning-clean and `cargo test --workspace` green — that is the safety net that catches a missed match arm.
- **Measured bash ground truth** (from the spec, bash 5.2.21):
  - `${!_Q*}`/`${!_Q@}` with `_Qa=1 _Qb=2` → `_Qa _Qb` (sorted names); no match → empty, rc 0.
  - `${##}` (set -- a b c) → `1` (length of `$#`); `${!#}` → `c` (indirect of `$#`).
  - `${-3}`, `${-3:-x}`, `${$x}`, `${V@}`, `${H*}` → runtime `bash: line N: ${RAW}: bad substitution`, parse OK under `bash -n`.
  - `${x#$'a\t\'\tb'}` (x=aXb) → `aXb`; `${x#$'f'}` (x=foo) → `oo`.

---

### Task 1: M4 — recognize `$'…'` (and `$"…"`) inside the brace-body scan

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `scan_braced_operand` (~2577, the `Some('$')` arm).

**Interfaces:**
- Consumes: `CharCursor`. Produces: unchanged signature `fn scan_braced_operand(chars) -> Result<String, LexError>`.

The `Some('$')` arm currently peeks for `{` (nest) and `(` (cmdsub) but NOT `'`/`"`, so `$'…'`'s escaped `\'` derails the scan and yields a false `UnterminatedBrace`.

- [ ] **Step 1: Write failing unit tests** in the `lexer.rs` `mod tests` block (near the other `braced` tests, e.g. after `braced_operand_bare_brace_is_literal` ~line 4109):

```rust
    #[test]
    fn braced_operand_ansi_c_quote_with_escaped_quote() {
        // `$'a\t\'\tb'` inside the body: the escaped `\'` must NOT terminate
        // the scan, and the trailing `'` is the ANSI-C close, not a new span.
        let toks = tokenize(r#"${x#$'a\t\'\tb'}"#).unwrap();
        // It must tokenize (not error). Exactly one Word token with a single
        // ParamExpansion part (RemovePrefix).
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn braced_operand_ansi_c_quote_simple() {
        let toks = tokenize(r#"${x#$'f'}"#).unwrap();
        assert_eq!(toks.len(), 1);
    }
```

- [ ] **Step 2: Run, verify they fail**

Run: `cargo test -p huck-syntax --lib braced_operand_ansi_c`
Expected: FAIL (`UnterminatedBrace` / `UnterminatedQuote`).

- [ ] **Step 3: Implement** — in `scan_braced_operand`, extend the `Some('$')` arm (currently matches `{` and `(`) to also consume an ANSI-C `$'…'` (and locale `$"…"`) span verbatim with escape handling. Replace the `match chars.peek()` inside the `Some('$')` arm:

```rust
            Some('$') => {
                // `${` nests; `$(` is a cmdsub consumed verbatim; `$'…'` /
                // `$"…"` are ANSI-C / locale quoted spans whose internal `'`/`"`
                // (and `\'` escapes) must not be mistaken for plain quoting.
                body.push('$');
                match chars.peek() {
                    Some(&'{') => {
                        chars.next();
                        body.push('{');
                        depth += 1;
                    }
                    Some(&'(') => {
                        chars.next();
                        body.push('(');
                        consume_paren_cmdsub_verbatim(chars, &mut body)?;
                    }
                    Some(&'\'') => {
                        chars.next();
                        body.push('\'');
                        // ANSI-C span: `\` escapes the next char (incl. `\'`),
                        // closing on the first UNescaped `'`.
                        loop {
                            match chars.next() {
                                None => return Err(LexError::UnterminatedBrace),
                                Some('\\') => {
                                    body.push('\\');
                                    if let Some(c) = chars.next() { body.push(c); }
                                }
                                Some('\'') => { body.push('\''); break; }
                                Some(c) => body.push(c),
                            }
                        }
                    }
                    Some(&'"') => {
                        chars.next();
                        body.push('"');
                        // Locale `$"…"`: same scan as a normal double-quote span
                        // (handled by the outer `Some('"')` loop shape).
                        loop {
                            match chars.next() {
                                None => return Err(LexError::UnterminatedBrace),
                                Some('"') => { body.push('"'); break; }
                                Some('\\') => {
                                    body.push('\\');
                                    if let Some(c) = chars.next() { body.push(c); }
                                }
                                Some(c) => body.push(c),
                            }
                        }
                    }
                    _ => {}
                }
            }
```

- [ ] **Step 4: Run the new tests + the full lexer suite**

Run: `cargo test -p huck-syntax --lib`
Expected: PASS (new tests green, no regressions).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v233 M4: recognize \$'...' / \$\"...\" inside \${...} brace body

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: BadSubst — defer lexable-but-invalid `${…}` to a runtime "bad substitution"

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — add `ParamModifier::BadSubst { raw: String }`; add a `CharCursor` raw-slice accessor; add `recover_bad_subst`; redirect the defer points in `scan_braced_param_expansion` (the three `EmptyParamName` sites) and `dispatch_braced_modifier` (the catch-all `Some(c)`, the `:`→`}` site, the `@`-arm empty/unknown op, and the IndirectKeys-`[@]`-then-`@OP` combo).
- Modify: `crates/huck-engine/src/param_expansion.rs` — `BadSubst` arm in the scalar modifier evaluator (`match modifier`, ~106).
- Modify: `crates/huck-engine/src/expand.rs` — ensure `BadSubst` routes to the scalar evaluator (early guard or matching arm) so it is not silently absorbed by a `(modifier, subscript)` wildcard.
- Modify: `crates/huck-syntax/src/generate.rs` — `BadSubst` reconstruction arm (emit the raw text).

**Interfaces:**
- Produces: `ParamModifier::BadSubst { raw: String }` — `raw` is the literal `${…}` source (e.g. `"${$x}"`), used verbatim in the runtime message.
- The runtime message format: `<shell.error_prefix(None)>{raw}: bad substitution` (matches bash `bash: line N: ${…}: bad substitution`).

Lazy evaluation is required: a `BadSubst` node only errors when it is *expanded* (so `${H*}` behind a short-circuited `||` never errors — matching bash).

- [ ] **Step 1: Add the variant + a CharCursor raw accessor.** In `lexer.rs`, add to `enum ParamModifier` (after `Transform`):

```rust
    /// A `${…}` whose content is lexable (matching `}` found) but
    /// semantically invalid (bad modifier, special char as name, etc.).
    /// Parses successfully and defers to a RUNTIME "bad substitution"
    /// error, matching bash. `raw` is the literal `${…}` source for the
    /// message. Evaluated lazily — only errors when actually expanded.
    BadSubst { raw: String },
```

Add to `impl CharCursor` (after `line()`):

```rust
    /// Byte slice of the source from `start` to the current offset. Used to
    /// reconstruct the raw `${…}` text for a deferred bad-substitution.
    pub fn slice_from(&self, start: usize) -> &str {
        &self.s[start..self.pos]
    }
```

- [ ] **Step 2: Add the recovery helper** in `lexer.rs` (near `scan_braced_operand`). It consumes the remainder of the current `${…}` (depth + quote aware) through the matching `}`, then returns a `BadSubst` part whose `raw` is `${` + the full body. `dollar_start` is the byte offset of the leading `$` (captured by the caller before `${` is consumed).

```rust
/// Recovery for a lexable-but-invalid `${…}`: consume the rest of the brace
/// body through the matching `}`, then build a `BadSubst` ParamExpansion whose
/// `raw` is the literal `${…}` source (for the runtime message). `dollar_start`
/// is the offset of the leading `$`. Used so bad substitutions defer to runtime
/// instead of aborting the parse (matching bash).
fn recover_bad_subst(
    chars: &mut CharCursor<'_>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
    dollar_start: usize,
) -> Result<(), LexError> {
    // `scan_braced_operand` consumes through the matching `}` (depth + quote +
    // $'…' aware after Task 1). It returns the inner body; we don't need it —
    // we slice the raw source instead, which includes `${` … `}`.
    let _ = scan_braced_operand(chars)?; // may still error on genuinely unterminated
    let raw = chars.slice_from(dollar_start).to_string();
    parts.push(WordPart::ParamExpansion {
        name: String::new(),
        modifier: ParamModifier::BadSubst { raw },
        quoted,
        subscript: None,
        indirect: false,
    });
    Ok(())
}
```

- [ ] **Step 3: Thread `dollar_start` into `scan_braced_param_expansion`.** Its caller (`scan_dollar_expansion`, ~1801) consumes `$` then `{` before calling it. Capture the `$` offset there and pass it in. Change the signature to accept `dollar_start: usize` and update the single call site. In `scan_dollar_expansion`, immediately before consuming `$`/`{` for the brace form, record `let dollar_start = chars.offset() - 1;` (the `$` was just consumed; it is 1 byte). If the existing code structure makes the pre-`$` offset awkward, instead capture `let dollar_start = <offset of '$'>` at the point `$` is seen and thread it. Verify by asserting `chars.slice_from(dollar_start)` starts with `${` in a debug test.

- [ ] **Step 4: Redirect the defer points.** Replace each of these `Err(...)` returns with `return recover_bad_subst(chars, parts, quoted, dollar_start);`:
  - `scan_braced_param_expansion`: the three `return Err(LexError::EmptyParamName);` sites (~2868, ~2940, ~2969). (NOTE: `${}` with a truly empty body — `chars.peek() == Some('}')` — should still be `EmptyParamName`/bad-subst per bash: `${}` → bash "bad substitution". Route `${}` to `recover_bad_subst` too.)
  - `dispatch_braced_modifier`: the catch-all `Some(c) => Err(LexError::InvalidBraceModifier(c.to_string()))` (~3535) → `recover_bad_subst`. The `Some(':')`→`Some('}')` site (~3407) → `recover_bad_subst`. The `@`-arm unknown/empty op (`other =>` ~3512, and the post-op non-`}` `_ => Err(UnterminatedBrace)` ~3532 when a `}` does NOT follow but the op was valid — leave genuinely-unterminated as-is; only the *empty op* `${V@}` where `@` is immediately followed by `}` becomes BadSubst). For `${V@}`: in the `@` arm, before reading the op letter, if `chars.peek() == Some(&'}')` → `recover_bad_subst`.
  - `dispatch_braced_modifier` needs `dollar_start` too — thread it through as a parameter (update its signature + all call sites in `scan_braced_param_expansion`).
  - **M3 combo** `${!arr[@]@OP}`: in `scan_braced_param_expansion`, the `Some(SubscriptKind::All) | Some(SubscriptKind::Star)` arm (~2944) currently requires `}` next. If the next char is `@` (a trailing transform on indirect-keys — which bash errors on at runtime), call `recover_bad_subst` instead of `Err(UnterminatedBrace)`.

  Keep `UnterminatedBrace` (genuinely no matching `}`) as a parse error everywhere.

- [ ] **Step 5: Lexer unit tests** (in `lexer.rs` `mod tests`):

```rust
    #[test]
    fn bad_subst_dollar_name_defers() {
        let toks = tokenize("${$x}").unwrap(); // parses, no error
        let parts = single_param_expansion(&mut toks.clone());
        assert!(matches!(parts,
            WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { ref raw }, .. } if raw == "${$x}"));
    }

    #[test]
    fn bad_subst_empty_transform_op_defers() {
        assert!(tokenize("${V@}").is_ok());
    }

    #[test]
    fn bad_subst_dash_digit_defers() {
        assert!(tokenize("${-3}").is_ok());
        assert!(tokenize("${-3:-x}").is_ok());
    }

    #[test]
    fn bad_subst_star_modifier_defers() {
        assert!(tokenize("${H*}").is_ok()); // cond.tests shape
    }

    #[test]
    fn unterminated_brace_still_errors() {
        assert_eq!(tokenize("${x").unwrap_err(), LexError::UnterminatedBrace);
    }
```

(If `single_param_expansion` helper signature differs, adapt — it exists at ~4332.)

- [ ] **Step 6: Engine — evaluate BadSubst to a runtime error.** In `param_expansion.rs`, add to the `match modifier` (~106) an arm:

```rust
        ParamModifier::BadSubst { raw } => {
            with_err(|err| e!(err, "{}{}: bad substitution", shell.error_prefix(None), raw));
            ExpansionResult::Fatal { status: 1 }
        }
```

Confirm `with_err` and `shell.error_prefix` are in scope in this function (they are used elsewhere in the file / crate). In `expand.rs`, ensure a `BadSubst` modifier with `subscript: None` reaches this scalar evaluator and is NOT swallowed by a `(modifier, subscript)` wildcard returning empty — add an early guard at the top of the main param-expansion entry point:

```rust
    if let crate::lexer::ParamModifier::BadSubst { .. } = modifier {
        // Route to the scalar evaluator which emits the runtime error.
        // (falls through to the normal scalar path)
    }
```

If the existing routing already sends `subscript: None` to the scalar evaluator, no guard is needed — verify by test (Step 8). Add explicit `BadSubst` arms to any exhaustive `match modifier` in `expand.rs` that would otherwise fail to compile (the `#[non_exhaustive]` build error will point them out); each such arm should defer to the scalar evaluator or return `Fatal { status: 1 }` after emitting the message once.

- [ ] **Step 7: generate.rs reconstruction.** Add a `BadSubst` arm wherever `ParamModifier` is matched in `generate.rs` (~782, ~795). It reconstructs to the raw text:

```rust
        ParamModifier::BadSubst { raw } => raw.clone(),
```

(Place appropriately in `param_expansion_to_source` so `declare -f`/`type` reproduce the original `${…}`.)

- [ ] **Step 8: Integration test** — create `tests/param_expansion_badsubst_integration.rs`:

```rust
//! v233: lexable-but-invalid ${...} defers to a runtime "bad substitution"
//! (matching bash) instead of a parse abort.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v233bs_{}_{}_.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn bad_subst_errors_at_runtime_not_parse() {
    let (_o, e, _c) = run_file("echo before\necho ${$x}\necho after\n");
    // Parses (so "before" runs); the bad subst errors at runtime.
    assert!(e.contains("bad substitution"), "stderr: {e}");
}

#[test]
fn bad_subst_short_circuited_does_not_error() {
    // ${H*} behind a short-circuit is never evaluated -> no error, rc 0.
    let (_o, e, c) = run_file("[[ -n yes || -z ${H*} ]]\necho rc=$?\n");
    assert!(!e.contains("bad substitution"), "should not error: {e}");
    assert_eq!(c, 0);
}
```

- [ ] **Step 9: Run + commit**

Run: `cargo test -p huck-syntax --lib && cargo test --test param_expansion_badsubst_integration && cargo build --workspace`
Expected: PASS, warning-clean.

```bash
git add -A
git commit -m "v233: defer lexable-but-invalid \${...} to runtime bad substitution

ParamModifier::BadSubst{raw}: scan-to-} recovery in the lexer turns
bad modifiers / special-char names / \${V@} / \${!arr[@]@OP} into a lazy
node that errors at runtime (matching bash), not a parse abort.
Unterminated \${...} still parse-errors.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: M1 — prefix-name expansion `${!pfx*}` / `${!pfx@}`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — add `ParamModifier::PrefixNames { at: bool }`; emit it in the `${!…}` path of `scan_braced_param_expansion`.
- Modify: `crates/huck-engine/src/param_expansion.rs` — evaluate `PrefixNames` (sorted matching names; `*` joined, `@` separate).
- Modify: `crates/huck-engine/src/expand.rs` — route `PrefixNames` so the `@` form yields a word list (like `IndirectKeys`/`AllArgs`).
- Modify: `crates/huck-syntax/src/generate.rs` — reconstruct `${!name*}` / `${!name@}`.

**Interfaces:**
- `ParamModifier::PrefixNames { at: bool }` — `at=true` for `@`, `false` for `*`. `name` holds the prefix.

- [ ] **Step 1: Add the variant** in `lexer.rs`:

```rust
    /// `${!prefix*}` / `${!prefix@}` — expand to the sorted NAMES of set
    /// shell variables whose name starts with `prefix`. `at=false` (`*`)
    /// joins like `$*`; `at=true` (`@`) yields separate words like `$@`.
    PrefixNames { at: bool },
```

- [ ] **Step 2: Emit it in the lexer.** In `scan_braced_param_expansion`, the `${!…}` block (after `let name = scan_braced_name(chars)?;` at ~2938), BEFORE `scan_param_subscript`, check for a trailing `*`/`@` directly before `}`:

```rust
        let name = scan_braced_name(chars)?;
        if name.is_empty() {
            return recover_bad_subst(chars, parts, quoted, dollar_start);
        }
        // `${!prefix*}` / `${!prefix@}` — prefix-name expansion. Distinguish
        // `@}` (prefix) from `@OP}` (a transform on an indirect ref): only a
        // `*`/`@` IMMEDIATELY followed by `}` is the prefix form.
        match chars.peek().copied() {
            Some('*') => {
                let mut look = chars.clone();
                look.next();
                if look.peek() == Some(&'}') {
                    chars.next(); chars.next(); // consume '*' and '}'
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::PrefixNames { at: false },
                        quoted, subscript: None, indirect: false,
                    });
                    return Ok(());
                }
            }
            Some('@') => {
                let mut look = chars.clone();
                look.next();
                if look.peek() == Some(&'}') {
                    chars.next(); chars.next(); // consume '@' and '}'
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::PrefixNames { at: true },
                        quoted, subscript: None, indirect: false,
                    });
                    return Ok(());
                }
            }
            _ => {}
        }
        let subscript = scan_param_subscript(chars, opts)?;
        // ... existing match subscript { ... } unchanged ...
```

(`CharCursor` derives `Clone` already — it is a small `Copy`-of-fields struct with a `&str`; if it does not derive `Clone`, add `#[derive(Clone)]` to it, or re-implement the two-char lookahead by peeking the next char and, if `*`/`@`, reading it and peeking again, pushing back is not available — so prefer adding `#[derive(Clone)]`. Verify which is true and use the simplest that compiles.)

- [ ] **Step 3: Engine — evaluate PrefixNames.** In `param_expansion.rs` `match modifier`, add:

```rust
        ParamModifier::PrefixNames { at } => {
            // Sorted names of set shell variables starting with `name`.
            let mut names: Vec<String> = shell
                .var_names_with_prefix(name);   // implement/locate: iterate the
                                                // variable map, filter set vars
                                                // by `starts_with(name)`, collect.
            names.sort();
            if *at && !quoted {
                ExpansionResult::Fields(names)
            } else if *at {
                // quoted `@`: bash still yields separate words; mirror IFS join
                // behavior of the surrounding context via Fields when unquoted,
                // and a single IFS-joined field when quoted is acceptable here.
                ExpansionResult::Fields(names)
            } else {
                // `*`: join with first char of IFS (default space).
                let sep = shell.ifs_first_char().unwrap_or(' ');
                ExpansionResult::Value(names.join(&sep.to_string()))
            }
        }
```

Locate the real variable-map accessor (the file already uses `shell.lookup_var`; find the iterator over variable names — e.g. a `vars`/`variables` map or an existing `names()`/`exported()` helper) and the IFS-first-char helper (or read `shell.lookup_var("IFS")`). If no `var_names_with_prefix`/`ifs_first_char` helper exists, inline the logic. Do NOT include unset variables. Sorting is byte/locale-C order (matches bash default).

- [ ] **Step 4: expand.rs routing.** Ensure `PrefixNames` reaches the scalar evaluator and its `Fields` result is splat into the word list (the same path `IndirectKeys`/`AllArgs` use). Add explicit `PrefixNames` arms to any `(modifier, subscript)` match that fails to compile; for `subscript: None` it should call the scalar evaluator.

- [ ] **Step 5: generate.rs.** Add:

```rust
        ParamModifier::PrefixNames { at } =>
            format!("${{!{name}{}}}", if *at { "@" } else { "*" }),
```

- [ ] **Step 6: Tests.** Lexer unit (parses to `PrefixNames { at }`) + engine integration in a new `tests/param_expansion_prefix_integration.rs`:

```rust
// (same run_file harness as Task 2's integration file)
#[test]
fn prefix_names_star_lists_sorted() {
    let (o, _e, c) = run_file("_Qa=1\n_Qb=2\necho ${!_Q*}\n");
    assert_eq!(c, 0);
    assert_eq!(o, "_Qa _Qb\n");
}
#[test]
fn prefix_names_at_iterates() {
    let (o, _e, _c) = run_file("_Qa=1\n_Qb=2\nfor k in ${!_Q@}; do echo $k; done\n");
    assert_eq!(o, "_Qa\n_Qb\n");
}
#[test]
fn prefix_names_no_match_empty() {
    let (o, _e, c) = run_file("echo \"[${!NOSUCHPREFIX_XYZ*}]\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "[]\n");
}
```

- [ ] **Step 7: Run + commit**

Run: `cargo test -p huck-syntax --lib && cargo test --test param_expansion_prefix_integration && cargo build --workspace`

```bash
git add -A
git commit -m "v233 M1: \${!prefix*} / \${!prefix@} prefix-name expansion

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: M2 — special parameters as the name in `${#…}` and `${!…}`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `scan_braced_param_expansion`: accept a single special-parameter char (`# @ * $ ! ? -` and digit runs) as the name in the Length form (`${#<sp>}`) and the indirect form (`${!<sp>}`).

**Interfaces:** none new — reuses `ParamModifier::Length` and `indirect: true` with the special-parameter `name`.

Measured: `${##}` → length of `$#`; `${!#}` → indirect of `$#`. The engine already resolves special-parameter names via `lookup_var` (Length arm at ~110 special-cases `@`/`*`; indirect resolution uses the name's value), so the change is **lexer-only** — let the special char through as `name`.

- [ ] **Step 1: Failing tests** in `lexer.rs` `mod tests`:

```rust
    #[test]
    fn length_of_special_param_hash() {
        // ${##} = ${#<#>} = length of $#
        let toks = tokenize("${##}").unwrap();
        let part = single_param_expansion(&mut toks.clone());
        assert!(matches!(part,
            WordPart::ParamExpansion { ref name, modifier: ParamModifier::Length, .. } if name == "#"));
    }

    #[test]
    fn indirect_of_special_param_hash() {
        // ${!#} = indirect through $#
        let toks = tokenize("${!#}").unwrap();
        let part = single_param_expansion(&mut toks.clone());
        assert!(matches!(part,
            WordPart::ParamExpansion { ref name, indirect: true, .. } if name == "#"));
    }
```

- [ ] **Step 2: Run, verify fail** (`cargo test -p huck-syntax --lib special_param`) — currently `EmptyParamName`.

- [ ] **Step 3: Implement.** Add a helper near `scan_braced_name`:

```rust
/// The special single-char parameter names valid as the operand of the
/// length (`${#X}`) and indirect (`${!X}`) forms.
fn special_param_char(c: char) -> bool {
    matches!(c, '#' | '@' | '*' | '$' | '!' | '?' | '-')
}
```

In `scan_braced_param_expansion`:
- **Length form** (`${#…}`, ~2855): extend the `name` match so a special-param char is taken as the name. Add before the `_ => scan_braced_name(chars)?` arm:

```rust
            Some(c) if super::special_param_char(c) => { chars.next(); c.to_string() }
```

(use the local function name as in-scope; drop `super::` if defined in the same module). So `${##}` reads name `"#"` and emits `Length`.

- **Indirect form** (`${!…}`, after the digit branch ~2937, before `scan_braced_name`): add:

```rust
        if matches!(chars.peek().copied(), Some(c) if special_param_char(c)) {
            let c = chars.next().unwrap();
            return dispatch_braced_modifier(c.to_string(), quoted, None, chars, parts, /* indirect */ true, dollar_start, opts);
        }
```

(thread `dollar_start` per Task 2's signature change.) So `${!#}` → indirect of `$#`.

- [ ] **Step 4: Engine sanity.** Confirm the Length arm computes the length of the special param's value, and indirect resolves `$#`'s value as a name. If `${##}` (length of `$#`) needs the special-param value rather than positional count, verify the `Length` arm (~110): for `name == "#"` with `ParamLookup::Scalar`, `lookup_v` returns `$#`'s string; `.chars().count()` gives its length. Add a targeted engine integration assertion (Step 5) and adjust only if it diverges from bash.

- [ ] **Step 5: Integration tests** in a new `tests/param_expansion_special_integration.rs`:

```rust
// (same run_file harness)
#[test]
fn length_of_arg_count() {
    let (o, _e, c) = run_file("set -- a b c\necho ${##}\n"); // len("3") = 1
    assert_eq!(c, 0);
    assert_eq!(o, "1\n");
}
#[test]
fn indirect_through_arg_count() {
    let (o, _e, c) = run_file("set -- a b c\necho ${!#}\n"); // $# = 3 -> $3 = c
    assert_eq!(c, 0);
    assert_eq!(o, "c\n");
}
```

- [ ] **Step 6: Run + commit**

Run: `cargo test -p huck-syntax --lib && cargo test --test param_expansion_special_integration && cargo build --workspace`

```bash
git add -A
git commit -m "v233 M2: special parameters as the name in \${#X} and \${!X}

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Diff harness, re-parse gate, and category measurement

**Files:**
- Create: `tests/scripts/param_expansion_diff_check.sh`

**Interfaces:** uses `$HUCK_BIN` (default `target/debug/huck`), mirroring `tests/scripts/alias_case_diff_check.sh`.

- [ ] **Step 1: Write the harness** at `tests/scripts/param_expansion_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v233: ${...} parse robustness
# (M1 prefix, M2 special-param names, M3 @-edges, M4 $'...', bad-subst defer).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-pe.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# M1 prefix-name expansion
checkf "prefix star"      '_Qa=1; _Qb=2; echo ${!_Q*}'
checkf "prefix at loop"   '_Qa=1; _Qb=2; for k in ${!_Q@}; do echo $k; done'
checkf "prefix no match"  'echo "[${!NOSUCHPFX_ZZ*}]"'
# M2 special-param names
checkf "len of argc"      'set -- a b c; echo ${##}'
checkf "indirect argc"    'set -- a b c; echo ${!#}'
# M4 $'...' in pattern
checkf "ansi-c pattern"   "x=aXb; printf '<%s>\\n' \"\${x#\$'a\\t\\'\\tb'}\""
checkf "ansi-c strip"     "x=foo; echo \"\${x#\$'f'}\""
# bad-subst defer (M2/M3) — must MATCH bash's runtime error + continuation
checkf "bad dollar name"  'echo before; echo ${$x}; echo after'
checkf "bad empty xform"  'V=42; echo ${V@}; echo after'
checkf "bad dash digit"   'echo "[${-3}]"; echo after'
checkf "short-circuit"    '[[ -n yes || -z ${H*} ]]; echo rc=$?'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: chmod + build + run**

Run: `chmod +x tests/scripts/param_expansion_diff_check.sh && cargo build -p huck && ./tests/scripts/param_expansion_diff_check.sh`
Expected: `Total: 11, Pass: 11, Fail: 0`. If a bad-subst case FAILs only on the error-message *prologue* (line number / `huck:` vs `bash:`), record it as a known prologue divergence (consistent with the staged error-prologue rollout) rather than weakening the test — but the `${…}: bad substitution` tail and the exit/continuation MUST match. Investigate any other FAIL as a real bug.

- [ ] **Step 3: Re-parse gate** — confirm all nine real suite files no longer parse-abort:

Run:
```bash
for f in new-exp.tests new-exp3.sub new-exp10.sub new-exp13.sub nameref20.sub varenv13.sub exp.tests errors2.sub errors6.sub posixexp7.sub cond.tests; do
  e=$(/home/john/projects/huck/target/release/huck -n "/tmp/bash-5.2.21/tests/$f" 2>&1 >/dev/null)
  [ -n "$e" ] && echo "STILL FAILS: $f -> $(echo "$e" | head -1)"
done; echo "re-parse gate done"
```
(Build release first: `cargo build --release -p huck`.) Expected: only `cond.tests`/`exp.tests`/`new-exp.tests` may still report a *different* later-line gap unrelated to `${…}` — record any residual; the M1–M4 lines must be gone. If a file still fails on the targeted construct, it's a bug in Tasks 1–4.

- [ ] **Step 4: Full sweep**

Run: `cargo test --workspace`
Expected: 0 failures.

- [ ] **Step 5: Commit the harness**

```bash
git add tests/scripts/param_expansion_diff_check.sh
git commit -m "v233: param_expansion_diff_check harness + re-parse gate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Measure (controller does this post-review, not the implementer).** Re-run the affected bash-test categories and record flip/shrink honestly:

```bash
for cat in new-exp posixexp posixexp2 errors cond varenv nameref exp-tests; do
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$cat bash tests/bash-test-suite/runner.sh 2>&1 | grep -E "PASS:|FAIL:|\| $cat \|"
done
```

---

## Post-implementation (controller, after final review)

- Update `docs/bash-divergences.md`: note v233 closed the four `${…}` parse gaps and added the defer-to-runtime model; add a deferred `L-*` for any residual (the two surrounding-context "unterminated quote" files array6/nquote2; the M3-combo `${!arr[@]@OP}` text divergence — huck says "bad substitution", bash says "invalid variable name"; any non-`${…}` later-line parse gaps surfaced by the re-parse gate).
- Record v233 in `MEMORY.md` + `project_huck_iterations.md`.

## Self-Review

- **Spec coverage:** M1 → Task 3 (`PrefixNames`). M2 → Task 4 (special-param names) + Task 2 (invalid `${-3}`/`${$x}` → BadSubst). M3 → Task 2 (`${V@}` + `${!arr[@]@OP}` → BadSubst). M4 → Task 1. Defer-to-runtime model → Task 2. Testing → Tasks 1–5 (unit + 3 integration files + diff harness + re-parse gate + measure). All spec sections covered.
- **Placeholder scan:** the engine-side helper names (`var_names_with_prefix`, `ifs_first_char`) are flagged as "locate or inline" with the concrete fallback (iterate the variable map; read `$IFS`) — not left as TODO. All code steps carry real code.
- **Type consistency:** `ParamModifier::BadSubst { raw: String }` and `PrefixNames { at: bool }` are used identically across lexer construction, engine match, and generate reconstruction; `dollar_start: usize` is threaded through `scan_braced_param_expansion` and `dispatch_braced_modifier` consistently; `recover_bad_subst` signature matches its call sites.
- **Risk note for the executor:** Task 2's `dollar_start` threading and the `#[non_exhaustive]` match-arm propagation are the most error-prone; rely on `cargo build --workspace` to surface every missed arm before moving on.
