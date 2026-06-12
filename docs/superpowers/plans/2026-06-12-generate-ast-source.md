# huck v146 — `generate` (AST→source) module + `declare -f` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `generate` module that renders a parsed `Command` AST back to normalized, re-parseable shell source, wired so `declare -f NAME` prints the real function body.

**Architecture:** New `src/generate.rs` with `function_to_source`/`command_to_source` + per-AST-type helpers, each EXHAUSTIVELY matching its enum (drift-guard — no `_ =>` wildcard). Output is one consistent normalized bash style (NOT byte-identical to bash). Correctness is verified by ROUND-TRIP idempotence (`parse→serialize→parse→serialize` is a stable fixpoint) + execution-equivalence, not bash-diff.

**Tech Stack:** Rust; `src/generate.rs` (NEW), `src/command.rs`/`src/lexer.rs` (AST types — read-only reference), `src/builtins.rs` (`declare -f` wiring). Reuse `escape_double_quote_value` (builtins.rs).

**Reference:** spec at `docs/superpowers/specs/2026-06-12-generate-ast-source-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>`. Stay on `v146-generate-source`. Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** BINARY crate — `cargo test --bin huck <filter>`, `cargo clippy --all-targets`. Builds take minutes.

**Core principle for the implementer:** the ROUND-TRIP corpus tests are the precise behavior spec. You do NOT need to predict exact output strings — each test asserts `s1 == s2` (idempotence) and (where marked) `a == b` (AST equality) and/or execution-equivalence. Implement each enum arm to whatever consistent, re-parseable form you choose; the tests fail until it round-trips. Match every enum EXHAUSTIVELY (the compiler enforces coverage — do not add `_ =>`).

