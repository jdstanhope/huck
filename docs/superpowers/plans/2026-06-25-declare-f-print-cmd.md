# bash-faithful function reconstruction (`declare -f` / `type`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck's function/command reconstructor byte-identical to bash 5.2.21's `print_cmd.c` for `declare -f` / `type`, fix `type` to print the body, and fix `declare -F NAME`.

**Architecture:** Port bash's `print_cmd.c` `inside_function_def` formatting into `crates/huck-syntax/src/generate.rs` (the AST→source pretty-printer used by `declare -f`, `type`, and function export). Three `builtins.rs` touch-ups: `type`/`command -V` emit the reconstructed body; `declare -F NAME` prints the bare name. A new gold-standard `declare_f_diff_check.sh` byte-diffs huck against system bash.

**Tech Stack:** Rust (workspace crates `huck-syntax`, `huck-engine`); bash diff harnesses under `tests/scripts/*_diff_check.sh`.

## Global Constraints

- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run the FULL suite with `cargo test --workspace` (~3648 tests). A plain `cargo test` runs only the root crate (~1207) and silently skips `huck-engine`/`huck-syntax`/`huck-cli`.
- The reconstructor's output MUST stay round-trip-safe: rendering re-parses to a semantically equivalent AST (the existing `assert_rt` / `assert_rt_ast_eq` tests in `generate.rs` must keep passing).
- Byte-faithfulness target is bash **5.2.21** specifically, in non-interactive script/`-c` mode (`inside_function_def`). System `bash` on the dev box is 5.2.21 — use it as the oracle.
- GPL posture: bash source under `/tmp/bash-5.2.21` (or `$BASH_SOURCE_DIR`) may be READ to understand the algorithm; never vendor/copy it or paste bash output into committed files. Re-implement in Rust; capture expected strings into tests by hand.
- Do NOT push to main or merge without explicit user confirmation (CLAUDE.md).

### The bash `print_cmd.c` ruleset (authoritative reference for all tasks)

Captured live from `bash 5.2.21`. `␣` marks a literal trailing space; indentation is 4 spaces/level.

- **Function:** `NAME () ␣\n{ ␣\n<body @ +1>\n}`. The body is always wrapped in `{ }`; a brace-group body is unwrapped (no double brace).
- **`;`/newline between connected commands** = separator emitted AFTER the left operand + newline + indent: `a;\nb;\nc` (final command has none).
- **`&` connector (mid-list)** = inline ` & ` separator: `a & b`. A trailing background command = ` &`.
- **`&&`/`||`** = inline ` && ` / ` || `.
- **Body terminator (`semicolon()`):** `if`/`while`/`until`/`for`/arith-`for`/`select` bodies get a trailing `;` on the last command, UNLESS the rendered body ends in `&` or `\n`. `{ }` group / function / subshell / `case` bodies get NO trailing `;`.
- **`do` placement:** `while`/`until` put `do` on the test line (`; do`); `for … in`/`select` put `;` then `do` on its OWN line; arith-`for` puts `))` then `do` on its OWN line (no `;`).
- **`if`:** `if <cond>; then\n<then @ +1>` then the elif/else chain, then `fi`. **`elif` has no node in bash** — it is a nested `else\n  if … fi;` deepening one indent level per branch; the inner `fi` gets a `;`, the outermost does not.
- **subshell:** inline `( <body> )` — body rendered at the SAME indent (first command inline, continuations on new lines at that indent).
- **group `{ }`:** multiline `{ ␣\n<body @ +1>\n}`.
- **`case`:** `case W in ␣\n`, each clause `<pat0> | <pat1>)\n<body @ +2>\n<term>` where `<term>` (`;;`/`;&`/`;;&`) is on its OWN line at clause indent; body has no trailing `;`. Final `esac`.
- **`(( … ))`:** `((<expr>))` — expression rendered WITHOUT quotes.
- **`[[ … ]]`:** unchanged (`[[ <expr> ]]`).

---

### Task 1: Arithmetic body verbatim renderer (divergence D)

**Files:**
- Modify: `crates/huck-syntax/src/generate.rs` (add `arith_body_to_source`; change `Command::Arith` at ~line 47 and `WordPart::Arith` at ~line 496)
- Test: `crates/huck-syntax/src/generate.rs` (tests module, ~line 710)

**Interfaces:**
- Produces: `fn arith_body_to_source(w: &Word) -> String` — renders an arithmetic
  body Word as raw expression text (no quotes). Consumed here for `Command::Arith`
  and `WordPart::Arith`; consumed by Task 2 for arith-`for` header sections.

