//! Alias expansion. Runs after tokenize, before parse. Substitutes
//! aliases at command position with cycle protection and the bash
//! trailing-space rule. Command position is tracked with reserved-word
//! recognition and compound-command context (`case` / `for` / `[[ ]]`),
//! so words that are not the first word of a simple command (case
//! patterns, for-lists, `[[ ]]` interiors, reserved words themselves)
//! are never alias-expanded.

use std::collections::{HashMap, HashSet};

use crate::lexer::{LexError, Operator, Token, Word, WordPart};

/// Compound-command context. The stack handles nesting (e.g. a `case`
/// inside a `case` body).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    CaseSubject,   // after `case`, before `in`
    CasePattern,   // pattern list: after `in`, or after ;;/;&/;;&
    CaseBody,      // clause body — normal command position resumes
    ForName,       // after `for`/`select`, before `in`
    ForList,       // after for/select `in`, until separator or `do`
    DoubleBracket, // inside [[ ... ]]
}

/// Walks a token stream substituting aliases at command position.
pub fn expand_aliases_in_tokens(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<Vec<Token>, LexError> {
    expand_aliases_in_tokens_mapped(tokens, aliases).map(|(t, _)| t)
}

/// Like `expand_aliases_in_tokens` but also returns, per output token, the index
/// of the SOURCE token it originated from. Alias-body tokens inherit the index of
/// the alias-name token they replaced; untouched tokens map to themselves. Used by
/// the non-interactive source loop to remap byte-offsets/lines back to the raw
/// source after expansion rewrites the token stream.
pub fn expand_aliases_in_tokens_mapped(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<(Vec<Token>, Vec<usize>), LexError> {
    let mut ex = Expander::new(aliases);
    for (src_idx, token) in tokens.into_iter().enumerate() {
        ex.feed(token, src_idx)?;
    }
    Ok((ex.out, ex.map))
}

struct Expander<'a> {
    out: Vec<Token>,
    map: Vec<usize>,
    active: HashSet<String>,
    eligible: bool,
    ctx: Vec<Ctx>,
    aliases: &'a HashMap<String, String>,
}

impl<'a> Expander<'a> {
    fn new(aliases: &'a HashMap<String, String>) -> Self {
        Expander {
            out: Vec::new(),
            map: Vec::new(),
            active: HashSet::new(),
            eligible: true,
            ctx: Vec::new(),
            aliases,
        }
    }

    fn top(&self) -> Option<Ctx> {
        self.ctx.last().copied()
    }

    fn push(&mut self, token: Token, src_idx: usize) {
        self.out.push(token);
        self.map.push(src_idx);
    }

    /// Feed one source token, updating output, map, eligibility, and context.
    fn feed(&mut self, token: Token, src_idx: usize) -> Result<(), LexError> {
        match token {
            Token::Word(w) => self.feed_word(w, src_idx),
            Token::Op(op) => {
                self.feed_op(op, src_idx);
                Ok(())
            }
            Token::Newline => {
                self.feed_newline(src_idx);
                Ok(())
            }
            // Heredoc / ArithBlock / RedirFd: not command-position changing.
            other => {
                self.push(other, src_idx);
                Ok(())
            }
        }
    }

    fn feed_word(&mut self, w: Word, src_idx: usize) -> Result<(), LexError> {
        let text = simple_word_text(&w);

        // Context-driven handling first: words inside subject/pattern/for/[[
        // positions are never alias-expanded; some drive transitions.
        match self.top() {
            Some(Ctx::CaseSubject) => {
                if text.as_deref() == Some("in") {
                    *self.ctx.last_mut().unwrap() = Ctx::CasePattern;
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::CasePattern) => {
                if text.as_deref() == Some("esac") {
                    self.ctx.pop();
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::ForName) => {
                if text.as_deref() == Some("in") {
                    *self.ctx.last_mut().unwrap() = Ctx::ForList;
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::ForList) => {
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::DoubleBracket) => {
                if text.as_deref() == Some("]]") {
                    self.ctx.pop();
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::CaseBody) | None => {}
        }

        // Normal command-position handling (CaseBody or empty stack).
        if self.eligible {
            if let Some(t) = text.as_deref() {
                if let Some(next_elig) = self.handle_reserved(t) {
                    self.push(Token::Word(w), src_idx);
                    self.eligible = next_elig;
                    return Ok(());
                }
                if !self.active.contains(t)
                    && let Some(body) = self.aliases.get(t).cloned()
                {
                    return self.expand_alias(t.to_string(), body, src_idx);
                }
            }
            // Ordinary command word (no alias): consumes command position.
            self.push(Token::Word(w), src_idx);
            self.eligible = false;
            return Ok(());
        }

        // Argument word.
        self.push(Token::Word(w), src_idx);
        self.eligible = false;
        Ok(())
    }

    /// If `t` is a reserved word recognized at command position, update
    /// context and return `Some(next_eligible)`. Returns `None` if `t` is
    /// not a reserved word (caller then tries alias expansion).
    fn handle_reserved(&mut self, t: &str) -> Option<bool> {
        match t {
            "case" => {
                self.ctx.push(Ctx::CaseSubject);
                Some(false)
            }
            "for" | "select" => {
                self.ctx.push(Ctx::ForName);
                Some(false)
            }
            "[[" => {
                self.ctx.push(Ctx::DoubleBracket);
                Some(false)
            }
            "function" => Some(false),
            "if" | "then" | "elif" | "else" | "do" | "while" | "until" | "{" | "!" | "time" => {
                Some(true)
            }
            "fi" | "done" | "}" => Some(false),
            "esac" => {
                if self.top() == Some(Ctx::CaseBody) {
                    self.ctx.pop();
                }
                Some(false)
            }
            _ => None,
        }
    }

    fn expand_alias(
        &mut self,
        name: String,
        body: String,
        src_idx: usize,
    ) -> Result<(), LexError> {
        self.active.insert(name.clone());
        let inner_tokens = crate::lexer::tokenize(&body)?;
        // The alias body begins at command position.
        self.eligible = true;
        for inner in inner_tokens {
            // Body tokens inherit the alias-name token's source index.
            self.feed(inner, src_idx)?;
        }
        self.active.remove(&name);
        // Trailing-blank rule: a body ending in whitespace makes the next
        // source token alias-eligible.
        self.eligible = body.chars().last().is_some_and(|c| c.is_whitespace());
        Ok(())
    }

    fn feed_op(&mut self, op: Operator, src_idx: usize) {
        match self.top() {
            Some(Ctx::CasePattern) => {
                self.push(Token::Op(op), src_idx);
                if matches!(op, Operator::RParen) {
                    *self.ctx.last_mut().unwrap() = Ctx::CaseBody;
                    self.eligible = true;
                } else {
                    // `|` (pattern alternative), leading `(`, or any other op:
                    // stay in pattern position.
                    self.eligible = false;
                }
            }
            Some(Ctx::CaseBody)
                if matches!(
                    op,
                    Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp
                ) =>
            {
                self.push(Token::Op(op), src_idx);
                *self.ctx.last_mut().unwrap() = Ctx::CasePattern;
                self.eligible = false;
            }
            Some(Ctx::ForName) | Some(Ctx::ForList) if matches!(op, Operator::Semi) => {
                self.push(Token::Op(op), src_idx);
                self.ctx.pop();
                self.eligible = true;
            }
            Some(Ctx::CaseSubject)
            | Some(Ctx::DoubleBracket)
            | Some(Ctx::ForName)
            | Some(Ctx::ForList) => {
                self.push(Token::Op(op), src_idx);
                self.eligible = false;
            }
            _ => {
                // CaseBody (non-;;) or empty stack: normal separator logic.
                self.push(Token::Op(op), src_idx);
                self.eligible = matches!(
                    op,
                    Operator::Pipe
                        | Operator::And
                        | Operator::Or
                        | Operator::Semi
                        | Operator::Background
                        | Operator::LParen
                );
            }
        }
    }

    fn feed_newline(&mut self, src_idx: usize) {
        match self.top() {
            Some(Ctx::ForName) | Some(Ctx::ForList) => {
                self.ctx.pop();
                self.push(Token::Newline, src_idx);
                self.eligible = true;
            }
            Some(Ctx::CaseSubject) | Some(Ctx::CasePattern) | Some(Ctx::DoubleBracket) => {
                self.push(Token::Newline, src_idx);
                self.eligible = false;
            }
            _ => {
                // CaseBody or empty stack: newline is a command separator.
                self.push(Token::Newline, src_idx);
                self.eligible = true;
            }
        }
    }
}

/// Returns the concatenated literal text of a Word iff every part is
/// an unquoted Literal. Returns None for any quoted, Var, Arith,
/// CommandSub, or Tilde part — aliases only expand from plain
/// unquoted identifiers.
pub(crate) fn simple_word_text(w: &Word) -> Option<String> {
    let mut text = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text: t, quoted: false } => text.push_str(t),
            _ => return None,
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn make_aliases(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// Compare two token streams by re-tokenizing the expected source
    /// (avoids hand-constructing complex Token::Word values).
    fn assert_tokens_eq(actual: &[Token], expected_source: &str) {
        let expected = tokenize(expected_source).expect("expected source must tokenize");
        assert_eq!(actual, &expected[..], "actual:\n  {:?}\nexpected:\n  {:?}", actual, expected);
    }

    #[test]
    fn simple_expansion() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "ls -l");
    }

    #[test]
    fn no_expansion_outside_command_position() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("echo ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "echo ll");
    }

    #[test]
    fn recursive_expansion() {
        let aliases = make_aliases(&[("ls", "ls --color"), ("ll", "ls -l")]);
        let toks = tokenize("ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "ls --color -l");
    }

    #[test]
    fn cycle_protection() {
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks = tokenize("ls").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // Only one substitution — the inner `ls` is in `active` and
        // does not re-expand.
        assert_tokens_eq(&out, "ls --color");
    }

    #[test]
    fn expansion_after_pipe() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("cat | ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "cat | ls -l");
    }

    #[test]
    fn expansion_after_semi() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("true; ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "true; ls -l");
    }

    #[test]
    fn trailing_space_chains_expansion() {
        // Note the trailing space in the `sudo` body.
        let aliases = make_aliases(&[("sudo", "sudo "), ("ll", "ls -l")]);
        let toks = tokenize("sudo ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "sudo ls -l");
    }

    #[test]
    fn quoted_word_not_expanded() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("'ll'").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // `'ll'` is a quoted Literal — `simple_word_text` returns None
        // because `quoted: true`. So no expansion fires.
        assert_eq!(out, tokenize("'ll'").unwrap());
    }

    #[test]
    fn mapped_expansion_tracks_source_indices() {
        // alias ll='ls -l'; tokens: [ll, /usr] → [ls, -l, /usr] with map [0,0,1].
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = crate::lexer::tokenize("ll /usr").unwrap();
        let (out, map) = super::expand_aliases_in_tokens_mapped(toks, &aliases).unwrap();
        let words: Vec<String> = out.iter().filter_map(|t| match t {
            crate::lexer::Token::Word(w) => super::simple_word_text(w), _ => None }).collect();
        assert_eq!(words, vec!["ls", "-l", "/usr"]);
        assert_eq!(map, vec![0, 0, 1]);
    }

    #[test]
    fn mapped_noop_is_identity() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = crate::lexer::tokenize("echo hi").unwrap(); // no alias at cmd pos
        let n = toks.len();
        let (out, map) = super::expand_aliases_in_tokens_mapped(toks, &aliases).unwrap();
        assert_eq!(out.len(), n);
        assert_eq!(map, (0..n).collect::<Vec<_>>());
    }

    #[test]
    fn case_pattern_word_not_expanded() {
        // The v231 regression: `ls` after `|` is a case pattern, not a command.
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks = tokenize("case $x in use | ls | list) echo hi ;; *) echo no ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case $x in use | ls | list) echo hi ;; *) echo no ;; esac");
    }

    #[test]
    fn case_subject_word_not_expanded() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("case ll in a) echo x ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case ll in a) echo x ;; esac");
    }

    #[test]
    fn case_body_command_is_expanded() {
        // Inside a clause body we ARE at command position.
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("case $x in a) ll ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case $x in a) ls -l ;; esac");
    }

    #[test]
    fn nested_case_patterns_not_expanded() {
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks =
            tokenize("case $x in a) case $y in ls) echo z ;; esac ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case $x in a) case $y in ls) echo z ;; esac ;; esac");
    }

    #[test]
    fn expand_after_then_and_do() {
        // The opposite latent bug: reserved words introduce command position.
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("if true; then ll; fi").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "if true; then ls -l; fi");

        let toks = tokenize("while true; do ll; done").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "while true; do ls -l; done");
    }

    #[test]
    fn reserved_word_not_expanded() {
        // An alias whose name is a reserved word is not expanded in that slot.
        let aliases = make_aliases(&[("then", "echo BAD")]);
        let toks = tokenize("if true; then echo ok; fi").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "if true; then echo ok; fi");
    }

    #[test]
    fn for_list_words_not_expanded_body_is() {
        let aliases = make_aliases(&[("ls", "ls --color"), ("ll", "ls -l")]);
        let toks = tokenize("for x in ls ll; do ll; done").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // `ls` and `ll` in the for-list stay literal; `ll` in the body expands
        // recursively: ll → ls -l → ls --color -l (same as recursive_expansion test).
        assert_tokens_eq(&out, "for x in ls ll; do ls --color -l; done");
    }

    #[test]
    fn double_bracket_interior_not_expanded() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("[[ ll == x ]]").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "[[ ll == x ]]");
    }

    #[test]
    fn mapped_indices_preserved_through_case() {
        // Offsets must still anchor to raw source token indices.
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks = tokenize("case $x in ls) echo z ;; esac").unwrap();
        let n = toks.len();
        let (out, map) = expand_aliases_in_tokens_mapped(toks, &aliases).unwrap();
        // No expansion happened, so output is identity-mapped.
        assert_eq!(out.len(), n);
        assert_eq!(map, (0..n).collect::<Vec<_>>());
    }
}