**Round-trip helper (used by every task's tests):** add this to `src/generate.rs` mod tests:
```rust
#[cfg(test)]
fn rt(src: &str) -> (String, String) {
    use crate::{lexer, command};
    let a = command::parse(lexer::tokenize(src).expect("lex")).expect("parse").expect("non-empty");
    let s1 = sequence_to_source(&a, 0);
    let b = command::parse(lexer::tokenize(&s1).expect("lex s1")).expect("parse s1").expect("non-empty s1");
    let s2 = sequence_to_source(&b, 0);
    (s1, s2)
}
// idempotent: serialize is a stable fixpoint
#[cfg(test)]
fn assert_rt(src: &str) {
    let (s1, s2) = rt(src);
    assert_eq!(s1, s2, "not idempotent for {src:?}\n s1={s1:?}\n s2={s2:?}");
    // also re-parseable + non-empty
    assert!(!s1.trim().is_empty(), "empty output for {src:?}");
}
// strong: AST equality too (use only where serialization is representation-preserving)
#[cfg(test)]
fn assert_rt_ast_eq(src: &str) {
    use crate::{lexer, command};
    assert_rt(src);
    let a = command::parse(lexer::tokenize(src).unwrap()).unwrap().unwrap();
    let s1 = sequence_to_source(&a, 0);
    let b = command::parse(lexer::tokenize(&s1).unwrap()).unwrap().unwrap();
    assert_eq!(a, b, "AST changed across round-trip for {src:?}");
}
```

---

### Task 1: module skeleton + `word_to_source` (the core)

**Files:**
- Create: `src/generate.rs`
- Modify: `src/main.rs` (or the crate root that declares modules) — add `mod generate;`

- [ ] **Step 1: Register the module, expose the quoting helper, stub the entry points.**
  - In `src/main.rs` add `mod generate;` to the alphabetical module list (it sits between `mod expand;` (line 11) and `mod glob_match;`).
  - In `src/builtins.rs` change `fn escape_double_quote_value(s: &str) -> String` (line ~749) to `pub(crate) fn escape_double_quote_value(...)` so `generate.rs` can reuse it (it's currently private). (`xtrace_quote` in param_expansion.rs is already `pub(crate)` if you need a metachar-quoter; `escape_filename` in completion.rs is private — don't depend on it, write a small `escape_bareword` in generate.rs instead.)
  - Create `src/generate.rs` with signatures + the test helper above and a `mod tests`:
```rust
//! Render a parsed `Command` AST back to normalized, re-parseable shell source.
//! Output is a single consistent style (NOT byte-identical to bash's `declare -f`
//! pretty-printer); correctness is round-trip idempotence, verified in tests.
use crate::command::{
    Command, Sequence, Pipeline, SimpleCommand, ExecCommand, Connector,
    Assignment, AssignTarget, Redirect, TestExpr, IfClause, WhileClause, ForClause,
    CaseClause, CaseItem, CaseTerminator, SelectClause, ArithForClause, ElifBranch,
};
use crate::lexer::{Word, WordPart, ParamModifier, SubscriptKind, TildeSpec};

/// Render a function definition for `declare -f`: `NAME ()\n<body>`.
pub fn function_to_source(name: &str, body: &Command) -> String {
    format!("{name} ()\n{}", command_to_source(body, 0))
}

/// Render any command at nesting depth `indent` (4 spaces per level).
pub fn command_to_source(_cmd: &Command, _indent: usize) -> String {
    String::new() // implemented in Tasks 2-3
}

fn sequence_to_source(_seq: &Sequence, _indent: usize) -> String {
    String::new() // implemented in Task 2
}

fn word_to_source(_w: &Word) -> String {
    String::new() // implemented in this task
}

fn pad(indent: usize) -> String { "    ".repeat(indent) }
```
(Adjust the imports to the exact names/paths; some live in `lexer`, some in `command`. Remove unused-import warnings as you implement.)

- [ ] **Step 2: Write the failing `word_to_source` round-trip tests** — in `mod tests`:
```rust
#[test] fn rt_simple_word() { assert_rt_ast_eq("echo hello"); }
#[test] fn rt_double_quoted() { assert_rt("echo \"a  b\""); }
#[test] fn rt_single_quoted() { assert_rt("echo 'a  b'"); }
#[test] fn rt_escaped_space() { assert_rt("echo a\\ b"); }
#[test] fn rt_var() { assert_rt_ast_eq("echo $HOME"); }
#[test] fn rt_braced_var() { assert_rt_ast_eq("echo ${HOME}"); }
#[test] fn rt_last_status() { assert_rt_ast_eq("echo $?"); }
#[test] fn rt_all_args() { assert_rt("echo \"$@\""); }
#[test] fn rt_cmdsub() { assert_rt("echo $(date)"); }
#[test] fn rt_arith() { assert_rt("echo $((1 + 2))"); }
#[test] fn rt_param_default() { assert_rt("echo ${x:-def}"); }
#[test] fn rt_param_alt() { assert_rt("echo ${x:+alt}"); }
#[test] fn rt_param_remove_suffix() { assert_rt("echo ${x%.txt}"); }
#[test] fn rt_param_subst() { assert_rt("echo ${x/a/b}"); }
#[test] fn rt_param_substring() { assert_rt("echo ${x:1:2}"); }
#[test] fn rt_param_length() { assert_rt("echo ${#x}"); }
#[test] fn rt_array_index() { assert_rt("echo ${a[2]}"); }
#[test] fn rt_array_all() { assert_rt("echo \"${a[@]}\""); }
#[test] fn rt_transform_Q() { assert_rt("echo ${x@Q}"); }
#[test] fn rt_tilde() { assert_rt("echo ~"); }
#[test] fn rt_mixed() { assert_rt("echo pre$HOME\"post $x\"$(id)"); }
```

- [ ] **Step 3: Run — verify failure**
`cargo test --bin huck generate::tests::rt_ 2>&1 | tail -25` → all fail (empty output / not idempotent). Record.

- [ ] **Step 4: Implement `word_to_source`** — render each `WordPart`, concatenated. Match EXHAUSTIVELY (read `enum WordPart` in src/lexer.rs:199 + `ParamModifier` + `SubscriptKind` + `TildeSpec`). Structure:
```rust
fn word_to_source(w: &Word) -> String {
    let mut s = String::new();
    for part in &w.0 {
        s.push_str(&part_to_source(part));
    }
    s
}

fn part_to_source(part: &WordPart) -> String {
    match part {
        WordPart::Literal { text, quoted } => {
            if *quoted {
                // Preserve literally inside double quotes (empty -> '').
                if text.is_empty() { "''".to_string() }
                else { format!("\"{}\"", crate::builtins::escape_double_quote_value(text)) }
            } else {
                escape_bareword(text) // backslash-escape shell metachars; see helper below
            }
        }
        WordPart::Var { name, quoted } => quote_if(*quoted, format!("${name}")),
        WordPart::LastStatus { quoted } => quote_if(*quoted, "$?".to_string()),
        WordPart::AllArgs { quoted, joined } => {
            let body = if *joined { "$*" } else { "$@" };
            quote_if(*quoted, body.to_string())
        }
        WordPart::CommandSub { sequence, quoted } => {
            quote_if(*quoted, format!("$({})", sequence_to_source(sequence, 0).trim_end()))
        }
        WordPart::Arith { body, quoted } => {
            quote_if(*quoted, format!("$(({}))", word_to_source(body)))
        }
        WordPart::Tilde(t) => match t {
            TildeSpec::Home => "~".to_string(),
            TildeSpec::User(u) => format!("~{u}"),
            TildeSpec::Pwd => "~+".to_string(),
            TildeSpec::OldPwd => "~-".to_string(),
        },
        WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect } => {
            quote_if(*quoted, param_expansion_to_source(name, modifier, subscript, *indirect))
        }
        WordPart::AssignPrefix { target, append } => {
            // LHS prefix of an assignment word (name[sub]= / name+= / name[sub]+=).
            format!("{}{}=", assign_target_to_source(target), if *append { "+" } else { "" })
        }
        WordPart::ArrayLiteral(elems) => array_literal_to_source(elems),
    }
}

fn quote_if(quoted: bool, body: String) -> String {
    if quoted { format!("\"{body}\"") } else { body }
}
```
Add helpers:
- `escape_bareword(text)` — backslash-escape characters special outside quotes (space, tab, `'"\$;&|<>()*?[]~#`` `` ` ``{}` etc.). REUSE the existing filename/xtrace escaper if its char set matches; otherwise write a small one. (Round-trip tests `rt_escaped_space` etc. gate this.)
- `param_expansion_to_source(name, modifier, subscript, indirect)` — build `${ [!] name [sub] [modifier-suffix] }`. EXHAUSTIVELY match `ParamModifier` (None, Length→`#name`, IndirectKeys, UseDefault/AssignDefault/ErrorIfUnset/UseAlternate→`:-`/`-`/`:=`/`=`/`:?`/`?`/`:+`/`+` per `colon`, RemovePrefix/Suffix→`#`/`##`/`%`/`%%` per `longest`, Substitute→`/`/`//`/`/#`/`/%` per `anchor`+`all` then `pattern/replacement`, Substring→`:off[:len]`, Case→`^`/`^^`/`,`/`,,` per direction+all, Transform→`@OP`). `subscript`: `SubscriptKind::All`→`[@]`, `Star`→`[*]`, `Index(w)`→`[<word>]`. `indirect`→ leading `!`. The pattern/word sub-fields are `Word`s → `word_to_source`. Read `ParamModifier`/`SubstAnchor`/`CaseDirection`/`TransformOp` in src/lexer.rs and invert each to its `${…}` syntax.
- `assign_target_to_source(target)` — `Bare(n)`→`n`; `Indexed{name, subscript}`→`format!("{name}[{}]", word_to_source(subscript))`.
- `array_literal_to_source(elems)` — `( elem … )`; each `ArrayLiteralElement` → `[sub]=value` or bare `value` (read `ArrayLiteralElement`).

NOTE: `sequence_to_source` is still a stub here; `rt_cmdsub` needs it. Either implement a MINIMAL `sequence_to_source` (single simple command) now, or order the work so Task 2 lands first. Pragmatic: implement enough of `sequence_to_source`/`simple_to_source` inline in this task to make `rt_cmdsub` pass (a one-command sequence), and Task 2 completes the connector/redirect logic. (The compiler + tests guide you.)

- [ ] **Step 5: Run tests + clippy**
`cargo test --bin huck generate 2>&1 | tail -20` → the word tests pass.
`cargo clippy --all-targets 2>&1 | tail -8` → clean (resolve unused-import warnings for the not-yet-used types by `#[allow]` or wiring them in Task 2/3).

- [ ] **Step 6: Commit**
```bash
git add src/generate.rs src/main.rs src/builtins.rs
git commit -m "$(printf 'feat: generate module + word_to_source (AST->source)\n\nReuses escape_double_quote_value (now pub(crate)); round-trip idempotence tests.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: simple commands, pipelines, sequences, redirects, assignments

**Files:**
- Modify: `src/generate.rs`

- [ ] **Step 1: Write failing round-trip tests** — in `mod tests`:
```rust
#[test] fn rt_args() { assert_rt_ast_eq("ls -l /tmp"); }
#[test] fn rt_assign_prefix() { assert_rt("FOO=bar BAZ=1 cmd a b"); }
#[test] fn rt_bare_assign() { assert_rt("x=1"); }
#[test] fn rt_append_assign() { assert_rt("x+=tail"); }
#[test] fn rt_array_assign() { assert_rt("a=(1 2 3)"); }
#[test] fn rt_indexed_assign() { assert_rt("a[2]=v"); }
#[test] fn rt_pipeline() { assert_rt_ast_eq("a | b | c"); }
#[test] fn rt_negated_pipeline() { assert_rt_ast_eq("! grep x f"); }
#[test] fn rt_semi() { assert_rt_ast_eq("a; b; c"); }
#[test] fn rt_and_or() { assert_rt_ast_eq("a && b || c"); }
#[test] fn rt_background() { assert_rt("sleep 1 &"); }
#[test] fn rt_redir_trunc() { assert_rt("echo hi > out"); }
#[test] fn rt_redir_append() { assert_rt("echo hi >> out"); }
#[test] fn rt_redir_read() { assert_rt("cat < in"); }
#[test] fn rt_redir_clobber() { assert_rt("echo hi >| out"); }
#[test] fn rt_redir_dup() { assert_rt("cmd 2>&1"); }
#[test] fn rt_redir_dup_file() { assert_rt("cmd > out 2>&1"); }
#[test] fn rt_herestring() { assert_rt("cat <<< word"); }
```

- [ ] **Step 2: Run — verify failure** (`cargo test --bin huck generate 2>&1 | tail -25`). Record.

- [ ] **Step 3: Implement** `sequence_to_source`, `pipeline_to_source`, `simple_to_source`/`exec_to_source`, `redirect_to_source`, `assignment_to_source`, and the `Command::{Pipeline,Simple}` arms of `command_to_source`. EXHAUSTIVELY match `Connector`, `Redirect`, `AssignTarget`, `SimpleCommand`. Pattern:
```rust
fn sequence_to_source(seq: &Sequence, indent: usize) -> String {
    let mut out = String::new();
    out.push_str(&command_to_source(&seq.first, indent));
    for (conn, cmd) in &seq.rest {
        match conn {
            Connector::Semi => { out.push_str(";\n"); out.push_str(&pad(indent)); }
            Connector::And  => out.push_str(" && "),
            Connector::Or   => out.push_str(" || "),
            Connector::Amp  => { out.push_str(" &\n"); out.push_str(&pad(indent)); }
        }
        out.push_str(&command_to_source(cmd, indent));
    }
    if seq.background { out.push_str(" &"); }
    out
}
```
(`command_to_source(&seq.first, indent)` must emit the FIRST line WITHOUT a leading `pad` — the caller positions the first line; helpers that open a new line push `pad(indent)` themselves. Keep this discipline consistent — the round-trip tests + indentation in Task 3 will expose violations.)

`command_to_source` for the simple/pipeline arms:
```rust
pub fn command_to_source(cmd: &Command, indent: usize) -> String {
    match cmd {
        Command::Simple(s) => simple_to_source(s, indent),
        Command::Pipeline(p) => pipeline_to_source(p, indent),
        // compounds: Task 3
        _ => command_to_source_compound(cmd, indent), // add a second fn in Task 3, or inline
    }
}

fn pipeline_to_source(p: &Pipeline, indent: usize) -> String {
    let mut s = if p.negate { "! ".to_string() } else { String::new() };
    let stages: Vec<String> = p.commands.iter().map(|c| command_to_source(c, indent)).collect();
    s.push_str(&stages.join(" | "));
    s
}

fn exec_to_source(e: &ExecCommand) -> String {
    let mut parts: Vec<String> = Vec::new();
    for a in &e.inline_assignments { parts.push(assignment_to_source(a)); }
    parts.push(word_to_source(&e.program));
    for w in &e.args { parts.push(word_to_source(w)); }
    let mut s = parts.join(" ");
    // redirects (order: stdin, stdout, stderr — bash-acceptable)
    if let Some(r) = &e.stdin  { s.push(' '); s.push_str(&redirect_to_source(r, 0)); }
    if let Some(r) = &e.stdout { s.push(' '); s.push_str(&redirect_to_source(r, 1)); }
    if let Some(r) = &e.stderr { s.push(' '); s.push_str(&redirect_to_source(r, 2)); }
    s
}
```
`redirect_to_source(r, default_fd)` — EXHAUSTIVE match `Redirect`: `Read`→`< w`, `Truncate`→`> w`/`2> w` (prefix the fd when default_fd==2), `Append`→`>> w`, `Clobber`→`>| w`, `Dup{fd,source}`→`{fd}>&{source}`, `HereString`→`<<< w`, `Heredoc{body,expand,strip_tabs}`→ see Task 3's heredoc note (a `Heredoc` redirect can appear on a simple command too; render `<<DELIM\n<body>\nDELIM` with a fresh delimiter, OR — if multi-line heredoc round-trip is deferred — convert to an equivalent here-string `<<< "<body>"` and document; pick one and make the corpus assert it). `assignment_to_source(a)` → `target [+]= value` reusing `assign_target_to_source` + `word_to_source(&a.value)`.

- [ ] **Step 4: tests + clippy** — `cargo test --bin huck generate 2>&1 | tail -20` (Task 1 + 2 tests pass), `cargo clippy --all-targets 2>&1 | tail -6` clean.

- [ ] **Step 5: Commit**
```bash
git add src/generate.rs
git commit -m "$(printf 'feat: generate simple/pipeline/sequence/redirect/assignment\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: compound commands + TestExpr

**Files:**
- Modify: `src/generate.rs`

- [ ] **Step 1: Write failing round-trip tests:**
```rust
#[test] fn rt_if() { assert_rt("if a; then b; fi"); }
#[test] fn rt_if_else() { assert_rt("if a; then b; else c; fi"); }
#[test] fn rt_if_elif() { assert_rt("if a; then b; elif c; then d; else e; fi"); }
#[test] fn rt_while() { assert_rt("while a; do b; done"); }
#[test] fn rt_until() { assert_rt("until a; do b; done"); }
#[test] fn rt_for() { assert_rt("for x in 1 2 3; do echo $x; done"); }
#[test] fn rt_for_noin() { assert_rt("for x; do echo $x; done"); }
#[test] fn rt_arith_for() { assert_rt("for ((i=0; i<3; i++)); do echo $i; done"); }
#[test] fn rt_select() { assert_rt("select x in a b; do echo $x; done"); }
#[test] fn rt_case() { assert_rt("case $x in a) echo A;; b|c) echo BC;; esac"); }
#[test] fn rt_case_fallthrough() { assert_rt("case $x in a) echo A;& *) echo D;; esac"); }
#[test] fn rt_subshell() { assert_rt("(a; b)"); }
#[test] fn rt_brace_group() { assert_rt("{ a; b; }"); }
#[test] fn rt_arith_cmd() { assert_rt("((x + 1))"); }
#[test] fn rt_dbracket_unary() { assert_rt("[[ -f /etc/passwd ]]"); }
#[test] fn rt_dbracket_binary() { assert_rt("[[ $x == y ]]"); }
#[test] fn rt_dbracket_regex() { assert_rt("[[ $x =~ ^a ]]"); }
#[test] fn rt_dbracket_logic() { assert_rt("[[ -n $x && $y == z ]]"); }
#[test] fn rt_redirected_compound() { assert_rt("while a; do b; done > out"); }
#[test] fn rt_nested() { assert_rt("if a; then for x in 1 2; do echo $x; done; fi"); }
```

- [ ] **Step 2: Run — verify failure.** Record.

- [ ] **Step 3: Implement the remaining `command_to_source` arms** + `testexpr_to_source`. EXHAUSTIVELY match `Command` (every variant) and `TestExpr`/`TestUnaryOp`/`TestBinaryOp`. Layout pattern (a compound opens lines, indents its body at `indent+1`, closes at `indent`):
```rust
// inside command_to_source's match:
Command::If(c) => {
    let mut s = format!("if {}; then\n", sequence_inline(&c.condition));
    s.push_str(&body_block(&c.then_body, indent + 1));
    for elif in &c.elif_branches {
        s.push_str(&pad(indent));
        s.push_str(&format!("elif {}; then\n", sequence_inline(&elif.condition)));
        s.push_str(&body_block(&elif.body, indent + 1));
    }
    if let Some(eb) = &c.else_body {
        s.push_str(&pad(indent)); s.push_str("else\n");
        s.push_str(&body_block(eb, indent + 1));
    }
    s.push_str(&pad(indent)); s.push('f'); s.push('i'); // "fi"
    s
}
```
where helpers:
- `body_block(seq, indent)` → `format!("{}{};\n", pad(indent), sequence_to_source(seq, indent))` — emits the body lines, each indented, terminated.
- `sequence_inline(seq)` → `sequence_to_source(seq, 0)` for conditions/`in`-lists that sit on the header line.
Implement the analogous arms for `While`(+`until` keyword), `For` (`for VAR[ in W…]; do`/body/`done`), `ArithFor` (`for ((init; cond; step)); do`…; sections are `Option<Word>` → `word_to_source` or empty), `Select`, `Case` (subject + items: `PAT | PAT)`/body/terminator `;;`|`;&`|`;;&` via `CaseTerminator`), `Subshell` (`(`/body/`)`), `BraceGroup` (`{`/body/`}` — note `{` needs surrounding space/`;` to re-parse: `{\n … ;\n}`), `Arith` (`(( <word> ))`), `DoubleBracket` (`<inline-assigns> [[ <testexpr> ]]`), `Redirected` (inner + trailing redirects via `redirect_to_source`).
`testexpr_to_source`: `Unary{op,operand}`→`<op-token> <operand>`; `Binary{op,lhs,rhs}`→`<lhs> <op-token> <rhs>`; `Regex{lhs,pattern}`→`<lhs> =~ <pattern>`; `Not`→`! ( <e> )`; `And`→`<a> && <b>`; `Or`→`<a> || <b>`. Map each `TestUnaryOp`/`TestBinaryOp` to its operator token by inverting the parse table in src/command.rs (e.g. `FileExists`→`-e`, `StringNonEmpty`→`-n`, `StrEq`→`==`, `NumEq`→`-eq`, …). Match EXHAUSTIVELY.

NOTE (heredoc): if a compound carries a `Heredoc` redirect (or a simple command does), apply the SAME heredoc decision from Task 2 consistently. Decide once: full `<<DELIM\n…\nDELIM` round-trip, OR document a here-string-equivalent fallback. Add a corpus test for whichever you choose (`rt_heredoc`).

- [ ] **Step 4: tests + clippy** — `cargo test --bin huck generate 2>&1 | tail -25` (ALL generate tests pass — Tasks 1-3), `cargo clippy --all-targets 2>&1 | tail -6` clean. The exhaustive matches mean NO `_ =>` wildcard remains in `command_to_source`/`part_to_source`/`testexpr_to_source`/`redirect_to_source`.

- [ ] **Step 5: Commit**
```bash
git add src/generate.rs
git commit -m "$(printf 'feat: generate compound commands + TestExpr (full AST coverage)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: wire `declare -f`, execution-equivalence tests, docs, regression

**Files:**
- Modify: `src/builtins.rs` (`declare -f` ~908), `docs/bash-divergences.md`
- Modify: `src/generate.rs` (execution-equivalence test) or a new integration test file

- [ ] **Step 1: Write the failing `declare -f` integration test** — `tests/declare_f_integration.rs` (new) OR add to an existing integration test file. Via the huck binary:
```rust
use std::process::{Command, Stdio};
fn huck_c(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).arg("-c").arg(s)
        .stdin(Stdio::null()).output().unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}
#[test]
fn declare_f_prints_body() {
    let out = huck_c("f(){ echo hi; }; declare -f f");
    assert!(out.contains("echo hi"), "body not printed: {out:?}");
    assert!(!out.trim().eq("declare -f f"), "still the stub: {out:?}");
}
#[test]
fn declare_f_reparse_executes() {
    // The printed definition, re-evaluated, runs equivalently.
    let out = huck_c("g(){ for x in 1 2 3; do echo $x; done; }; eval \"$(declare -f g)\"; g");
    assert_eq!(out, "1\n2\n3\n", "round-tripped function changed behavior: {out:?}");
}
```

- [ ] **Step 2: Run — verify failure** (`cargo test --test declare_f_integration 2>&1 | tail -10`) → the stub still prints `declare -f f`. Record.

- [ ] **Step 3: Wire `declare -f`** — `src/builtins.rs` (~908, the function-listing block that currently emits `writeln!(out, "declare -f {name}")`). Replace the body-emit so, for each named (or all) function present in `shell.functions`, it prints `generate::function_to_source(name, body)`:
```rust
// for a function that EXISTS:
if let Some(body) = shell.functions.get(name) {
    let _ = writeln!(out, "{}", crate::generate::function_to_source(name, body));
} else {
    // bash: declare -f on a missing function is silent, rc 1 (existing behavior)
}
```
Preserve `declare -F` (names-only) behavior unchanged. Find the exact current loop (it iterates names or all functions) and swap only the per-function emit. `shell.functions.get(name)` returns `Option<&Box<Command>>` — deref to `&Command` for `function_to_source`.

- [ ] **Step 4: Run tests** — `cargo test --test declare_f_integration 2>&1 | tail -8` → pass. `cargo test --bin huck generate 2>&1 | tail -6` → all round-trip tests pass.

- [ ] **Step 5: Docs** — `docs/bash-divergences.md`: (a) update the M-121 entry — `declare -f` now prints a NORMALIZED (non-byte-identical) body; the remaining `export -f` work (env encoding + child import) stays deferred under M-121 (narrow its text to that). (b) Add a one-line `[intentional]` low note (or fold into M-121) that `declare -f` output is normalized, not bash-byte-identical (semantically equivalent + re-parseable). Keep counts consistent (no new Tier-2; if you add a Tier-4 `[intentional]` note, bump Tier-4 by 1 — verify the current count first).

- [ ] **Step 6: Full regression**
`cargo test 2>&1 | grep -E "test result: FAILED|[1-9][0-9]* failed|error\[" | head || echo NONE` → NONE.
`cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo "FAIL ($f)"; done` → every harness OK (declare-related harnesses must still pass — `declare -f` format change shouldn't be in a bash-diff harness, but `declare -p` ones must be unaffected).
`cargo clippy --all-targets 2>&1 | tail -6` → clean.

- [ ] **Step 7: Payoff + commit**
`target/debug/huck -c 'f(){ if [ -n "$1" ]; then echo "$1"; fi; }; declare -f f'` → prints a readable, re-parseable body. Paste it.
```bash
git add src/builtins.rs docs/bash-divergences.md tests/declare_f_integration.rs
git commit -m "$(printf 'feat: declare -f prints the function body via generate; docs\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer
- **Round-trip idempotence (`s1 == s2`) is the hard gate.** You don't need to match bash's output — only be self-consistent + re-parseable. Use `assert_rt`; use `assert_rt_ast_eq` only where serialization is representation-preserving (NOT for quoting-normalized forms).
- **EXHAUSTIVE matches, no `_ =>` wildcard** in `part_to_source`/`command_to_source`/`testexpr_to_source`/`redirect_to_source` — a future AST variant must break the build here (drift-guard).
- **First-line / indent discipline:** a command emits its first line WITHOUT a leading `pad(indent)`; helpers that open a NEW line push `pad(indent)` themselves. Keep this uniform or nested-compound indentation drifts (the `rt_nested` test guards it).
- **Reuse `escape_double_quote_value`** (builtins.rs) for quoted-literal bodies; don't reinvent quoting.
- **Heredoc:** make ONE decision (full heredoc round-trip vs documented here-string-equivalent) and apply it in both Task 2 and Task 3; add a corpus test for it.
- **`function_to_source` body:** for a `BraceGroup` body the output is `NAME ()\n{\n    …\n}`; for other body forms (subshell/compound) render that form — `command_to_source(body, 0)` handles it.
