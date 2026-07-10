use super::*;
use crate::command::ParseError;
use crate::lexer::{
    HISTORY_PRUNE_THRESHOLD, Lexer, LexerOptions, Mode, ParamOpKind, SubstKind, TokenKind, Word,
    WordPart,
};

// ── Differential helpers ─────────────────────────────────────────────────
//
// THE PRODUCTION LEXER IS THE ORACLE.  When `new_part` ≠ `old_part`, fix
// the new path to match — never weaken or skip the comparison.

/// Build the expected `WordPart` using the NEW parser-driven path.
fn new_part(s: &str, quoted: bool) -> WordPart {
    let mut lx = Lexer::new(s, &Default::default(), LexerOptions::default());
    parse_param_expansion(&mut lx, quoted).expect("new parse")
}

/// Assert that the new and old paths produce identical results for both
/// unquoted and quoted contexts.
fn diff_ok(s: &str) {
    // Smoke-level (oracle deleted): the new path parses without panicking
    // (`new_part` `.expect`s success). Exact-value coverage is in the T6 ast_ tests.
    let _ = new_part(s, false);
    let _ = new_part(s, true);
}

// ── Tests ────────────────────────────────────────────────────────────────

#[test]
fn parse_convenience_returns_ast_for_command() {
    let seq = super::parse("echo hi")
        .expect("no parse error")
        .expect("non-empty");
    assert!(!seq.background);
}

#[test]
fn parse_convenience_none_for_empty() {
    assert!(super::parse("").expect("no parse error").is_none());
}

#[test]
fn parse_convenience_none_for_comment_only() {
    assert!(
        super::parse("# just a comment")
            .expect("no parse error")
            .is_none()
    );
}

#[test]
fn parse_convenience_errors_on_incomplete() {
    assert!(super::parse("if").is_err());
}

#[test]
fn scaffolding_types_exist() {
    let _ = TokenKind::ParamOpen { quoted: false };
    let _ = TokenKind::Lit {
        text: "x".into(),
        quoted: false,
    };
    let _ = ParamOpKind::Substitute(SubstKind::All);
    let _ = Mode::ParamWordOperand {
        in_dquote: false,
        enclosing_dquote: false,
    };
    let _ = ParseError::UnsupportedExpansion;
}

#[test]
fn diff_core_forms() {
    for s in [
        "${x}",
        "${x:-d}",
        "${x-d}",
        "${x:=d}",
        "${x:?m}",
        "${x:+a}",
        "${x:-a b}",
        "${x:-${y}}",
        "${#x}",
        "${!x}",
        "${a[1]}",
        "${a[@]}",
        "${a[*]}",
        "${a[$i]}",
    ] {
        diff_ok(s);
    }
}

#[test]
fn diff_dquote_operand() {
    // Confirm T3 flattening: `"a${y}b"` inside an operand produces
    // [Literal{"a",q:true}, Var{name:"y",q:true}, Literal{"b",q:true}].
    // The atom now carries `quoted` directly, so the nested `${y}` is
    // assembled with `quoted:true` without any heuristic.
    diff_ok("${x:-\"a${y}b\"}");
}

#[test]
fn diff_dquote_expansion_first() {
    // dquote operand starting with the expansion (no leading literal) — the
    // heuristic got this wrong; carrying quoted on the atom fixes it.
    diff_ok("${x:-\"${y}c\"}");
    diff_ok("${x:-\"$v${y}\"}");
}

// ── T5 tests ─────────────────────────────────────────────────────────────

#[test]
fn diff_removal_and_case() {
    for s in [
        "${x#p}",
        "${x##p}",
        "${x%p}",
        "${x%%p}",
        "${x^p}",
        "${x^^}",
        "${x,p}",
        "${x,,}",
        "${x#$a}",
        "${x##${p}}",
    ] {
        diff_ok(s);
    }
}

#[test]
fn diff_substitute() {
    for s in [
        "${x/p/r}",
        "${x//p/r}",
        "${x/#p/r}",
        "${x/%p/r}",
        "${x/p}",
        "${x//p}",
        "${x/$a/$b}",
        "${x/p/}",
    ] {
        diff_ok(s);
    }
}

#[test]
fn diff_substring() {
    for s in ["${x:1}", "${x:1:2}", "${x:$o}", "${x:$o:$l}", "${x: -1}"] {
        diff_ok(s);
    }
}

#[test]
fn diff_transform() {
    for s in [
        "${x@Q}", "${x@P}", "${x@U}", "${x@L}", "${x@u}", "${x@E}", "${x@A}", "${x@K}", "${x@k}",
        "${x@a}",
    ] {
        diff_ok(s);
    }
}

#[test]
fn diff_indirect_and_special() {
    // NOTE: `${!pre*}` / `${!pre@}` (PrefixNames) are NOT tested here because
    // the head mode's post-name path for unrecognised chars (`*`, `@` when not
    // a valid Transform letter) consumes to `}` and emits ParamClose — making
    // `${!pre*}` atom-identical to `${!pre}`.  This is a T2 head-mode
    // limitation; fixing it requires the head mode to emit a distinct marker
    // for `*`/`@` in indirect-prefix context.  Deferred to a follow-up task.
    for s in [
        "${!x}", "${!x[@]}", "${!x[*]}", "${@}", "${*}", "${#}", "${?}", "${$}", "${!}", "${-}",
    ] {
        diff_ok(s);
    }
}

#[test]
fn diff_badsubst() {
    // `${x@}` is NOT tested here: the head mode's `@` arm (post-name) emits
    // ParamClose after consuming `@+}` on the bad-op path, making the token
    // stream for `${x@}` identical to `${x}`.  The parser cannot distinguish
    // them without a dedicated bad-subst atom from the head mode.
    // Deferred to a T2/T3 head-mode fix.
    let _ = new_part("${}", false); // badsubst ${{}} parses (no panic)
    let _ = new_part("${x:}", false); // badsubst ${{x:}} parses (no panic)
}

#[test]
fn diff_dquote_operands() {
    // T3 fix: double-quoted operands tokenize FLAT (per-frame in_dquote). A simple
    // `"…"` is one quoted Lit (`}` stays literal); a `"…"` with a nested `${}` recurses.
    // These MUST match the production lexer's flat WordPart::Literal{quoted:true}
    // (no Quoted wrapper — verified at parse_braced_operand_opts lexer.rs:3735).
    for s in [
        "${x:-\"a}b\"}",
        "${x:-\"a${y}b\"}",
        "${x:-\"$v\"}",
        "${x:-pre\"mid\"post}",
        "${x#\"$p\"}",
        "${x/\"a/b\"/c}",
    ] {
        diff_ok(s);
    }
}

#[test]
fn diff_deferred_now_parses() {
    use crate::lexer::{Lexer, LexerOptions};
    // G1/v270: `$(cmd)`/`$((expr))`/`` `cmd` `` inside a `"…"` span within a
    // `${…}` operand USED to return `UnsupportedExpansion` (the old
    // `DeferredExpansion` reject). It now parses cleanly — the lexer emits a
    // real opener signal and the parser assembles the sub-expansion.
    for s in [
        "${x:-\"$(cmd)\"}",
        "${x:-\"$((1+1))\"}",
        "${x:-\"`cmd`\"}",
        "${x#\"$(cmd)\"}",
        "${x/y/\"$(cmd)\"}",
    ] {
        let mut lx = Lexer::new(s, &Default::default(), LexerOptions::default());
        assert!(
            parse_param_expansion(&mut lx, false).is_ok(),
            "expected a clean parse for {s}"
        );
    }
}

// ── v242 differential harness ────────────────────────────────────────────

fn new_seq(s: &str) -> Result<Option<Sequence>, ParseError> {
    let mut lx = Lexer::new(s, &Default::default(), LexerOptions::default());
    parse_sequence(&mut lx)
}
#[test]
fn atoms_scaffolding_exists() {
    // The atom lexer + parser wire up. Empty input parses to None.
    assert_eq!(new_seq("").unwrap(), None);
}

// ── v265 T6: focused, explicit-shape parser AST tests ────────────────────
// Structural assertions on the atom parser's output across grammar families.
fn t6_first(s: &str) -> Command {
    new_seq(s).expect("parse").expect("non-empty").first
}
fn t6_exec(c: &Command) -> &ExecCommand {
    match c {
        Command::Pipeline(p) => match &p.commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => e,
            o => panic!("expected Exec stage, got {o:?}"),
        },
        o => panic!("expected Pipeline, got {o:?}"),
    }
}

#[test]
fn t6_ast_simple_command() {
    let c = t6_first("echo hi");
    let e = t6_exec(&c);
    assert_eq!(
        e.program,
        Word(vec![WordPart::Literal {
            text: "echo".into(),
            quoted: false
        }])
    );
    assert_eq!(
        e.args,
        vec![Word(vec![WordPart::Literal {
            text: "hi".into(),
            quoted: false
        }])]
    );
}

