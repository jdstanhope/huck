# v198: `set -x` xtrace for compound commands — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Under `set -x`, emit a `$PS4`-prefixed trace line for each compound-command header — `[[ ]]` (leaf-by-leaf, expanded, short-circuit), `(( ))`, `case`, `for-in` (per iteration), `select`, and C-style `for ((;;))` — matching bash. `while`/`until`/`if` need no own line (their condition commands already trace).

**Architecture:** A pure `reconstruct_word_source(&Word)` renders a parsed `Word` back to UNEXPANDED source text (for the raw-header forms). A thin `xtrace_compound(shell, body)` reuses the existing `ps4`/`xtrace_emit` path. One emit call is added per compound `run_*`; the `[[ ]]` case threads a `suppress` flag through `eval_test_expr` and renders each evaluated leaf with EXPANDED operands. No lexer/parser/AST changes.

**Tech Stack:** Rust. Files: `src/expand.rs` (reconstructor), `src/executor.rs` (emit sites + `[[ ]]` hook), `tests/scripts/xtrace_compound_diff_check.sh` (harness).

**Spec:** `docs/superpowers/specs/2026-06-20-xtrace-compound-design.md`

**Branch:** `v198-xtrace-compound`

**Background the implementer needs:**
- Existing xtrace plumbing in `src/executor.rs`: `ps4(shell) -> String` (renders the `$PS4` prefix with the right depth), `xtrace_emit(line: &str)` (single `write(2)` to fd 2, adds the `\n`). Simple commands trace via `if shell.shell_options.xtrace { … xtrace_emit(&format!("{p4}{body}")) }`.
- `expand_assignment(word: &Word, shell: &mut Shell) -> String` (`src/expand.rs`) expands a Word to one string (no splitting). Used for `[[ ]]` operands (which bash shows EXPANDED).
- `render_tilde_literal(spec: &TildeSpec) -> String` exists in `src/expand.rs`.
- `WordPart` variants: `Literal{text,quoted}`, `Var{name,quoted}`, `LastStatus{quoted}`, `CommandSub{sequence,quoted}`, `ProcessSub{sequence,dir}`, `Arith{body,quoted}`, `ParamExpansion{name,modifier,quoted,subscript,indirect}`, `AllArgs{quoted,joined}`, `Tilde(TildeSpec)`, `AssignPrefix{..}`, `ArrayLiteral(..)`.
- `TestExpr` (`src/command.rs`): `Unary{op,operand}`, `Binary{op,lhs,rhs}`, `Regex{lhs,pattern}`, `Not(Box)`, `And(Box,Box)`, `Or(Box,Box)`.
- `eval_test_expr(expr, shell) -> Result<bool,String>` (`src/executor.rs:1753`) is the only caller-visible test evaluator; `run_double_bracket` (1729) calls it.
- Run-fns: `run_for_inner` (1225), `run_case` (1646), `run_arith` (1378), `run_arith_for_inner` (1404), `run_select_inner` (1498).

---

## Task 1: `reconstruct_word_source` — Word → raw source text

**Files:**
- Modify: `src/expand.rs` (add the reconstructor + helpers near `reconstruct_array_literal` ~line 1110).
- Test: `src/expand.rs` `mod tests`.

- [ ] **Step 1: Write failing unit tests.** Add to `src/expand.rs` `mod tests`:

```rust
    #[test]
    fn reconstruct_source_scalars() {
        use crate::lexer::tokenize;
        // Helper: lex a single word's source and reconstruct it.
        fn rt(src: &str) -> String {
            let toks = tokenize(src).expect("lex");
            let w = toks.iter().find_map(|t| match t {
                crate::lexer::Token::Word(w) => Some(w.clone()),
                _ => None,
            }).expect("a Word token");
            reconstruct_word_source(&w)
        }
        assert_eq!(rt("abc"), "abc");
        assert_eq!(rt("$xs"), "$xs");
        assert_eq!(rt("a$x.b"), "a$x.b");
        assert_eq!(rt("${x}"), "${x}");
        assert_eq!(rt("${x:-d}"), "${x:-d}");
        assert_eq!(rt("${x##*/}"), "${x##*/}");
        assert_eq!(rt("${arr[@]}"), "${arr[@]}");
        assert_eq!(rt("${#x}"), "${#x}");
        assert_eq!(rt("$((1+2))"), "$((1+2))");
        assert_eq!(rt("$(ls -l)"), "$(ls -l)");
    }
```