**Background:** `arith_string_to_word` (lexer) marks arith literal/expansion parts `quoted: true` so expansion-time quote-removal works. But `part_to_source` wraps quoted parts in `"…"`, so huck renders `(( i < 3 ))` as `((" i < 3 "))`, `$(( i + 1 ))` as `$((" i + 1 "))`, and `(( x + $y ))` as `((" x + ""$y"" "))`. bash prints the bare expression.

- [ ] **Step 1: Write the failing tests**

In the `tests` module of `generate.rs`, add:

```rust
#[test]
fn arith_command_renders_unquoted() {
    use crate::{command, lexer};
    let seq = command::parse(lexer::tokenize("f(){ (( i < 3 )); }").unwrap())
        .unwrap().unwrap();
    let command::Command::FunctionDef { name, body } = seq.first else { panic!() };
    let s = function_to_source(&name, &body);
    assert!(s.contains("(( i < 3 ))"), "got: {s:?}");
    assert!(!s.contains("((\""), "spurious quote in: {s:?}");
}

#[test]
fn arith_expansion_renders_unquoted() {
    use crate::{command, lexer};
    let seq = command::parse(lexer::tokenize("f(){ i=$(( i + 1 )); }").unwrap())
        .unwrap().unwrap();
    let command::Command::FunctionDef { name, body } = seq.first else { panic!() };
    let s = function_to_source(&name, &body);
    assert!(s.contains("$(( i + 1 ))"), "got: {s:?}");
    assert!(!s.contains("$((\""), "spurious quote in: {s:?}");
}

#[test]
fn arith_with_var_renders_unquoted() {
    use crate::{command, lexer};
    let seq = command::parse(lexer::tokenize("f(){ (( x + $y )); }").unwrap())
        .unwrap().unwrap();
    let command::Command::FunctionDef { name, body } = seq.first else { panic!() };
    let s = function_to_source(&name, &body);
    assert!(s.contains("(( x + $y ))"), "got: {s:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-syntax arith_command_renders_unquoted arith_expansion_renders_unquoted arith_with_var_renders_unquoted`
Expected: FAIL (huck emits `((" i < 3 "))` etc.)

- [ ] **Step 3: Add `arith_body_to_source`**

Add this function next to `word_to_source` (after `heredoc_body_to_source`, ~line 473):

```rust
/// Render an arithmetic body Word as raw expression text. The lexer marks
/// arith literal/expansion parts `quoted: true` (so expansion-time quote
/// removal applies), but bash's `print_cmd.c` prints the expression WITHOUT
/// those quotes (`(( i < 3 ))`, not `((" i < 3 "))`). Emit each part bare:
/// Literal text verbatim, expansions via their `$…` source form.
fn arith_body_to_source(w: &Word) -> String {
    let mut out = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text, .. } => out.push_str(text),
            WordPart::Var { name, .. } => out.push_str(&format!("${name}")),
            WordPart::LastStatus { .. } => out.push_str("$?"),
            WordPart::AllArgs { joined, .. } => {
                out.push_str(if *joined { "$*" } else { "$@" })
            }
            WordPart::CommandSub { sequence, .. } => {
                out.push_str(&format!("$({})", sequence_to_source(sequence, 0).trim_end()))
            }
            WordPart::Arith { body, .. } => {
                out.push_str(&format!("$(({}))", arith_body_to_source(body)))
            }
            WordPart::ParamExpansion { name, modifier, subscript, indirect, .. } => out
                .push_str(&param_expansion_to_source(
                    name,
                    modifier,
                    subscript.as_ref(),
                    *indirect,
                )),
            other => out.push_str(&part_to_source(other)),
        }
    }
    out
}
```

- [ ] **Step 4: Wire it into the two non-loop arith sites**

In `command_to_source`, change the `Command::Arith` arm (~line 47):

```rust
        Command::Arith(word) => format!("(({}))", arith_body_to_source(word)),
```

In `part_to_source`, change the `WordPart::Arith` arm (~line 496):

```rust
        WordPart::Arith { body, quoted } => {
            quote_if(*quoted, format!("$(({}))", arith_body_to_source(body)))
        }
```

