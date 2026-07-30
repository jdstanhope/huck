use super::*;
use crate::executor::{StderrSink, StdoutSink};
use crate::shell_state::Shell;

/// Runs `line` through the engine's single-line entry point with alias
/// expansion enabled (mirrors how `-c`/script execution decides the bool —
/// see `crate::shell::process_line`'s callers), capturing stdout+stderr into
/// owned `String`s alongside the outcome.
fn run_line_with_aliases(line: &str, shell: &mut Shell) -> (ExecOutcome, String, String) {
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = crate::shell::process_line_in_sinks(
        line,
        shell,
        true,
        &mut StdoutSink::Capture(&mut out),
        &mut StderrSink::Capture(&mut err),
    );
    (
        outcome,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn alias_no_args_lists_empty() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("alias", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(
        buf.is_empty(),
        "expected empty output, got {:?}",
        String::from_utf8_lossy(&buf)
    );
}

#[test]
fn alias_no_args_lists_sorted() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    shell.aliases.insert("la".to_string(), "ls -A".to_string());
    shell.aliases.insert("l".to_string(), "ls".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("alias", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        vec!["alias l='ls'", "alias la='ls -A'", "alias ll='ls -l'",]
    );
}

#[test]
fn alias_defines_simple() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "alias",
        &["ll=ls -l".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.aliases.get("ll").map(|s| s.as_str()), Some("ls -l"));
}

#[test]
fn alias_lookup_existing_prints() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "alias",
        &["ll".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out, "alias ll='ls -l'\n");
}