- [ ] **Step 2: Run to confirm it FAILS.**

Run: `cargo test --lib reconstruct_source_scalars 2>&1 | tail -6`
Expected: FAIL — `reconstruct_word_source` is not defined.

- [ ] **Step 3: Implement the reconstructor.** Add to `src/expand.rs` (after `reconstruct_array_literal`):

```rust
/// Re-render a parsed `Word` back to its (approximate) SOURCE text, UNEXPANDED.
/// Used for `set -x` traces of compound-command headers (case/for/select/arith),
/// which bash shows as the raw source word, not the expanded value. Pure — no
/// `Shell`, no expansion. Quote *style* is not recoverable (`'x'`/`"x"`/`x` all
/// render as `x`); deeply-nested command substitutions render their inner
/// command best-effort (single pipeline of simple commands; see
/// `reconstruct_sequence_source`).
pub(crate) fn reconstruct_word_source(word: &Word) -> String {
    let mut out = String::new();
    for part in &word.0 {
        reconstruct_part(part, &mut out);
    }
    out
}

fn reconstruct_part(part: &WordPart, out: &mut String) {
    use crate::lexer::{ProcDir, WordPart as P};
    match part {
        P::Literal { text, .. } => out.push_str(text),
        P::Var { name, .. } => {
            out.push('$');
            out.push_str(name);
        }
        P::LastStatus { .. } => out.push_str("$?"),
        P::AllArgs { joined, .. } => out.push_str(if *joined { "$*" } else { "$@" }),
        P::Arith { body, .. } => {
            out.push_str("$((");
            out.push_str(&reconstruct_word_source(body));
            out.push_str("))");
        }
        P::Tilde(spec) => out.push_str(&render_tilde_literal(spec)),
        P::CommandSub { sequence, .. } => {
            out.push_str("$(");
            out.push_str(&reconstruct_sequence_source(sequence));
            out.push(')');
        }
        P::ProcessSub { sequence, dir } => {
            out.push_str(match dir { ProcDir::In => "<(", ProcDir::Out => ">(" });
            out.push_str(&reconstruct_sequence_source(sequence));
            out.push(')');
        }
        P::ParamExpansion { name, modifier, subscript, indirect, .. } => {
            reconstruct_param_expansion(name, modifier, subscript.as_ref(), *indirect, out);
        }
        // Not reachable inside a compound-command header word; render nothing.
        P::AssignPrefix { .. } | P::ArrayLiteral(_) => {}
    }
}

fn reconstruct_param_expansion(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    subscript: Option<&crate::lexer::SubscriptKind>,
    indirect: bool,
    out: &mut String,
) {
    use crate::lexer::{ParamModifier as M, SubstAnchor, CaseDirection, SubscriptKind as S, TransformOp};
    out.push_str("${");
    if indirect || matches!(modifier, M::IndirectKeys) {
        out.push('!');
    }
    if matches!(modifier, M::Length) {
        out.push('#');
    }
    out.push_str(name);
    match subscript {
        None => {}
        Some(S::All) => out.push_str("[@]"),
        Some(S::Star) => out.push_str("[*]"),
        Some(S::Index(w)) => {
            out.push('[');
            out.push_str(&reconstruct_word_source(w));
            out.push(']');
        }
    }
    match modifier {
        M::None | M::Length | M::IndirectKeys => {}
        M::UseDefault { word, colon } => {
            out.push_str(if *colon { ":-" } else { "-" });
            out.push_str(&reconstruct_word_source(word));
        }
        M::AssignDefault { word, colon } => {
            out.push_str(if *colon { ":=" } else { "=" });
            out.push_str(&reconstruct_word_source(word));
        }
        M::ErrorIfUnset { word, colon } => {
            out.push_str(if *colon { ":?" } else { "?" });
            out.push_str(&reconstruct_word_source(word));
        }
        M::UseAlternate { word, colon } => {
            out.push_str(if *colon { ":+" } else { "+" });
            out.push_str(&reconstruct_word_source(word));
        }
        M::RemovePrefix { pattern, longest } => {
            out.push_str(if *longest { "##" } else { "#" });
            out.push_str(&reconstruct_word_source(pattern));
        }
        M::RemoveSuffix { pattern, longest } => {
            out.push_str(if *longest { "%%" } else { "%" });
            out.push_str(&reconstruct_word_source(pattern));
        }
        M::Substitute { pattern, replacement, anchor, all } => {
            out.push('/');
            if *all { out.push('/'); }
            match anchor {
                SubstAnchor::None => {}
                SubstAnchor::Prefix => out.push('#'),
                SubstAnchor::Suffix => out.push('%'),
            }
            out.push_str(&reconstruct_word_source(pattern));
            out.push('/');
            out.push_str(&reconstruct_word_source(replacement));
        }
        M::Substring { offset, length } => {
            out.push(':');
            out.push_str(&reconstruct_word_source(offset));
            if let Some(len) = length {
                out.push(':');
                out.push_str(&reconstruct_word_source(len));
            }
        }
        M::Case { direction, all, pattern } => {
            let c = match direction { CaseDirection::Upper => '^', CaseDirection::Lower => ',' };
            out.push(c);
            if *all { out.push(c); }
            if let Some(p) = pattern {
                out.push_str(&reconstruct_word_source(p));
            }
        }
        M::Transform { op } => {
            out.push('@');
            out.push(match op {
                TransformOp::PromptExpand => 'P',
                TransformOp::Quote => 'Q',
                TransformOp::Upper => 'U',
                TransformOp::Lower => 'L',
                TransformOp::UpperFirst => 'u',
                TransformOp::EscapeExpand => 'E',
            });
        }
    }
    out.push('}');
}

/// Best-effort source for a `$(…)` / `<(…)` body: renders a single pipeline of
/// simple commands (`cmd a | cmd b`). Compound/multi-connector bodies render
/// their first command only (documented approximation — rare in a trace header).
fn reconstruct_sequence_source(seq: &crate::command::Sequence) -> String {
    let mut s = reconstruct_command_source(&seq.first);
    for (_, cmd) in &seq.rest {
        s.push_str("; ");
        s.push_str(&reconstruct_command_source(cmd));
    }
    s
}

fn reconstruct_command_source(cmd: &crate::command::Command) -> String {
    use crate::command::{Command, SimpleCommand};
    match cmd {
        Command::Simple(SimpleCommand::Exec(e)) => {
            let mut parts = vec![reconstruct_word_source(&e.program)];
            parts.extend(e.args.iter().map(reconstruct_word_source));
            parts.join(" ")
        }
        Command::Pipeline(p) => p
            .commands
            .iter()
            .map(reconstruct_command_source)
            .collect::<Vec<_>>()
            .join(" | "),
        // Other command kinds inside a header `$()` are rare; approximate empty.
        _ => String::new(),
    }
}
```