(The outer `quote_if` stays — a `"$(( … ))"` inside double quotes keeps its
outer quotes; only the INNER expression loses the spurious quotes.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p huck-syntax arith_command_renders_unquoted arith_expansion_renders_unquoted arith_with_var_renders_unquoted`
Expected: PASS

- [ ] **Step 6: Run the syntax-crate suite for regressions**

Run: `cargo test -p huck-syntax`
Expected: PASS (existing `rt_arith`, `rt_arith_cmd` round-trips still hold)

- [ ] **Step 7: Commit**

```bash
git add crates/huck-syntax/src/generate.rs
git commit -m "$(printf 'v218 task 1: arith bodies render unquoted in reconstruction\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Port `print_cmd.c` compound/function format into `generate.rs` (A, B, C, structure, elif)

**Files:**
- Modify: `crates/huck-syntax/src/generate.rs` — rewrite `command_to_source`'s `BraceGroup`/`Subshell`/`FunctionDef` arms, `if_to_source`, `while_to_source`, `for_to_source`, `arith_for_to_source`, `select_to_source`, `case_to_source`, `case_item_to_source`, the `Connector::Amp` arm of `sequence_to_source`; replace `body_block` with `group_body` + `loop_body`; add `nested_elif`.
- Test: `crates/huck-syntax/src/generate.rs` (tests module)

**Interfaces:**
- Consumes: `arith_body_to_source` (Task 1).
- Produces: bash-faithful output from `function_to_source(name, body)` /
  `command_to_source(cmd, indent)` for all compound commands.

**Method note:** The compound printers return their FIRST line with NO leading
pad (the caller — `group_body`/`loop_body` — prepends `pad(indent)`); each
printer prepends `pad(indent)` itself for its own continuation lines
(`do`/`done`/`fi`/`esac`/`else`/closing `}`/`)`) and child bodies.

- [ ] **Step 1: Write the failing exact-match tests**

Add a helper and tests to the `tests` module. These strings are captured
verbatim from bash 5.2.21 — do not alter whitespace.

```rust
fn declf(src: &str) -> String {
    use crate::{command, lexer};
    let seq = command::parse(lexer::tokenize(src).unwrap()).unwrap().unwrap();
    let command::Command::FunctionDef { name, body } = seq.first else {
        panic!("expected a function def")
    };
    function_to_source(&name, &body)
}

#[test]
fn declf_simple_last_semi_suppressed() {
    assert_eq!(declf("f(){ echo a; echo b; }"),
        "f () \n{ \n    echo a;\n    echo b\n}");
}
#[test]
fn declf_subshell_inline() {
    assert_eq!(declf("f(){ ( exit 1 ); }"),
        "f () \n{ \n    ( exit 1 )\n}");
}
#[test]
fn declf_subshell_multi() {
    assert_eq!(declf("f(){ ( a; b ); }"),
        "f () \n{ \n    ( a;\n    b )\n}");
}
#[test]
fn declf_group_multiline() {
    assert_eq!(declf("f(){ { echo a; }; }"),
        "f () \n{ \n    { \n        echo a\n    }\n}");
}
#[test]
fn declf_andor_inline() {
    assert_eq!(declf("f(){ a && b || c; }"),
        "f () \n{ \n    a && b || c\n}");
}
#[test]
fn declf_mid_background_inline() {
    assert_eq!(declf("f(){ echo bg >/dev/null & echo next; }"),
        "f () \n{ \n    echo bg > /dev/null & echo next\n}");
}
#[test]
fn declf_if() {
    assert_eq!(declf("f(){ if a; then b; fi; }"),
        "f () \n{ \n    if a; then\n        b;\n    fi\n}");
}
#[test]
fn declf_if_elif_else() {
    assert_eq!(declf("f(){ if a; then b; elif c; then d; else e; fi; }"),
        "f () \n{ \n    if a; then\n        b;\n    else\n        if c; then\n            d;\n        else\n            e;\n        fi;\n    fi\n}");
}
#[test]
fn declf_while() {
    assert_eq!(declf("f(){ while a; do b; done; }"),
        "f () \n{ \n    while a; do\n        b;\n    done\n}");
}
#[test]
fn declf_until() {
    assert_eq!(declf("f(){ until a; do b; done; }"),
        "f () \n{ \n    until a; do\n        b;\n    done\n}");
}
#[test]
fn declf_for_in() {
    assert_eq!(declf("f(){ for x in 1 2; do echo $x; done; }"),
        "f () \n{ \n    for x in 1 2;\n    do\n        echo $x;\n    done\n}");
}
#[test]
fn declf_for_noin() {
    assert_eq!(declf("f(){ for x; do echo $x; done; }"),
        "f () \n{ \n    for x in \"$@\";\n    do\n        echo $x;\n    done\n}");
}
#[test]
fn declf_arith_for() {
    assert_eq!(declf("f(){ for ((i=0; i<3; i++)); do echo $i; done; }"),
        "f () \n{ \n    for ((i=0; i<3; i++))\n    do\n        echo $i;\n    done\n}");
}
#[test]
fn declf_select() {
    assert_eq!(declf("f(){ select x in a b; do echo $x; done; }"),
        "f () \n{ \n    select x in a b;\n    do\n        echo $x;\n    done\n}");
}
#[test]
fn declf_case() {
    assert_eq!(declf("f(){ case $x in a) echo A;; b|c) echo BC;; esac; }"),
        "f () \n{ \n    case $x in \n        a)\n            echo A\n        ;;\n        b | c)\n            echo BC\n        ;;\n    esac\n}");
}
#[test]
fn declf_loop_bg_tail_no_semi() {
    assert_eq!(declf("f(){ while a; do b & done; }"),
        "f () \n{ \n    while a; do\n        b &\n    done\n}");
}
#[test]
fn declf_nested_compound_tail_gets_semi() {
    assert_eq!(declf("f(){ while a; do for x in 1; do b; done; done; }"),
        "f () \n{ \n    while a; do\n        for x in 1;\n        do\n            b;\n        done;\n    done\n}");
}
#[test]
fn declf_subshell_body_wrapped() {
    assert_eq!(declf("f() ( echo hi )"),
        "f () \n{ \n    ( echo hi )\n}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-syntax declf_`
Expected: FAIL (current format lacks trailing spaces, has wrong terminators / do-placement / subshell layout / elif form).

- [ ] **Step 3: Add `group_body` / `loop_body`, remove `body_block`**

Replace `body_block` (lines ~94-99) and `inline_seq` stays. New helpers:

```rust
/// Group / function / subshell / case body: indented sequence, NO trailing
/// `;`, terminated by a newline. (bash: these bodies never call `semicolon()`.)
fn group_body(seq: &Sequence, indent: usize) -> String {
    format!("{}{}\n", pad(indent), sequence_to_source(seq, indent))
}

/// if / while / until / for / arith-for / select body: indented sequence with
/// bash's `semicolon()` terminator — a trailing `;` UNLESS the rendered body
/// already ends in `&` (a background command) or `\n` (e.g. a heredoc).
fn loop_body(seq: &Sequence, indent: usize) -> String {
    let inner = sequence_to_source(seq, indent);
    let semi = if inner.ends_with('&') || inner.ends_with('\n') { "" } else { ";" };
    format!("{}{}{}\n", pad(indent), inner, semi)
}
```

`inline_seq` (the header renderer for `if`/`while`) is unchanged.

- [ ] **Step 4: Rewrite the `BraceGroup` / `Subshell` / `FunctionDef` arms**

In `command_to_source`, replace the three arms:

```rust
        Command::BraceGroup(seq) => {
            format!("{{ \n{}{}}}", group_body(seq, indent + 1), pad(indent))
        }
        Command::Subshell { body } => {
            // bash prints subshells inline at the SAME indent: `( <body> )`.
            format!("( {} )", sequence_to_source(body, indent))
        }
```

```rust
        Command::FunctionDef { name, body } => {
            // bash always wraps the body in `{ }`; a brace-group body is
            // unwrapped (its inner sequence printed) to avoid double-bracing.
            let inner = match body.as_ref() {
                Command::BraceGroup(seq) => seq.clone(),
                other => Sequence {
                    first: other.clone(),
                    rest: Vec::new(),
                    background: false,
                },
            };
            format!(
                "{name} () \n{p}{{ \n{}{p}}}",
                group_body(&inner, indent + 1),
                p = pad(indent)
            )
        }
```

(`function_to_source` and `exported_function_value` keep delegating to these.)

- [ ] **Step 5: Rewrite `if_to_source` + add `nested_elif`**

```rust
fn if_to_source(c: &IfClause, indent: usize) -> String {
    let mut s = format!("if {}; then\n", inline_seq(&c.condition));
    s.push_str(&loop_body(&c.then_body, indent + 1));
    s.push_str(&nested_elif(&c.elif_branches, &c.else_body, indent));
    s.push_str(&pad(indent));
    s.push_str("fi");
    s
}

/// bash has no `elif` node — it renders `elif` as a nested `else { if … fi; }`,
/// deepening one indent level per branch. The inner `fi` takes a `;` (the outer
/// `semicolon()`); the outermost `fi` (emitted by `if_to_source`) does not.
fn nested_elif(
    elifs: &[ElifBranch],
    else_body: &Option<Sequence>,
    indent: usize,
) -> String {
    if let Some((head, tail)) = elifs.split_first() {
        let inner = indent + 1;
        let mut s = format!("{}else\n", pad(indent));
        s.push_str(&pad(inner));
        s.push_str(&format!("if {}; then\n", inline_seq(&head.condition)));
        s.push_str(&loop_body(&head.body, inner + 1));
        s.push_str(&nested_elif(tail, else_body, inner));
        s.push_str(&pad(inner));
        s.push_str("fi;\n");
        s
    } else if let Some(eb) = else_body {
        format!("{}else\n{}", pad(indent), loop_body(eb, indent + 1))
    } else {
        String::new()
    }
}
```

Add `ElifBranch` to the `use crate::command::{…}` import at the top of the file.

- [ ] **Step 6: Rewrite the loop / case printers**

```rust
fn while_to_source(c: &WhileClause, indent: usize) -> String {
    let kw = if c.until { "until" } else { "while" };
    let mut s = format!("{kw} {}; do\n", inline_seq(&c.condition));
    s.push_str(&loop_body(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn for_to_source(c: &ForClause, indent: usize) -> String {
    let mut header = format!("for {}", c.var);
    if c.has_in {
        header.push_str(" in");
        for w in &c.words {
            header.push(' ');
            header.push_str(&word_to_source(w));
        }
    } else {
        // bash desugars the no-`in` form to `in "$@"`; semantically identical.
        header.push_str(" in \"$@\"");
    }
    let mut s = format!("{header};\n{}do\n", pad(indent));
    s.push_str(&loop_body(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn arith_for_to_source(c: &crate::command::ArithForClause, indent: usize) -> String {
    let sec = |w: &Option<crate::lexer::Word>| {
        w.as_ref().map(arith_body_to_source).unwrap_or_default()
    };
    let mut s = format!(
        "for (({}; {}; {}))\n{}do\n",
        sec(&c.init),
        sec(&c.cond),
        sec(&c.step),
        pad(indent)
    );
    s.push_str(&loop_body(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn select_to_source(c: &SelectClause, indent: usize) -> String {
    let mut header = format!("select {}", c.var);
    if let Some(words) = &c.words {
        header.push_str(" in");
        for w in words {
            header.push(' ');
            header.push_str(&word_to_source(w));
        }
    }
    let mut s = format!("{header};\n{}do\n", pad(indent));
    s.push_str(&loop_body(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn case_to_source(c: &CaseClause, indent: usize) -> String {
    let mut s = format!("case {} in \n", word_to_source(&c.subject));
    for item in &c.items {
        s.push_str(&case_item_to_source(item, indent + 1));
    }
    s.push_str(&pad(indent));
    s.push_str("esac");
    s
}

fn case_item_to_source(item: &CaseItem, indent: usize) -> String {
    let patterns = item
        .patterns
        .iter()
        .map(pattern_word_to_source)
        .collect::<Vec<_>>()
        .join(" | ");
    let mut s = format!("{}{patterns})\n", pad(indent));
    if let Some(body) = &item.body {
        s.push_str(&group_body(body, indent + 1)); // case body: no trailing `;`
    }
    let term = match item.terminator {
        CaseTerminator::Break => ";;",
        CaseTerminator::FallThrough => ";&",
        CaseTerminator::ContinueMatch => ";;&",
    };
    s.push_str(&pad(indent));
    s.push_str(term);
    s.push('\n');
    s
}
```

- [ ] **Step 7: Fix the `&` connector in `sequence_to_source`**

In `sequence_to_source`, replace the `Connector::Amp` arm (~lines 334-337):

```rust
            Connector::Amp => out.push_str(" & "),
```

(Leaves `Connector::Semi` as `";\n"` + `pad`, and `And`/`Or` as ` && ` / ` || `.
The trailing-background `if seq.background { out.push_str(" &"); }` stays.)

- [ ] **Step 8: Run the new tests + full syntax crate**

Run: `cargo test -p huck-syntax`
Expected: PASS — all `declf_*` tests AND the existing `rt_*` round-trip tests
(idempotence preserved). If a `rt_*` test fails, the new format is not
idempotent for that construct — fix the printer, do not weaken the test.

- [ ] **Step 9: Commit**

```bash
git add crates/huck-syntax/src/generate.rs
git commit -m "$(printf 'v218 task 2: port print_cmd.c compound/function format to generate.rs\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: `type` / `command -V` print the function body (divergence G)

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — `emit_type_entry` (~line 6594; add `shell` param + body emission) and its caller (~line 6707); `builtin_command`'s `command -V` Function arm (~lines 6940-6945).
- Test: `crates/huck-engine/src/builtins.rs` (tests module)

**Interfaces:**
- Consumes: `crate::generate::function_to_source` (Task 2's bash-faithful output).
- `emit_type_entry` gains a trailing `shell: &Shell` parameter.

**Background:** huck's `type NAME` prints only `NAME is a function`; bash also prints the reconstructed body. cprint/func/arith-for drive the printer through `type` (and `command -V`), so this is required for any of them to pass.

- [ ] **Step 1: Write the failing test**

In `mod type_tests` (the module holding the `run(args, shell)` helper at
~line 11055), add a new test that defines a function and checks the body is
printed:

```rust
#[test]
fn type_prints_function_body() {
    let mut shell = Shell::new();
    let seq = crate::command::parse(
        crate::lexer::tokenize("tf(){ echo a; }").unwrap(),
    )
    .unwrap()
    .unwrap();
    let crate::command::Command::FunctionDef { name, body } = seq.first else {
        panic!("expected function def")
    };
    shell.define_function(name, body);
    let (oc, out) = run(&["tf"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "tf is a function\ntf () \n{ \n    echo a\n}\n");
}
```

The existing `type_default_function` test (~line 11080) asserts the OLD
body-less output and is now wrong. In this same step, replace its body with a
parsed one and update the expectation:

```rust
#[test]
fn type_default_function() {
    let mut shell = Shell::new();
    let seq = crate::command::parse(
        crate::lexer::tokenize("myfn(){ :; }").unwrap(),
    )
    .unwrap()
    .unwrap();
    let crate::command::Command::FunctionDef { name, body } = seq.first else {
        panic!("expected function def")
    };
    shell.define_function(name, body);
    let (oc, out) = run(&["myfn"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "myfn is a function\nmyfn () \n{ \n    :\n}\n");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p huck-engine type_prints_function_body`
Expected: FAIL (only `tf is a function` is printed)

- [ ] **Step 3: Thread `shell` into `emit_type_entry` and print the body**

Change the signature (~line 6594):

```rust
fn emit_type_entry(
    name: &str,
    res: &CommandResolution,
    type_only: bool,
    path_only: bool,
    out: &mut dyn std::io::Write,
    shell: &Shell,
) {
```

Change the verbose `CommandResolution::Function` arm (~line 6623):

```rust
        CommandResolution::Function => {
            let _ = writeln!(out, "{name} is a function");
            if let Some(body) = shell.functions.get(name) {
                let _ = writeln!(out, "{}", crate::generate::function_to_source(name, body));
            }
        }
```

Update the call site (~line 6707) to pass `shell`:

```rust
            emit_type_entry(name, res, type_only, path_only, out, shell);
```

(If the surrounding function lacks a `shell` binding in scope at 6707, thread
it through that function's signature too — check `cargo build` errors.)

- [ ] **Step 4: Mirror the fix for `command -V`**

In `builtin_command`'s verbose Function arm (~lines 6940-6945):

```rust
            CommandResolution::Function => {
                if concise {
                    let _ = writeln!(out, "{name}");
                } else {
                    let _ = writeln!(out, "{name} is a function");
                    if let Some(body) = shell.functions.get(*name) {
                        let _ = writeln!(out, "{}", crate::generate::function_to_source(name, body));
                    }
                }
            }
```

(Adjust `*name`/`name` to match the local binding type; `concise` is the
existing `-v` flag in that function. `type -t` / `command -v` stay body-less.)

- [ ] **Step 5: Run the test + engine suite**

Run: `cargo test -p huck-engine type_prints_function_body`
Expected: PASS

Run: `cargo test -p huck-engine`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs
git commit -m "$(printf 'v218 task 3: type / command -V print the reconstructed function body\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: `declare -F NAME` prints the bare name (divergence F)

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — `declare_list_functions` (~line 1028) + `emit_function` (~line 1056)
- Test: `crates/huck-engine/src/builtins.rs` (tests module)

**Interfaces:**
- `emit_function` gains an `explicit: bool` parameter (true when a specific name
  was requested).

**Background:** bash's `declare -F NAME` (explicit name) prints the bare `NAME`; `declare -F` (no args) prints `declare -f NAME` per function. huck prints `declare -f NAME` in both. (The no-arg branch stays as-is — bash's `-fx`-for-exported nuance there is a separate, out-of-scope divergence.)

- [ ] **Step 1: Write the failing test**

Add these to the `builtins.rs` tests module (use `run_declaration_builtin_strs`,
the test-only string-arg declare runner at ~line 244). A small local helper
keeps each test short:

```rust
#[cfg(test)]
fn define_fn(shell: &mut Shell, src: &str) {
    let seq = crate::command::parse(crate::lexer::tokenize(src).unwrap())
        .unwrap()
        .unwrap();
    let crate::command::Command::FunctionDef { name, body } = seq.first else {
        panic!("expected function def")
    };
    shell.define_function(name, body);
}

#[test]
fn declare_dash_f_explicit_name_is_bare() {
    let mut shell = Shell::new();
    define_fn(&mut shell, "f(){ echo hi; }");
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let args = vec!["-F".to_string(), "f".to_string()];
    run_declaration_builtin_strs("declare", &args, &mut out, &mut err, &mut shell);
    assert_eq!(String::from_utf8(out).unwrap(), "f\n");
}

#[test]
fn declare_dash_f_no_args_keeps_declare_prefix() {
    let mut shell = Shell::new();
    define_fn(&mut shell, "f(){ :; }");
    define_fn(&mut shell, "g(){ :; }");
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let args = vec!["-F".to_string()];
    run_declaration_builtin_strs("declare", &args, &mut out, &mut err, &mut shell);
    assert_eq!(String::from_utf8(out).unwrap(), "declare -f f\ndeclare -f g\n");
}
```

(Place `define_fn` once in the module if not already present; reuse it.)

- [ ] **Step 2: Run to verify the first fails**

Run: `cargo test -p huck-engine declare_dash_f_explicit_name_is_bare declare_dash_f_no_args_keeps_declare_prefix`
Expected: `declare_dash_f_explicit_name_is_bare` FAILS (prints `declare -f f`); the no-args test PASSES already.

- [ ] **Step 3: Thread `explicit` through and branch the `-F` output**

Change `emit_function` (~line 1056):

```rust
fn emit_function(
    name: &str,
    names_only: bool,
    explicit: bool,
    out: &mut dyn std::io::Write,
    shell: &Shell,
) {
    if names_only {
        if explicit {
            // bash `declare -F NAME` (specific name) → bare name.
            let _ = writeln!(out, "{name}");
        } else {
            // bash `declare -F` (listing) → re-declarable header form.
            let _ = writeln!(out, "declare -f {name}");
        }
    } else if let Some(body) = shell.functions.get(name) {
        let _ = writeln!(out, "{}", crate::generate::function_to_source(name, body));
    }
}
```

Update ALL THREE call sites (the bare-`declare` full listing at ~line 1014, and
the two in `declare_list_functions` at ~lines 1038, 1045). The bare-`declare`
and no-name listings pass `explicit: false`; only the explicit-name loop passes
`true`:

Line ~1014 (bare `declare`, full bodies — `explicit` is irrelevant when
`names_only` is false, pass `false`):

```rust
        for n in &fnames {
            emit_function(n, false, false, out, shell);
        }
```

Line ~1038 (`declare -f`/`-F` with no names — listing):

```rust
        for n in &fnames {
            emit_function(n, names_only, false, out, shell); // listing: not explicit
        }
```

Line ~1045 (`declare -f`/`-F NAME` — explicit name):

```rust
        if shell.functions.contains_key(name) {
            emit_function(name, names_only, true, out, shell); // explicit name
        } else {
```

- [ ] **Step 4: Run the tests + engine suite**

Run: `cargo test -p huck-engine declare_dash_f_explicit_name_is_bare declare_dash_f_no_args_keeps_declare_prefix`
Expected: PASS

Run: `cargo test -p huck-engine`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/builtins.rs
git commit -m "$(printf 'v218 task 4: declare -F NAME prints the bare function name\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Gold-standard diff harness + divergence/baseline bookkeeping

**Files:**
- Create: `tests/scripts/declare_f_diff_check.sh`
- Modify: `docs/bash-divergences.md` (delete the resolved `declare -f` entry; add `[deferred]` entries for E + the `time`-pipeline gap)
- Modify: `docs/bash-test-suite-baseline.md` (re-triage cprint/func/arith-for/herestr)

**Interfaces:** none (test infra + docs).

- [ ] **Step 1: Create the harness**

Create `tests/scripts/declare_f_diff_check.sh`, mirroring
`tests/scripts/arith_error_diff_check.sh`'s structure (shebang, `HUCK_BIN`
resolution to `target/release/huck`, `bash`-absent SKIP, a `fragments` array, a
PASS/FAIL loop with `diff`, and `exit $(( FAIL > 0 ? 1 : 0 ))`). Each fragment
defines a function and reconstructs it; compare STDOUT byte-for-byte:

```bash
#!/usr/bin/env bash
# v218 harness: `declare -f` / `type` / `declare -F` function reconstruction is
# byte-identical between bash 5.2.21 and huck (print_cmd.c inside_function_def
# format). stdout is compared. Requires bash 5.2.x on PATH (the reference).
set -u

_SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HUCK_BIN="${HUCK_BIN:-$_SCRIPT_DIR/../../target/release/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run: cargo build --release --bin huck" >&2
    exit 1
fi
if ! command -v bash >/dev/null 2>&1; then
    echo "SKIP: bash not found on PATH; this differential harness requires bash" >&2
    exit 0
fi

PASS=0; FAIL=0
fragments=(
  'tf(){ echo a; echo b; }; declare -f tf'
  'tf(){ echo a; }; type tf'
  'f(){ ( exit 1 ); }; declare -f f'
  'f(){ ( a; b ); }; declare -f f'
  'f(){ { echo a; }; }; declare -f f'
  'f(){ a && b || c; }; declare -f f'
  'f(){ echo bg >/dev/null & echo next; }; declare -f f'
  'f(){ echo a | cat - >/dev/null; }; declare -f f'
  'f(){ if a; then b; fi; }; declare -f f'
  'f(){ if a; then b; elif c; then d; else e; fi; }; declare -f f'
  'f(){ while a; do b; done; }; declare -f f'
  'f(){ until a; do b; done; }; declare -f f'
  'f(){ while a; do b & done; }; declare -f f'
  'f(){ for x in 1 2; do echo $x; done; }; declare -f f'
  'f(){ for x; do echo $x; done; }; declare -f f'
  'f(){ for ((i=0; i<3; i++)); do echo $i; done; }; declare -f f'
  'f(){ select x in a b; do echo $x; done; }; declare -f f'
  'f(){ case $x in a) echo A;; b|c) echo BC;; esac; }; declare -f f'
  'f(){ (( i < 3 )); }; declare -f f'
  'f(){ i=$(( i + 1 )); }; declare -f f'
  'f(){ [[ -f x && $y == z ]]; }; declare -f f'
  'f(){ echo hi; }; declare -F f'
  'f(){ for ((i=0; i<3; i++)); do echo $i; done; }; type f'
)

for frag in "${fragments[@]}"; do
    b_out=$(bash -c "$frag" 2>&1)
    h_out=$("$HUCK_BIN" -c "$frag" 2>&1)
    if [[ "$b_out" == "$h_out" ]]; then
        printf 'PASS: %s\n' "$frag"; PASS=$((PASS + 1))
    else
        printf 'FAIL: %s\n' "$frag"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Build release huck and run the harness**

Run:
```bash
cargo build --release --bin huck
bash tests/scripts/declare_f_diff_check.sh
```
Expected: every fragment `PASS`, `Fail: 0`. If a fragment FAILs, the `diff`
shows the exact byte divergence — fix the responsible printer in `generate.rs`
(Task 2) and re-run. (The `[[ … ]]` fragment confirms the unchanged double-
bracket path still matches.)

- [ ] **Step 3: Run the full workspace suite**

Run: `cargo test --workspace`
Expected: PASS (~3648 tests).

- [ ] **Step 4: Update `docs/bash-divergences.md`**

- DELETE the `declare -f` trailing-space / reconstruction-format divergence
  entry (now resolved).
- ADD two `[deferred]` entries:
  - here-string / word **quote provenance** in reconstruction: huck's `Word`
    normalizes quote style, so `<<< "$a"" ""$b"` re-renders as `"$a $b"` and a
    double-quoted literal can re-render single-quoted; byte-matching needs the
    lexer/`Word` to track original quote spans (same root cause as the xtrace
    quote-provenance residual). Severity low. Blocks herestr.
  - `time`-pipeline reconstruction: huck has no `time`/`CMD_TIME_PIPELINE` AST
    node, so a `time` pipeline inside a function cannot round-trip. Severity
    low. Partially blocks cprint.

- [ ] **Step 5: Re-triage `docs/bash-test-suite-baseline.md`**

With the release binary built, re-run the affected categories through the
runner (set `$BASH_SOURCE_DIR`, e.g. `/tmp/bash-5.2.21`):

```bash
for c in cprint func arith-for herestr; do
  HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh
done
```

Update each category's status/Note in the baseline doc to the MEASURED result
(prose only — do NOT paste bash `.right` output; GPL posture). Update the
Summary counts and the header (huck commit / sweep date). Expect func and
arith-for to flip or collapse to small residuals; cprint to remain FAIL gated
on the deferred `time` pipeline; herestr to remain FAIL gated on E.

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/declare_f_diff_check.sh docs/bash-divergences.md docs/bash-test-suite-baseline.md
git commit -m "$(printf 'v218 task 5: declare_f diff harness + divergence/baseline re-triage\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer

- **Oracle:** system `bash` is 5.2.21 — use `bash -c '<frag>; declare -f f' | cat -A` to see exact bytes (trailing spaces show as `$`). Never assume; capture.
- **Indentation invariant:** a compound printer returns its first line with NO
  leading pad; `group_body`/`loop_body` add it. Get this wrong and every nested
  construct shifts.
- **Round-trip is sacred:** if a `rt_*` test breaks, the new format isn't
  idempotent — fix the printer, never weaken the assertion.
- **herestr / `time` are out of scope** — do not attempt quote-provenance or a
  `time` AST node here; they are recorded as `[deferred]`.