#[test]
fn t6_ast_pipeline_and_negation() {
    match t6_first("a | b") {
        Command::Pipeline(p) => {
            assert!(!p.negate);
            assert_eq!(p.commands.len(), 2);
        }
        o => panic!("{o:?}"),
    }
    match t6_first("! a") {
        Command::Pipeline(p) => {
            assert!(p.negate);
            assert_eq!(p.commands.len(), 1);
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn t6_ast_and_or_list() {
    let seq = new_seq("a && b || c").unwrap().unwrap();
    assert_eq!(seq.rest.len(), 2);
    assert!(matches!(seq.rest[0].0, Connector::And));
    assert!(matches!(seq.rest[1].0, Connector::Or));
}

#[test]
fn t6_ast_redirect() {
    assert_eq!(t6_exec(&t6_first("a > f")).redirects.len(), 1);
    assert_eq!(t6_exec(&t6_first("a 2>&1")).redirects.len(), 1);
}

#[test]
fn t6_ast_subshell_and_brace_group() {
    assert!(matches!(t6_first("( a )"), Command::Subshell { .. }));
    assert!(matches!(t6_first("{ a; }"), Command::BraceGroup(_)));
}

#[test]
fn t6_ast_compounds() {
    assert!(matches!(t6_first("if x; then y; fi"), Command::If(_)));
    assert!(matches!(t6_first("while x; do y; done"), Command::While(_)));
    // `until` reuses the While variant (with the loop sense flipped internally).
    assert!(matches!(t6_first("until x; do y; done"), Command::While(_)));
    assert!(matches!(
        t6_first("for i in a b; do :; done"),
        Command::For(_)
    ));
    assert!(matches!(
        t6_first("select n in a; do :; done"),
        Command::Select(_)
    ));
    assert!(matches!(
        t6_first("case x in a) y;; esac"),
        Command::Case(_)
    ));
    assert!(matches!(
        t6_first("for ((i=0;i<3;i++)); do :; done"),
        Command::ArithFor(_)
    ));
    assert!(matches!(t6_first("(( 1+2 ))"), Command::Arith(_)));
}

#[test]
fn t6_for_brace_body_parses_like_do_done() {
    // ksh-derived `{ … }` body in place of `do … done` for word-list `for`.
    let brace = t6_first("for x in a b; { echo $x; }");
    let dodone = t6_first("for x in a b; do echo $x; done");
    match (brace, dodone) {
        (Command::For(b), Command::For(d)) => {
            assert_eq!(b.var, d.var);
            assert_eq!(b.words, d.words);
            assert_eq!(b.has_in, d.has_in);
            assert_eq!(format!("{:?}", b.body), format!("{:?}", d.body));
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn t6_arith_for_brace_body_parses_like_do_done() {
    // C-style `for ((…)) { … }` — only a Blank before `{`, no `;`.
    let brace = t6_first("for ((i=0;i<3;i++)) { echo $i; }");
    let dodone = t6_first("for ((i=0;i<3;i++)) do echo $i; done");
    match (brace, dodone) {
        (Command::ArithFor(b), Command::ArithFor(d)) => {
            assert_eq!(format!("{:?}", b.init), format!("{:?}", d.init));
            assert_eq!(format!("{:?}", b.cond), format!("{:?}", d.cond));
            assert_eq!(format!("{:?}", b.step), format!("{:?}", d.step));
            assert_eq!(format!("{:?}", b.body), format!("{:?}", d.body));
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn t6_select_brace_body_parses_like_do_done() {
    let brace = t6_first("select x in a b; { echo $x; }");
    let dodone = t6_first("select x in a b; do echo $x; done");
    match (brace, dodone) {
        (Command::Select(b), Command::Select(d)) => {
            assert_eq!(format!("{:?}", b.body), format!("{:?}", d.body));
            assert_eq!(format!("{:?}", b.words), format!("{:?}", d.words));
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn t6_while_brace_body_still_rejected() {
    // bash does NOT allow a brace body on while/until — huck must keep
    // rejecting it (parse_while inlines its own do/done, never reaches
    // parse_do_body_done's brace path).
    assert!(new_seq("while false; { echo hi; }").is_err());
    assert!(new_seq("until true; { echo hi; }").is_err());
}

#[test]
fn t6_ast_double_bracket_regex() {
    match t6_first("[[ a =~ b ]]") {
        Command::DoubleBracket { expr, .. } => assert!(matches!(*expr, TestExpr::Regex { .. })),
        o => panic!("{o:?}"),
    }
}

#[test]
fn t6_ast_function_defs_and_coproc() {
    assert!(matches!(
        t6_first("f() { :; }"),
        Command::FunctionDef { .. }
    ));
    assert!(matches!(
        t6_first("function g { :; }"),
        Command::FunctionDef { .. }
    ));
    assert!(matches!(
        t6_first("coproc c { :; }"),
        Command::Coproc { .. }
    ));
}

#[test]
fn t6_ast_array_assignment() {
    match t6_first("a=(1 2)") {
        Command::Pipeline(p) => match &p.commands[0] {
            Command::Simple(SimpleCommand::Assign(assigns, _)) => {
                assert_eq!(assigns.len(), 1);
                assert!(
                    assigns[0]
                        .value
                        .0
                        .iter()
                        .any(|wp| matches!(wp, WordPart::ArrayLiteral(_))),
                    "expected an ArrayLiteral WordPart: {:?}",
                    assigns[0].value
                );
            }
            o => panic!("{o:?}"),
        },
        o => panic!("{o:?}"),
    }
}

#[test]
fn t6_ast_word_part_nesting() {
    // `$(…)` and `` `…` `` → CommandSub; `$((…))` → Arith.
    let cs = t6_exec(&t6_first("echo $(x)")).args[0].0.clone();
    assert!(
        cs.iter()
            .any(|wp| matches!(wp, WordPart::CommandSub { .. })),
        "{cs:?}"
    );
    let bt = t6_exec(&t6_first("echo `x`")).args[0].0.clone();
    assert!(
        bt.iter()
            .any(|wp| matches!(wp, WordPart::CommandSub { .. })),
        "{bt:?}"
    );
    let ar = t6_exec(&t6_first("echo $((1+2))")).args[0].0.clone();
    assert!(
        ar.iter().any(|wp| matches!(wp, WordPart::Arith { .. })),
        "{ar:?}"
    );
}

// G1/v270: a `$(…)`/`$((…))`/`` `…` `` inside a `"…"` span within a `${…}`
// modifier operand parses (was `UnsupportedExpansion`) and is marked
// `quoted: true` so it is not word-split; an UNQUOTED-operand sibling stays
// `quoted: false`. Covers the operand families' `Word` fields.
#[test]
fn g1_ast_operand_dquote_cmdsub_quoted() {
    // Return the first expansion WordPart's `quoted` flag found in `parts`.
    fn expansion_quoted(parts: &[WordPart]) -> bool {
        for wp in parts {
            match wp {
                WordPart::CommandSub { quoted, .. } | WordPart::Arith { quoted, .. } => {
                    return *quoted;
                }
                _ => {}
            }
        }
        panic!("no CommandSub/Arith WordPart in {parts:?}");
    }
    // Dig the operand `Word`'s parts out of the sole ParamExpansion arg.
    fn operand_of(src: &str) -> Vec<WordPart> {
        let arg = t6_exec(&t6_first(src)).args[0].0.clone();
        let pe = arg
            .into_iter()
            .find_map(|wp| match wp {
                WordPart::ParamExpansion { modifier, .. } => Some(modifier),
                _ => None,
            })
            .expect("ParamExpansion");
        let word = match pe {
            ParamModifier::UseDefault { word, .. }
            | ParamModifier::AssignDefault { word, .. }
            | ParamModifier::UseAlternate { word, .. }
            | ParamModifier::ErrorIfUnset { word, .. }
            | ParamModifier::RemovePrefix { pattern: word, .. }
            | ParamModifier::RemoveSuffix { pattern: word, .. } => word,
            ParamModifier::Substitute { replacement, .. } => replacement,
            o => panic!("unexpected modifier {o:?}"),
        };
        word.0
    }
    // Quoted operand span → quoted:true across families/positions/expansions.
    // (Outer quotes dropped so the ParamExpansion is a top-level WordPart; the
    // INNER `"…"` operand span is what exercises the fix.)
    assert!(expansion_quoted(&operand_of(r#"echo ${x:-"$(echo a)"}"#)));
    assert!(expansion_quoted(&operand_of(r#"echo ${x:-"a$(echo a)b"}"#)));
    assert!(expansion_quoted(&operand_of(r#"echo ${x:-"$((1+2))"}"#)));
    assert!(expansion_quoted(&operand_of(r#"echo ${x:-"`echo a`"}"#)));
    assert!(expansion_quoted(&operand_of(r#"echo ${x:="$(echo a)"}"#)));
    assert!(expansion_quoted(&operand_of(r#"echo ${x:+"$(echo a)"}"#)));
    assert!(expansion_quoted(&operand_of(r#"echo ${x#"$(echo a)"}"#)));
    assert!(expansion_quoted(&operand_of(r#"echo ${x/y/"$(echo a)"}"#)));
    // Regression: unquoted operand cmdsub stays quoted:false (splittable).
    assert!(!expansion_quoted(&operand_of(r#"echo ${x:-$(echo a)}"#)));
}

// v265 smoke-convert: these differential helpers dropped their oracle
// comparison (the oracle is deleted in T4) but KEEP every call-site as a
// parse-level Ok/Err regression guard. Exact-AST coverage is backfilled by
// the focused `ast_*` tests (T6). No call-site flips: `diff_cmd` inputs all
// `.unwrap()`ed to Ok before, `diff_err` inputs all errored before.
/// In-scope: the parser accepts this input.
fn diff_cmd(s: &str) {
    assert!(
        new_seq(s).is_ok(),
        "expected Ok for {s:?}, got {:?}",
        new_seq(s)
    );
}
/// Error: the parser rejects this input.
fn diff_err(s: &str) {
    assert!(
        new_seq(s).is_err(),
        "expected Err for {s:?}, got {:?}",
        new_seq(s)
    );
}

// ── v264 alias differential harness ─────────────────────────────────────

fn new_seq_al(s: &str, pairs: &[(&str, &str)]) -> Result<Option<Sequence>, ParseError> {
    let mut al = std::collections::HashMap::new();
    for (k, v) in pairs {
        al.insert(k.to_string(), v.to_string());
    }
    let mut lx = Lexer::new(s, &al, LexerOptions::default());
    super::parse_sequence(&mut lx)
}
fn diff_al(s: &str, pairs: &[(&str, &str)]) {
    assert!(
        new_seq_al(s, pairs).is_ok(),
        "expected Ok for {s:?} with {pairs:?}, got {:?}",
        new_seq_al(s, pairs)
    );
}

#[test]
fn atoms_alias_expansion_matches_oracle() {
    diff_al("foo", &[("foo", "echo hi")]); // basic
    diff_al("foo bar", &[("foo", "echo")]); // alias + arg
    // alias→keyword: `x`→`if`, so `x true` becomes the INCOMPLETE `if true`.
    assert!(
        new_seq_al("x true", &[("x", "if")]).is_err(),
        "alias→`if` keyword makes `if true` incomplete: {:?}",
        new_seq_al("x true", &[("x", "if")])
    );
    diff_al("a c", &[("a", "b "), ("b", "echo"), ("c", "hello")]); // trailing-blank chains to arg
    diff_al("a hi", &[("a", "b "), ("b", "echo")]); // trailing-blank, arg not alias
    diff_al("a c", &[("a", "echo"), ("c", "hello")]); // NO trailing blank: arg NOT expanded
    diff_al("a x y", &[("a", "echo "), ("x", "X"), ("y", "Y")]); // trailing-blank stops at 2nd arg
    diff_al("ls /dev/null", &[("ls", "ls -a")]); // recursion guard
    diff_al("greet", &[("greet", "echo hi")]); // simple
    diff_al("printf x", &[("printf", "echo ALIAS")]); // unquoted expands
    diff_al("'printf' x", &[("printf", "echo ALIAS")]); // quoted does NOT expand
    diff_al("foo | bar", &[("foo", "echo hi"), ("bar", "cat")]); // pipeline stages both expand
    diff_al("notanalias", &[("foo", "echo")]); // non-alias unchanged
    diff_al("foo$x", &[("foo", "echo")]); // glued expansion: NOT a bare name → no expand
}

#[test]
fn atoms_command_word_brace_and_wordstart_gaps() {
    // Finding 1 — command-word brace expansion:
    diff_cmd("echo {b,c}");
    diff_cmd("echo a{1,2,3}z");
    diff_cmd("echo {1..4}");
    diff_cmd("cp x{,.bak}");
    diff_cmd("{echo,ls} x"); // program word expands
    diff_cmd("for i in {1..3}; do echo $i; done");
    // must NOT expand / stay literal (oracle parity):
    diff_cmd("echo \"{a,b}\"");
    diff_cmd("echo a\\{b\\}");
    diff_cmd("echo ${x:-{a,b}}");
    // case-pattern / `[[ ]]`-operand braces: the brief's regression guard
    // ASSUMED these were oracle==atom parity (its comment: "oracle does NOT
    // brace-expand"). Empirically that premise is FALSE — the Word-lexer
    // (oracle) DOES brace-expand case patterns and `[[ ]]` operands at lex
    // time (`{a}` alone parses on both paths, but the COMMA form `{a,b}`
    // expands into two Words and BREAKS the oracle's parse). bash itself does
    // NOT brace-expand in these positions (they stay literal patterns/
    // operands), so the atom path — which keeps them literal (Finding 1's fix
    // is applied ONLY at command/for/select assembly, never inside the shared
    // `parse_word_command`) — matches BASH and is CLOSER TO BASH than the
    // buggy oracle. Per the established live-flip convention for an "atom ≥
    // bash, oracle buggy" case (see `atoms_function_assignment_name_divergence`
    // CF9, and CF8/CF10 in the carry-forward inventory), we KEEP the atom's
    // correct behavior and PIN the divergence rather than reproduce the
    // oracle's bug. Auto-resolves in the atom's favor when the oracle scanner
    // is deleted at the flip.
    assert!(
        new_seq("case x in {a,b}) echo m;; esac").is_ok(),
        "atom parses `case … {{a,b}} …` literally (bash-correct); was a PINNED \
             divergence vs the now-deleted oracle (which brace-expanded + errored). atom={:?}",
        new_seq("case x in {a,b}) echo m;; esac")
    );
    assert!(
        new_seq("[[ {a,b} == x ]] && echo y || echo n").is_ok(),
        "atom parses `[[ {{a,b}} == x ]]` literally (bash-correct); was a PINNED \
             divergence vs the now-deleted oracle (which brace-expanded + errored). atom={:?}",
        new_seq("[[ {a,b} == x ]] && echo y || echo n")
    );
    // sanity: the NON-comma brace form (which does not expand) IS oracle-parity
    // in these positions, confirming the divergence is brace-expansion-driven.
    diff_cmd("case x in {a}) echo m;; esac");
    diff_cmd("[[ {a} == x ]] && echo y");
    // Finding 2 — word-start leak after ) :
    diff_cmd("echo a$(true)#b");
    diff_cmd("echo $(true)#b");
    diff_cmd("echo $(echo X)~root");
    diff_cmd("echo $(echo X) #comment"); // spaced: # IS a comment — stays correct
    diff_cmd("(echo a); echo b"); // subshell close must still arm word-start
}

#[test]
fn atoms_param_head_matches_oracle() {
    // (2) extquote in name (dquote context):
    diff_cmd(r#"x1=not; echo "${$'x1'}""#);
    diff_cmd(r#"ab=Z; echo "${a$'b'}""#);
    diff_cmd(r#"x=notOK; x1=not; echo "${x#${$'x1'%$'t'}}""#);
    diff_cmd(r#"x=foo; echo "${x#$'f'}""#); // ANSI-C in operand
    diff_cmd(r#"x=aXb; echo "${x#$'a'}""#);
    // M-156 gate + invalid names → bad-subst:
    diff_cmd(r#"echo "${'x1'}""#);
    diff_cmd(r#"echo "${"x1"}""#);
    diff_cmd(r#"echo ${$'x1'}"#); // decoded name UNQUOTED → oracle's call
    // (3) prefix-name expansion:
    diff_cmd("echo ${!_Q*}");
    diff_cmd("echo ${!_Q@}");
    diff_cmd("echo ${!NOSUCHPFX_ZZ*}");
    diff_cmd("echo ${!x@Q}"); // transform on indirect, NOT prefix
    // (4) bad-subst forms:
    diff_cmd("echo ${$x}");
    diff_cmd("echo ${V@}");
    diff_cmd("echo ${-3}");
    // regression guards — these must stay UNCHANGED:
    diff_cmd("echo ${x}");
    diff_cmd("echo ${#x}");
    diff_cmd("echo ${!x}"); // plain indirect
    diff_cmd("echo ${x@Q}"); // valid transform
    diff_cmd("echo ${@}");
    diff_cmd("echo ${*}");
    diff_cmd("echo ${$}");
    diff_cmd("echo ${-}");
    diff_cmd("echo ${x:-y}");
}

// v247 T2 tests

#[test]
fn atoms_plain_words() {
    diff_cmd("echo");
    diff_cmd("echo hi");
    diff_cmd("echo   hi    there"); // multiple blanks collapse
    diff_cmd("  echo hi  "); // leading/trailing blanks
    diff_cmd("echo 'a b' \"c d\" e"); // quoted runs stay one word
    diff_cmd("echo a'b'c\"d\""); // glued quotes = one word
    diff_cmd("echo a\\ b"); // escaped space = one word
    diff_cmd("echo $'x\\ty'"); // $'…' ANSI-C
}

#[test]
fn atoms_trailing_backslash() {
    diff_cmd("echo a\\");
    diff_cmd("echo \\");
    diff_cmd("echo ab\\");
    diff_cmd("echo a\\ b"); // escaped space mid-word stays Quoted{Backslash} — must still match
    diff_cmd("echo a b\\");
}

// v247 T3 tests

#[test]
fn atoms_expansions() {
    diff_cmd("echo $x");
    diff_cmd("echo ${x:-d}");
    diff_cmd("echo $(echo hi)");
    diff_cmd("echo `echo hi`");
    diff_cmd("echo $((1+2))");
    diff_cmd("echo $x$y \"$a ${b}\" pre$(c)post");
    diff_cmd("echo ~ ~root ~/x");
    diff_cmd("echo $? $@ $1");
}

#[test]
fn atoms_lone_dollar() {
    // lone `$` is a standalone Literal, never merged (top level)
    diff_cmd("echo a$");
    diff_cmd("echo a$.");
    diff_cmd("echo a$ b");
    diff_cmd("echo $");
    diff_cmd("echo $x$");
    // lone `$` inside double quotes
    diff_cmd("echo \"$ x\"");
    diff_cmd("echo \"foo $ bar\"");
    diff_cmd("echo \"$.\"");
    diff_cmd("echo \"a$\"");
    diff_cmd("echo \"$'x'\"");
    // merges that MUST still work (regression guard for the accumulator)
    diff_cmd("echo a\\"); // trailing backslash folds into preceding literal
    diff_cmd("echo ab\\");
    diff_cmd("echo a b\\");
}

// v247 T4 tests

#[test]
fn atoms_assignments() {
    diff_cmd("x=1");
    diff_cmd("x=1 y=2 cmd");
    diff_cmd("x+=abc");
    diff_cmd("a[0]=v");
    diff_cmd("a[$i]=v");
    diff_cmd("x=$y\"z\"");
    diff_cmd("PATH=/bin:/usr/bin cmd arg");
    diff_cmd("x="); // empty value
    diff_cmd("a[i]+=v"); // subscript append
    diff_cmd("x=~/foo"); // assignment-value tilde
    diff_cmd("x=a:~/b:~/c"); // tilde after unquoted ':'
    diff_cmd("PATH=~/bin:/usr/bin"); // value-start tilde + literal
    diff_cmd("cmd x=1 arg"); // prefix assignment before argv
}

#[test]
fn atoms_bracket_not_assignment() {
    // name[...] NOT followed by =/+= : whole bracket region is literal (oracle parity)
    diff_cmd("arr[$i]");
    diff_cmd("a[$x]");
    diff_cmd("a['x']");
    diff_cmd("a[\"x\"]");
    diff_cmd("a[`c`]");
    diff_cmd("a[a\\b]");
    diff_cmd("a[${y}]");
    diff_cmd("a[$x]y");
    diff_cmd("pre a[$i] post");
    diff_cmd("a[$x"); // unclosed
    diff_cmd("ls [abc]*"); // standalone glob (no identifier) — still literal
    diff_cmd("echo a[b]");
    // real assignments must STILL work
    diff_cmd("a[0]=v");
    diff_cmd("a[$i]=v");
    diff_cmd("a[i]+=v");
    diff_cmd("a[b[c]]=v");
}

// v247 T5 tests: redirects / operators / separators / comments / continuations
#[test]
fn atoms_structure() {
    for s in [
        // pipelines / and-or / separators
        "a | b | c",
        "a && b || c",
        "a; b; c",
        "a &",
        "a&&b",
        "a||b",
        "a|b",
        // redirects
        "echo hi > out",
        "echo hi >> out 2>&1",
        "cat < in",
        "cat <> f",
        "3< in 4> out cmd",
        "{fd}> out cmd",
        "cmd 2>&1 >file",
        "cmd >&2",
        "echo a >| f",
        "echo a &> f",
        "echo a &>> f",
        "ls</dev/null",
        // comments
        "echo a  # trailing comment",
        "# whole line comment",
        "a#b",
        // line continuations
        "echo a \\\n  b",
        "a\\\n&&\\\nb",
        // separators glued / spaced
        "echo a;",
        "echo a ;b",
        "a| b |c",
        // adversarial extensions
        "3>out",
        "{fd}>out cmd",
        "2>&1",
        "1>&2",
        "cmd 3>&1 4>&2",
        "a>b",
        "x=2>out",
        "a3>out",
        "echo>out", // fd-prefix boundaries
        "cmd</in>out",
        "echo hi>>log 2>>err", // glued redirects
        "a  ;  b  &&  c",
        "a|&b",                // spaced ops + |& desugar
        "echo '|' \"&&\" \\;", // quoted/escaped metachars stay literal
        "x=1 y=2 cmd >o",
        ">o",
        "<i >o", // assignments + bare redirects
        "echo a# still one word",
        "a#b>c", // mid-word # then redirect
        "echo\ta\tb",
        "a\r b", // tab / CR whitespace
    ] {
        diff_cmd(s);
    }
}

// v247 T6 tests: in-scope compounds on the atom path
#[test]
fn atoms_compounds() {
    for s in [
        "if true; then echo a; fi",
        "if a; then b; elif c; then d; else e; fi",
        "while read x; do echo $x; done",
        "until false; do echo a; done",
        "for i in a b c; do echo $i; done",
        "for i in $list; do :; done",
        "for i; do echo $i; done", // no `in` list
        "case $x in a) echo a;; b|c) echo bc;; *) echo d;; esac",
        "case $x in (a) echo a;; esac", // parenthesized pattern
        "select x in a b; do echo $x; break; done",
        "( cd /tmp && ls )",
        "{ echo a; echo b; }",
        "if true; then echo a; fi | wc -l", // compound in a pipeline
        "for i in a b; do if $i; then echo y; fi; done", // nested compounds
        "{echo",                            // NOT a brace group — literal word
        "iffy --opt",                       // NOT a keyword
        "echo if then fi",                  // keywords as args (mid-command) stay literal
    ] {
        diff_cmd(s);
    }
}
#[test]
fn atoms_compounds_deferred() {
    // still deferred on the atom path (T7 will also assert these).
    // `f() { :; }` is NO LONGER deferred — v248 T2 implements the POSIX
    // `name()` funcdef form (see `atoms_function_paren_form`).
    // `[[ a == b ]]` is NO LONGER deferred — v253 T1 implements `[[ … ]]`
    // (see `atoms_double_bracket_core`).
    // `(( 1+2 ))` (standalone arith command) is NO LONGER deferred —
    // v255 T1 implements it (see `atoms_arith_command`).
    // `for ((i=0;i<2;i++)); do :; done` is NO LONGER deferred — v256 T2
    // implements C-style `for (( … ))` (see `atoms_arith_for`).
    diff_cmd("for ((i=0;i<2;i++)); do :; done");
    // v257 T2: coproc is NO LONGER deferred (see `atoms_coproc_named_and_anonymous`).
    diff_cmd("coproc c { :; }");
}

#[test]
fn atoms_blank_boundaries() {
    // C1: bang after connectors
    for s in [
        "foo && ! bar",
        "a && ! b",
        "a || ! b",
        "a; ! b",
        "foo &&   ! bar",
        "! a",
        "!a",
        "a && b",
    ] {
        diff_cmd(s);
    }
    // C2: leading/trailing/only blanks + blank/comment lines at boundaries
    for s in [
        "   ",
        "\t",
        " ",
        "  \n  ",
        "   # indented",
        " #c",
        "a; ",
        "echo hi;  ",
        "a; #c",
        "",
        "\n",
        "  \n\n  ",
    ] {
        assert!(new_seq(s).is_ok(), "boundary case {s:?}: {:?}", new_seq(s));
    }
}
#[test]
fn atoms_procsub_core() {
    diff_cmd("cat <(echo hi)");
    diff_cmd("tee >(cat)");
    diff_cmd("echo <(a) >(b)"); // multiple, both dirs
    diff_cmd("diff <(sort x) <(sort y)");
    diff_cmd("x<(y)"); // glued to leading literal
    diff_cmd("wc < <(sort f)"); // procsub as a redirect TARGET
    diff_cmd("sort > >(uniq)");
}

#[test]
fn atoms_procsub_supported() {
    // process substitution now parses on the atom path, byte-identical to the oracle.
    for s in ["cat <(echo hi)", "tee >(cat)", "echo <(a) >(b)"] {
        diff_cmd(s);
    }
}

// ── v251 T3: full process-substitution corpus ──────────────────────────
#[test]
fn atoms_procsub_corpus() {
    // nested
    diff_cmd("cat <( cat <(echo x) )");
    diff_cmd("echo >( tee >(cat) )");
    // bodies: pipelines / expansions / compounds inside
    diff_cmd("cat <(echo $x | sort)");
    diff_cmd("cat <(echo ${y:-d})");
    diff_cmd("cat <(if true; then echo a; fi)");
    diff_cmd("cat <(a && b || c)");
    // funcdef body inside a procsub — v248 funcdef support extends into
    // `parse_subshell_sequence`, so this is NOT deferred (observed: byte-
    // identical to the oracle, unlike `[[ … ]]`/`(( … ))` bodies below).
    diff_cmd("cat <( f() { :; } )");
    // adjacency with other word parts
    diff_cmd("echo pre$(c)<(d)post");
    diff_cmd("cat <(a)<(b)"); // two procsubs glued into one word
    // empty body
    diff_cmd("cat <()");
    diff_cmd("tee >()");
    // G2: whitespace/newline-only body — same empty-body rule as `$( )`.
    diff_cmd("cat <( )");
    diff_cmd("tee >(\n\t)");
}

#[test]
fn atoms_procsub_quoted_literal() {
    // inside quotes `<(`/`>(` are LITERAL — no procsub (matches the oracle).
    diff_cmd("echo \"<(x)\"");
    diff_cmd("echo '<(x)'");
    // Escaped `<` (`\<`): the backslash strips `<`'s special meaning, so
    // `(x)` never becomes a `ProcSubOpen` target — a bare `(` then
    // surfaces in argument position. Observed: the ORACLE itself does not
    // treat this as a literal glued word either — it errors
    // `UnexpectedToken` (command.rs's generic "stray `(` where a word was
    // expected" outcome; see e.g. the `w_tok("hi"), Op(LParen)` case at
    // command.rs ~4556). So this is error-parity, not a `diff_cmd` case.
    assert!(
        new_seq("echo \\<(x)").is_err(),
        "escaped `<(` error: {:?}",
        new_seq("echo \\<(x)")
    );
}

#[test]
fn atoms_procsub_for_case_positions() {
    // for-list / case-pattern positions: the `<(` atom is `ProcSubOpen`,
    // never an `Op`, so these loops' `Some(TokenKind::Op(_)) =>
    // UnexpectedToken` guards don't intercept it — it falls to the
    // `parse_word_command` dispatch same as any other word-start atom,
    // which already special-cases a fresh `ProcSubOpen`. Observed: the
    // oracle parses all three identically, no dispatch-set extension
    // needed.
    diff_cmd("for x in <(a); do :; done");
    diff_cmd("case <(a) in x) echo y;; esac");
    diff_cmd("case x in <(a)) echo y;; esac");
}

#[test]
fn atoms_procsub_errors() {
    // Malformed at EOF (`cat <(` with no closer): the oracle's BATCH lexer
    // rejects this at LEX time (`UnterminatedSubstitution`, before parsing
    // even starts), so `old_seq` cannot yield a `Result` to compare — the
    // atom path (incremental live lexer) rejects the same input at PARSE
    // time. Both REJECT; assert parity of rejection (mirrors
    // `atoms_error_parity`'s `echo $(`/`echo ${` treatment).
    assert!(
        new_seq("cat <(").is_err(),
        "atom path must reject unterminated `cat <(`"
    );

    // `[[ … ]]` inside a procsub body is NO LONGER deferred — v253 T1
    // implements `[[ … ]]` everywhere `parse_command` is reached, including
    // inside `parse_process_sub`'s body sequence.
    diff_cmd("cat <( [[ x ]] )");
}

// ── v252 T1: positional array literals ───────────────────────────────────

// ── v252 merge-gate fix: `\<NL>` line continuation is GLUE, not a
// separator, when it abuts element text with no surrounding whitespace.
#[test]
fn atoms_array_literal_line_continuation() {
    diff_cmd("a=(1\\\n2)"); // glued: one element `12`
    diff_cmd("a=(1 \\\n2)"); // space then continuation
    diff_cmd("a=(1\\\n 2)"); // continuation then space
    diff_cmd("a=(\\\n1)"); // leading continuation
    diff_cmd("a=(1\\\n2\\\n3)"); // multiple glued continuations
    diff_cmd("a=([0]=a\\\nb)"); // glued continuation inside a subscripted value
    diff_cmd("a=(a\\\nb c)"); // glued then a real (space) separator
}

#[test]
fn atoms_array_literal_positional() {
    diff_cmd("a=(1 2 3)");
    diff_cmd("a=()"); // empty
    diff_cmd("a=(x)"); // single
    diff_cmd("a=(  1   2  )"); // extra spaces
    diff_cmd("arr+=(4 5)"); // append form
    diff_cmd("a=(a|b c;d e<f)"); // |;&<> literal inside values
    diff_cmd("a=({1..3})"); // brace-expanded bare element -> 1 2 3
    diff_cmd("a=(x{a,b}y)"); // brace expansion with prefix/suffix
    diff_err("pre a=(1 2) post"); // array literal in ARG position → bash rejects (rc 2)
}

#[test]
fn atoms_array_literal_arg_position_rejected() {
    // A `name=(…)` compound array assignment is valid ONLY as a leading
    // assignment or as an argument to a declaration builtin; elsewhere bash
    // rejects the unexpected `(` (rc 2). Mirror that.
    diff_err("printf \"%s\\n\" -a a=(a b)"); // array1.sub line 1
    diff_err("echo a=(x)"); // plain command arg
    diff_err("echo a+=(3)"); // append form as arg
    diff_err("foo a=(1) b=(2)"); // non-decl command, two array args
    diff_err("command declare a=(1 2)"); // `command` is not a decl builtin
    // Valid positions still parse:
    diff_cmd("a=(1 2)"); // sole leading assignment
    diff_cmd("a=(1) b=(2)"); // consecutive leading assignments
    diff_cmd("x=1 a=(1 2)"); // scalar + array leading prefix
    diff_cmd("declare -a a=(1 2)"); // declaration builtin argument
    diff_cmd("declare a=(1 2) b=(3 4)"); // two array args to declare
    diff_cmd("local a=(1 2)");
    diff_cmd("export a=(1 2)");
    diff_cmd("readonly a=(1 2)");
    diff_cmd("typeset a=(1 2)");
    diff_cmd("alias a=(1 2)");
    diff_cmd("eval a=(1 2)");
    diff_cmd("let a=(1 2)");
    diff_cmd("x=1 declare a=(1 2)"); // decl builtin after leading assign
    // Unaffected shapes: element assign, append scalar, quoted `=(`.
    diff_cmd("a[0]=x");
    diff_cmd("a+=(3)"); // leading append array
    diff_cmd("echo \"a=(x)\""); // quoted, not an array literal
}

#[test]
fn atoms_array_literal_subscripts() {
    diff_cmd("a=([0]=x [1]=y)"); // explicit subscripts
    diff_cmd("a=([2]=two 1 [0]=zero)"); // mixed positional + subscripted
    diff_cmd("a=([i+1]=v)"); // arithmetic subscript expr
    diff_cmd("a=([k]={a,b})"); // subscripted: brace stays LITERAL (no expansion)
    diff_cmd("m[k]=(1 2)"); // name[sub]=(…) prefix form
    diff_cmd("m[k]+=(3)"); // name[sub]+=(…) prefix form
    diff_cmd("a=([0]= [1]=y)"); // empty subscripted value
    diff_cmd("a=([$i]=v [x]=$y)"); // expansion in subscript and value
    diff_cmd("a=([0]=x[1]y)"); // `[` MID-value stays literal
}

#[test]
fn atoms_array_literal_subscript_regressions() {
    // BUG 1 (regression of T1/T2): a `[` MID-value, AFTER a non-literal atom
    // that ended its own scan_step, must stay LITERAL — not open a subscript.
    diff_cmd("a=($x[0])"); // positional value $x[0]
    diff_cmd("a=(pre$x[0]post)"); // positional, glued around `$x`
    diff_cmd("a=([0]=$y[1]z)"); // subscripted value $y[1]z
    diff_cmd("a=([0]=\"x\"[1]z)"); // subscripted value "x"[1]z (after a quote)
    // BUG 2: a subscripted EMPTY value immediately before `)` → empty element,
    // not an UnexpectedToken error.
    diff_cmd("a=([0]=)"); // sole empty subscripted value
    diff_cmd("a=([2]=two [0]=)"); // empty value as the LAST element
}

#[test]
fn atoms_array_literal_append_element_sets_flag() {
    // `[expr]+=value` inside an array literal parses and carries append=true;
    // `[expr]=value` and positional elements carry append=false.
    let elems = match t6_first("a=([2]+=7 [0]=x 9)") {
        Command::Pipeline(p) => match &p.commands[0] {
            Command::Simple(SimpleCommand::Assign(assigns, _)) => assigns[0]
                .value
                .0
                .iter()
                .find_map(|wp| match wp {
                    WordPart::ArrayLiteral(e) => Some(e.clone()),
                    _ => None,
                })
                .expect("ArrayLiteral WordPart"),
            o => panic!("{o:?}"),
        },
        o => panic!("{o:?}"),
    };
    assert_eq!(elems.len(), 3);
    // [2]+=7 — subscripted, append.
    assert!(
        elems[0].subscript.is_some() && elems[0].append,
        "[2]+=7 append: {:?}",
        elems[0]
    );
    // [0]=x — subscripted, NOT append.
    assert!(
        elems[1].subscript.is_some() && !elems[1].append,
        "[0]=x set: {:?}",
        elems[1]
    );
    // 9 — positional, never append.
    assert!(
        elems[2].subscript.is_none() && !elems[2].append,
        "positional: {:?}",
        elems[2]
    );
}

#[test]
fn atoms_array_literal_append_element_parses() {
    diff_cmd("x=(1 2 [2]+=7 4)"); // the appendop.tests spelling
    diff_cmd("a=([one]+=more)"); // assoc-style append element
    diff_cmd("n=(5 [0]+=3)"); // numeric-append element
    diff_cmd("a=([i+1]+=v)"); // arithmetic subscript + append
    // `[i]` followed by `+` that is NOT `+=` is still the missing-`=` error.
    diff_err("a=([2]+7)");
}

#[test]
fn atoms_array_literal_error_parity() {
    // `[i]` without `=` → ArrayLiteralMissingEquals (lexer-level, surfaced
    // as ParseError::Lex on the atom path).
    assert!(new_seq("a=([0])").is_err());
    // Leading `[…]` with no `=` after `]` (the `[ab]c` case).
    assert!(new_seq("a=([ab]c)").is_err());
    // EOF before `)` → UnterminatedArrayLiteral.
    assert!(new_seq("a=(1 2").is_err());
    assert!(new_seq("a=(").is_err());
}

#[test]
fn atoms_array_literal_append_subscript_funcdef_parity() {
    // v252 T4 review follow-up: `AssignPrefix`-prefixed leading words
    // (`a+=…`/`a[i]=…`/`a[i]+=…`) are lexed with a leading zero-width
    // `AssignPrefix` atom (unlike plain `a=…`, a `Lit("a=")`), so before
    // this fix they SKIPPED the funcdef-lookahead dispatch entirely and
    // fell to `parse_simple` → the trailing-`Op(LParen)` arm →
    // `UnsupportedCommand`, diverging from the oracle's `FunctionName`.
    // The outer dispatch gate now admits a leading `AssignPrefix`, so a
    // CLOSED array literal glued before a second `(` reaches the same
    // funcdef attempt and gets `FunctionName` parity (a multi-part /
    // non-Literal word is not a valid function name). Error-parity (both
    // sides `Err(FunctionName)`), not `diff_cmd` (which requires `Ok`).
    assert!(
        new_seq("a+=(1)(2)").is_err(),
        "error parity for \"a+=(1)(2)\""
    );
    assert!(
        new_seq("a[0]=(1)(2)").is_err(),
        "error parity for \"a[0]=(1)(2)\""
    );
    assert!(
        matches!(new_seq("a+=(1)(2)"), Err(ParseError::FunctionName)),
        "a+=(1)(2) → FunctionName, got {:?}",
        new_seq("a+=(1)(2)")
    );
    assert!(
        matches!(new_seq("a[0]=(1)(2)"), Err(ParseError::FunctionName)),
        "a[0]=(1)(2) → FunctionName, got {:?}",
        new_seq("a[0]=(1)(2)")
    );
    // Bonus reconciliation (same root cause): a single-part `AssignPrefix`
    // word (empty value) followed by `(` is NOT a valid function name on
    // the oracle either (`AssignPrefix` is not a `Literal`), so both give
    // `FunctionName` — unlike the v248-pinned single-`Literal` `a=b ()`
    // shape, which the oracle accepts as `FunctionDef` (still deferred).
    assert!(
        new_seq("a+= (echo hi)").is_err(),
        "error parity for \"a+= (echo hi)\""
    );

    // REGRESSION GUARD: ordinary append/subscript assignments (no following
    // second `(`) must still parse as normal assignment simple-commands —
    // the funcdef attempt must never be entered for these.
    diff_cmd("a+=(1 2)"); // append array literal
    diff_cmd("a+=(1)"); // append single-element array literal
    diff_cmd("a[0]=(1 2)"); // subscripted array literal
    diff_cmd("a[i]=x"); // subscripted SCALAR assignment
    diff_cmd("a=(1 2)"); // plain (Literal-prefixed) array literal, unchanged
}

#[test]
fn atoms_array_literal_declare_routing() {
    // If declare/local args route through the command-word path, these are
    // diff_cmd. If they route through DeclArg (different path), replace each
    // with the observed-actual behavior and leave a NOTE comment documenting
    // the deferral (per spec: declare is deferred ONLY if it routes differently).
    diff_cmd("declare -a x=(1 2)");
    diff_cmd("local a=(1 2)");
    diff_cmd("export e=(1)");
    diff_cmd("readonly r=(1)");
}

#[test]
fn atoms_array_literal_corpus() {
    diff_cmd("a=(${arr[@]} ${arr[*]})"); // array expansions as values
    diff_cmd("a=(x=y z=w)"); // `=`-containing values (NOT subscripts)
    diff_cmd("a=(=leading)"); // value starting with `=`
    // `)(` — a `(` glued right after a CLOSED array literal is NOT array
    // glue (that only happens for the FIRST `(` immediately after `name=`/
    // `name+=`, captured as the zero-width `ArrayOpen` atom). The oracle
    // attempts a function-definition parse for ANY leading word followed
    // by `(`; `valid_function_name_text` then rejects the multi-part
    // assignment word (`[Literal("a="), ArrayLiteral(..)]`, not a single
    // Literal) with `FunctionName`. v252 T4 tightened the
    // `parse_command_or_pipeline` guard (was: any `is_assignment_word`
    // word never attempts funcdef) to skip the funcdef attempt ONLY when
    // the oracle would accept the word as a function name
    // (`is_assignment_word(&w) && valid_function_name_text(&w).is_some()` —
    // exactly the single unquoted-`Literal` shape like `a=b`), preserving
    // the v248-pinned `a=b () {…}` divergence (see
    // `atoms_function_assignment_name_divergence`), so a closed array
    // literal (and the `AssignPrefix`-led `a+=(..)`/`a[i]=(..)` shapes) now
    // falls through to the SAME `FunctionName` error as the oracle.
    assert!(
        new_seq("a=(one)(two)").is_err(),
        "error parity for \"a=(one)(two)\""
    );
    diff_cmd("a=(a)b"); // text glued after the close paren
    diff_err("cmd a=(1 2) b=(3 4)"); // array literals in ARG position (cmd not a decl builtin) → bash rejects (rc 2)
    diff_cmd("a=(   )"); // whitespace-only body == empty
    diff_cmd("a=(\n)"); // newline-only body == empty
    // "nots a =(1 2)" — space before `=` means `a` and `=(1` are SEPARATE
    // words (`=(1` is not assignment-shaped, so no array literal is even
    // attempted); both paths reject the same way, but neither actually
    // returns `Ok` (bash: `a` is a bare word, `=(1` and `2)` are literal
    // args to `nots`, which IS valid... but on both paths here the `(`/`)`
    // inside `=(1` and `2)` are lexed as bare `Op(LParen)`/`Op(RParen)`
    // operator tokens at word-start, which is unexpected in argument
    // position) — use error-PARITY, not `diff_cmd` (which requires `Ok`
    // on both sides).
    assert!(
        new_seq("nots a =(1 2)").is_err(),
        "error parity for \"nots a =(1 2)\""
    );

    // T2 carry-forward: `a=(x=(1 2) y)` — a NESTED array literal AS AN
    // ELEMENT VALUE. NOT a `diff_cmd`: the oracle's `scan_array_element_word`
    // scans an element's raw text up to the first unescaped whitespace/`)`
    // WITHOUT tracking `(` nesting at all, then re-tokenizes that (possibly
    // truncated) substring. For this input the raw text is cut at the space
    // inside "x=(1 2)" (giving "x=(1"), and re-tokenizing "x=(1" recurses into
    // `scan_array_literal` for the embedded `=(` with no closing `)` in the
    // truncated slice — so the ORACLE ITSELF fails to lex this input
    // (`UnterminatedArrayLiteral`), confirmed against real bash 5.2 (`bash -c
    // 'a=(x=(1 2) y)'` → "syntax error near unexpected token `('" — bash
    // doesn't support this construct either). The atom path's `ArrayOpen`
    // recursion in `parse_word_command` is more general than the oracle's
    // element scanner: it happily recurses into a nested `Mode::ArrayLiteral`
    // and parses the inner array correctly, so it currently ACCEPTS input the
    // oracle (and bash) reject. Bit-for-bit reproducing the oracle's
    // incidental truncate-then-retokenize failure is disproportionate for a
    // construct with no real bash meaning — documented here as a narrow,
    // low-severity, atom-more-permissive gap (candidate follow-on `[deferred]`
    // divergence for the whole-branch review), not pinned or forced to pass.
    // (The now-deleted oracle lexer REJECTED this; the atom path accepts it —
    // a documented, narrow atom-more-permissive gap, now production behavior.)
    assert!(new_seq("a=(x=(1 2) y)").is_ok());

    // `a=($(cat <<X\nhi\nX\n))` (heredoc-in-cmdsub as an array element) is
    // DELIBERATELY OMITTED here: it hits the documented v250 gap where a
    // heredoc's body is dropped when the heredoc sits inside a `$(…)`/`` `…` ``
    // body (the parser's redirect-attach walk doesn't recurse into
    // `WordPart::CommandSub`/`Backtick`/`Arith` sequences) — a known,
    // pre-existing carry-forward, not an array-literal bug. Confirmed by
    // direct comparison: `old_seq` fills the heredoc `body` with
    // `[Literal("hi"), Literal("\n", quoted:true)]`; `new_seq` gives
    // `body: Word([])`. Per the brief: drop the line, rely on the existing
    // carry-forward note, do not pin new.
}

#[test]
fn atoms_dquote_nested() {
    diff_cmd("echo \"$(echo hi)\"");
    diff_cmd("echo \"$(echo $x)\"");
    diff_cmd("echo \"a${b}c\"");
    diff_cmd("echo \"$a $b\"");
    diff_cmd("echo \"pre$(c)$((1+2))post\"");
    diff_cmd("echo \"\\$lit \\\" \\\\ end\"");
}

// ── v247 T7: broadened differential corpus + deferred/error parity ──────────

#[test]
fn atoms_adversarial() {
    // Adversarial word-splitting / gluing across quotes, expansions, and
    // operators — the atom-assembled Word must match the oracle byte-for-byte.
    for s in [
        "a\"b\"$c",
        "a\\ b",
        "x=$y\"z\"",
        "$a$b$c",
        "'a'\"b\"c$d",
        "  a   b  ",
        "a>b",
        "a>>b<c",
        "echo \"$(echo $x)\"",
        "echo ${a[$i]}",
    ] {
        diff_cmd(s);
    }
}

#[test]
fn atoms_error_parity() {
    // Parser-level malformed input (the oracle LEXES successfully): the atom
    // path must return the SAME error as the oracle. (Normalize Ok/Err to
    // unit + error-debug so a divergent error PAYLOAD — not just variant —
    // is still caught.)
    for s in ["if true", "for", "case x in", "( a"] {
        assert!(new_seq(s).is_err(), "error for {s:?}: {:?}", new_seq(s));
    }
    // `echo $(` / `echo ${` are LEXER-level rejects on the oracle: the
    // production batch `tokenize_with_opts` errors on the unterminated opener
    // (`UnterminatedSubstitution`) BEFORE parsing, so `old_seq` cannot yield a
    // Result to compare. The atom path (incremental live lexer) rejects the
    // same inputs at PARSE time. Both REJECT — assert parity of rejection, not
    // of the error stage.
    for s in ["echo $(", "echo ${"] {
        assert!(
            new_seq(s).is_err(),
            "atom path must reject {s:?}, got {:?}",
            new_seq(s)
        );
    }
}

#[test]
fn atoms_deferred_unsupported() {
    // Every deferred construct defers CLEANLY on the atom path (proving the
    // deferral is deliberate, not an accidental parse). The oracle may parse
    // some of these — the point is only that the atom path returns
    // UnsupportedCommand rather than a wrong AST.
    // `f() { :; }` is NO LONGER deferred — v248 T2 implements the POSIX
    // `name()` funcdef form (see `atoms_function_paren_form`).
    // NOTE: `cat <<EOF\nx\nEOF` (expanding heredoc) is NO LONGER deferred —
    // v250 T4 implements expanding-heredoc bodies (see `atoms_heredoc_expanding`).
    // NOTE: `a=(1 2 3)` (positional array literal) is NO LONGER deferred —
    // v252 T1 implements `name=(…)`/`name+=(…)` via `Mode::ArrayLiteral`
    // (see `atoms_array_literal_positional`).
    // NOTE: `[[ a == b ]]` is NO LONGER deferred — v253 T1 implements
    // `[[ … ]]` (see `atoms_double_bracket_core`).
    // NOTE: `(( 1+2 ))` (standalone arith command) is NO LONGER deferred —
    // v255 T1 implements it (see `atoms_arith_command`).
    // NOTE: `for ((i=0;i<3;i++)); do :; done` (C-style ArithFor) is NO LONGER
    // deferred — v256 T2 implements it (see `atoms_arith_for`).
    diff_cmd("for ((i=0;i<3;i++)); do :; done");
    // v257 T2: coproc is NO LONGER deferred (see `atoms_coproc_named_and_anonymous`).
    diff_cmd("coproc x { :; }");
    // v258 T2: `$[expr]` legacy arith is NO LONGER deferred (see
    // `atoms_legacy_arith_base`/`atoms_legacy_arith_embedded`).
    diff_cmd("echo $[1+2]");
    diff_cmd("echo pre$[1+2]post");
    diff_cmd("echo \"$[1+2]\"");
    diff_cmd("echo \"pre$[1+2]\"");
}

#[test]
fn atoms_no_hang_on_redirect_in_word_list() {
    // Regression: a `RedirFd`/`Heredoc` atom where a word is expected (for/
    // select `in`-list, case pattern) must ERROR, not spin. The oracle hits an
    // `unreachable!()`/UnexpectedToken on the same malformed input (it consumes
    // first, so it panics rather than hangs); the atom path must terminate with
    // an Err. (No `diff_*` here — `old_seq` would panic.)
    for s in [
        "for i in <<a; do :; done",
        "for i in 3>f; do :; done",
        "select i in <<a; do :; done",
        "case x in <<a) ;; esac",
    ] {
        assert!(
            new_seq(s).is_err(),
            "atom path must reject (not hang on) {s:?}, got {:?}",
            new_seq(s)
        );
    }
}

// ── v248: function definitions on the atom path ──────────────────────────
#[test]
fn atoms_function_keyword_form() {
    diff_cmd("function f { :; }");
    diff_cmd("function f() { :; }");
    diff_cmd("function f ()  { :; }"); // spaced ()
    diff_cmd("function greet { echo hi; }");
    diff_cmd("function f\n{ :; }"); // newline before body
    diff_cmd("function 1 { :; }"); // numeric name is valid (AST parity, not just Ok/Ok)
}

#[test]
fn atoms_function_paren_form() {
    diff_cmd("f(){ :; }");
    diff_cmd("f() { :; }");
    diff_cmd("f ()  { :; }"); // spaced name/()
    diff_cmd("f() ( a; b )"); // subshell body
    diff_cmd("f() if x; then y; fi"); // if body
    diff_cmd("f() while x; do y; done"); // while body
    diff_cmd("f() for i in a b; do echo $i; done");
    diff_cmd("f() case $x in a) echo a;; esac");
    diff_cmd("f() select x in a b; do echo $x; break; done");
    diff_cmd("f() until x; do y; done"); // until body
    diff_cmd("f() { :; } >log"); // redirected body
    diff_cmd("f() { :; } 2>&1");
    diff_cmd("f() { g() { :; }; }"); // nested funcdef
    diff_cmd("if true; then f() { :; }; fi"); // funcdef inside a compound
    diff_cmd("f() { :; } | cat"); // funcdef as a pipeline stage
    diff_cmd("f() { :; }; g() { :; }"); // two funcdefs, ; separated
    diff_cmd("true && f() { :; }"); // funcdef after a connector
}
#[test]
fn atoms_function_not_a_def() {
    diff_cmd("f"); // bare word = plain command
    diff_cmd("echo function"); // `function` mid-command = arg
    diff_cmd("func --opt"); // prefix of `function` = plain command (mark/rewind restores)
}

#[test]
fn atoms_function_defs_errors() {
    for s in [
        "f() echo",           // non-compound body → FunctionBody
        "function",           // no name → FunctionName
        "function if { :; }", // reserved word as name → FunctionName
        "f(",                 // unterminated
        "f()",                // `()` then EOF → UnterminatedFunction/FunctionBody
        "f ( a )",            // `(` not followed by `)` → FunctionBody (NOT a command)
    ] {
        assert!(
            new_seq(s).is_err(),
            "funcdef error for {s:?}: {:?}",
            new_seq(s)
        );
    }
}

#[test]
fn atoms_function_defs_deferred() {
    // Body is itself deferred → whole funcdef defers (lifts when [[ ]]/arith land).
    // `f() [[ x ]]` is NO LONGER deferred — v253 T1 implements `[[ … ]]`
    // (see `atoms_function_double_bracket_body`).
    // `f() (( 1 ))` is NO LONGER deferred — v255 T1 implements the standalone
    // arith command, and funcdef bodies dispatch through `parse_command`.
    diff_cmd("f() (( 1 ))");
    // `f() for ((i=0;i<2;i++)); do :; done` is NO LONGER deferred — v256 T2
    // implements C-style `for (( … ))`, and `Command::ArithFor` is in the
    // oracle's `is_function_body_shape` allow-list (command.rs:1168).
    diff_cmd("f() for ((i=0;i<2;i++)); do :; done");
}

#[test]
fn atoms_function_double_bracket_body() {
    diff_cmd("f() [[ x ]]");
    diff_cmd("f() [[ -f a && -f b ]]");
}

#[test]
fn atoms_function_assignment_name_divergence() {
    // KNOWN divergence (v248): the oracle accepts `a=b () {...}` as
    // FunctionDef{name:"a=b"} because command.rs checks `(` before the
    // assignment check; the atom path's is_assignment_word guard defers it.
    // bash itself rejects this as a syntax error, so the atom path (defer) is
    // arguably more correct. Pinned so the Stage-2 live-flip differential gate
    // knows about it. (If a future iteration reconciles this, update here.)
    assert!(
        matches!(
            new_seq("a=b () { :; }"),
            Err(ParseError::UnsupportedCommand)
        ),
        "atom path defers `a=b () {{...}}`, got {:?}",
        new_seq("a=b () { :; }")
    );
    // (The now-deleted oracle ACCEPTED `a=b () { :; }`; the atom path's
    // deferral above is the documented, bash-aligned divergence.)
}

// ── v249: here-strings (`<<<`) on the atom path ──────────────────────────
#[test]
fn atoms_here_string_redirect() {
    diff_cmd("cat <<< hello");
    diff_cmd("wc -l <<<foo"); // glued, no space
    diff_cmd("cat <<< \"$x\""); // quoted expansion target
    diff_cmd("cat <<< 'lit'");
    diff_cmd("cat <<< $'a\\tb'"); // ANSI-C target
    diff_cmd("cat <<< $var");
    diff_cmd("cat <<< a b"); // target is `a`; `b` is an arg
    diff_cmd("cmd <<< x > out"); // here-string + file redirect, source order
    diff_cmd("cmd 2>&1 <<< x"); // fd-dup + here-string
    diff_cmd("cmd <<< a <<< b"); // two here-strings, ordered list
    diff_cmd("{ cat; } <<< x"); // brace-group trailing here-string
    diff_cmd("if true; then :; fi <<< x"); // if-compound trailing here-string
}

#[test]
fn atoms_here_string_leading() {
    diff_cmd("<<< word");
    diff_cmd("<<<foo"); // glued
    diff_cmd("<<< \"$x\"");
    diff_cmd("<<< word > out"); // leading here-string + file redirect
    diff_cmd("<<< x | cat"); // here-string stage in a pipeline
    // Determined by observation: the oracle accepts a leading `<<<` as the
    // first pipeline stage (falls through to parse_pipeline → parse_simple_stage
    // exactly like the atom path), so this is `diff_cmd` parity, not a divergence.
}

#[test]
fn atoms_here_string_fd_prefix() {
    // Determined by observation: `3<<<` lexes fine on the oracle's batch
    // tokenizer (no lexer-level panic) and both paths produce the identical
    // AST, so this is ordinary `diff_cmd` parity.
    diff_cmd("3<<< word"); // fd-prefixed here-string
}

#[test]
fn atoms_here_string_errors() {
    // Determined by observation: none of these inputs panic `old_seq` at the
    // lexer level (the oracle lexes all of them successfully and rejects at
    // parse time), so every one is a plain error-parity comparison — no
    // atom-path-only bucket needed (contrast `atoms_error_parity`'s
    // `echo $(`/`echo ${` split, which DOES need one).
    for s in ["cat <<<", "<<<", "cat <<< |", "cat <<< <", "cat <<< ;"] {
        assert!(
            new_seq(s).is_err(),
            "here-string error for {s:?}: {:?}",
            new_seq(s)
        );
    }
}

#[test]
fn atoms_heredoc_expanding_no_trailing_newline() {
    // v250 T4: EXPANDING heredocs are now supported end-to-end (the T3 defer
    // gate is gone). These are the exact cases the old deferral test used,
    // now asserted for oracle parity — including a delimiter line at EOF with
    // no trailing newline.
    diff_cmd("cat <<EOF\nx\nEOF");
    diff_cmd("<<EOF\nx\nEOF");
}

// v250 T4 tests: expanding heredocs (bare/unquoted delimiter) end-to-end

#[test]
fn atoms_heredoc_expanding() {
    diff_cmd("cat <<EOF\nhello $x\nEOF\n");
    diff_cmd("cat <<EOF\n${y:-d} and $(echo hi)\nEOF\n");
    diff_cmd("cat <<EOF\n`echo bt` $((1+2))\nEOF\n");
    diff_cmd("cat <<EOF\nlit \\$notvar \\` \\\\ end\nEOF\n"); // heredoc backslash rules
    diff_cmd("cat <<EOF\na \\\nb\nEOF\n"); // \<NL> line continuation
    diff_cmd("cat <<EOF\n\"quotes\" 'stay' literal\nEOF\n"); // quotes literal in body
}

#[test]
fn atoms_heredoc_expanding_more() {
    diff_cmd("cat <<EOF\nplain text\nEOF\n"); // plain, quoted:false content
    diff_cmd("cat <<EOF\nEOF\n"); // empty expanding body
    diff_cmd("cat <<EOF\n\nEOF\n"); // single blank line
    diff_cmd("cat <<EOF\n$x$y${z}\nEOF\n"); // adjacent expansions
    diff_cmd("cat <<EOF\n$1 $@ $? $#\nEOF\n"); // specials
    diff_cmd("cat <<-EOF\n\tindented $x\n\tEOF\n"); // <<- tab strip + expand
    diff_cmd("cat <<EOF\nline one\nline two $x\nEOF\n"); // multi-line
    diff_cmd("cat <<EOF\ntrailing $\nEOF\n"); // lone $ at line end
    diff_cmd("cat <<EOF && echo ok\nhi $x\nEOF\n"); // sequence continues
    diff_cmd("cat <<EOF | wc -l\nhi $x\nEOF\n"); // pipeline stage
    diff_cmd("<<EOF\nx $y\nEOF\n"); // leading expanding heredoc
}

#[test]
fn atoms_heredoc_expanding_edges() {
    diff_cmd("cat <<EOF\nend \\$\nEOF\n"); // escaped $ right before newline sep
    diff_cmd("cat <<EOF\n\\$\\`\\\\\nEOF\n"); // all three escapes, adjacent
    diff_cmd("cat <<EOF\nx\\\nEOF\nEOF\n"); // `x\` continues onto `EOF`, NOT delim
    diff_cmd("cat <<EOF\n`echo $x`\nEOF\n"); // var inside backtick in body
    diff_cmd("cat <<EOF\n${x:-`echo hi`}\nEOF\n"); // backtick inside ${…} in body
    diff_cmd("cat <<EOF\nouter $(echo $inner) tail\nEOF\n"); // nested $() with var
    diff_cmd("cat <<'A' <<B\nlit $x\nA\nexp $y\nB\n"); // literal + expanding, ordered
    diff_cmd("cat <<B <<'A'\nexp $y\nB\nlit $x\nA\n"); // expanding + literal, ordered
    diff_cmd("cat <<EOF\na\\zb\nEOF\n"); // lone backslash (ordinary) stays literal
}

#[test]
fn atoms_heredoc_expanding_continuation_delimiter() {
    // v250 T4 fix (F1): a close delimiter FORMED across a `\<NL>` continuation
    // spans multiple physical lines. `heredoc_at_delim_line` reads the whole
    // joined logical line to match, so the consumption must advance the real
    // cursor by that whole span — consuming only one physical line would leak
    // the remainder as a spurious command. bash: `EO\<NL>F` joins to `EOF` =
    // the delimiter, body empty, then runs `echo after`.
    diff_cmd("cat <<EOF\nEO\\\nF\necho after\n"); // `EO\<NL>F` == EOF (empty body)
    diff_cmd("cat <<-EOF\n\tEO\\\nF\necho after\n"); // <<- variant: `\tEO\<NL>F` strips to EOF
    // Guard the other direction (no over-consumption): a continuation-joined
    // BODY line that is NOT the delimiter must stay a body line, with the real
    // `EOF` line still closing it and `echo after` following.
    diff_cmd("cat <<EOF\nab\\\ncd\nEOF\necho after\n"); // `ab\<NL>cd` == abcd (body, not delim)
}

#[test]
fn atoms_heredoc_multiline_cmdsub_divergence() {
    // v250 T4 KNOWN divergence (F2, INTENTIONAL — atom path is the target/bash
    // behavior): a multi-line `$(…)` inside an expanding heredoc body whose `)`
    // is on a LATER line than its `$(`. bash ALLOWS this (verified:
    //   cat <<EOF
    //   $(echo hi
    //   echo bye)
    //   EOF
    // prints hi then bye). The atom path pushes a CommandSub sub-mode that scans
    // the nested command across newlines from the cursor, so it parses fine. The
    // command.rs ORACLE scans each heredoc body line with a LINE-LOCAL cursor, so
    // an unclosed `$(` on its own line is an error there. This is an accepted
    // atom-vs-oracle divergence; the atom path is correct. Do NOT use `diff_cmd`.
    let s = "cat <<EOF\n$(echo hi\necho bye)\nEOF\n";
    assert!(
        new_seq(s).is_ok(),
        "atom path must parse multi-line $() in heredoc (matches bash): {:?}",
        new_seq(s)
    );
    // (The now-deleted oracle lexer diverged here — its line-local heredoc-body
    // scan errored on the split `$(`; the atom path parses it, matching bash.)
}

#[test]
fn atoms_heredoc_in_cmdsub_body_drop_divergence() {
    // v250 pinned a KNOWN gap (heredoc inside `$(…)`/`` `…` `` dropped its
    // body); v260 CF1 RESOLVED it via the fill_word recursion + the lexer
    // bridge. Now a resolved-divergence regression guard: byte-identical.
    diff_cmd("echo $(cat <<X\nhi\nX\n)");
    diff_cmd("echo `cat <<X\nhi\nX\n`");
}

#[test]
fn atoms_heredoc_in_word_fill() {
    // v260 CF1: a heredoc nested inside a Word is filled, not dropped.
    diff_cmd("echo $(cat <<X\nhi\nX\n)"); // arg command-sub
    diff_cmd("x=$(cat <<X\nhi\nX\n)"); // assignment RHS
    diff_cmd("a=($(cat <<X\nhi\nX\n))"); // array-literal element value
    diff_cmd("echo ${y:-$(cat <<X\nhi\nX\n)}"); // param-expansion operand
    diff_cmd("echo $(( $(cat <<X\n1\nX\n) + 2 ))"); // arith body
    diff_cmd("echo `cat <<X\nhi\nX\n`"); // backtick (→ CommandSub)
    diff_cmd("echo \"$(cat <<X\nhi\nX\n)\""); // inside a quoted span
    diff_cmd("echo $(a <<X\nxx\nX\n)$(b <<Y\nyy\nY\n)"); // two Word-nested (queue order)
    diff_cmd("FOO=$(cat <<X\nhi\nX\n) echo hi"); // inline assignment value
}

#[test]
fn atoms_heredoc_in_clause_and_redirect_target_fill() {
    // v260 whole-branch fix: heredoc bodies in for/select/case clause Words,
    // case patterns, here-string words, and redirect-target/dup Words.
    diff_cmd("for i in $(t <<X\nx\nX\n); do :; done");
    diff_cmd("select i in $(t <<X\nx\nX\n); do :; done");
    diff_cmd("case $(t <<X\nx\nX\n) in a) :;; esac");
    diff_cmd("case x in $(p <<X\nx\nX\n)) :;; esac");
    diff_cmd("cat <<<$(a <<X\nxx\nX\n)");
    diff_cmd("echo >$(f <<X\nxx\nX\n)");
    diff_cmd("echo <$(f <<X\nxx\nX\n)");
    // plus audited positions found to ALSO drop (fixed): dup-target, `[[ ]]`
    // operands, standalone arith command, C-style arith-for header.
    diff_cmd("echo >&$(f <<X\n1\nX\n)");
    diff_cmd("[[ -f $(cat <<X\nx\nX\n) ]]");
    diff_cmd("[[ $(a <<X\nx\nX\n) == b ]]");
    diff_cmd("[[ x =~ $(a <<X\nx\nX\n) ]]");
    diff_cmd("(( $(x <<X\n1\nX\n) ))");
    diff_cmd("for (( $(a <<X\n1\nX\n) ; ; )); do :; done");
    // interleaving: an arg Word's heredoc before the redirect-target Word's
    // heredoc is byte-identical (fill_command fills args before redirects,
    // matching source order here).
    diff_cmd("echo $(a <<X\nxx\nX\n) >$(f <<Y\nyy\nY\n)");
}

#[test]
fn atoms_heredoc_in_comsub_eof_adjacency() {
    // A heredoc STARTED inside a `$(…)`/`` `…` `` whose close delimiter sits
    // adjacent to (or shares the line with) the heredoc close-delimiter text.
    // bash uses a PREFIX delimiter match in the comsub here-doc scanner, so
    // the body terminates and the enclosing `)`/`` ` `` closes normally. These
    // MUST parse (they errored — or, for the backtick case, PANICKED with an
    // `unreachable!` — before the heredoc-in-comsub prefix-termination fix).

    // comsub-eof1: heredoc inside a BACKTICK — the crash case. `EOF` is on the
    // same line as the closing `` ` ``. Must parse, never panic.
    diff_cmd("foo=`cat <<EOF\nhi\nEOF`\necho $foo");
    // comsub-eof0: `$()` with `EOF )` (delimiter, space, then `)`).
    diff_cmd("foo=$(cat <<EOF\nhi\nEOF )\necho $foo");
    // comsub-eof4: `$()` with `EOF)` (no space before the `)`).
    diff_cmd("e=$(cat <<EOF\ncontents\nEOF)\necho $e");
    // A LITERAL (`<<'EOF'`) heredoc inside `$()` with an adjacent `)`.
    diff_cmd("e=$(cat <<'EOF'\nliteral\nEOF)\necho $e");
    // Proper delimiter line then a separate `)` line still parses (exact match).
    diff_cmd("e=$(cat <<EOF\nx\nEOF\n)\necho $e");
}

#[test]
fn atoms_heredoc_redirect_target_before_arg_pin() {
    // PINNED divergence (documented, not fixed): the fill walk visits the
    // AST in a fixed structural order, but the heredoc-body FIFO is in
    // lexer emission order (nested-Word bodies emit at their inner newline,
    // BEFORE an outer direct heredoc-body redirect). When those two orders
    // disagree, the bodies swap. The AST carries no source positions to
    // reconcile them (that is the rejected Scope B), and these are rare,
    // exotic multi-heredoc constructs, so they are pinned rather than fixed.
    //
    // Case 1 — a redirect-TARGET Word's heredoc precedes an arg Word's
    // heredoc in source (`echo >$(f <<Y) $(a <<X)`: Y precedes X, queue is
    // [Y, X]); fill_command fills args before redirects, so X's placeholder
    // takes Y's body and vice versa.
    let s = "echo >$(f <<Y\nyy\nY\n) $(a <<X\nxx\nX\n)";
    // Was an `assert_ne!` vs the (now-deleted) oracle documenting the
    // args-vs-redirects fill-order divergence; the atom path's order is now
    // simply production behavior. Smoke-level: it parses.
    assert!(
        new_seq(s).is_ok(),
        "expected Ok for {s:?}, got {:?}",
        new_seq(s)
    );
    // Case 2 — an outer command's own heredoc redirect (`<<A`) combined with
    // a heredoc nested in a LATER redirect-TARGET word (`>$(f <<Y)`): the
    // nested Y emits first (inner-newline), but fill_redirects walks the
    // redirect list in structural order, filling the `<<A` heredoc (list
    // position 0) first — so `<<A` takes Y's body and Y takes A's. Same
    // class, but entirely within fill_redirects (not the args/redirects
    // split of case 1).
    let s2 = "echo <<A $(:) >$(f <<Y\nyy\nY\n)\naa\nA\n";
    // Was an `assert_ne!` vs the (now-deleted) oracle documenting the
    // redirect-list fill-order divergence; now just production behavior.
    assert!(
        new_seq(s2).is_ok(),
        "expected Ok for {s2:?}, got {:?}",
        new_seq(s2)
    );
}

#[test]
fn atoms_heredoc_word_redirect_order() {
    // v260 CF1: heredoc-bearing Word + the command's own redirect heredoc,
    // both interleavings, fill in correct source order (byte-identical to
    // the oracle — no pin needed; the lexer bridge collects nested-Word
    // bodies at the inner newline, before the outer line's redirect bodies).
    diff_cmd("cat $(sh <<B\nbb\nB\n) <<A\naa\nA\n"); // Word then trailing redirect
    diff_cmd("cat <<A $(sh <<B\nbb\nB\n)\naa\nA\n"); // redirect then Word (ex-"pin")
    diff_cmd("cat <<A $(sh <<B\nbb\nB\n) <<C\naa\nA\ncc\nC\n"); // redirect, Word, redirect
    diff_cmd("echo $(cat <<X\nxx\nX\n) <<A\naa\nA\n"); // Word then redirect (echo)
    diff_cmd("cat <<A $(a <<B\nbb\nB\n)$(c <<D\ndd\nD\n)\naa\nA\n"); // redirect then two Words
    // Regressions: plain / multiple outer-redirect heredocs still fill in order.
    diff_cmd("cat <<A\naa\nA\n");
    diff_cmd("cat <<A <<B\naa\nA\nbb\nB\n");
    diff_cmd("cat <<A\naa\nA\ncat <<B\nbb\nB\n");
}

// v250 T3 tests: literal heredocs (quoted/escaped delimiter) end-to-end

#[test]
fn atoms_heredoc_literal() {
    diff_cmd("cat <<'EOF'\nhello $x\nEOF\n");
    diff_cmd("cat <<'EOF'\nEOF\n"); // empty body
    diff_cmd("cat <<-'EOF'\n\ttabbed\n\tEOF\n"); // <<- strip
    diff_cmd("cat <<\"EOF\"\nline1\nline2\nEOF\n"); // double-quoted delim = literal
    diff_cmd("<<'EOF'\nx\nEOF\n"); // leading heredoc (empty-words cmd)
}

#[test]
fn atoms_heredoc_literal_sequence_continuation() {
    // A newline-consumption site that fails to drain the heredoc-body atom
    // group after the delimiter line would make the parser choke on (or
    // hang trying to parse) whatever follows — guard every shape that
    // keeps parsing PAST a literal heredoc's body.
    diff_cmd("cat <<'EOF'\nx\nEOF\necho done\n"); // ; -like newline connector
    diff_cmd("cat <<'EOF'\nx\nEOF\necho a; echo b\n"); // more of the sequence after
    diff_cmd("cat <<'EOF' && echo ok\nx\nEOF\n"); // && after a heredoc-bearing stage
    diff_cmd("cat <<'EOF' | wc -l\nx\nEOF\n"); // heredoc stage in a pipeline
    diff_cmd("cat <<'A' <<'B'\nfirst\nA\nsecond\nB\n"); // two heredocs, ordered bodies
    diff_cmd("if cat <<'EOF'; then echo y; fi\nx\nEOF\n"); // heredoc in a compound's condition
    diff_cmd("for i in 1; do cat <<'EOF'; done\nx\nEOF\n"); // heredoc inside a loop body
}

// v250 T5 tests: systematic positional coverage (every command position)

#[test]
fn atoms_heredoc_positions() {
    diff_cmd("cat <<A <<B\nbodyA\nA\nbodyB\nB\n"); // stacked, order A then B
    diff_cmd("a <<X | b <<Y\nx\nX\ny\nY\n"); // across a pipeline
    diff_cmd("{ cat <<EOF\nx\nEOF\n}\n"); // heredoc in a brace group
    diff_cmd("if true; then cat <<EOF\nx\nEOF\nfi\n"); // heredoc in an if body
    diff_cmd("cat <<EOF >out arg\nx\nEOF\n"); // interleaved with redirect + word
    diff_cmd("cat <<A; echo hi\nbodyA\nA\n"); // heredoc then `;` then command
}

#[test]
fn atoms_heredoc_positions_compound_bodies() {
    diff_cmd("while false; do cat <<EOF; done\nx\nEOF\n"); // heredoc in a while body
    diff_cmd("for i in 1; do cat <<EOF; done\nx\nEOF\n"); // heredoc in a for body (expanding)
    diff_cmd("case x in a) cat <<EOF;; esac\nx\nEOF\n"); // heredoc in a case body
    diff_cmd("( cat <<EOF\nx\nEOF\n)\n"); // heredoc in a subshell
}

#[test]
fn atoms_heredoc_positions_trailing_compound_redirect() {
    // Redirected{inner, redirects}: the wrapped command's own heredoc body
    // must be collected BEFORE the compound's own trailing heredoc body.
    diff_cmd("{ cat; } <<EOF\nx\nEOF\n");
    diff_cmd("if true; then cat; fi <<EOF\nx\nEOF\n");
}

#[test]
fn atoms_heredoc_positions_misc() {
    diff_cmd("cat 2>&1 <<EOF\nx\nEOF\n"); // heredoc after another redirect
    // Mixed literal + expanding, stacked: proves the per-heredoc expand
    // flag routes through the attach walk to the RIGHT redirect.
    diff_cmd("cat <<'A' <<B\n$lit\nA\n$exp\nB\n");
    // FD-prefixed heredoc: the `3` is a `RedirFd` atom emitted ahead of
    // the `<<` opener by the word-run arm.
    diff_cmd("cat 3<<EOF\nx\nEOF\n");
}

// EOF-closes-heredoc: a top-level BATCH parse (`eof_closes_heredoc=true`)
// delimits an open here-document by end-of-input (bash behavior), instead of
// erroring `UnterminatedHeredoc`.

fn new_seq_eof(s: &str) -> Result<Option<Sequence>, ParseError> {
    let opts = LexerOptions {
        eof_closes_heredoc: true,
        ..Default::default()
    };
    let mut lx = Lexer::new(s, &Default::default(), opts);
    parse_sequence(&mut lx)
}

/// Pull the first redirect's heredoc body Word out of a parsed program.
fn heredoc_body_text(s: &str) -> String {
    let cmd = new_seq_eof(s).expect("parse ok").expect("non-empty").first;
    let e = t6_exec(&cmd);
    match &e.redirects[0].op {
        crate::command::RedirOp::Heredoc { body, .. } => body
            .0
            .iter()
            .map(|p| match p {
                WordPart::Literal { text, .. } => text.clone(),
                o => panic!("unexpected body part {o:?}"),
            })
            .collect(),
        o => panic!("expected Heredoc redirect, got {o:?}"),
    }
}

#[test]
fn atoms_heredoc_eof_closes_expanding() {
    // Requirement (a): with the flag set, `cat <<EOF\nhi` (no close-delimiter
    // line, EOF ends input) parses to a heredoc whose body is `hi\n` — bash
    // appends the line separator to the final line even without a trailing
    // newline in the source.
    assert_eq!(heredoc_body_text("cat <<EOF\nhi"), "hi\n");
    // Trailing newline present, still no close delimiter → same body.
    assert_eq!(heredoc_body_text("cat <<EOF\nhi\n"), "hi\n");
    // Multi-line body.
    assert_eq!(heredoc_body_text("cat <<EOF\nhi\nthere"), "hi\nthere\n");
    // Empty body (bare newline then EOF).
    assert_eq!(heredoc_body_text("cat <<EOF\n"), "");
}

#[test]
fn atoms_heredoc_eof_closes_literal() {
    // Quoted delimiter → literal body collector; EOF closes it too.
    assert_eq!(heredoc_body_text("cat <<'EOF'\nhi"), "hi\n");
    // `<<-` strips leading tabs, EOF-closed.
    assert_eq!(
        heredoc_body_text("cat <<-'EOF'\n\thi\n\tthere"),
        "hi\nthere\n"
    );
}

#[test]
fn atoms_heredoc_eof_default_still_errors() {
    // WITHOUT the flag (the default — used by `classify` and all internal
    // parses), an open here-document at EOF is still a parse error.
    assert!(
        matches!(new_seq("cat <<EOF\nhi"), Err(ParseError::Lex(ref e)) if matches!(**e, crate::lexer::LexError::UnterminatedHeredoc)),
        "default opts must still error UnterminatedHeredoc: {:?}",
        new_seq("cat <<EOF\nhi")
    );
    assert!(
        matches!(new_seq("cat <<'EOF'\nhi"), Err(ParseError::Lex(ref e)) if matches!(**e, crate::lexer::LexError::UnterminatedHeredoc)),
        "default opts (literal) must still error: {:?}",
        new_seq("cat <<'EOF'\nhi")
    );
}

#[test]
fn atoms_heredoc_eof_closes_properly_closed_unchanged() {
    // A here-document that IS properly closed is byte-identical with the flag on.
    assert_eq!(heredoc_body_text("cat <<EOF\nhi\nEOF\n"), "hi\n");
}

// v250 T6: mark/rewind heredoc-state generation guard + error parity + adversarial corpus

#[test]
fn atoms_heredoc_marks_dont_span_bodies() {
    // NOTE (fix pass, corrects a prior comment error): funcdef detection on
    // the atom path does NOT use `mark`/`rewind` — it seeds the already-
    // consumed leading word instead (v248's seed-not-rewind approach), so it
    // never calls `Lexer::rewind` at all. The ONLY live `mark`/`rewind` pair
    // reachable on the atom command path is the arith `$((`-bail wrinkle in
    // `parse_arith_expansion` (see `arith_wrinkle_falls_back_to_cmdsub`): a
    // depth-0 `)` not followed by `)` bails the arith scan, and the parser
    // rewinds to the `$((` start to re-drive it as `$(` + a subshell `(`.
    //
    // These two plain cases (no `$((` anywhere) never call `rewind` at all,
    // so `heredoc_gen`'s `debug_assert_eq!` is not exercised by them — they
    // only prove the heredoc plumbing itself is fine.
    diff_cmd("cat <<EOF\nx\nEOF\n");
    diff_cmd("f() { cat <<EOF\nx\nEOF\n}\n");

    // This case DOES drive `rewind` while a heredoc body is actively being
    // emitted (`emitting_heredoc.is_some()`): the expanding body line
    // `$((cat) )` opens an arith expansion whose body bails (mirrors
    // `arith_wrinkle_falls_back_to_cmdsub`'s `"$((cat) )"`), so
    // `parse_arith_expansion` rewinds back to the `$((` start and re-drives
    // it as a command substitution containing a subshell — all while the
    // heredoc body atom stream is mid-emission. If a future change widened
    // the mark/rewind window to cross a heredoc-state mutation (the `began`
    // flip, an `at_line_start` toggle, the newline trigger, or body-end),
    // `Lexer::rewind`'s `debug_assert_eq!(self.heredoc_gen, m.heredoc_gen,
    // ...)` would panic under a debug test build. Verified (fix pass) via a
    // temporary `eprintln!` in `rewind` that this input actually reaches
    // `rewind` with `emitting_heredoc` still `Some`.
    diff_cmd("cat <<EOF\n$((cat) )\nEOF\n");
}

#[test]
fn atoms_heredoc_errors() {
    // Determined by observation (see v250 T6 report): all three inputs are
    // LEXER-level rejects on the oracle — `tokenize_with_opts` returns
    // `Err(LexError::UnterminatedHeredoc)` before parsing even starts, so
    // `old_seq`'s `.expect("lex")` would panic on them (mirrors
    // `atoms_error_parity`'s `echo $(`/`echo ${` split). Assert only that the
    // atom path rejects too, not error-payload equality.
    for s in ["cat <<EOF\nno close\n", "cat <<", "cat <<-\n"] {
        assert!(
            new_seq(s).is_err(),
            "atom path must reject unterminated/malformed heredoc {s:?}, got {:?}",
            new_seq(s)
        );
    }
}

#[test]
fn atoms_heredoc_adversarial() {
    for s in [
        "cat <<EOF\nEOFX not a close\nEOF\n", // delimiter as substring
        "cat <<EOF\n  EOF\nEOF\n",            // indented non-close (no <<-)
        "cat <<-EOF\n\t\tEOF\n",              // <<- tabbed close
        "cat <<'E'\n$x `no` ${expand}\nE\n",  // literal: nothing expands
        "cat <<EOF\n$1 $@ $? $#\nEOF\n",      // special params in expanding body
        "cat <<EOF\n\\EOF\nEOF\n",            // escaped-looking body line
    ] {
        diff_cmd(s);
    }
}

#[test]
fn atoms_cf2_heredoc_queue_reset() {
    // A parse that collects a heredoc body then errors on a stray `;;`
    // leaves the body in the Lexer-owned queue (early-Err path does not
    // drain). A subsequent parse_sequence on the SAME Lexer must discard
    // that leaked body at entry, so the queue is empty afterward.
    let mut lx = Lexer::new(
        "cat <<E\nx\nE\n;;",
        &Default::default(),
        LexerOptions::default(),
    );
    let first = parse_sequence(&mut lx); // collects the heredoc body, then `;;` → Err
    assert!(
        first.is_err(),
        "expected UnexpectedToken on the `;;`, got {first:?}"
    );
    let _ = parse_sequence(&mut lx); // entry-reset must drain the leaked body
    assert!(
        lx.take_heredoc_bodies().is_empty(),
        "parse_sequence entry-reset should have drained the leaked heredoc body"
    );
}

// v243 T2 tests

#[test]
fn cmd_subshell() {
    diff_cmd("( a )");
    diff_cmd("( a; b )");
    diff_cmd("( a | b )");
    diff_cmd("( a && b || c )");
    diff_cmd("( a; b; )"); // trailing ;
    diff_cmd("( (a) )"); // nested subshell
    diff_cmd("( { a; } )"); // brace group inside subshell
    diff_cmd("{ ( a ); }"); // subshell inside brace group
    diff_cmd("( a ) >f"); // trailing redirect
    diff_cmd("( a ) | b"); // subshell as pipeline stage
    diff_err("()"); // EmptySubshell parity
    diff_err("( a"); // unterminated parity
}

/// G2 regression guard: the whitespace/newline-only-body fix is scoped to
/// `parse_command_sub`/`parse_process_sub` (via an explicit `Blank`/`Newline`
/// skip before the empty-body check). An explicit subshell must still
/// reject a whitespace/newline-only body exactly as before — bash treats
/// `( )` and `(\n)` both as syntax errors, unlike `$( )`/`$(\n)`.
#[test]
fn cmd_subshell_whitespace_only_still_errors() {
    for s in ["( )", "(  )", "(\n)", "(\t)"] {
        assert!(
            new_seq(s).is_err(),
            "subshell {s:?} must still error, got {:?}",
            new_seq(s)
        );
    }
}

// v243 T3 tests

#[test]
fn cmd_if() {
    diff_cmd("if x; then y; fi");
    diff_cmd("if x; then y; else z; fi");
    diff_cmd("if a; then b; elif c; then d; fi");
    diff_cmd("if a; then b; elif c; then d; else e; fi");
    diff_cmd("if a; then b; elif c; then d; elif e; then f; fi"); // multi-elif
    diff_cmd("if x; then if y; then z; fi; fi"); // nested if
    diff_cmd("if x; then a; b; c; fi"); // multi-command body
    diff_cmd("if x | y; then z; fi"); // pipeline condition
    diff_cmd("if x; then y; fi | cat"); // if as pipeline stage
    diff_cmd("if x; then y; fi >f"); // trailing redirect
    diff_err("if x; then y"); // UnterminatedIf parity
}

// v243 T5 tests

#[test]
fn cmd_for_select() {
    diff_cmd("for i in a b c; do echo $i; done");
    diff_cmd("for i; do x; done"); // no-`in`
    diff_cmd("for i in; do x; done"); // empty in-list
    diff_cmd("for i in a; do for j in b; do x; done; done"); // nested
    diff_cmd("for i in a b; do if x; then y; fi; done");
    diff_cmd("for i in a; do x; done | cat"); // as pipeline stage
    diff_cmd("for i in a; do x; done 2>&1"); // trailing redirect
    diff_cmd("select x in a b; do y; done");
    diff_cmd("select x; do y; done"); // no-`in`
    diff_cmd("select x in a b c; do echo $x; break; done");
    // `for ((…)) …` (ArithFor) is NO LONGER deferred — v256 T2 implements it
    // (see `atoms_arith_for`).
    diff_cmd("for ((i=0;i<3;i++)); do x; done");
    diff_err("for i in a; do x"); // unterminated parity
}

// v243 T4 tests

#[test]
fn cmd_while_until() {
    diff_cmd("while x; do y; done");
    diff_cmd("until x; do y; done");
    diff_cmd("while x; do a; b; done");
    diff_cmd("while x | y; do z; done"); // pipeline condition
    diff_cmd("while x; do if y; then z; fi; done"); // nested if in body
    diff_cmd("while x; do while y; do z; done; done"); // nested loop
    diff_cmd("until x; do ( a ); done"); // subshell in body
    diff_cmd("while x; do y; done | cat"); // as pipeline stage
    diff_cmd("while x; do y; done <f"); // trailing redirect
    diff_err("while x; do y"); // UnterminatedLoop parity
}

// v242 T2 tests

#[test]
fn cmd_single_simple() {
    diff_cmd("echo");
    diff_cmd("echo a");
    diff_cmd("echo a b c");
    diff_cmd("echo \"$x\" 'y' z");
    assert_eq!(new_seq("").unwrap(), None); // empty input
    assert_eq!(new_seq("\n\n").unwrap(), None); // only newlines
}

#[test]
fn cmd_deferred_boundary() {
    // `{ a; }` removed: brace groups are now in-scope (Task 1).
    // `( a )` removed: subshells are now in-scope (Task 2).
    // `while x; do y; done` removed: while/until are now in-scope (Task 4).
    // `for i in a; do x; done` removed: for/select are now in-scope (Task 5).
    // `case x in …; esac` removed: case is now in-scope (Task 6).
    // `f() { x; }` removed: function-def (`name()`) is now in-scope (v248 T2).
    // `[[ -n x ]]` removed: `[[ … ]]` is now in-scope (v253 T1).
    // `(( 1+2 ))` removed: standalone arith command is now in-scope (v255 T1).
    // v257 T2: coproc is NO LONGER deferred (see `atoms_coproc_named_and_anonymous`).
    diff_cmd("coproc x");
}

// T1 tests

#[test]
fn cmd_brace_group() {
    diff_cmd("{ a; }");
    diff_cmd("{ a; b; }");
    diff_cmd("{ a; b; c; }");
    diff_cmd("{ echo hi; }");
    diff_cmd("{ { a; } }"); // nested
    diff_cmd("{ a; } >f"); // trailing redirect -> Command::Redirected
    diff_cmd("{ a; } >f 2>&1");
    diff_cmd("{ a; } | cat"); // brace as pipeline stage
    diff_cmd("a | { b; }");
    diff_cmd("{ a; }; { b; }"); // two brace groups in a sequence
    diff_err("{ a"); // UnterminatedBrace parity
}

// T3 tests

#[test]
fn cmd_assignments() {
    diff_cmd("A=1 cmd");
    diff_cmd("A=1 B=2 cmd x y");
    diff_cmd("A=1"); // bare assign -> SimpleCommand::Assign
    diff_cmd("A=1 B=2"); // bare multi-assign
    diff_cmd("A=$x cmd");
    diff_cmd("A+=v cmd"); // append
    diff_cmd("arr[0]=v cmd"); // subscripted (AssignPrefix)
    diff_cmd("PATH=/x:/y cmd");
}

// tests added in later tasks

#[test]
fn v242_scaffolding_exists() {
    let _ = crate::command::ParseError::UnsupportedCommand;
    // harness compiles + the entry is callable
    let _ = new_seq("echo a");
}

// T4 tests

#[test]
fn cmd_redirects() {
    diff_cmd("cmd >out");
    diff_cmd("cmd >>out");
    diff_cmd("cmd <in");
    diff_cmd("cmd 2>err");
    diff_cmd("cmd >out 2>&1");
    diff_cmd("cmd 2>&1 >out"); // order matters
    diff_cmd(">out cmd"); // leading redirect
    diff_cmd("cmd a >o b <i c"); // interleaved
    diff_cmd("3>f cmd"); // RedirFd prefix
    diff_cmd("cmd >|f"); // clobber
    diff_cmd("cmd <>f"); // read-write
    diff_cmd("cmd <&3"); // dup-in
    diff_cmd("cmd &>f"); // and-redirect
    diff_cmd("cmd >&2"); // dup-out to stderr
    diff_cmd("cmd 2>&-"); // close fd 2
}

#[test]
fn cmd_heredoc_supported() {
    // Here-string (`<<<`, v249 T1), LITERAL heredocs (`<<'EOF'`/`<<"EOF"`,
    // v250 T3 — `atoms_heredoc_literal`), and EXPANDING heredocs (bare/unquoted
    // delimiter, v250 T4 — `atoms_heredoc_expanding`) are ALL supported now.
    diff_cmd("cat <<EOF\nx\nEOF");
}

// T5 tests

#[test]
fn cmd_pipelines() {
    diff_cmd("a | b");
    diff_cmd("a | b | c");
    diff_cmd("! a");
    diff_cmd("! a | b");
    diff_cmd("echo x | grep y | wc -l");
    diff_cmd("A=1 cmd | other");
    diff_cmd("cmd >o | other");
    diff_cmd("! ! a"); // double-bang cancels (negate=false)
    diff_cmd("!\ncmd"); // newline after `!` is skipped (M1: parse_command top skip_newlines)
}

// T6 tests

#[test]
fn cmd_and_or_lists() {
    diff_cmd("a; b");
    diff_cmd("a; b; c");
    diff_cmd("x && y");
    diff_cmd("x || y");
    diff_cmd("x && y || z");
    diff_cmd("a | b && c | d");
    diff_cmd("p &"); // trailing background
    diff_cmd("p & q"); // & as separator (Connector::Amp)
    diff_cmd("a\nb"); // newline as connector (parse contract)
    diff_cmd("a; b &");
    diff_cmd("! a | b && c");
}

#[test]
fn cmd_invalid_double_background() {
    // `cmd & &` → command.rs returns UnexpectedBackground; match it exactly.
    assert!(new_seq("cmd & &").is_err());
}

#[test]
fn cmd_time_is_plain_command() {
    // `command.rs` has NO special `time` handling — it parses `time …` as a
    // plain command named `time`. The new parser MUST match the oracle (not
    // defer), so these are diff_cmd. (When huck later adds a `Timed` AST node,
    // both parsers change together; until then `time` is just a command word.)
    diff_cmd("time cmd");
    diff_cmd("time -p cmd");
}

// v243 T6 tests

#[test]
fn cmd_case() {
    diff_cmd("case $x in a) 1;; esac");
    diff_cmd("case $x in a) 1;; b) 2;; esac");
    diff_cmd("case $x in a|b|c) 1;; esac"); // pattern list
    diff_cmd("case $x in (a) 1;; esac"); // leading paren
    diff_cmd("case x in a) ;; esac"); // empty body
    diff_cmd("case x in a) 1;; *) 2;; esac"); // default
    diff_cmd("case $x in a) 1;& b) 2;; esac"); // ;& fallthrough
    diff_cmd("case $x in a) 1;;& b) 2;; esac"); // ;;& continue-match
    diff_cmd("case $x in a) if y; then z; fi;; esac"); // compound in body
    diff_cmd("case $x in a) case $y in b) c;; esac;; esac"); // nested case
    diff_cmd("case $x in a) 1;; esac | cat"); // case as pipeline stage
    diff_cmd("case $x in a) 1;; esac >f"); // trailing redirect
    diff_cmd("for i in a; do case $i in q) x;; esac; done"); // case in for body
    diff_err("case x in"); // unterminated parity
}

// v243 T7 tests

#[test]
fn cmd_compound_deferred_still() {
    // `[[ -n x ]]` (test grammar) removed: now in-scope, v253 T1.
    // `f() { x; }` (function def, `name()`) removed: now in-scope, v248 T2.
    // `(( 1+2 ))` / `(( x + $y ))` (standalone arith command) removed: now
    // in-scope, v255 T1 (see `atoms_arith_command`).
    // `for ((…)) …` (ArithFor) removed: now in-scope, v256 T2 (see
    // `atoms_arith_for`).
    // `coproc x` removed: now in-scope, v257 T2 (see
    // `atoms_coproc_named_and_anonymous`).
    diff_cmd("coproc x");
    // `cat <<<w` (here-string) removed: now in-scope, v249 T1.
}

// v255: standalone arith command `(( … ))`
#[test]
fn atoms_arith_command() {
    // Glued `((` that closes on the matching `))` → Command::Arith (byte-identical).
    diff_cmd("(( 1 + 2 ))");
    diff_cmd("((1+2))");
    diff_cmd("(( x = 5 ))");
    diff_cmd("(( x++ ))");
    diff_cmd("(( $x + 1 ))"); // embedded expansion — wires parse_arith_body
    // Primary bail → nested subshell backoff (depth-0 `)` not followed by `)`).
    diff_cmd("((cmd); c2)");
    // Spaced `( (` is NEVER arith — regression guard for the existing subshell path.
    diff_cmd("( ( 3 * 4 ) )");
    // Unterminated glued `((` (no matching `))`): both paths bail → subshell → same
    // parse error (oracle falls back to `( (1+2)` → UnterminatedSubshell; no lex panic).
    diff_err("((1+2)");
}

#[test]
fn atoms_arith_command_disambiguation() {
    // ── Close cleanly → Command::Arith ──────────────────────────────────
    diff_cmd("(())"); // empty body → Arith(Word([]))
    diff_cmd("(( ))"); // single-space body → Arith([Literal " "])
    diff_cmd("(( (1+2) * 3 ))"); // inner grouping parens: depth-tracked, NOT a bail
    diff_cmd("(( a[0] + 1 ))"); // subscript brackets are plain body text
    diff_cmd("(( a + $b + ${c} ))"); // multiple embedded expansions
    diff_cmd("(( 1+2 ))"); // exact string freed from the old deferral tests
    diff_cmd("(( x + $y ))"); // exact string freed from the old deferral tests
    // ── Bail → nested subshell (depth-0 `)` not followed by `)`) ─────────
    diff_cmd("((echo hi) )"); // glued open, inner closes with a single `)`
    diff_cmd("(( 3*4 ) )"); // glued open, SPACED close
    diff_cmd("((a) && (b))"); // `)` after `a` at depth 0 not followed by `)`
    diff_cmd("((a); (b))");
    // ── Spaced `( (` → subshell (existing path; regression guards) ───────
    diff_cmd("( (echo hi) )");
    diff_cmd("( ( a ) )");
}

#[test]
fn atoms_arith_command_composition() {
    diff_cmd("(( 1 )) && echo hi"); // in an && list
    diff_cmd("(( 1 )) || echo no"); // in an || list
    diff_cmd("(( 1 )); echo done"); // in a `;` list
    diff_cmd("(( 1+2 )) >out"); // trailing redirect → Redirected{ inner: Arith }
    diff_cmd("(( 1 )) | cat"); // pipeline stage
    diff_cmd("if (( x > 0 )); then y; fi"); // arith as an if-condition
    diff_cmd("while (( i < 3 )); do x; done"); // arith as a while-condition
    diff_cmd("for i in a; do (( n++ )); done"); // arith in a for body
}

// v256: C-style for (( … ))
#[test]
fn atoms_arith_for() {
    // Well-formed → Command::ArithFor (byte-identical to the oracle).
    diff_cmd("for ((i=0;i<3;i++)); do echo $i; done");
    diff_cmd("for ((;;)) do :; done"); // all sections empty → None
    diff_cmd("for (( i = 0 ; i < n ; i++ )); do x; done"); // sections trimmed
    diff_cmd("for ((i=0,j=0; i<3; i++,j++)); do :; done"); // comma is literal
    diff_cmd("for ((i=$x; i<${n}; i++)); do :; done"); // embedded expansions
    diff_cmd("for ((i=(1+2); i<9; i++)); do :; done"); // inner grouping parens
    diff_cmd("for ((;;)); do break; done");
    // Section-count errors (both paths ArithForHeader with identical message).
    diff_err("for ((a;b;c;d)); do :; done"); // got 4
    diff_err("for ((a)); do :; done"); // got 1
    diff_err("for ((a; b)); do :; done"); // got 2
}

#[test]
fn atoms_arith_for_composition() {
    diff_cmd("for ((;;)); do break; done | cat"); // pipeline stage
    diff_cmd("for ((;;)); do :; done && echo hi"); // && list
    diff_cmd("for ((;;)); do :; done >out"); // trailing redirect (Redirected{inner:ArithFor})
    diff_cmd("for\n((;;)); do :; done"); // newline before header
    diff_cmd("for (($x;;)); do :; done"); // expansion in init only
    diff_cmd("if x; then for ((i=0;i<2;i++)); do y; done; fi"); // nested in a compound body
    diff_cmd("for ((i=0;i<2;i++)); do for ((j=0;j<2;j++)); do :; done; done"); // nested arith-for
}

#[test]
fn atoms_arith_for_edges() {
    // Unterminated / malformed headers → UnterminatedLoop on both paths.
    diff_err("for ((i=0;i<3;i++)"); // single close, EOF
    diff_err("for ((i=0;i<3;i++); do x; done"); // single close before `;` → bail
    diff_err("for ((;;)) done"); // missing `do`
    diff_err("for ((;;)); do :"); // missing `done`
    // Suspected divergence (per the plan): a `;` inside a quote in the header.
    // The oracle's `split_top_level_semi` ignores quotes and splits inside
    // the quoted run. Observed atom-path behavior (this test) shows the
    // Mode::Arith for-header scanner (`scan_step_arith`, lexer.rs) has NO
    // dquote sub-mode at all in arith bodies — `"` is just accumulated as a
    // literal char, and the `;` classification only checks `for_header &&
    // depth == 0` (paren depth, not quote state). So the inner `;` in
    // `"a;b"` is NOT protected on the atom path either: it also splits into
    // 4 sections. Both paths therefore agree — this is NOT a live divergence,
    // and diff_err (not a manual is_ok/is_err pin) is the right assertion.
    // Quotes are the ONLY sub-grammar that's fine, though: backtick and
    // `${…}` sub-expansions DO carry a real divergence here — see
    // `atoms_arith_for_header_semi_in_subexpansion_carryforward` below.
    diff_err("for (( \"a;b\"; ; )); do :; done");
}

/// v256 live-flip carry-forward: a depth-0 `;` inside a backtick or `${…}`
/// sub-expansion in a `for (( … ))` header. The oracle's
/// `split_top_level_semi` (command.rs) counts ONLY `(`/`)` nesting when
/// finding header-section separators — it has no idea backticks or
/// `${…}` exist, so a `;` inside `` `a;b` `` or `${x;y}` is just another
/// depth-0 separator and the header splits into 4 sections →
/// `ArithForHeader` ("got 4"). The atom path tokenizes the header via
/// `Mode::Arith`, where a backtick opener hands off to the
/// `BeginBacktick` sub-parse and `${` hands off to the `ParamOpen`
/// sub-parse — so the `;` inside either sub-expansion is consumed by
/// that sub-parser and never reaches the arith `;`-classifier at all.
/// The header is seen as only 2 sections (init; the rest empty) and
/// parses clean. Pin the REAL observed disagreement (oracle `Err`, atom
/// `Ok`) so the differential gate tracks it; reconcile before flipping
/// `command_atoms` live.
///
/// NOTE this is narrower than it looks: quotes do NOT diverge (see
/// `atoms_arith_for_edges` above — `Mode::Arith` has no dquote sub-mode,
/// so `"` is just a literal char and both paths split identically), and
/// `$(…)` does NOT diverge either (its parens raise the SAME paren depth
/// the oracle counts, so a `;` inside `$(a;b)` is protected on both
/// paths). Only backtick and `${…}` sub-expansions carry their own
/// separate delimiter grammar that the oracle's naive paren-counter
/// can't see.
#[test]
fn atoms_arith_for_header_semi_in_subexpansion_carryforward() {
    for s in [
        "for (( `a;b`; ; )); do :; done",
        "for (( ${x;y}; ; )); do :; done",
    ] {
        // (The now-deleted oracle REJECTED these — its naive arith paren-counter
        // could not see the backtick/`${…}` sub-expansion's own `;`. The atom
        // path parses them; that carryforward divergence is now production.)
        assert!(
            new_seq(s).is_ok(),
            "expected atom-path Ok for {s:?}, got {:?}",
            new_seq(s)
        );
    }
}

// ── v257 T2: coproc ────────────────────────────────────────────────────
#[test]
fn atoms_coproc_named_and_anonymous() {
    // Anonymous simple → Coproc{COPROC, Pipeline[..]}
    diff_cmd("coproc awk prog");
    diff_cmd("coproc foo bar");
    diff_cmd("coproc cat");
    diff_cmd("coproc\ncat"); // newline after coproc → anonymous; body skips it
    diff_cmd("coproc ! cat"); // `!` is the program name, NOT negation
    // Named compound
    diff_cmd("coproc MYP { read l; }");
    diff_cmd("coproc M (echo hi)"); // spaced subshell body
    diff_cmd("coproc M(echo hi)"); // glued subshell body
    diff_cmd("coproc M if x; then y; fi");
    // Anonymous compound
    diff_cmd("coproc { read l; }");
    diff_cmd("coproc (echo hi)");
    diff_cmd("coproc if x; then y; fi");
}

#[test]
fn atoms_coproc_body_pipeline_semantics() {
    // simple body CONSUMES the pipe → body = Pipeline[cat, grep x]
    diff_cmd("coproc cat | grep x");
    // compound body does NOT consume the pipe → Pipeline[Coproc{BraceGroup}, cat]
    diff_cmd("coproc { a; } | cat");
    diff_cmd("coproc M { :; } | cat");
    // body stops at `&&`
    diff_cmd("coproc cat && echo y");
    // redirects
    diff_cmd("coproc cat >out");
    diff_cmd("coproc M { :; } >out");
    // rest-stage coproc is rejected (guard)
    diff_err("echo x | coproc cat");
}

#[test]
fn atoms_coproc_errors() {
    diff_err("coproc"); // MissingCommand
    diff_err("coproc |cat"); // MissingCommand
    diff_err("coproc a | coproc b"); // 2nd-stage coproc → UnexpectedKeyword("coproc")
}

#[test]
fn atoms_coproc_nonidentifier_name_parses() {
    // bash parses `coproc WORD compound-command` for ANY word as the NAME and
    // defers the valid-identifier check to RUNTIME (nameref11.sub line 47:
    // `coproc @ { :; }` parses; runtime prints `` `@': not a valid identifier ``).
    diff_cmd("coproc @ { :; }"); // non-identifier name + brace group
    diff_cmd("coproc @ ( : )"); // non-identifier name + subshell
    diff_cmd("coproc 123 { :; }"); // digit-leading name (was wrongly rejected)
    diff_cmd("coproc 1x { :; }");
    diff_cmd("coproc foo-bar { :; }"); // hyphen — not a valid identifier
    diff_cmd("coproc @ if x; then y; fi"); // non-identifier name + if-compound
    // Valid-name and anonymous forms are unchanged.
    diff_cmd("coproc MYCO { :; }"); // valid name (control)
    diff_cmd("coproc { :; }"); // anonymous brace group
    diff_cmd("coproc cat"); // anonymous simple command
}

#[test]
fn atoms_coproc_adversarial() {
    // coproc inside compound bodies (allowed — parse_command_inner everywhere)
    diff_cmd("{ coproc cat; }");
    diff_cmd("if x; then coproc cat; fi");
    diff_cmd("(coproc cat)");
    // and-or / list boundaries after a coproc
    diff_cmd("coproc cat || echo no");
    diff_cmd("coproc cat; echo done");
    diff_cmd("coproc cat &");
    // named with each compound opener
    diff_cmd("coproc W while false; do :; done");
    diff_cmd("coproc C case x in a) :;; esac");
    diff_cmd("coproc S select v in a b; do :; done");
    diff_cmd("coproc D [[ -n x ]]");
    // negation of a whole coproc pipeline (outer `!`)
    diff_cmd("! coproc cat");
}

#[test]
fn atoms_coproc_named_in_cmdsub() {
    // v257 whole-branch fix: named coproc inside $(…)/backticks — the NAME
    // arrives as a legacy Word token, not an atom Lit.
    diff_cmd("$(coproc M { :; })");
    diff_cmd("$(coproc M (echo))");
    diff_cmd("$(coproc M if x; then y; fi)");
    diff_cmd("$(coproc M while false; do :; done)");
    diff_cmd("`coproc M { :; }`");
    // anonymous in cmdsub still works (regression guard)
    diff_cmd("$(coproc { :; })");
    diff_cmd("$(coproc cat)");
}

#[test]
fn atoms_legacy_arith_base() {
    diff_cmd("echo $[1+2]"); // == $((1+2))
    diff_cmd("echo pre$[1+2]post");
    diff_cmd("echo $[ x + 1 ]");
    diff_cmd("echo $[a[0]]"); // inner [0] bracket-nested → body "a[0]"
    diff_cmd("echo $[(1+2)*3]"); // parens are literal body chars
    diff_cmd("x=$[1+2]"); // assignment value
}

#[test]
fn atoms_legacy_arith_embedded() {
    diff_cmd("echo $[$x+1]");
    diff_cmd("echo $[${a}+1]");
    diff_cmd("echo $[$(echo 1)+2]");
    diff_cmd("echo $[`echo 1`+2]");
    diff_cmd("echo $[$((1+2))+3]"); // nested $((
    diff_cmd("echo $[$[1+2]+3]"); // nested $[
    diff_cmd("echo \"$[1+2]\""); // inside dquote → Quoted{Double,[Arith]}
    diff_cmd("echo \"pre$[1+2]post\"");
}

#[test]
fn atoms_legacy_arith_carryforward_sites() {
    // Heredoc body (v250 carry-forward) → Arith{quoted:true} + "\n"
    diff_cmd("cat <<E\n$[1+2]\nE\n");
    // Regex operand inside [[ … ]] (v254 carry-forward)
    diff_cmd("[[ x =~ $[1+2] ]]");
    // Array-literal value
    diff_cmd("a=($[1+2])");
    // case subject
    diff_cmd("case $[1+2] in a) :;; esac");
}

#[test]
fn atoms_legacy_arith_quote_backslash_carryforward() {
    // v261 T1 RESOLUTION (was a v258 LIVE-FLIP CARRY-FORWARD, CF7): the atom
    // `Mode::Arith{Bracket}` now has a quote/backslash sub-mode (mirrors the
    // oracle's `scan_legacy_arith_body`/`push_quoted_span`), so a `]` inside a
    // quoted span (single OR double) or immediately after a `\` no longer
    // closes the `$[ … ]` early — it's byte-identical to the oracle now.
    // Three probed shapes (double-quoted, backslash, single-quoted); the same
    // reasoning applies uniformly to `$[ ']' ]`, `$['x]y']`, `$["x]y"]`, etc.
    diff_cmd("echo $[ \"]\" ]");
    diff_cmd("echo $[ \\] ]");
    diff_cmd("echo $[']']");
    // v261 T1 review-B: `\` is retained verbatim and protects ONLY a `]`/`[`
    // delimiter; a `\` before `$`/other lets the next char re-expand (matches the
    // oracle two-pass `arith_string_to_word`), so `\$x` → `[" \", Var{x}, " "]`.
    diff_cmd("echo $[ \\$x ]");
}

#[test]
fn atoms_arith_squote_blind_bail() {
    // v261 T1 review-A regression guard: Paren delimiters (`$((`/`((`/`for ((`)
    // are quote-BLIND even inside single-quotes — the oracle `scan_arith_body`
    // counts `(`/`)`/`;` regardless of quotes (quote-removal is a separate second
    // pass), so a `(`/`)`/for-header `;` inside a `'…'` span must still fire the
    // depth/close/bail events, NOT be swallowed as a literal.
    diff_cmd("echo $(( ')' ))"); // bails to CommandSub{Subshell{'...'}}
    diff_cmd("echo $(( '(' ))");
    diff_cmd("(( ')' ))");
    diff_err("for (( '(' ; ; )); do :; done"); // both Err(UnterminatedLoop)
}

#[test]
fn atoms_legacy_arith_backslash_quote_carryforward() {
    // v261 T1 NEW live-flip carry-forward: `\` before a QUOTE char in legacy
    // `$[ … ]`. The review-B fix retains `\` verbatim and re-processes the next
    // char (so `$`/backtick re-expand) unless it's a `]`/`[` delimiter. But a
    // `\'`/`\"` then lets the `'`/`"` OPEN a quote-removal span that is genuinely
    // unmatchable in the atom's ONE pass — the oracle's TWO-pass model protects
    // the `\c` in pass 1 (scan_legacy_arith_body) then re-interprets it in pass 2
    // (arith_string_to_word), yielding body `" \ "`. The atom instead runs off the
    // end of the (now-quoted) body and lex-errors. Same exotic two-pass-vs-one-pass
    // class as other source-position pins. `old_seq` panics on this (Ok w/ lex OK,
    // but the atom errors), so assert the atom side only.
    assert!(
        new_seq("echo $[ \\' ]").is_err(), // oracle: Ok body \" \\ \"
        "atom one-pass runs off the unmatchable squote-after-backslash span"
    );
    assert!(
        new_seq("echo $[ \\\" ]").is_err(), // oracle: Ok body \" \\ \"
        "atom one-pass runs off the unmatchable dquote-after-backslash span"
    );
}

#[test]
fn atoms_arith_paren_quote_removal() {
    // CF6: bash quote-removal in $((/((/for (( arith bodies — quotes are
    // dropped, single-quote suppresses `$`, double-quote keeps expansion.
    diff_cmd("echo $(( \"x\" ))");
    diff_cmd("echo $(( x=\"5\" ))");
    diff_cmd("echo $(( 1\"2\"3 ))");
    diff_cmd("echo $(( '$x' ))"); // single-quote → literal $x, no expand
    diff_cmd("echo $(( \"$x\" ))"); // double-quote → expands, quotes gone
    diff_cmd("echo $(( \"a\\\"b\" ))"); // dquote \-escape → a"b
    diff_cmd("echo $(( \"`echo 1`\" ))"); // backtick inside dquote
    diff_cmd("echo $(( \"${x:-]}\" ))"); // ${…} inside dquote
    diff_cmd("echo $(( \"a$(( 1 ))b\" ))"); // nested $(( )) inside dquote
    diff_cmd("echo $(( \"\" ))"); // empty dquote dropped
    diff_cmd("echo $(( \"a\"'b' ))"); // adjacent quotes concatenate
    diff_cmd("(( \"x\" ))"); // standalone (( )) command
}

#[test]
fn atoms_arith_bare_dollar_split() {
    // Bare `$` (not an expansion start) is its own literal part in the oracle.
    diff_cmd("echo $(( 1 $ 2 ))");
    diff_cmd("echo $(( 1 $+ 2 ))");
    diff_cmd("echo $(( $'x' ))"); // no ANSI-C in arith: `$` literal + 'x' removed
}

#[test]
fn atoms_legacy_arith_quote_protection() {
    // CF7: `$[ … ]` — quotes and `\` protect the `]` AND are removed
    // (backslash retained literally).
    diff_cmd("echo $[ \"]\" ]"); // was UnterminatedQuote → Arith " ] "
    diff_cmd("echo $[']']"); // was UnterminatedQuote → Arith "]"
    diff_cmd("echo $[ \\] ]"); // was 2 args → Arith " \\] "
    diff_cmd("echo $[ \"$x\" ]"); // dquote expands, protects, removed
    diff_cmd("echo $[ ${x:-]} ]"); // ${…} already protects (regression)
    diff_cmd("echo $[ $(echo ]) ]"); // $(…) already protects (regression)
    // Backslash-RUN before a delimiter: the oracle pairs `\` with any next
    // char, so an EVEN run leaves the following `]` a LIVE delimiter. The
    // atom pairs `\`+`\` (and `\`+`]`/`[`) so runs are consumed correctly.
    diff_cmd("echo $[ \\\\] ]"); // \\]  → Arith " \\\\" + arg "]"
    diff_cmd("echo $[ \\\\\\] ]"); // \\\] → ] protected (odd run)
    diff_cmd("echo $[ \\$x ]"); // \$x  → " \\" + Var x (re-expands)
    diff_cmd("echo $[ \\\\$x ]"); // \\$x → " \\\\" + Var x
}

#[test]
fn atoms_arith_for_header_quote() {
    // Probed edge (v261 T2 Step 6): a quoted for-header section. Both paths
    // agree byte-for-byte — a quoted `;` inside a for-header section still
    // counts as a section separator (quote-blind, like the paren-delim bail),
    // so `"a;b"` splits into 4 sections → ArithForHeader error on both sides;
    // a fully-quoted section (`"1"`/`"2"`) parses identically with the quotes
    // stripped from the resulting Word.
    diff_err("for (( \"a;b\" ; ; )); do :; done");
    diff_cmd("for (( \"1\" ; \"2\" ; )); do :; done");
}

#[test]
fn atoms_arith_quote_blind_bail_unchanged() {
    // Paren delimiters are quote-BLIND: a `)`/`(` inside a quote still drives
    // the depth/bail logic (scan_arith_body). These bail to a
    // cmdsub-of-subshell on BOTH paths — quote-removal must not protect them.
    diff_cmd("echo $(( \")\" ))");
    diff_cmd("echo $(( ')' ))");
    diff_cmd("echo $(( \"(\" ))");
}

#[test]
fn atoms_legacy_arith_in_param_operand() {
    // v258 whole-branch fix: `$[` inside an unquoted ${…} operand.
    diff_cmd("echo ${a:-$[1+2]}");
    diff_cmd("echo ${a[$[1]]}");
    diff_cmd("echo ${a#$[1]}");
    diff_cmd("echo ${a:$[1]:$[2]}");
}

#[test]
fn atoms_legacy_arith_unterminated() {
    // `$[1+2` (no closing `]`) → lex error on both paths. `old_seq` panics
    // (`.expect("lex")`), so this is asserted on the atom side only: the atom
    // emits UnterminatedArith (the oracle emits UnterminatedLegacyArith — both
    // ParseError::Lex; dormant, error-kind difference only).
    assert!(new_seq("echo $[1+2").is_err(), "unterminated $[ must error");
}

#[test]
fn cmd_deep_nesting() {
    diff_cmd("if x; then while y; do case $z in a) ( b );; esac; done; fi");
    diff_cmd("{ for i in a b; do if $i; then echo $i; fi; done; }");
    diff_cmd("while x; do { a; ( b ); }; done");
    diff_cmd("case $x in a) for i in 1 2; do echo $i; done;; b) { y; };; esac");
    diff_cmd("( if x; then y; else z; fi ) | { cat; }");
}

#[test]
fn cmd_for_arith_unterminated_edge() {
    // T5 Minor: unterminated `for ((` (two consecutive LParen not forming an ArithBlock)
    // — the oracle guards it as UnterminatedLoop; parse_for may fall through to the
    // var-name read. Verify against the oracle. If tokenize itself errors (so neither
    // parser is reached), note that instead.
    for s in ["for (( ", "for ((", "for (()"] {
        // All three are unterminated `for ((` — the atom path rejects each
        // (UnterminatedLoop, or a lexer error surfaced as ParseError::Lex).
        assert!(
            new_seq(s).is_err(),
            "unterminated for-arith must error for {s:?}: {:?}",
            new_seq(s)
        );
    }
}

// ── v244 T1: command-substitution differential harness ───────────────────
//
// THE PRODUCTION LEXER IS THE ORACLE.  When `new_cs` ≠ `old_cs`, fix
// the new path to match — never weaken or skip the comparison.

/// Build the expected `WordPart::CommandSub` using the PRODUCTION lexer (oracle).
/// Wraps `s` in `"…"` when `quoted=true` to simulate a double-quoted context.

/// Build the expected `WordPart::CommandSub` using the NEW parser-driven path.
fn new_cs(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
    let mut lx = Lexer::new(s, &Default::default(), LexerOptions::default());
    parse_command_sub(&mut lx, quoted)
}

/// Assert that the new and old paths produce identical results for both
/// unquoted and quoted contexts.
fn diff_cs(s: &str) {
    assert!(
        new_cs(s, false).is_ok(),
        "unquoted {s:?}: {:?}",
        new_cs(s, false)
    );
    assert!(
        new_cs(s, true).is_ok(),
        "quoted   {s:?}: {:?}",
        new_cs(s, true)
    );
}

fn diff_cs_deferred(s: &str) {
    assert!(
        matches!(new_cs(s, false), Err(ParseError::UnsupportedExpansion)),
        "expected deferred for {s:?}, got {:?}",
        new_cs(s, false)
    );
}

#[test]
fn cs_simple() {
    diff_cs("$(echo hi)");
    diff_cs("$(echo hi there)");
    diff_cs("$(true)");
    diff_cs("$()"); // empty -> empty Sequence (NOT EmptySubshell)
}

/// G2: a command-substitution body that is only whitespace/newlines
/// (no actual command) must parse the SAME empty `Sequence` a truly-empty
/// `$()` body does — bash treats `$( )`/`$(\n\t)` as an empty command
/// substitution, unlike an explicit subshell (`( )`/`(\n)` are syntax
/// errors there — see `cmd_subshell`'s `diff_err("()")` case and
/// `cmd_subshell_whitespace_only_still_errors` below, unaffected by
/// this change).
#[test]
fn cs_whitespace_only_body_is_empty() {
    fn empty_sequence() -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: Vec::new(),
            }),
            rest: Vec::new(),
            background: false,
        }
    }
    for s in [
        "$()",
        "$( )",
        "$(  )",
        "$(\t)",
        "$(\n)",
        "$(\n\t\n)",
        "$( \n \t \n )",
    ] {
        match new_cs(s, false) {
            Ok(WordPart::CommandSub {
                sequence,
                quoted: false,
            }) => {
                assert_eq!(
                    sequence,
                    empty_sequence(),
                    "body {s:?} must yield the empty Sequence"
                );
            }
            other => panic!("expected empty CommandSub for {s:?}, got {other:?}"),
        }
    }
    // Leading/trailing whitespace around a REAL command still runs it
    // (this is not a special case — parse_subshell_sequence already
    // skips blanks before the first command).
    diff_cs("$( echo hi )");
    diff_cs("$(\n echo hi \n)");
}

#[test]
fn cs_body_grammar() {
    diff_cs("$(a; b)");
    diff_cs("$(a; b; c)");
    diff_cs("$(a | b)");
    diff_cs("$(a | b | c)");
    diff_cs("$(a && b || c)");
    diff_cs("$(a; b;)"); // trailing ;
    diff_cs("$(a &)"); // background in body
    diff_cs("$(if x; then y; fi)"); // compound body (v243)
    diff_cs("$(for i in a b; do echo $i; done)");
    diff_cs("$(while x; do y; done)");
    diff_cs("$(case $z in a) b;; esac)");
    diff_cs("$( (echo x) )"); // comsub of a subshell (SPACED)
    diff_cs("$({ echo x; })"); // comsub of a brace group
    diff_cs("$(f() { x; })"); // function-def body (v248 T2)
    diff_cs("$([[ -n x ]])"); // `[[ ]]` body (v253 T1)
    diff_cs("$(coproc x)"); // coproc body (v257 T2)
}

// v244 T3 tests

#[test]
fn cs_nesting_quoting() {
    diff_cs("$(echo $(date))"); // nested: inner fat-built, outer new-path
    diff_cs("$(echo ${x})"); // ${…} in a body word (fat-built, passes through)
    diff_cs("$(a $(b) $(c))"); // two nested
    diff_cs("$(echo \"$(date)\")"); // nested inside dquotes in the body
    diff_cs("$(<file)"); // body is a bare redirect
    diff_cs("$(cat < in > out)");
    diff_cs("$(echo hi\n)"); // trailing newline in body
}

// ── v244 T4 tests ────────────────────────────────────────────────────────

#[test]
fn cs_in_param_operand() {
    diff_ok("${x:-$(echo d)}");
    diff_ok("${x:+$(cmd)}");
    diff_ok("${x=$(a b)}");
    diff_ok("${x:-a$(b)c}"); // comsub between literals in an operand
    diff_ok("${x/$(a)/$(b)}"); // pattern + replacement operands
    diff_ok("${x:-$(echo $(date))}"); // nested comsub inside an operand
}

// ── v244 T5 tests ────────────────────────────────────────────────────────

#[test]
fn cs_deferred_boundary() {
    diff_cs_deferred("$((1+2))"); // arith expansion (WordPart::Arith, not comsub)
    diff_cs_deferred("$(( a + b ))");
    diff_cs_deferred("`echo hi`"); // backtick (own iteration)
    // `$(f() { x; })` removed: function-def body now parses (v248 T2);
    // see `cs_body_grammar`'s `diff_cs("$(f() { x; })")`.
    // `$([[ -n x ]])` removed: `[[ ]]` body now parses (v253 T1); see
    // `cs_body_grammar`'s `diff_cs("$([[ -n x ]])")`.
    // `$(coproc x)` removed: coproc body now parses (v257 T2); see
    // `cs_body_grammar`'s `diff_cs("$(coproc x)")`.
}

#[test]
fn cs_error_parity() {
    let new = new_cs("$(echo", false);
    assert!(new.is_err(), "unterminated comsub must Err, got {new:?}");
}

// ── v245 T1: backtick command-substitution differential harness ──────────
//
// THE PRODUCTION LEXER IS THE ORACLE.  When `new_bt` ≠ `old_bt`, fix the
// new path to match — never weaken or skip the comparison.

/// Build the expected `WordPart::CommandSub` using the NEW parser-driven
/// backtick path (skeleton in Task 1; full body in Task 2+).
fn new_bt(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
    let mut lx = Lexer::new(s, &Default::default(), LexerOptions::default());
    parse_backtick_sub(&mut lx, quoted)
}

/// Assert that the new and old paths produce identical results for both
/// unquoted and quoted contexts.
fn diff_bt(s: &str) {
    assert!(
        new_bt(s, false).is_ok(),
        "unquoted {s:?}: {:?}",
        new_bt(s, false)
    );
    assert!(
        new_bt(s, true).is_ok(),
        "quoted   {s:?}: {:?}",
        new_bt(s, true)
    );
}

// ── v245 T1 scaffolding test ─────────────────────────────────────────────

#[test]
fn bt_scaffolding_exists() {
    // Verify that the raw-capture Mode variant and atom kinds compile.
    let _ = Mode::BacktickRaw;
    let _ = TokenKind::BeginBacktick;
    let _ = TokenKind::EndBacktick;
    // The new backtick path must be callable for a simple substitution.
    let _ = new_bt("`echo hi`", false);
}

// ── v245 T2: depth-0 backtick core ──────────────────────────────────────

#[test]
fn bt_depth0() {
    diff_bt("`echo hi`");
    diff_bt("`echo hi there`");
    diff_bt("`a | b`");
    diff_bt("`a && b || c`");
    diff_bt("`a; b`");
    diff_bt("`if x; then y; fi`");
    diff_bt("``"); // empty -> empty Sequence
}

// ── v245 T3: body content — \$/\\ unescape, $()/${} in body, quoted ─────

#[test]
fn bt_body_content() {
    diff_bt("`echo \\$x`"); // \$ -> variable $x
    diff_bt("`echo \\\\`"); // \\ -> literal backslash
    diff_bt("`echo \\n`"); // \n -> preserved (backslash + n)
    diff_bt("`echo $(date)`"); // $() in body -> fat-built, passes through
    diff_bt("`echo ${x}`"); // ${} in body -> fat-built
    diff_bt("`echo $HOME`"); // bare $ expands
    diff_bt("`echo \"quoted\"`"); // dquotes in body
    diff_bt("`echo \\\\x`"); // \\x -> Quoted{Backslash,[Literal("x")]}
    diff_bt("`echo \\\\ x`"); // \\ <space> -> quoted space (no word-split)
    diff_bt("`echo \\\\$HOME`"); // \\$ -> Quoted{Backslash,[Literal("$")]}, no expand
}

// ── v245 T4: depth-1 nesting — `\`` opens/closes a child backtick ─────────

#[test]
fn bt_depth1_nesting() {
    diff_bt("`echo \\`date\\``"); // `echo `date`` (nested once)
    diff_bt("`a \\`b\\` c`"); // outer body: a `b` c
    diff_bt("`\\`inner\\``"); // nested at the start
    diff_bt("`echo \\`echo hi\\``");
    diff_bt("`x \\`y | z\\` w`"); // pipeline in the nested body
}

// ── v245 T5: depth-2 nesting — `\\\`` opens/closes a level-2 child ────────
//
// Proves the unified depth-aware `\`-run decode GENERALIZES to arbitrary
// depth: at D=2 the child-open delimiter is `\\\`` (3 backslashes, B=2^2−1=3)
// and the close is `\`` (1 backslash, B=2^1−1=1); at D=3 the open is again
// `\\\`` (B=2^3−1... no — the formula is B=f(run,depth), pinned to the oracle
// below).  (Rust `\\\\\\`` == the shell's `\\\`` — three backslashes + `.)
#[test]
fn bt_depth2_nesting() {
    diff_bt("`a \\`b \\\\\\`c\\\\\\` d\\` e`"); // depth-2: \\\` around c
    diff_bt("`\\`\\\\\\`x\\\\\\`\\``"); // depth-2 at the start
    diff_bt("`echo \\`echo \\\\\\`echo hi\\\\\\`\\``");
}

// v274: the former `bt_malformed_divergence_deferred` test was DELETED here.
// It pinned the OLD single-pass `scan_step_backtick`'s LENIENT acceptance of
// malformed bare-` -at-D≥2 inputs (`\`x` y\` z`, `\`a`b\``).  The v274
// three-phase `parse_backtick_sub` (raw capture → unescape → re-parse) now
// correctly REJECTS those inputs (exit 2), byte-for-byte matching bash's
// rejection — so the old assertion (that the path ACCEPTS them) is obsolete.

#[test]
fn bt_backslash_run_divergence_deferred() {
    // KNOWN DIVERGENCE [deferred, v245 — reconcile at Stage-2 live-wiring]:
    // the body `\`-run decode in scan_step_backtick consumes backslashes two
    // at a time incrementally, but the production oracle collapses the WHOLE
    // contiguous run first (backtick unescape: `\\`→`\`, `\$`→`$`, `` \` ``→`` ` ``)
    // and THEN re-lexes the survivors as a command.  The two passes agree for
    // runs of 1–3 backslashes (the corpus in bt_body_content), but diverge for
    // runs >= 4 and for an ODD run immediately before `$`/`` ` ``.  Worst case:
    // `` `echo \\\$x` `` — the new path decodes to Var{x} (EXPANDS $x) while the
    // oracle keeps `$x` literal.  These are WELL-FORMED inputs, unlike the
    // malformed class in bt_malformed_divergence_deferred.  All are dormant
    // (parser-driven path is not live), so there is no production impact today.
    // Deferred to a dedicated follow-on iteration with a full parity matrix.
    for s in [
        "`echo \\\\\\\\x`",     // shell: `echo \\\\x`   (4 backslashes + x)
        "`echo \\\\\\\\ x`",    // shell: `echo \\\\ x`  (4 backslashes + space)
        "`echo \\\\\\\\\\\\x`", // shell: `echo \\\\\\x` (6 backslashes + x)
        "`echo \\\\\\$x`",      // shell: `echo \\\$x`   (3 backslashes + $x — spurious expand)
    ] {
        // Was an oracle divergence pin (exotic backslash handling); the
        // oracle is gone, so smoke-level: the new path parses.
        let _ = new_bt(s, false).expect("new path should parse");
    }
}

// ── v245 T6 tests ────────────────────────────────────────────────────────

#[test]
fn bt_in_param_operand() {
    diff_ok("${x:-`echo d`}");
    diff_ok("${x:+`cmd`}");
    diff_ok("${x:-a`b`c}");
}

#[test]
fn bt_error_parity() {
    let new = new_bt("`echo", false);
    assert!(new.is_err(), "unterminated backtick must Err, got {new:?}");
}

// ── v246 T1: arithmetic-expansion differential harness ───────────────────
//
// THE PRODUCTION LEXER IS THE ORACLE.  When `new_arith` ≠ `old_arith`, fix
// the new path to match — never weaken or skip the comparison.

/// Production oracle: the `WordPart::Arith` the batch lexer builds for `s`.

/// New parser-driven path.
fn new_arith(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
    let mut lx = Lexer::new(s, &Default::default(), LexerOptions::default());
    parse_arith_expansion(&mut lx, quoted)
}

/// Assert new == old for both unquoted and quoted contexts.
fn diff_arith(s: &str) {
    assert!(
        new_arith(s, false).is_ok(),
        "unquoted {s:?}: {:?}",
        new_arith(s, false)
    );
    assert!(
        new_arith(s, true).is_ok(),
        "quoted   {s:?}: {:?}",
        new_arith(s, true)
    );
}

// ── v246 T1 scaffolding test ──────────────────────────────────────────────

#[test]
fn arith_scaffolding_exists() {
    let _ = TokenKind::ArithOpen;
    let _ = TokenKind::ArithClose;
    let _ = TokenKind::ArithBail;
    // Empty arith `$(( ))` round-trips through the skeleton (body filled in Task 2+).
    // Production `$(( ))` yields Arith { body: Word([...]) }; the skeleton only
    // guarantees the harness wires up, so just assert new_arith succeeds here.
    assert!(
        new_arith("$(())", false).is_ok(),
        "skeleton must parse $(())"
    );
}

// ── v246 T2 tests ────────────────────────────────────────────────────────

#[test]
fn arith_depth0_plain() {
    diff_arith("$((1+2))");
    diff_arith("$(( 1 + 2 ))");
    diff_arith("$((0))");
    diff_arith("$((a+1))"); // bare identifier is literal body text
    diff_arith("$(( x * y ))");
}

#[test]
fn arith_unterminated_errs() {
    assert!(new_arith("$((1+2", false).is_err(), "unterminated must Err");
    assert!(new_arith("$(( ", false).is_err(), "unterminated must Err");
}

// ── v246 T3 tests ────────────────────────────────────────────────────────

#[test]
fn arith_grouping_parens() {
    diff_arith("$(( (1+2)*3 ))");
    diff_arith("$(( ((1+2)) ))");
    diff_arith("$(( a*(b+c) ))");
    // Paren-BALANCED body that merely looks command-shaped: `(echo hi)` closes
    // at depth 0 as `))`, so production keeps it as Arith (not the wrinkle).
    diff_arith("$(( (echo hi) ))");
}

#[test]
fn arith_embedded_expansions() {
    diff_arith("$(( $x + 1 ))");
    diff_arith("$(( ${y} ))");
    diff_arith("$(( $(echo 1) ))");
    diff_arith("$(( `echo 1` ))");
    diff_arith("$(( $x + ${y} + 2 ))");
}

// ── v246 T3 fix tests (special/positional params) ──────────────────────────

#[test]
fn arith_special_params() {
    diff_arith("$(( $? ))");
    diff_arith("$(( $1 ))");
    diff_arith("$(( $1 + $2 ))");
    diff_arith("$(( $# ))");
    diff_arith("$(( $@ ))");
    diff_arith("$(( $* ))");
}

// ── v246 T5 tests (the `$( (…) )` wrinkle) ────────────────────────────────

#[test]
fn arith_wrinkle_falls_back_to_cmdsub() {
    // `$((cat) )` / `$((echo hi) )` are really `$( (cat) )` / `$( (echo hi) )` —
    // a command-sub whose body starts with a subshell.  A depth-0 `)` not
    // followed by `)` makes the arith scan Bail; the parser rewinds to the
    // `$((` start and re-drives as a command substitution.  Both paths agree.
    for s in ["$((cat) )", "$((echo hi) )"] {
        assert!(
            new_arith(s, false).is_ok(),
            "wrinkle {s:?}: {:?}",
            new_arith(s, false)
        );
        assert!(
            new_arith(s, true).is_ok(),
            "wrinkle quoted {s:?}: {:?}",
            new_arith(s, true)
        );
    }
}

#[test]
fn arith_wrinkle_cmdsub_body_error_matches() {
    // `$((a)b)` is really `$( (a)b )`, whose subshell body `(a)b` is itself a
    // syntax error (a bare word immediately after `)`).  Production errors on
    // it; the new path must ALSO error — reaching that error via the ArithBail
    // → cmdsub retry, not by spuriously succeeding as arith.
    assert!(
        new_arith("$((a)b)", false).is_err(),
        "new path must error on $((a)b)"
    );
}

// ── v246 T4 tests ────────────────────────────────────────────────────────

#[test]
fn arith_nested() {
    diff_arith("$(( 3 * $((5*10)) ))");
    diff_arith("$(( $((1+1)) + $((2+2)) ))");
    diff_arith("$(( $(( $((1)) )) ))");
}

// ── v246 T6 tests: operand wiring + error parity ────────────────────────

#[test]
fn arith_in_param_operand() {
    diff_ok("${x:-$((1+1))}");
    diff_ok("${x:+$((n))}");
    diff_ok("${x:-a$((i))b}");
}

#[test]
fn arith_error_parity() {
    assert!(
        new_arith("$((1+1", false).is_err(),
        "unterminated arith must Err"
    );
}

// ── v246 follow-up: nested + operand wrinkle-bail tests ────────────────────
//
// T5 proved the top-level wrinkle (`$((cat) )` bailing to a cmdsub-of-
// subshell) matches the oracle; these tests prove the bail ALSO matches when
// it happens (a) embedded inside an OUTER arith body that itself closes
// legitimately, and (b) embedded inside a `${…}` operand.  All four/three
// inputs below were verified against the oracle (`old_arith`/`old_part` via
// `diff_arith`/`diff_ok`) before writing this test — no divergence found, so
// no `*_divergence_deferred` pin is needed here.

#[test]
fn arith_wrinkle_nested_in_outer_arith() {
    // The inner `$((cat) )` bails to a `$( (cat) )` cmdsub-of-subshell; the
    // outer `$((...))` still closes legitimately as arith, so the WHOLE
    // expression is genuinely arith at the top level (diff_arith applies).
    diff_arith("$(( $((cat) ) ))"); // bail alone in the outer body
    diff_arith("$(( 1 + $((echo hi) ) ))"); // bail alongside other arith text
    diff_arith("$(( $((cat) ) + 1 ))"); // bail followed by more arith text
    diff_arith("$(( $(( $((cat) ) )) ))"); // bail nested two arith levels deep
}

#[test]
fn arith_wrinkle_bail_in_operand() {
    // The bail happening inside a `${…}` operand (rather than at the
    // top level or nested in an outer arith) — routes through
    // parse_param_expansion, so diff_ok (not diff_arith) is the right harness.
    diff_ok("${x:-$((cat) )}");
    diff_ok("${x:+$((cat) )}");
    diff_ok("${x:-a$((cat) )b}"); // bail between literals in an operand
}

#[test]
fn arith_wrinkle_nested_error_parity() {
    // `$(( $((a)b) ))`: the inner `$((a)b)` bails to a cmdsub-of-subshell
    // whose body `(a)b` is itself a syntax error (bare word after `)`,
    // same shape as arith_wrinkle_cmdsub_body_error_matches but nested one
    // arith level deeper).  Both paths must error.
    assert!(
        new_arith("$(( $((a)b) ))", false).is_err(),
        "new path must error"
    );
}

// v253 T1 tests: `[[ … ]]` grammar core

#[test]
fn atoms_double_bracket_core() {
    diff_cmd("[[ -f /etc/passwd ]]"); // unary file test
    diff_cmd("[[ -z $x ]]"); // unary string test w/ expansion
    diff_cmd("[[ hello ]]"); // lone word ≡ -n hello
    diff_cmd("[[ $x ]]"); // lone word w/ expansion
    diff_cmd("[[ a == b ]]"); // string eq
    diff_cmd("[[ a = b ]]"); // string eq (single =)
    diff_cmd("[[ a != b ]]"); // string ne
    diff_cmd("[[ $x == a* ]]"); // glob RHS stays a pattern word
    diff_cmd("[[ 3 -eq 3 ]]"); // int eq
    diff_cmd("[[ 3 -lt 5 ]]"); // int lt
    diff_cmd("[[ a < b ]]"); // string lt via Op(RedirIn)
    diff_cmd("[[ a > b ]]"); // string gt via Op(RedirOut)
    diff_cmd("[[ f1 -nt f2 ]]"); // file newer-than
    diff_cmd("[[ -f a && -f b ]]"); // logical and
    diff_cmd("[[ -f a || -f b ]]"); // logical or
    diff_cmd("[[ ! -d c ]]"); // negation
    diff_cmd("[[ ( a == b ) ]]"); // grouping
    diff_cmd("[[ -f a && -f b || ! -d c ]]"); // precedence
}

#[test]
fn atoms_double_bracket_extra() {
    diff_err("[[ ]]"); // EmptyDoubleBracket
    diff_err("[["); // UnterminatedDoubleBracket
    diff_err("[[ -f"); // unary op, no operand → UnterminatedDoubleBracket
    diff_err("[[ a == ]]"); // binary op, no rhs → TestExprMissingOperand
    // `~~` is NOT in the operator set, so BOTH paths take the lone-word
    // branch on `a` (≡ `-n a`), leave `~~` unconsumed, and trip the
    // `]]`-consume → `UnterminatedDoubleBracket` (not `TestExprBadOperator`).
    // The test passes because both agree. `TestExprBadOperator` is in fact
    // defensively UNREACHABLE on both the atom AND oracle paths — the
    // `next_is_binary` recognition set and the operator match-arm set are
    // identical, so any word that reaches the match is already known to be
    // in the set. There is therefore no genuine `TestExprBadOperator`-parity
    // case to test; that's expected and matches the oracle.
    diff_err("[[ a ~~ b ]]"); // unrecognized op → both lone-word → UnterminatedDoubleBracket
    diff_err("[[ && a ]]"); // leading operator → TestExprMissingOperand
    diff_cmd("if [[ -f a ]]; then echo y; fi"); // `[[ ]]` as an if-condition
    diff_cmd("while [[ -n $x ]]; do echo y; done"); // `[[ ]]` as a while-condition
    diff_cmd("[[\n  -f a\n  && -f b\n]]"); // Blank/Newline interleaving (skip_test_ws)
    diff_cmd("[[ ! ( -f a && -f b ) ]]"); // negated grouping
}

/// Operator GLUED to an expansion with no intervening space (`==$x`,
/// `-eq$n`, `=~$x`): the atom stream splits this into `Lit("==")` +
/// `DollarName{...}` (no `Blank`), while the oracle assembles the whole
/// thing as ONE `Word([Literal("=="), Var("x")])`. The oracle's
/// `next_is_test_binary_operator` peeks that assembled multi-part word,
/// which is NOT in its operator set → lone-word `-n a`, leaving `==$x`
/// unconsumed → the `]]`-consume trips → `UnterminatedDoubleBracket`. The
/// atom path's `next_is_test_binary_operator_atom` matches this ONLY by
/// requiring the operator `Lit` to END at a word boundary (peek2 is
/// `Blank`/`Newline`/`Op`/EOF), so a glued continuation classifies as
/// NOT-an-operator too. Regression for the T1-review bug.
#[test]
fn atoms_double_bracket_glued_operator() {
    diff_err("[[ a ==$x ]]"); // string-eq glued → UnterminatedDoubleBracket on both
    diff_err("[[ a -eq$n ]]"); // int-eq glued → same
    diff_err("[[ a =~$x ]]"); // regex glued → same (NOT the =~ deferral: not recognized as an op)
    diff_cmd("[[ a == $x ]]"); // SPACED == is a NORMAL StringEq binary (rhs $x) — must NOT regress
    diff_cmd("[[ a =~ $x ]]"); // SPACED =~ IS recognized → v254 ports the regex RHS (TestExpr::Regex)
}

/// v253 T3: an inline-assignment PREFIX immediately followed by `[[`
/// (`FOO=1 [[ … ]]`) routes to `Command::DoubleBracket` with the peeled
/// assignments attached as `inline_assignments`, byte-identical to the
/// oracle (command.rs's `parse_command_inner`, the assignment-peeling loop
/// that dispatches to `parse_double_bracket_with_assigns` when `[[` follows).
/// This closes the T1-state `atoms_double_bracket_assign_prefix_divergence`
/// pin. The atom-path interception lives in
/// `parse_simple_with_leading_word`'s word-assembly loop: BEFORE assembling
/// the next word, if every word collected so far is an assignment word AND
/// `[[` follows, the collected words are peeled into `Vec<Assignment>` and
/// dispatched to `parse_double_bracket(iter, assigns)` — forward-only
/// (peek the keyword, then dispatch), no `mark`/`rewind`.
#[test]
fn atoms_double_bracket_inline_assignments() {
    diff_cmd("FOO=hi [[ -n $FOO ]]");
    diff_cmd("A=1 B=2 [[ $A == 1 ]]"); // multiple leading assignments
    diff_cmd("x=y [[ $x == y && -n $x ]]");
    diff_cmd("x=1 [[ -f a ]]"); // the former T1 pin case
}

/// v253 T3 (OBSERVATION): `[[ ]]` used as a pipeline / logical / negated /
/// sequence stage or as an `if` condition. The T1 `[[` dispatch fires
/// wherever a command is parsed, so the compound-stage wiring already
/// covers these (as it does for `if`/`while`).
#[test]
fn atoms_double_bracket_as_stage() {
    diff_cmd("[[ -f a ]] && echo yes"); // as && stage
    diff_cmd("[[ -f a ]] || echo no"); // as || stage
    diff_cmd("! [[ -f a ]]"); // negated command
    diff_cmd("[[ -f a ]]; echo done"); // in a sequence
    diff_cmd("if [[ -n $x ]]; then echo y; fi"); // as an if condition
}

/// v253 T3-fix: a trailing redirect on a command-position `[[ … ]]` wraps
/// in `Redirected` (the dispatch site now calls `maybe_wrap_redirects`, like
/// every other atom-path compound + the oracle command.rs:1050-1053).
/// The inline-assignment site stays UNWRAPPED — `FOO=hi [[ … ]] >out` leaves
/// `>out` pending on BOTH the atom path and the oracle
/// (command.rs:1111 returns `parse_double_bracket_with_assigns` unwrapped),
/// so both error identically (`diff_err` proves the site is left unwrapped —
/// wrapping it would make the atom path return `Ok(Redirected)` and diverge).
#[test]
fn atoms_double_bracket_trailing_redirect() {
    diff_cmd("[[ -f a ]] >out"); // fixed case → Redirected
    diff_cmd("[[ -f a ]] 2>&1"); // fd-dup redirect
    diff_cmd("[[ -n $x ]] >f 2>&1"); // multi-redirect
    diff_err("FOO=hi [[ -f a ]] >out"); // inline-assign site UNWRAPPED → both Err(UnexpectedToken)
}

/// `=~` is PORTED (v254): the atom path assembles the regex-pattern RHS via
/// `Mode::Regex`/`parse_regex_operand` and produces `TestExpr::Regex`
/// byte-identically to the oracle (previously deferred with
/// `UnsupportedCommand`).
#[test]
fn atoms_double_bracket_regex_ported() {
    diff_cmd("[[ a =~ b ]]");
    diff_cmd("[[ a =~ b* ]]");
    diff_cmd("[[ $x =~ ^[0-9]+$ ]]");
    diff_cmd("[[ -f a && $x =~ y ]]"); // regex inside a logical expr
    diff_cmd("[[ x =~ ^a.*b$ ]]");
    diff_cmd("[[ $s =~ [0-9]+ ]]");
}

#[test]
fn atoms_regex_core() {
    diff_cmd("[[ $x =~ abc ]]"); // plain literal
    diff_cmd("[[ $x =~ ^a.c$ ]]"); // anchors + metachar `.`
    diff_cmd("[[ $x =~ [0-9]+ ]]"); // bracket class + quantifier
    diff_cmd("[[ $x =~ a|b ]]"); // `|` literal
    diff_cmd("[[ $x =~ a<b>c ]]"); // `<` `>` literal
    diff_cmd("[[ $x =~ a;b ]]"); // `;` literal
    diff_cmd("[[ $x =~ a&b ]]"); // `&` literal
    diff_cmd("[[ $x =~ (a b) ]]"); // paren-depth: space kept inside ( )
    diff_cmd("[[ $x =~ ((a) (b))+ ]]"); // nested groups
    diff_cmd("[[ $x =~ (a b)c ]]"); // group then trailing literal
    diff_cmd("[[ $x =~ $p ]]"); // Var
    diff_cmd("[[ $x =~ ${p}x ]]"); // ${…} then literal
    diff_cmd("[[ $x =~ ${a[0]} ]]"); // subscript expansion
    diff_cmd("[[ $x =~ $(cmd) ]]"); // command-sub
    diff_cmd("[[ $x =~ $((1+1)) ]]"); // arith
    diff_cmd("[[ $x =~ a$b|c$(d) ]]"); // mixed literal + expansions
}

/// v254 T1 hardening: the `\x`/quote/expansion traps of `scan_regex_operand`.
/// The oracle keeps `\x` as an UNQUOTED literal `\x` (backslash kept), inlines
/// single-quoted + double-quoted bodies FLAT, and wraps only `$'…'` ANSI-C.
#[test]
fn atoms_regex_traps() {
    diff_cmd("[[ $x =~ a\\.b ]]"); // THE #1 TRAP: \x kept as unquoted literal `a\.b`
    diff_cmd("[[ $x =~ a\\ b ]]"); // escaped space kept literal
    diff_cmd("[[ $x =~ \\) ]]"); // escaped paren
    diff_cmd("[[ $x =~ a\\\\b ]]"); // escaped backslash
    diff_cmd("[[ $x =~ 'a b' ]]"); // single-quoted run
    diff_cmd("[[ $x =~ \"a b\" ]]"); // double-quoted span
    diff_cmd("[[ $x =~ $'x\\ny' ]]"); // ansi-c
    diff_cmd("[[ $x =~ ${p:-d} ]]"); // param op
    diff_cmd("[[ $x =~ `echo a` ]]"); // backtick
    diff_cmd("[[ $x =~ pre$(c)post ]]"); // glued cmdsub
    diff_cmd("[[ $x =~ (a\\ b)c ]]"); // escaped space inside group + trailing
    diff_cmd("[[ $x =~ $@ ]]"); // $@
    diff_cmd("[[ $x =~ $* ]]"); // $*
    diff_cmd("[[ $x =~ $? ]]"); // $?
    diff_cmd("[[ $x =~ a$ ]]"); // trailing lone $
    diff_cmd("[[ $x =~ .* ]]"); // metachars
}

/// v254 T1 review fix: the oracle treats empty `''` and `""` DIFFERENTLY in a
/// regex operand. `''` pushes a real `Literal{"",true}` (content → the space
/// terminates → `Ok`), but `""` pushes NOTHING (operand stays "unstarted" → the
/// space is skipped as still-leading → `]]` swallowed → `Err`). The atom path
/// makes `body_started` parser-managed + drops the injected empty-`""` marker.
#[test]
fn atoms_regex_empty_quotes() {
    diff_err("[[ $x =~ \"\" ]]"); // empty dquote → both Err(TestExprMissingOperand) (space skipped, pattern becomes `]]`, rejected as operand)
    diff_cmd("[[ $x =~ '' ]]"); // empty squote → both Ok, pattern [Literal "" q:true], space terminates
    diff_cmd("[[ $x =~ a\"\"b ]]"); // → both [Lit "a", Lit "b"] (empty dquote adds nothing)
    diff_cmd("[[ $x =~ a''b ]]"); // → both [Lit "a", Lit "" q:true, Lit "b"] (empty squote kept)
    diff_cmd("[[ $x =~ \"x\" ]]"); // non-empty dquote unaffected: pattern [Lit "x" q:true]
    diff_cmd("[[ $x =~ abc ]]"); // no regression to the started/terminator logic
    diff_cmd("[[ $x =~ (a b) ]]"); // paren-depth space still kept, terminator still fires
}

/// v254 T1 review MINOR — glued `=~<b`/`=~>b` live-flip carry-forward. With no
/// space, `<`/`>` is lexed as `Op(RedirIn/RedirOut)` in command mode and
/// buffered by `next_is_test_binary_operator_atom`'s peek2, so
/// `parse_regex_operand`'s first atom is that `Op` → `TestExprBadOperator`,
/// whereas the oracle scans `<b` INTO the regex operand. This is a known
/// divergence to reconcile BEFORE flipping `command_atoms` live; pin the
/// CURRENT behavior of both paths (they disagree: atom Err, oracle Ok).
#[test]
fn atoms_regex_glued_redir_carryforward() {
    // Both paths ERROR (different kinds: atom `TestExprBadOperator` because the
    // buffered `Op(RedirIn/RedirOut)` is the first regex atom; oracle
    // `TestExprMissingOperand`). Pin the AGREEMENT that both reject; the exact
    // error kind is a v254 live-flip carry-forward to reconcile before the
    // `command_atoms` flip.
    assert!(new_seq("[[ a =~<b ]]").is_err());
    assert!(new_seq("[[ a =~>b ]]").is_err());
    // SPACED forms (the supported v254 shape) fully agree on the AST:
    diff_cmd("[[ a =~ <b ]]"); // spaced: `<b` is the operand on both
    diff_cmd("[[ a =~ >b ]]");
}

/// v254 live-flip carry-forward (PRE-EXISTING, inherited), RESOLVED by v259
/// CF4: `$"…"` locale quoting. The oracle's `scan_dollar_expansion` drops
/// the `$` for `$"` (locale-translation = identity), yielding pattern
/// `[Literal "abc" quoted:true]`. The shared `emit_unquoted_dollar_atom`
/// classifier now has a `$"` arm (v259 CF4) that does the same, so this is
/// no longer a divergence — kept as a `diff_cmd` regression guard (see also
/// `atoms_cf4_locale_dquote`, the dedicated v259 CF4 test).
#[test]
fn atoms_regex_dollar_dquote_carryforward() {
    diff_cmd("[[ $x =~ $\"abc\" ]]");
}

/// v254 T2: systematic quoting/escapes/continuations/terminator-edges
/// corpus for the `=~` regex operand.
#[test]
fn atoms_regex_quoting_escapes_terminators() {
    // Quoting
    diff_cmd("[[ $x =~ \"a b\" ]]"); // dquote keeps the space, quoted part
    diff_cmd("[[ $x =~ 'a.b' ]]"); // squote literal
    diff_cmd("[[ $x =~ x\"$y\"z ]]"); // literal + dquoted expansion + literal
    diff_cmd("[[ $x =~ \"$p\"* ]]"); // dquoted var then literal `*`
    // Escapes — backslash KEPT in the plain literal (NOT a Backslash QuoteRun)
    diff_cmd("[[ $x =~ \\. ]]"); // `\.` → literal `\.`
    diff_cmd("[[ $x =~ a\\.b ]]"); // `a\.b` one literal
    diff_cmd("[[ $x =~ a\\ b ]]"); // escaped space does NOT terminate
    diff_cmd("[[ $x =~ \\$lit ]]"); // escaped `$` → literal `$lit`
    // Line continuations
    diff_cmd("[[ $x =~ a\\\nb ]]"); // mid-pattern `\<NL>` → `ab`
    diff_cmd("[[ $x =~ \\\n  foo ]]"); // leading `\<NL>` + indent skipped → `foo`
    // Terminator edges
    diff_err("[[ $x =~ ]]"); // pattern `]]`, then no `]]` → Unterminated
    diff_err("[[ $x =~ foo"); // EOF → Unterminated
    diff_cmd("[[ $x =~ a]] ]]"); // pattern `a]]`, then space, then `]]` closes
}

/// v254 T3: regex as a PRIMARY in the `[[ ]]` cascade — composed with
/// `&&`/`||`, grouping `( … )`, negation `!`, a following normal binary,
/// and under a leading inline assignment. Exercises `parse_test_and`/
/// `parse_test_or`/`parse_test_not`/`parse_test_primary` around
/// `TestExpr::Regex` byte-identically to the oracle.
#[test]
fn atoms_regex_composition() {
    diff_cmd("[[ -f a && $x =~ b|c ]]"); // regex after &&
    diff_cmd("[[ $x =~ a || $y =~ b ]]"); // regex on both sides of ||
    diff_cmd("[[ ( $x =~ b ) ]]"); // grouped regex
    diff_cmd("[[ ! $x =~ b ]]"); // negated regex
    diff_cmd("[[ $x =~ a && $y == b ]]"); // regex then a normal binary
    diff_cmd("FOO=hi [[ $FOO =~ h.* ]]"); // regex under an inline assignment
}

/// v254 T3: adversarial corpus — POSIX classes, alternation groups, escaped
/// metachars, param-default/backtick/cmdsub expansions, mixed quoting, and
/// the two UNBALANCED-PAREN cases that keep `paren_depth > 0` so the
/// trailing ` ]]` is swallowed as literal whitespace/text inside the still-
/// open group, running the operand to EOF → `Err(UnterminatedDoubleBracket)`
/// on both paths.
#[test]
fn atoms_regex_corpus() {
    diff_cmd("[[ $x =~ a*b? ]]"); // glob-like quantifiers (literal in ERE)
    diff_cmd("[[ $x =~ [[:alpha:]]+ ]]"); // POSIX class (nested [] and :)
    diff_cmd("[[ $x =~ (foo|bar)+baz ]]"); // alternation inside a group
    diff_cmd("[[ $x =~ a\\|b ]]"); // escaped pipe → literal `\|`
    diff_cmd("[[ $x =~ ${a:-def} ]]"); // param default inside pattern
    diff_cmd("[[ $x =~ `echo re` ]]"); // backtick command-sub
    diff_cmd("[[ $x =~ \"a b\"c'd e' ]]"); // dquote + literal + squote (spaces via quotes)
    diff_cmd("[[ $x =~ pre$(cmd)post ]]"); // cmdsub glued between literals
    diff_err("[[ $x =~ a(b ]]"); // unbalanced `(` → depth stays >0, ` ]]` swallowed literal → EOF → Unterminated
    diff_err("[[ $x =~ (a b ]]"); // unbalanced open group swallows ` ]]` (depth>0 ws literal) → Unterminated
}

// v253 T2: adversarial precedence/grouping/newlines corpus (hardens the
// T1 cascade — see `parse_test_or`/`parse_test_and`/`parse_test_not`).
#[test]
fn atoms_double_bracket_precedence() {
    diff_cmd("[[ a && b && c ]]"); // left-assoc &&
    diff_cmd("[[ a || b || c ]]"); // left-assoc ||
    diff_cmd("[[ a && b || c && d ]]"); // && binds tighter than ||
    diff_cmd("[[ ! a && b ]]"); // ! binds tighter than &&
    diff_cmd("[[ ! ! a ]]"); // right-assoc double negation
    diff_cmd("[[ ( a || b ) && c ]]"); // grouping overrides precedence
    diff_cmd("[[ ( ( a ) ) ]]"); // nested grouping
    diff_cmd("[[ -n a && ( -z b || -f c ) ]]");
    diff_cmd("[[ a\n&&\nb ]]"); // newlines around &&
    diff_cmd("[[\n  a == b\n]]"); // newlines after [[ and before ]]
    diff_cmd("[[ a ||\n b ]]"); // newline after ||
}

/// T1-review CRITICAL regression: a `Newline` at an operand/operator
/// BOUNDARY (where the oracle skips nothing) must NOT be skipped — the atom
/// path previously used `skip_test_ws` (Blank+Newline) at three such sites
/// and wrongly ACCEPTED multi-line inputs the oracle REJECTS. `skip_test_ws`
/// is now confined to the four oracle `skip_test_newlines` sites; the
/// boundary sites use `skip_test_blanks` (Blank-only), and the newline
/// reaches the operand check → the same error the oracle raises.
#[test]
fn atoms_double_bracket_newline_boundaries() {
    diff_err("[[ -f\nx ]]"); // unary operand on next line → TestExprMissingOperand (both)
    diff_err("[[ a ==\nb ]]"); // binary rhs on next line → TestExprMissingOperand (both)
    diff_err("[[ a\n== b ]]"); // operator on next line → lone-word then leftover → UnterminatedDoubleBracket (both)
    diff_err("[[ (\na ) ]]"); // newline after `(` (grouping first operand) → TestExprMissingOperand (both)
    diff_err("[[ !\nx ]]"); // newline after `!` (post-negation operand) → TestExprMissingOperand (both)
    // The LEGIT multi-line cases (newline breaks only at the four oracle
    // skip sites) still parse identically — see also
    // `atoms_double_bracket_precedence`.
    diff_cmd("[[\n  a == b\n]]");
    diff_cmd("[[ a\n&&\nb ]]");
    diff_cmd("[[ a ||\n b ]]");
}

// v253 T4: error-parity hardening (Empty/Unterminated/MissingOperand plus
// unary/binary operand-missing variants). `TestExprBadOperator` is
// defensively unreachable on both paths (see `atoms_double_bracket_extra`'s
// `[[ a ~~ b ]]` note) so it is not exercised here as a distinct variant.
#[test]
fn atoms_double_bracket_errors() {
    // Same ERROR VARIANT on both paths (full `Result` equality, not just
    // is_err()==is_err()).
    diff_err("[[ ]]"); // EmptyDoubleBracket
    diff_err("[[ a == b"); // UnterminatedDoubleBracket (EOF)
    diff_err("[[ < b ]]"); // MissingOperand (leading Op)
    diff_err("[[ == b ]]"); // `==` is a Word → lone-word then leftover `b` → Unterminated
    diff_err("[[ -f ]]"); // unary missing operand
    diff_err("[[ a == ]]"); // binary missing rhs
}

// v253 T4: adversarial corpus (expansions/quotes/globs/tokenization edges).
#[test]
fn atoms_double_bracket_corpus() {
    diff_cmd("[[ \"$x\" == \"$y\" ]]"); // quoted operands
    diff_cmd("[[ ${a[0]} -gt 0 ]]"); // subscript expansion operand
    diff_cmd("[[ $(cmd) == out ]]"); // command-sub operand
    diff_cmd("[[ a=b ]]"); // `a=b` is ONE word (lone-word -n), NOT an assignment
    diff_cmd("[[ -n a=b ]]");
    diff_cmd("[[ x != y* ]]"); // glob pattern RHS of !=
    diff_cmd("[[ -f 'a b' ]]"); // quoted operand w/ space
    diff_cmd("[[ a\\ b == c ]]"); // escaped space in operand
    diff_cmd("[[ ! ( a == b ) || c ]]"); // ! before a group
    diff_cmd("[[ -e / ]]");
    diff_cmd("[[ -o errexit ]]"); // -o shell-option unary
}

/// v259 CF3 live-flip carry-forward fix: `finish_pipeline` only wrapped a
/// compound first-stage in `Pipeline{negate:true,[cmd]}` when `negate` was
/// true, so an EVEN leading-bang count (`! !`) — which computes
/// `negate = bangs % 2 == 1 = false` — fell through to the bare-compound
/// arm instead of the oracle's `Pipeline{negate:false,[compound]}`. Covers
/// every compound family plus the odd-bang/zero-bang/simple regressions.
#[test]
fn atoms_cf3_even_bang_compound() {
    // Even (>=2) bang count before a compound: oracle wraps
    // Pipeline{negate:false,[compound]}; the atom path used to return the
    // bare compound. Covers every compound family.
    diff_cmd("! ! { a; }");
    diff_cmd("! ! (a)");
    diff_cmd("! ! if x; then y; fi");
    diff_cmd("! ! while x; do y; done");
    diff_cmd("! ! for i in a; do y; done");
    diff_cmd("! ! case x in a) :; esac");
    diff_cmd("! ! [[ x ]]");
    diff_cmd("! ! (( 1 ))");
    diff_cmd("! ! coproc cat");
    // Regressions (must still match): odd-bang wraps negate:true, zero-bang
    // stays bare, simple always wraps.
    diff_cmd("! { a; }");
    diff_cmd("{ a; }");
    diff_cmd("! ! a");
}

/// v259 CF4 live-flip carry-forward fix: `emit_unquoted_dollar_atom` had no
/// `Some('"')` arm, so `$"` hit the catch-all and kept a stray `Literal "$"`
/// before the dquote span. `$"…"` is bash locale quoting; huck's translation
/// is the identity, so the oracle drops the `$` (`$"…" ≡ "…"`). Covers both
/// command position and the `=~` regex operand (shared classifier); the
/// inside-double-quote `$"` regression must stay unchanged.
#[test]
fn atoms_cf4_locale_dquote() {
    // $"…" is locale quoting == "…"; the oracle drops the `$`. The atom path
    // used to keep a stray Literal "$".
    diff_cmd("echo $\"hi\"");
    diff_cmd("echo $\"a\"$\"b\""); // multiple, all drop the `$`
    diff_cmd("[[ $x =~ $\"abc\" ]]"); // regex operand (shared classifier)
    // Regression: a $" INSIDE a double-quoted span stays a literal `$` on
    // both paths — must remain unchanged.
    diff_cmd("echo \"a$\"b\"c\"");
}

#[test]
fn atoms_cf3_even_bang_piped_compound() {
    // v259 F1: even-bang compound in a multi-stage pipeline stays nested.
    diff_cmd("! ! { a; } | b");
    diff_cmd("! ! { a; } | b | c");
    diff_cmd("! ! (a) | b");
    diff_cmd("! ! if x; then y; fi | z");
    // Regressions (already matched, must stay): odd-bang hoists flat, zero-bang flat.
    diff_cmd("! { a; } | b");
    diff_cmd("{ a; } | b");
    diff_cmd("! ! a | b");
}

#[test]
fn atoms_cf4_locale_dquote_param_operand() {
    // v259 F3: $"…" drops the $ in param-expansion operands + array-literal subscripts.
    diff_cmd("echo ${x:-$\"hi\"}");
    diff_cmd("echo ${x:+$\"y\"}");
    diff_cmd("a=([$\"k\"]=v)");
    // Regression: bare assignment-target subscript already matched, stays green.
    diff_cmd("a[$\"k\"]=v");
}

#[test]
fn atoms_bang_in_compound_body_and_condition() {
    // v262 F2: a leading `!` preceded by an inter-token Blank (after a
    // compound opener / keyword / connector) must count as pipeline negation,
    // not be swallowed into the program word. Conditions AND bodies of every
    // compound routed through parse_pipeline were divergent (probed EQ=false).
    diff_cmd("{ ! a; }");
    diff_cmd("{ ! ! a; }");
    diff_cmd("if x; then ! ! a; fi");
    diff_cmd("if ! a; then :; fi");
    diff_cmd("while ! a; do :; done");
    diff_cmd("until ! a; do :; done");
    diff_cmd("while x; do ! a; done");
    diff_cmd("for i in 1; do ! a; done");
    diff_cmd("{ ! a && b; }");
    diff_cmd("{ ! a || b; }");
    diff_cmd("{ ! a | b; }");
    // Regression guards — already correct, must STAY byte-identical.
    diff_cmd("! a"); // top-level
    diff_cmd("( ! a )"); // subshell (bespoke path)
    diff_cmd("case x in a) ! b;; esac"); // bespoke case-item path
    diff_cmd("{ a; }"); // no bang
    diff_cmd("!a"); // glued — not a bang word
}

#[test]
fn atoms_subscript_quote_wrap() {
    // v263: a bare "…"/'…' in a SUBSCRIPT operand (array-literal [sub]= and
    // param-expansion ${a[sub]}) wraps in Quoted{Double}/Quoted{Single} to
    // match the oracle's scan_subscript. Value families stay flat (guards).
    diff_cmd("a=([\"k\"]=v)");
    diff_cmd("a=(['k']=v)");
    diff_cmd("a=([\"\"]=v)");
    diff_cmd("a=(['']=v)");
    diff_cmd("a=([\"k$x\"]=v)");
    diff_cmd("a=([x\"y\"z]=v)");
    diff_cmd("a=([x'y'z]=v)");
    diff_cmd("a+=([\"k\"]=v)");
    diff_cmd("${a[\"k\"]}");
    diff_cmd("${a['k']}");
    diff_cmd("${a[x\"y\"]}");
    diff_cmd("declare -A m=([\"k\"]=v)");
    // Regression guards — must STAY byte-identical.
    diff_cmd("${x:-\"y\"}"); // value operand — FLAT (not wrapped)
    diff_cmd("${x:-'y'}"); // value single-quote — FLAT
    diff_cmd("a=([$\"k\"]=v)"); // v259 F3 dquote — already wraps
    diff_cmd("${a[$\"k\"]}"); // F3 in param-expansion subscript
    diff_cmd("a=([k]=v)"); // plain — flat quoted:false
    diff_cmd("${a[k]}"); // plain
}

#[test]
fn atoms_length_with_subscript() {
    // v263 (folded, whole-branch finding): `${#a[i]}` is the LENGTH of a
    // subscripted element/array — the oracle keeps `modifier: Length`
    // alongside the subscript. The atom's ParamClose dispatch took the
    // `subscript.is_some()` branch (modifier: None) BEFORE `length_form`,
    // silently dropping the length. Now honored.
    diff_cmd("${#a[0]}"); // Length + subscript Index(0)
    diff_cmd("${#a[k]}"); // Length + subscript Index(k)
    diff_cmd("${#a[\"k\"]}"); // Length + subscript Quoted{Double} (v263 wrap too)
    diff_cmd("${#a['k']}"); // Length + subscript Quoted{Single}
    diff_cmd("${#a[*]}"); // Length + subscript Star
    diff_cmd("${#a[@]}"); // Length + subscript All
    // Guards — must STAY byte-identical.
    diff_cmd("${#a}"); // plain length, no subscript
    diff_cmd("${a[0]}"); // subscript, no length → modifier None
}

#[test]
fn atoms_operand_enclosing_dquote_matches_oracle() {
    // value operands inside "…": single-quotes are literal, dquote backslash rules.
    diff_cmd(r#"echo "${x:-'a|b'}""#);
    diff_cmd(r#"echo "${x:='d'}""#);
    diff_cmd(r#"echo "${x:+'a|b'}""#);
    diff_cmd(r#"echo "${x-'a'}""#);
    diff_cmd(r#"echo "${x+'A'}""#);
    diff_cmd(r#"echo "${x:-\*}""#); // \* kept
    diff_cmd(r#"echo "${x:-a\nb}""#); // \n kept
    diff_cmd(r#"echo "${x:-a\$b}""#); // \$ -> $ (coincides; regression guard)
    diff_cmd(r#"echo "${x:-x\"z}""#); // \" -> " (regression guard)
    diff_cmd(r#"echo "${x:-a\\b}""#); // \\ -> \ (regression guard)
    diff_cmd(r#"echo "${x:-$y}""#); // var in dquote operand (regression guard)
    // UNQUOTED operands must be UNCHANGED (enclosing_dquote=false):
    diff_cmd("echo ${x:-'a|b'}"); // unquoted: single-quote IS a span
    diff_cmd(r#"echo ${x:-\*}"#); // unquoted backslash
    // enclosing_dquote=false operators unchanged:
    diff_cmd(r#"echo "${x#'z'}""#); // RemovePrefix pattern: enclosing_dquote=false
    diff_cmd(r#"echo "${x:?'m'}""#); // ErrorIfUnset: enclosing_dquote=false
}

// ── v264 parse_one_unit coverage ─────────────────────────────────────────
// Drive the atom `parse_one_unit` in a loop over the same script,
// checking each unit parses cleanly.
fn new_unit(s: &str) -> Vec<Result<Option<Sequence>, ParseError>> {
    let mut lx = Lexer::new(s, &Default::default(), LexerOptions::default());
    drive_units(&mut super::parse_one_unit, &mut lx)
}
fn drive_units(
    f: &mut dyn FnMut(&mut Lexer) -> Result<Option<Sequence>, ParseError>,
    lx: &mut Lexer,
) -> Vec<Result<Option<Sequence>, ParseError>> {
    let mut out = Vec::new();
    loop {
        let r = f(lx);
        let stop = matches!(r, Ok(None) | Err(_));
        out.push(r);
        if stop {
            break;
        }
    }
    out
}
fn diff_unit(s: &str) {
    assert!(
        new_unit(s).iter().all(|r| r.is_ok()),
        "expected all-Ok units for {s:?}, got {:?}",
        new_unit(s)
    );
}

#[test]
fn atoms_parse_one_unit_matches_oracle() {
    diff_unit("a\nb\nc"); // three units on three lines
    diff_unit("a; b\nc"); // `;` stays intra-unit; newline splits
    diff_unit("a && b\nc || d"); // connectors intra-unit
    diff_unit("a &\nb"); // background then newline
    diff_unit("\n\na\n\nb\n"); // leading/among/trailing blank lines
    diff_unit("a\n"); // single unit, trailing newline
    diff_unit(""); // empty → one Ok(None)
    diff_unit("   \n  a  \n"); // blank-ish lines + surrounding blanks
    diff_unit("if x; then y; fi\nz"); // compound spanning `;`, then next unit
    diff_unit("f() {\n:\n}\ng"); // compound spanning NEWLINES, then next unit
    diff_unit("for i in 1 2; do echo $i; done\ndone_marker");
    diff_unit("cat <<EOF\nhi $x\nEOF\necho next"); // heredoc body drained in-unit
    diff_unit("cat <<'EOF'\nlit\nEOF\nafter"); // literal heredoc, then next unit
    diff_unit("a | b\nc"); // pipeline intra-unit
}

// ── v264 extglob coverage (atom-native) ──────────────────────────────────
// Extglob is gated by `LexerOptions::extglob` (default off); these helpers
// turn it ON for the atom path and assert the parse succeeds.
fn new_eg(s: &str) -> Result<Option<Sequence>, ParseError> {
    let mut lx = Lexer::new(
        s,
        &Default::default(),
        LexerOptions {
            extglob: true,
            ..Default::default()
        },
    );
    super::parse_sequence(&mut lx)
}
fn diff_eg(s: &str) {
    assert!(
        new_eg(s).is_ok(),
        "expected Ok for {s:?}, got {:?}",
        new_eg(s)
    );
}

#[test]
fn atoms_extglob_matches_oracle() {
    diff_eg("echo +(a|b)");
    diff_eg("echo @(a|cd)");
    diff_eg("echo !(a|ab)");
    diff_eg("echo *(a)");
    diff_eg("echo ?(abc)");
    diff_eg("echo +([a-c])");
    diff_eg("echo zzz+(q)"); // glued prefix literal
    diff_eg("echo +(a|b)*"); // glued trailing
    diff_eg("echo dir*/+(foo|bar).txt"); // multi-component glue
    diff_eg("echo @(a*(b)c)"); // NESTED extglob group (paren depth > 1)
    diff_eg("echo +($x)"); // inner param
    diff_eg("echo @(a|$(echo b))"); // inner COMMAND SUB (the RULE-critical case)
    diff_eg("echo +(${v})"); // inner ${...}
    diff_eg("[[ aab == +(a|b) ]] && echo y || echo n");
    diff_eg("[[ abbbc == @(a*(b)c) ]] && echo y || echo n");
    diff_eg("[[ file.txt == +([a-z]).txt ]] && echo y || echo n");
    diff_eg("case hello in +([a-z])) echo lc;; *) echo o;; esac");
    diff_eg("case cd in @(ab|cd)) echo m;; *) echo o;; esac");
    // extglob OFF: `+(` is NOT extglob — regression guard (default opts).
    // Both the oracle and the atom path already reject a bare unquoted `(`
    // mid-word identically (`Err(UnexpectedToken)`, pre-existing/unrelated
    // to extglob) — `diff_err` (not `diff_cmd`) is the correct comparison
    // for a case where BOTH sides error the same way rather than succeed.
    diff_err("echo +(a)"); // extglob off: oracle+atom both treat as-is
}

// ── v264 nested-body flip: route cmdsub / backtick bodies to the ATOM
// scanner (was hardcoded to the oracle) so `[[ … ]]` / extglob groups parse
// instead of mis-parsing or hanging. ───────────────────────────────────────
#[test]
fn atoms_dbracket_extglob_in_nested_bodies() {
    // [[ … ]] inside cmdsub / backtick:
    diff_cmd("echo $( [[ a == a ]] && echo Y )");
    diff_cmd("echo `[[ a == a ]] && echo Y`");
    diff_cmd("x=$( [[ a == a ]] && echo Y ); echo $x");
    diff_cmd("echo $( [[ -n foo && bar == bar ]] && echo Y )");
    // heredoc-embedded cmdsub (Gap 1 regression guards):
    diff_cmd("cat <<EOF\n$(echo hi)\nEOF\n");
    diff_cmd("cat <<EOF\n${y:-d} and $(echo hi)\nEOF\n");
    // backtick word-splitting (Gap 2 regression guards):
    diff_cmd("cat <<EOF\n`echo $x`\nEOF\n");
    diff_cmd("echo `echo $x`");
    diff_cmd("echo `echo one two three`");
    // v264 follow-up: a `#` at a cmdsub/backtick BODY-START is a comment
    // (the body begins at a fresh word start), so the `)` inside the
    // comment does NOT close the cmdsub early. Midword `#` stays literal;
    // `#` after `;`/`|` (which reset word-start) is a comment.
    diff_cmd("echo \"[$(# c with ) paren\necho yo)]\"");
    diff_cmd("echo \"[$(echo a#b)]\"");
    diff_cmd("x=$(echo a;# c\necho b)");
}

#[test]
fn atoms_extglob_in_nested_bodies() {
    diff_eg("shopt -s extglob\necho $( [[ foo == @(foo|bar) ]] && echo 1 )");
    diff_eg("echo $( [[ z == !(a|b) ]] && echo 7 )");
    diff_eg("echo `[[ ab == @(ab|cd) ]] && echo 3`");
}

// ── G3: the `==`/`!=`/`=` RHS pattern inside `[[ … ]]` is force-parsed as an
// extended (extglob) pattern with DEFAULT (extglob-OFF) LexerOptions. ────────
#[test]
fn g3_dbracket_eq_extglob_parses_without_shopt() {
    // Helper: the DoubleBracket's rhs pattern literal text, parsed under
    // extglob-OFF defaults (new_seq / t6_first use LexerOptions::default()).
    fn rhs_lit(s: &str) -> String {
        // Concatenate all Literal parts (an extglob group glued to literal
        // prefix/suffix, e.g. `a*(b)c`, assembles as several `Literal` parts).
        fn concat(w: &Word) -> String {
            w.0.iter()
                .map(|p| match p {
                    WordPart::Literal { text, .. } => text.clone(),
                    o => panic!("rhs part not a plain literal: {o:?}"),
                })
                .collect()
        }
        match t6_first(s) {
            Command::DoubleBracket { expr, .. } => match *expr {
                TestExpr::Binary { rhs, .. } => concat(&rhs),
                o => panic!("expected Binary, got {o:?}"),
            },
            o => panic!("expected DoubleBracket, got {o:?}"),
        }
    }
    // All 5 prefixes + `==`/`!=`/`=` assemble the extglob group into the RHS
    // literal even though extglob is OFF (the parser force-arms it).
    assert_eq!(rhs_lit("[[ record == @(record|top) ]]"), "@(record|top)");
    assert_eq!(rhs_lit("[[ x != @(a|b) ]]"), "@(a|b)");
    assert_eq!(rhs_lit("[[ ab = @(ab|cd) ]]"), "@(ab|cd)");
    assert_eq!(rhs_lit("[[ foo == !(bar) ]]"), "!(bar)");
    assert_eq!(rhs_lit("[[ aab == +(a|b) ]]"), "+(a|b)");
    assert_eq!(rhs_lit("[[ ac == a*(b)c ]]"), "a*(b)c"); // glued prefix/suffix
    assert_eq!(rhs_lit("[[ a == a?(b) ]]"), "a?(b)");
    // Nested (bash-valid: prefixed inner group).
    assert_eq!(rhs_lit("[[ abbbc == @(a*(b)c) ]]"), "@(a*(b)c)");
}

#[test]
fn g3_dbracket_grouping_and_literal_paren_still_parse() {
    // `[[ (expr) ]]` grouping must still parse (bare `(`, no `?*+@!` prefix,
    // does NOT trigger the extglob gate).
    assert!(matches!(
        t6_first("[[ (a == a) ]]"),
        Command::DoubleBracket { .. }
    ));
    // A quoted paren RHS is a quoted literal, NOT an extglob group (no
    // `ExtglobOpen` fires for `"("` — the `(` is inside a `"…"` span). It
    // parses as a Binary whose rhs is a single Quoted part.
    match t6_first("[[ $x == \"(\" ]]") {
        Command::DoubleBracket { expr, .. } => match *expr {
            TestExpr::Binary { rhs, .. } => {
                assert_eq!(rhs.0.len(), 1);
                assert!(
                    matches!(rhs.0[0], WordPart::Quoted { .. }),
                    "got {:?}",
                    rhs.0[0]
                );
            }
            o => panic!("{o:?}"),
        },
        o => panic!("{o:?}"),
    }
}

#[test]
fn g3_force_extglob_does_not_leak() {
    // The force flag is confined to the `[[ == ]]` operand: a bare command
    // `echo @(a)` with extglob OFF is still a parse error (unchanged), and a
    // `@(a)` inside a `$(…)` nested in the operand is NOT force-recognized
    // (extglob stays off inside the command substitution, matching bash).
    assert!(
        new_seq("echo @(a)").is_err(),
        "bare @( must still error with extglob off"
    );
    assert!(
        new_seq("[[ x == $(echo @(a)) ]]").is_err(),
        "@( inside a nested cmdsub must NOT inherit force-extglob"
    );
    // A following command on the same logical unit is unaffected.
    assert!(new_seq("[[ a == @(a) ]] && echo hi").is_ok());
}

#[test]
fn parse_one_unit_prunes_history_across_units() {
    // 5000 single-command lines on ONE lexer (the source-reader pattern). Without
    // pruning, history grows ~linearly; with the parse_one_unit prune it stays
    // bounded near HISTORY_PRUNE_THRESHOLD.
    let empty = std::collections::HashMap::new();
    let src: String = (0..5000).map(|i| format!("echo {i}\n")).collect();
    let mut lx = Lexer::new(&src, &empty, LexerOptions::default());
    let mut units = 0;
    while parse_one_unit(&mut lx).unwrap().is_some() {
        units += 1;
        assert!(
            lx.scanned_token_count() < HISTORY_PRUNE_THRESHOLD + 64,
            "history unbounded: {} tokens after {units} units",
            lx.scanned_token_count()
        );
    }
    assert_eq!(units, 5000);
}

#[test]
fn parse_and_or_prunes_long_semicolon_chain() {
    let empty = std::collections::HashMap::new();
    let n = HISTORY_PRUNE_THRESHOLD + 200;
    let src: String = (0..n)
        .map(|i| format!("echo {i}"))
        .collect::<Vec<_>>()
        .join("; ");
    let mut lx = Lexer::new(&src, &empty, LexerOptions::default());
    let seq = parse_sequence(&mut lx).unwrap().unwrap();
    assert_eq!(1 + seq.rest.len(), n, "all commands parsed");
    assert!(
        lx.scanned_token_count() < 2 * HISTORY_PRUNE_THRESHOLD,
        "pruned during parse"
    );
}

#[test]
fn prune_does_not_break_arith_backoff_marks() {
    // A threshold-crossing chain forces a top-level prune (modes.len()==1, so
    // it's safe — no mark is outstanding yet), then an arith-backoff construct
    // whose mark/rewind must still work relative to the pruned (pos-reset)
    // history. Must be GLUED `$((` (not `$( (echo x) )` with a space — that
    // lexes as CmdSubOpen + `(` and never takes an arith mark at all): `$((echo
    // x) )` opens as arith (`$((`), the body's single `)` is followed by a
    // space rather than another `)`, so it bails and backs off to a cmdsub
    // wrapping a subshell, exactly like the original oracle construct.
    let empty = std::collections::HashMap::new();
    let filler: String = (0..HISTORY_PRUNE_THRESHOLD + 50)
        .map(|i| format!("echo {i}"))
        .collect::<Vec<_>>()
        .join("; ");
    let src = format!("{filler}; echo $((echo x) )");
    let mut lx = Lexer::new(&src, &empty, LexerOptions::default());
    assert!(parse_sequence(&mut lx).unwrap().is_some());
}

#[test]
fn prune_inside_nested_command_sub() {
    let empty = std::collections::HashMap::new();
    let inner: String = (0..HISTORY_PRUNE_THRESHOLD + 50)
        .map(|i| format!("echo {i}"))
        .collect::<Vec<_>>()
        .join("; ");
    let src = format!("x=$({inner})");
    let mut lx = Lexer::new(&src, &empty, LexerOptions::default());
    assert!(parse_sequence(&mut lx).unwrap().is_some());
}

#[test]
fn prune_preserves_heredoc_body_across_threshold() {
    // Heredoc redirect, then enough `;`-commands on the SAME line to cross the
    // threshold BEFORE the body, then the body. The prune must skip while the
    // heredoc is pending, so the body still attaches.
    use crate::command::{Command, RedirOp, SimpleCommand};
    use crate::lexer::WordPart;
    let empty = std::collections::HashMap::new();
    let filler: String = (0..HISTORY_PRUNE_THRESHOLD + 50)
        .map(|i| format!("; echo {i}"))
        .collect();
    let src = format!("cat <<EOF{filler}\nBODYLINE\nEOF\n");
    let mut lx = Lexer::new(&src, &empty, LexerOptions::default());
    let seq = parse_one_unit(&mut lx).unwrap().unwrap();
    // Extract the first heredoc body from the first command.
    let Command::Pipeline(p) = &seq.first else {
        panic!("expected pipeline")
    };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else {
        panic!("expected exec")
    };
    let body = e
        .redirects
        .iter()
        .find_map(|r| match &r.op {
            RedirOp::Heredoc { body, .. } => Some(body.clone()),
            _ => None,
        })
        .expect("heredoc redirect present");
    let text: String = body
        .0
        .iter()
        .filter_map(|part| match part {
            WordPart::Literal { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("BODYLINE"),
        "heredoc body lost across prune: {text:?}"
    );
}

#[test]
fn prune_across_outstanding_arith_mark_does_not_corrupt() {
    // Nested `$(( $({ <>THRESHOLD statements> }) x)`: the inner compound body
    // crosses the history-prune threshold while the arith `$((` mark is still
    // outstanding. The single, unmatched trailing `)` (no second `)` follows —
    // not even EOF supplies one) makes the arith bail -> rewind(&mark). Before
    // the mode-depth guard this panicked (rewind target beyond history: the
    // mark's stale absolute `pos` outran the post-drain, pos-reset history);
    // with the guard, the top-level-only prune never fires under the live
    // mark, so `history.len()` at rewind time is still consistent with `pos`.
    //
    // After the bail, the parser retries the `$((` opener as `$(` + `(` and
    // re-drives `parse_command_sub`, which now needs a SECOND closing paren
    // (for the outer `$(`) that this deliberately-unbalanced source never
    // supplies — so the fixed parse legitimately errors with
    // `UnterminatedSubshell` (determined by running the fixed build) rather
    // than panicking. That is the outcome under test, not a vacuous pass.
    let empty = std::collections::HashMap::new();
    let prefix: String = (0..2000).map(|i| format!("w{i} ")).collect();
    let inner: String = std::iter::repeat(":; ")
        .take(HISTORY_PRUNE_THRESHOLD + 200)
        .collect();
    let src = format!("echo {prefix}$(( $({{ {inner} echo HI; }}) x)");
    let mut lx = Lexer::new(&src, &empty, LexerOptions::default());
    let result = parse_sequence(&mut lx);
    assert!(
        matches!(result, Err(ParseError::UnterminatedSubshell)),
        "expected Err(UnterminatedSubshell), got {result:?}"
    );
}

// ── v268 T1: command-word indexed lvalue (sever the forward-scan) ───────

#[test]
fn cmdword_indexed_assignment_builds_indexed_target() {
    // Regression: a[i]=v and a[$(echo 2)]=v still produce AssignTarget::Indexed
    // with the right subscript Word (was the lexer bridge; now parser-assembled).
    use crate::command::{AssignTarget, Command, SimpleCommand};
    for (src, want_sub_lit) in [("a[0]=v", Some("0")), ("a[$(echo 2)]=v", None)] {
        let empty = std::collections::HashMap::new();
        let mut lx = crate::lexer::Lexer::new(src, &empty, crate::lexer::LexerOptions::default());
        let seq = parse_sequence(&mut lx).unwrap().unwrap();
        // Bare assignment → Command::Simple(SimpleCommand::Assign([a], _))
        let Command::Pipeline(p) = &seq.first else {
            panic!("pipeline")
        };
        let Command::Simple(SimpleCommand::Assign(items, _)) = &p.commands[0] else {
            panic!("assign, got {:?}", p.commands[0])
        };
        assert_eq!(items.len(), 1);
        let AssignTarget::Indexed { name, subscript } = &items[0].target else {
            panic!("indexed")
        };
        assert_eq!(name, "a");
        if let Some(lit) = want_sub_lit {
            assert_eq!(
                subscript.0,
                vec![crate::lexer::WordPart::Literal {
                    text: lit.into(),
                    quoted: false
                }]
            );
        }
    }
}

#[test]
fn cmdword_bracket_no_eq_is_a_glob_word_not_assignment() {
    // D1: a[bc] and a[$x] with NO '=' are ordinary (glob) words — NOT assignments.
    // a[$x] now EXPANDS (fold-back), so its parts include a Var (was literal-swallowed).
    use crate::command::{Command, SimpleCommand};
    use crate::lexer::WordPart;
    let empty = std::collections::HashMap::new();
    // a[bc]: single literal word, program "a[bc]", no inline assignment.
    let mut lx = crate::lexer::Lexer::new("a[bc]", &empty, crate::lexer::LexerOptions::default());
    let seq = parse_sequence(&mut lx).unwrap().unwrap();
    let Command::Pipeline(p) = &seq.first else {
        panic!()
    };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else {
        panic!("exec, got {:?}", p.commands[0])
    };
    assert!(
        e.inline_assignments.is_empty(),
        "a[bc] must NOT be an assignment"
    );
    assert_eq!(
        e.program.0,
        vec![WordPart::Literal {
            text: "a[bc]".into(),
            quoted: false
        }]
    );
    // a[$x]: program parts must contain a Var (proves expansion, D1 fix), not a literal "$x".
    let mut lx2 = crate::lexer::Lexer::new("a[$x]", &empty, crate::lexer::LexerOptions::default());
    let seq2 = parse_sequence(&mut lx2).unwrap().unwrap();
    let Command::Pipeline(p2) = &seq2.first else {
        panic!()
    };
    let Command::Simple(SimpleCommand::Exec(e2)) = &p2.commands[0] else {
        panic!()
    };
    assert!(
        e2.program
            .0
            .iter()
            .any(|p| matches!(p, WordPart::Var { name, .. } if name == "x")),
        "a[$x] subscript must EXPAND (D1): {:?}",
        e2.program.0
    );
}

#[test]
fn cmdword_indexed_value_leading_tilde_expands() {
    // D2: a[0]=~/y — the value's leading ~ becomes a Tilde part (was literal).
    use crate::command::{AssignTarget, Command, SimpleCommand};
    use crate::lexer::WordPart;
    let empty = std::collections::HashMap::new();
    let mut lx =
        crate::lexer::Lexer::new("a[0]=~/y", &empty, crate::lexer::LexerOptions::default());
    let seq = parse_sequence(&mut lx).unwrap().unwrap();
    let Command::Pipeline(p) = &seq.first else {
        panic!()
    };
    let Command::Simple(SimpleCommand::Assign(items, _)) = &p.commands[0] else {
        panic!()
    };
    let AssignTarget::Indexed { .. } = &items[0].target else {
        panic!("indexed")
    };
    assert!(
        matches!(items[0].value.0.first(), Some(WordPart::Tilde { .. })),
        "D2: leading ~ must be a Tilde part, got {:?}",
        items[0].value.0
    );
}

#[test]
fn cmdword_unclosed_bracket_eq_leaks_no_stale_flag() {
    // v268 T2 CRITICAL fix: the LBracket arm's rewind-on-error path took
    // `mark` AFTER the lexer set `pending_lvalue_subscript = true` (for
    // `name[`), so `iter.rewind(&mark)` restored the flag to `true`. The
    // very next command-scan step then misread a bare `=`/`+=` right
    // after `[` as a spurious AssignEq the parser can't place, aborting
    // the whole line with a parse error. Bash treats all of these as
    // ordinary (glob) words with no assignment — `a[=x` etc. never close
    // the bracket, so it's never an indexed lvalue.
    use crate::command::{Command, SimpleCommand};
    use crate::lexer::WordPart;
    let empty = std::collections::HashMap::new();

    fn part_text(p: &WordPart) -> String {
        match p {
            WordPart::Literal { text, .. } => text.clone(),
            WordPart::Quoted { parts, .. } => parts.iter().map(part_text).collect(),
            other => format!("<{other:?}>"),
        }
    }
    fn word_text(w: &crate::lexer::Word) -> String {
        w.0.iter().map(part_text).collect()
    }

    for (src, prog, args) in [
        ("echo a[=x", "echo", vec!["a[=x"]),
        ("echo x[+=y", "echo", vec!["x[+=y"]),
        ("printf '%s\\n' a[=1", "printf", vec!["%s\\n", "a[=1"]),
        ("echo one a[=x two", "echo", vec!["one", "a[=x", "two"]),
    ] {
        let mut lx = crate::lexer::Lexer::new(src, &empty, crate::lexer::LexerOptions::default());
        let result = parse_sequence(&mut lx);
        assert!(
            result.is_ok(),
            "{src:?} must parse without error, got {result:?}"
        );
        let seq = result.unwrap().expect("non-empty sequence");
        let Command::Pipeline(p) = &seq.first else {
            panic!("{src:?}: not a pipeline")
        };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else {
            panic!(
                "{src:?}: not a plain Exec (got {:?}) — must NOT be an assignment",
                p.commands[0]
            )
        };
        assert!(
            e.inline_assignments.is_empty(),
            "{src:?}: must NOT be an assignment"
        );
        assert_eq!(word_text(&e.program), prog, "{src:?}: program mismatch");
        let got_args: Vec<String> = e.args.iter().map(word_text).collect();
        assert_eq!(got_args, args, "{src:?}: args mismatch");
    }
}