- [ ] **Step 4: Run to confirm PASS.**

Run: `cargo test --lib reconstruct_source_scalars 2>&1 | tail -6`
Expected: PASS. If `${arr[@]}` or `$(ls -l)` differ, check `reconstruct_param_expansion` / `reconstruct_command_source`.

- [ ] **Step 5: clippy + commit.**

Run: `cargo clippy --lib 2>&1 | tail -3` (expect clean).

```bash
git add src/expand.rs
git commit -m "$(cat <<'EOF'
v198 task 1: reconstruct_word_source — Word to raw source text

Pure AST->source renderer (unexpanded) for set -x compound-header traces:
scalars ($name/$@/$?/$((..))), ${..} with subscripts + all modifiers, and
$(..) via a simple-pipeline pretty-printer. Quote style + deeply-nested
compound-in-$() are documented approximations.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `xtrace_compound` + raw-header emit sites (case / for / select / `(( ))` / C-for)

**Files:**
- Modify: `src/executor.rs` — add `xtrace_compound`; add emit calls in `run_case`, `run_for_inner`, `run_select_inner`, `run_arith`, `run_arith_for_inner`.
- Create: `tests/scripts/xtrace_compound_diff_check.sh`.

- [ ] **Step 1: Add the helper.** In `src/executor.rs`, just after `xtrace_command_line` (~line 3012):

```rust
/// Emit one xtrace line for a compound-command header (gated on `set -x`).
/// Reuses the simple-command `ps4`/`xtrace_emit` path so depth/PS4/single-write
/// behavior is identical.
fn xtrace_compound(shell: &mut Shell, body: &str) {
    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        xtrace_emit(&format!("{p4}{body}"));
    }
}
```

- [ ] **Step 2: `case` emit.** In `run_case` (`src/executor.rs:1646`), right after `let subject = expand_assignment(&clause.subject, shell);`:

```rust
    xtrace_compound(
        shell,
        &format!("case {} in", crate::expand::reconstruct_word_source(&clause.subject)),
    );