#[test]
fn alias_lookup_missing_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "alias",
        &["xyz".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn unalias_removes_existing() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "unalias",
        &["ll".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(!shell.aliases.contains_key("ll"));
}

#[test]
fn unalias_missing_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "unalias",
        &["xyz".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn unalias_dash_a_clears_all() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    shell.aliases.insert("la".to_string(), "ls -A".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "unalias",
        &["-a".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.aliases.is_empty());
}

#[test]
fn unalias_no_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("unalias", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

// --- v345 (#329, R2): command-word alias expansion after leading
// redirections / an inline-assignment prefix. ---
//
// bash expands a command-word alias regardless of what precedes it on the
// line (leading redirects, an inline-assignment prefix) — only the WORD
// POSITION matters, not what's ahead of it. `parse_command`'s one-shot
// `expand_command_alias()` call at absolute command start only ever sees a
// bare command-name `Lit`/`Word` when NOTHING precedes the command word; a
// leading redirect operator or `AssignPrefix` token in that position made it
// a no-op with nothing re-driving it once the redirect/assignment was
// consumed and the parser reached the real command word.
//
// Every case below hand-checked against `bash --norc --noprofile` (5.2.21).
// Two brief-example values needed correcting against that ground truth (see
// task-1-report.md for the full hand-check log):
//  - a bare leading OUTPUT redirect (`> /dev/null a`) redirects the aliased
//    command's OWN stdout away, so real bash prints nothing for that exact
//    fragment (rc 0, no output) — not "OK". Substituted `2>/dev/null a`
//    (redirect stderr away, leaving stdout intact) to exercise the same
//    "leading redirect precedes the alias command word" shape while still
//    asserting the brief's literal "OK" stdout.
//  - `echo hi < foo` with a NONEXISTENT `foo` never reaches `echo` at all in
//    real bash: the redirect is set up before the command runs, and opening
//    a missing file for input fails the command outright (rc 1, "No such
//    file or directory", no "hi" on stdout). The brief's "no error about
//    `bar`" is the meaningful assertion (proving `foo` — the literal target
//    text — was opened, not the alias body `bar`); "hi" printing requires an
//    ACTUAL readable file at that path, which the test below supplies.
//
// Also note: alias substitution is READ-TIME, so definitions taking effect
// only for a LATER `process_line` call (not textually later in the same
// line/`-c` string) is a separate, pre-existing, correct bash divergence —
// unrelated to this fix. `shell.aliases` is populated directly here (as the
// pre-existing tests above already do) specifically to sidestep that timing
// concern and isolate the R2 root cause under test.

#[test]
fn command_word_alias_expands_after_leading_input_redirect() {
    let mut shell = Shell::new();
    shell.aliases.insert("foo".to_string(), "echo".to_string());
    let (outcome, out, err) = run_line_with_aliases("< /dev/null foo bar", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "bar\n");
}

#[test]
fn command_word_alias_expands_after_leading_output_redirect() {
    let mut shell = Shell::new();
    shell.aliases.insert("a".to_string(), "echo OK".to_string());
    let (outcome, out, err) = run_line_with_aliases("2>/dev/null a", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "OK\n");
}

#[test]
fn command_word_alias_expands_in_eval_body_after_leading_redirect() {
    let mut shell = Shell::new();
    shell.aliases.insert("e".to_string(), "echo".to_string());
    let (outcome, out, err) = run_line_with_aliases(r#"eval "</dev/null e ok 3""#, &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "ok 3\n");
}

#[test]
fn command_word_alias_expands_after_assignment_prefix() {
    let mut shell = Shell::new();
    shell.aliases.insert("e".to_string(), "echo".to_string());
    let (outcome, out, err) = run_line_with_aliases("a=true e ok 4", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "ok 4\n");
}

// v345 fix-loop round 1: bash's rule is order-sensitive, not just
// word-shape-sensitive. A redirect consumed BEFORE any assignment-prefix
// word still allows the command-word alias to expand; a redirect consumed
// AFTER an assignment-prefix word suppresses it (empirically mapped against
// bash 5.2.21). Both hand-checked byte-identical to bash for a wider matrix
// (see task-1-report.md); these two lock the interleaved case the initial
// (word-shape-only) guard got wrong.

#[test]
fn command_word_alias_expands_when_redirect_precedes_assignment_prefix() {
    let mut shell = Shell::new();
    shell.aliases.insert("foo".to_string(), "echo".to_string());
    let (outcome, out, err) = run_line_with_aliases("< /dev/null a=1 foo bar", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "bar\n");
}

#[test]
fn command_word_alias_suppressed_when_redirect_follows_assignment_prefix() {
    let mut shell = Shell::new();
    shell.aliases.insert("foo".to_string(), "echo".to_string());
    let (outcome, out, err) = run_line_with_aliases("a=1 < /dev/null foo bar", &mut shell);
    // bash does NOT expand `foo` here: `foo` runs as a literal external/PATH
    // lookup and fails with 127 ("command not found"), matching real bash.
    assert!(
        matches!(outcome, ExecOutcome::Continue(127)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert!(out.is_empty(), "out={out:?}");
    assert!(
        err.contains("foo") && err.contains("not found"),
        "err={err:?}"
    );
}

#[test]
fn redirect_target_word_is_not_alias_expanded() {
    let target = tempfile::NamedTempFile::new().expect("create temp redirect target");
    let target_path = target.path().to_str().expect("utf8 temp path").to_string();
    let mut shell = Shell::new();
    // If the redirect target were (wrongly) looked up as an alias, this body
    // — a bogus command name, not a path — would make the open/exec fail.
    shell.aliases.insert(
        target_path.clone(),
        "bogus_command_not_a_path_xyz".to_string(),
    );
    let (outcome, out, err) =
        run_line_with_aliases(&format!("echo hi < {target_path}"), &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "hi\n");
    assert!(
        !err.contains("bogus_command_not_a_path_xyz"),
        "redirect target was alias-expanded: {err:?}"
    );
}

#[test]
fn command_word_alias_still_expands_with_no_leading_redirect() {
    let mut shell = Shell::new();
    shell.aliases.insert("foo".to_string(), "echo".to_string());
    let (outcome, out, err) = run_line_with_aliases("foo x", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "x\n");
}

// --- v345 (#329, R3): an alias whose expansion begins with `#` (at a word
// boundary) starts a comment. ---
//
// bash's comment recognition applies to the injected alias BODY too: `alias
// comment='#'; comment` is a no-op (the injected `#` is a fresh command word,
// so it hits the same word-boundary comment gate a literal leading `#` would)
// and the comment runs to the end of the LOGICAL (physical) line only — a
// `;`-joined command on the SAME line is swallowed right along with it (a
// real `#` comment doesn't stop at `;` either), but a command on the NEXT
// physical line is unaffected and still runs. Both cases below feed the
// whole multi-line program through ONE `process_line_in_sinks` call (an
// embedded `\n`, mirroring how `-c` with embedded newlines — or a single
// `eval`/`source`d chunk — parses: one `parse_sequence` over the full text),
// so the alias's swallow-to-end-of-ITS-line and the following line's
// independent parse are both exercised in a single assertion.
//
// Hand-checked against `bash --norc --noprofile` (5.2.21) via multi-line
// input (same-line alias-definition-then-use does NOT expand in bash, so the
// definition must be on its own line/`process_line` call — irrelevant here
// since `shell.aliases` is populated directly, as the R2 tests above do).

#[test]
fn alias_expanding_to_bare_hash_is_a_comment() {
    let mut shell = Shell::new();
    shell.aliases.insert("comment".to_string(), "#".to_string());
    let (outcome, out, err) = run_line_with_aliases("comment\necho done", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    // `comment`'s own line is a no-op (swallowed by the `#` it expands to);
    // `echo done` on the NEXT line is unaffected.
    assert_eq!(out, "done\n");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn alias_expanding_to_hash_prefix_swallows_rest_of_its_own_line() {
    let mut shell = Shell::new();
    shell
        .aliases
        .insert("lc".to_string(), "# for x in ".to_string());
    let (outcome, out, err) = run_line_with_aliases("lc text after\necho k", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    // `lc`'s expansion `# for x in ` starts a comment that swallows
    // ` text after` (same line) — but NOT `echo k` on the next line.
    assert_eq!(out, "k\n");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn alias_expanding_to_hash_also_swallows_a_semicolon_joined_same_line_command() {
    // A real `#` comment doesn't stop at `;` — it runs to the end of the
    // PHYSICAL line, full stop. So an alias-turned-comment on the same line
    // as a `;`-joined follow-on command swallows that follow-on too (unlike
    // the embedded-newline cases above, where the follow-on is genuinely on
    // a different line).
    let mut shell = Shell::new();
    shell.aliases.insert("comment".to_string(), "#".to_string());
    let (outcome, out, err) = run_line_with_aliases("comment; echo done", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert_eq!(out, "");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn mid_word_hash_is_still_literal_after_alias_fix() {
    // Regression: a `#` that is NOT at a word boundary (mid-word, no alias
    // involved at all) must remain literal — the R3 fix must not make `#`
    // start a comment anywhere it didn't before.
    let mut shell = Shell::new();
    let (outcome, out, err) = run_line_with_aliases("echo a#b", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} err={err:?}"
    );
    assert_eq!(out, "a#b\n");
}

// --- v345 (#329, R1): a leading array-literal assignment (`a=(...)` /
// `a+=(...)`) as the FIRST word of an injected alias body must parse as an
// array literal, not surface a bare `(` and fail with
// `ParseError::UnsupportedCommand`.
//
// Hand-checked against `bash --norc --noprofile` (5.2.21) via multi-line
// input (alias def on its own line, per the same-line-timing rule — moot
// here since `shell.aliases` is populated directly).

#[test]
fn leading_array_assign_in_alias_body_parses_as_array_literal() {
    let mut shell = Shell::new();
    shell
        .aliases
        .insert("foo".to_string(), "a=(1 2 3); echo ${a[@]}".to_string());
    let (outcome, out, err) = run_line_with_aliases("foo", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert_eq!(out, "1 2 3\n");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn leading_array_append_in_alias_body_parses_as_array_literal() {
    let mut shell = Shell::new();
    shell
        .aliases
        .insert("foo".to_string(), "a+=(2 3); echo ${a[@]}".to_string());
    let (outcome, out, err) = run_line_with_aliases("a=(1)\nfoo", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert_eq!(out, "1 2 3\n");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn non_leading_array_assign_in_alias_body_still_works() {
    // Regression: array assignment NOT in leading-command-word position
    // (already worked pre-fix) must keep working.
    let mut shell = Shell::new();
    shell.aliases.insert(
        "foo".to_string(),
        "echo x; a=(1 2); echo ${a[@]}".to_string(),
    );
    let (outcome, out, err) = run_line_with_aliases("foo", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert_eq!(out, "x\n1 2\n");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn leading_scalar_assign_in_alias_body_still_works() {
    // Regression: a leading SCALAR assignment (`name=value`, no `(`) in an
    // alias body already worked pre-fix and must keep working.
    let mut shell = Shell::new();
    shell
        .aliases
        .insert("foo".to_string(), "x=5; echo $x".to_string());
    let (outcome, out, err) = run_line_with_aliases("foo", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert_eq!(out, "5\n");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn eval_leading_array_assign_still_works() {
    // Regression: `eval` re-lexing a leading array literal (a separate code
    // path from alias-body injection) must be unaffected by this fix.
    let mut shell = Shell::new();
    let (outcome, out, err) =
        run_line_with_aliases(r#"eval "a=(1 2 3); echo \${a[@]}""#, &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert_eq!(out, "1 2 3\n");
    assert!(err.is_empty(), "err={err:?}");
}

#[test]
fn leading_array_assign_alias_name_collision_does_not_loop() {
    // Edge case: an alias named the same as the array variable used in
    // another alias's leading array assignment must not confuse the
    // recursion guard or cause a hang/panic — `a` here is a COMMAND alias,
    // unrelated to the `a=(...)` assignment target in `foo`'s body.
    let mut shell = Shell::new();
    shell
        .aliases
        .insert("a".to_string(), "echo notused".to_string());
    shell
        .aliases
        .insert("foo".to_string(), "a=(1 2 3); echo ${a[@]}".to_string());
    let (outcome, out, err) = run_line_with_aliases("foo", &mut shell);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "outcome={outcome:?} out={out:?} err={err:?}"
    );
    assert_eq!(out, "1 2 3\n");
    assert!(err.is_empty(), "err={err:?}");
}