```

- [ ] **Step 3: `for-in` emit (per iteration).** In `run_for_inner`, inside the `for value in values {` loop, as the FIRST statement of the loop body (before the `check_interrupt`):

```rust
        if shell.shell_options.xtrace {
            let words = clause
                .words
                .iter()
                .map(crate::expand::reconstruct_word_source)
                .collect::<Vec<_>>()
                .join(" ");
            let body = if clause.has_in {
                format!("for {} in {}", clause.var, words)
            } else {
                format!("for {}", clause.var)
            };
            xtrace_compound(shell, &body);
        }
```

Note: the `xtrace` guard is duplicated here only to avoid building `words` when tracing is off; `xtrace_compound` re-checks. (bash shows `for V in WORDS` per iteration; the no-`in` form is `for V`.)

- [ ] **Step 4: `select` emit (once).** In `run_select_inner` (`src/executor.rs:1498`), immediately after the `let items: Vec<String> = …;` block (before the empty-list early return), add:

```rust
    if shell.shell_options.xtrace {
        let body = match &clause.words {
            Some(words) => format!(
                "select {} in {}",
                clause.var,
                words.iter().map(crate::expand::reconstruct_word_source)
                    .collect::<Vec<_>>().join(" ")
            ),
            None => format!("select {}", clause.var),
        };
        xtrace_compound(shell, &body);
    }
```

- [ ] **Step 5: standalone `(( ))` emit.** In `run_arith` (`src/executor.rs:1378`), as the first line of the function body:

```rust
    xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source(body)));
```

- [ ] **Step 6: C-style `for ((;;))` emits.** In `run_arith_for_inner`:
  - Before evaluating `init` (just inside the function, before the `if let Some(init)` block):
    ```rust
    if let Some(init) = &clause.init {
        xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source(init)));
    }
    ```
    (then the existing `if let Some(init) = &clause.init { … eval … }` runs the init.)
  - Inside the `loop {`, before evaluating `cond` (before the `let cond_value = …`):
    ```rust
    if let Some(c) = &clause.cond {
        xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source(c)));
    }
    ```
  - After the body match, before evaluating `step` (before the `if let Some(step) = &clause.step` eval block):
    ```rust
    if let Some(step) = &clause.step {
        xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source(step)));
    }
    ```

- [ ] **Step 7: Build.** Run: `cargo build 2>&1 | tail -3` (expect Finished, no errors).

- [ ] **Step 8: Create the harness** `tests/scripts/xtrace_compound_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v198: set -x traces of compound commands.
# Combined stderr (where the trace goes); $PS4 default `+ ` is identical in both,
# so no normalization is needed for these cases.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK_BIN" -c "$frag" 2>&1)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# --- case ---
check "case match"     'set -x; x=hi; case "$x" in hi) :;; esac'
check "case no-match"  'set -x; case z in a) :;; esac'
check "case modifier"  'set -x; p=a/b/c; case ${p##*/} in c) :;; esac'
# --- for-in (per iteration) ---
check "for literal"    'set -x; for i in a b c; do :; done'
check "for raw var"    'set -x; xs="a b"; for i in $xs; do :; done'
check "for quoted"     'set -x; for i in a "b c"; do :; done'
check "for empty list" 'set -x; for i in; do :; done; echo done'
check "for cmdsub"     'set -x; for i in $(echo p q); do :; done'  # unquoted args: quote-style residual avoided
# --- select ---
check "select"         'set -x; select x in a b; do break; done <<< 1'
# --- standalone (( )) ---
check "arith simple"   'set -x; ((1+1))'
check "arith spaces"   'set -x; (( v=3, v+1 ))'
# --- C-style for ---
check "c-for"          'set -x; for ((i=0;i<2;i++)); do :; done'
# --- while/if regression (no own header; condition traces) ---
check "while cond"     'set -x; n=0; while (( n < 2 )); do (( n++ )); done'
check "if cond"        'set -x; if [[ 1 == 1 ]]; then :; fi'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

NOTE: the `if`/`while` cases also exercise `[[ ]]`/`(( ))` as conditions — those leaf traces are added in Task 3, so those two cases (and any other `[[ ]]`-containing case) are expected to still FAIL after Task 2 and PASS after Task 3.

- [ ] **Step 9: Run the harness (partial PASS expected).**

Run: `chmod +x tests/scripts/xtrace_compound_diff_check.sh && bash tests/scripts/xtrace_compound_diff_check.sh`
Expected: the `case`/`for`/`select`/`arith`/`c-for` cases PASS; `while cond` / `if cond` FAIL (they need the Task 3 `[[ ]]`/`(( ))`-as-condition leaf traces — `(( n < 2 ))` as a while-condition is a `Command::Arith` and DOES trace now, but `if [[ 1 == 1 ]]` needs Task 3). Confirm at least the 12 non-`[[ ]]` cases pass.

- [ ] **Step 10: Commit.**

```bash
git add src/executor.rs tests/scripts/xtrace_compound_diff_check.sh
git commit -m "$(cat <<'EOF'
v198 task 2: xtrace for case/for/select/(( ))/C-for headers

xtrace_compound() reuses the ps4/xtrace_emit path; one emit per run_* for the
raw-header forms (case at entry, for-in per iteration, select once, standalone
and C-style arith clauses). Headers rendered via reconstruct_word_source.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `[[ ]]` leaf hook — per-leaf expanded traces with short-circuit

**Files:**
- Modify: `src/executor.rs` — op→string maps, `render_test_leaf`, thread a `suppress` flag through `eval_test_expr`.
- Test: `src/executor.rs` `mod tests` + the harness `while`/`if` cases now pass.

- [ ] **Step 1: Write failing unit test.** Add to `src/executor.rs` `mod tests`:

```rust
    #[test]
    fn render_test_leaf_forms() {
        let mut shell = Shell::new();
        shell.set("v", "hi".into());
        // Binary with expanded lhs, raw-ish rhs pattern.
        let parse_expr = |src: &str| {
            let toks = crate::lexer::tokenize(src).expect("lex");
            match crate::command::parse(toks).expect("parse").expect("seq").first {
                crate::command::Command::DoubleBracket { expr, .. } => *expr,
                other => panic!("expected [[ ]], got {other:?}"),
            }
        };
        assert_eq!(render_test_leaf(&parse_expr("[[ -n $v ]]"), &mut shell), "-n hi");
        assert_eq!(render_test_leaf(&parse_expr("[[ -z \"\" ]]"), &mut shell), "-z ''");
        assert_eq!(render_test_leaf(&parse_expr("[[ $v == h* ]]"), &mut shell), "hi == h*");
        assert_eq!(render_test_leaf(&parse_expr("[[ 5 -gt 3 ]]"), &mut shell), "5 -gt 3");
    }
```

- [ ] **Step 2: Run to confirm it FAILS.**

Run: `cargo test --lib render_test_leaf_forms 2>&1 | tail -6`
Expected: FAIL — `render_test_leaf` not defined.

- [ ] **Step 3: Add op→string maps + `render_test_leaf`.** In `src/executor.rs`, near `eval_test_expr`:

```rust
fn test_unary_op_str(op: crate::command::TestUnaryOp) -> &'static str {
    use crate::command::TestUnaryOp as U;
    match op {
        U::FileExists => "-e", U::IsRegFile => "-f", U::IsDir => "-d",
        U::IsReadable => "-r", U::IsWritable => "-w", U::IsExecutable => "-x",
        U::IsNonEmpty => "-s", U::IsSymlink => "-L", U::StringNonEmpty => "-n",
        U::StringEmpty => "-z", U::VarSet => "-v", U::OptEnabled => "-o",
        U::IsFifo => "-p", U::IsSocket => "-S", U::IsBlockDev => "-b",
        U::IsCharDev => "-c", U::OwnedByEuid => "-O", U::OwnedByEgid => "-G",
        U::NewerThanRead => "-N", U::IsSticky => "-k", U::IsSetuid => "-u",
        U::IsSetgid => "-g", U::IsTerminal => "-t",
    }
}

fn test_binary_op_str(op: crate::command::TestBinaryOp) -> &'static str {
    use crate::command::TestBinaryOp as B;
    match op {
        B::StringEq => "==", B::StringNe => "!=", B::StringLt => "<", B::StringGt => ">",
        B::IntEq => "-eq", B::IntNe => "-ne", B::IntLt => "-lt", B::IntGt => "-gt",
        B::IntLe => "-le", B::IntGe => "-ge", B::NewerThan => "-nt",
        B::OlderThan => "-ot", B::SameFile => "-ef",
    }
}

/// bash shows an empty `[[ ]]` operand as `''` and a non-empty one raw.
fn xtrace_operand(s: &str) -> String {
    if s.is_empty() { "''".to_string() } else { s.to_string() }
}

/// Render the `[[ … ]]` body for a single leaf (operands EXPANDED), for `set -x`.
/// Matches bash for the common cases; an exotic mixed-quote rhs *pattern* may
/// differ (documented L-21(a) residual).
fn render_test_leaf(expr: &TestExpr, shell: &mut Shell) -> String {
    match expr {
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            format!("{} {}", test_unary_op_str(*op), xtrace_operand(&s))
        }
        TestExpr::Binary { op, lhs, rhs } => {
            let l = expand_assignment(lhs, shell);
            let r = expand_assignment(rhs, shell);
            format!("{} {} {}", xtrace_operand(&l), test_binary_op_str(*op), xtrace_operand(&r))
        }
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            let p = expand_assignment(pattern, shell);
            format!("{} =~ {}", xtrace_operand(&l), xtrace_operand(&p))
        }
        // Non-leaf — not called directly on these (see eval_test_expr_traced).
        TestExpr::Not(_) | TestExpr::And(_, _) | TestExpr::Or(_, _) => String::new(),
    }
}
```

- [ ] **Step 4: Run to confirm the unit test PASSES.**

Run: `cargo test --lib render_test_leaf_forms 2>&1 | tail -6`
Expected: PASS. (Note: `expand_assignment` re-expands `rhs`/`pattern`; for a `Binary` that is harmless because `eval_binary` does its own expansion — see Step 5's caveat.)

- [ ] **Step 5: Thread `suppress` through `eval_test_expr` and emit.** Replace `eval_test_expr` (`src/executor.rs:1753`) with a thin wrapper + a traced inner. Keep ALL existing evaluation logic identical; only ADD the trace emits:

```rust
fn eval_test_expr(expr: &TestExpr, shell: &mut Shell) -> Result<bool, String> {
    eval_test_expr_traced(expr, shell, false)
}

/// `suppress` = the caller (a `Not` of a leaf) already emitted this leaf's trace,
/// so the leaf must not emit again.
fn eval_test_expr_traced(expr: &TestExpr, shell: &mut Shell, suppress: bool) -> Result<bool, String> {
    // Emit `+ [[ <leaf> ]]` before evaluating a leaf (operands expanded once for
    // the trace; the evaluator below re-expands, which is side-effect-equivalent
    // for these read-only operands). Short-circuit falls out of And/Or recursion.
    if !suppress
        && shell.shell_options.xtrace
        && matches!(expr, TestExpr::Unary { .. } | TestExpr::Binary { .. } | TestExpr::Regex { .. })
    {
        let body = render_test_leaf(expr, shell);
        let p4 = ps4(shell);
        xtrace_emit(&format!("{p4}[[ {body} ]]"));
    }
    match expr {
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            if matches!(op, TestUnaryOp::VarSet) {
                return Ok(shell.element_or_var_is_set(&s));
            }
            if matches!(op, TestUnaryOp::OptEnabled) {
                return Ok(crate::builtins::option_get(shell, &s).unwrap_or(false));
            }
            Ok(eval_unary(*op, &s))
        }
        TestExpr::Binary { op, lhs, rhs } => {
            let l = expand_assignment(lhs, shell);
            eval_binary(*op, &l, rhs, shell)
        }
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            let p = expand_assignment(pattern, shell);
            let p = if shell.nocasematch() { format!("(?i){p}") } else { p };
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            match re.captures(&l) {
                Some(caps) => {
                    let map: std::collections::BTreeMap<usize, String> = (0..caps.len())
                        .map(|i| {
                            (i, caps.get(i).map(|m| m.as_str().to_string()).unwrap_or_default())
                        })
                        .collect();
                    let _ = shell.replace_indexed("BASH_REMATCH", map);
                    Ok(true)
                }
                None => {
                    let _ = shell.replace_indexed("BASH_REMATCH", std::collections::BTreeMap::new());
                    Ok(false)
                }
            }
        }
        TestExpr::Not(inner) => {
            // bash folds `! <leaf>` into one line; emit the combined form and
            // suppress the inner leaf's own emit. A non-leaf inner recurses
            // normally (its leaves trace individually — documented edge).
            if !suppress
                && shell.shell_options.xtrace
                && matches!(**inner, TestExpr::Unary { .. } | TestExpr::Binary { .. } | TestExpr::Regex { .. })
            {
                let body = render_test_leaf(inner, shell);
                let p4 = ps4(shell);
                xtrace_emit(&format!("{p4}[[ ! {body} ]]"));
                return eval_test_expr_traced(inner, shell, true).map(|b| !b);
            }
            eval_test_expr_traced(inner, shell, suppress).map(|b| !b)
        }
        TestExpr::And(a, b) => {
            if eval_test_expr_traced(a, shell, false)? {
                eval_test_expr_traced(b, shell, false)
            } else {
                Ok(false)
            }
        }
        TestExpr::Or(a, b) => {
            if eval_test_expr_traced(a, shell, false)? {
                Ok(true)
            } else {
                eval_test_expr_traced(b, shell, false)
            }
        }
    }
}
```

- [ ] **Step 6: Build + run the full harness.**

Run:
```bash
cargo build 2>&1 | tail -1
bash tests/scripts/xtrace_compound_diff_check.sh
```
Expected: ALL cases PASS now (`if cond` / `while cond` included). If a `[[ ]]` case differs, inspect the diff — the likely culprits are the `''` empty-operand rule or an op-string mismatch.

- [ ] **Step 7: Add `[[ ]]` harness cases.** Append to `tests/scripts/xtrace_compound_diff_check.sh` (before the `echo ""` summary):

```bash
# --- [[ ]] leaf-by-leaf ---
check "dbracket single"   'set -x; v=5; [[ $v -gt 3 ]]'
check "dbracket regex"    'set -x; [[ "" =~ ^[0-9]+$ ]]'
check "dbracket and"      'set -x; a=1;b=2; [[ $a == 1 && $b == 2 ]]'
check "dbracket or-short" 'set -x; a=1;b=2; [[ $a == 1 || $b == 9 ]]'
check "dbracket and-fail" 'set -x; a=1;b=2; [[ $a == 9 && $b == 2 ]]'
check "dbracket not"      'set -x; [[ ! -e /nonesuch ]]'
check "dbracket parens"   'set -x; [[ ( 1 == 1 ) ]]'
check "dbracket glob"     'set -x; [[ hi == h* ]]'
```

- [ ] **Step 8: Run the harness (full PASS).**

Run: `bash tests/scripts/xtrace_compound_diff_check.sh | tail -3`
Expected: `Fail: 0`. Known residuals (do NOT add as asserting cases): a quoted-pattern rhs like `[[ $x == "p q" ]]` (bash `\p\ \q`), and `=` rendered as `==`. If you want to confirm a residual, add it as a COMMENT, not a `check`.

- [ ] **Step 9: clippy + commit.**

Run: `cargo clippy --all-targets 2>&1 | tail -3` (expect clean).

```bash
git add src/executor.rs tests/scripts/xtrace_compound_diff_check.sh
git commit -m "$(cat <<'EOF'
v198 task 3: [[ ]] leaf-by-leaf xtrace with short-circuit

eval_test_expr threads a `suppress` flag and emits `+ [[ <leaf> ]]` per evaluated
Unary/Binary/Regex leaf with EXPANDED operands; And/Or short-circuit drops the
untaken branch's line; `! <leaf>` folds into one line. Op->string maps +
render_test_leaf. Empty operand -> ''.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Full verification, docs, memory

- [ ] **Step 1: Full suite + harnesses + clippy.**

Run:
```bash
cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -iE "Fail: [1-9]|[1-9] failed" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | grep -cE "^warning|^error" | xargs -I{} echo "clippy: {}"
```
Expected: `ALL GREEN`; `ALL HARNESSES GREEN`; `clippy: 0`.

- [ ] **Step 2: Prove non-tautology.**

Run:
```bash
BASE=$(git merge-base HEAD main); git worktree add -d /tmp/huck-v198 "$BASE" 2>&1 | tail -1
( cd /tmp/huck-v198 && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-v198/target/debug/huck bash tests/scripts/xtrace_compound_diff_check.sh | tail -3
git worktree remove --force /tmp/huck-v198 2>&1 | tail -1
```
Expected: pre-fix `Fail:` is large (the pre-fix binary emits no compound traces), confirming the harness exercises the new behavior.

- [ ] **Step 3: Update L-21(a)** in `docs/bash-divergences.md`. Change the `(a)` clause from "huck emits NOTHING for these compound headers" to: the per-construct compound traces ARE now emitted (v198); narrow the residual to the documented divergences — (i) `[[ ]]` rhs *pattern* quote-provenance escaping (`"p q"` → bash `\p\ \q`, huck `p q`); (ii) `=` rendered canonically as `==`; (iii) reconstructed-header quote *style* (`'x'`/`"x"`/`x` all render `x`) and deeply-nested compound-in-`$()`; (iv) a command-substitution *operand* inside `[[ ]]` is expanded twice under `set -x` (once for the trace line, once for the comparison), so a `$(cmd)` with side effects may run twice — same family as the existing residual (b). Keep residuals (b)/(c)/(d) unchanged.

- [ ] **Step 4: Report back** (controller merges + records memory): the STATUS, the Task-1/3 unit FAIL→PASS, the harness result + pre-fix Fail count, full `cargo test` summary, clippy status, and the L-21 edit.

---

## Report-back (Task 4)

Report: STATUS, each task's commit SHA, the harness PASS count + pre-fix Fail count (non-tautology), full `cargo test` + all-harness + clippy results, and the `docs/bash-divergences.md` L-21 diff.
