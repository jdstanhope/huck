//! The command-position state machine (v361, #641).
//!
//! `Mode::Command` used to be a fieldless variant whose sub-states lived as
//! three parallel booleans on the `Lexer`: `cmd_at_word_start` (23 write sites),
//! `in_assignment_value` (7) and `assign_val_tilde_ok` (10). Three booleans is
//! eight representable combinations for what is really two small state machines,
//! and nothing prevented reaching a meaningless one.
//!
//! Which combinations actually occur was MEASURED, not inferred from the write
//! sites, by arming each hypothesis as a `debug_assert!` and running the 250-form
//! `${…}` corpus and 3103 real scripts through `huck -n`:
//!
//! | hypothesis | result |
//! | --- | --- |
//! | word-start and assignment-value co-occur | **reachable** — fired on 1684 of 3103 scripts |
//! | tilde eligibility outside an assignment value | never, 0 of 3103 |
//! | tilde eligibility at word start | never, 0 of 3103 |
//!
//! The first is why this is two dimensions rather than one enum: they describe
//! DIFFERENT NESTING LEVELS. In `x=$(echo hi)` the substitution body is at a
//! fresh word start while the outer word is still an assignment value, because
//! `$(` sets the word position for the body it is about to scan.
//!
//! That is also the known wart. The word position is per-nesting-level state kept
//! in one global slot, so entering a command substitution CLOBBERS the outer
//! word's position instead of stacking it — and `Lexer::clear_cmd_at_word_start`
//! exists as the parser's compensation for the leak on the continuation path.
//! Moving the position onto the frame would fix that by construction, but it
//! changes behaviour (the outer position would be restored on pop rather than
//! left clobbered), so it is deliberately NOT done here: v361 is a re-encoding
//! that must stay inert. See #641.
//!
//! The other two hypotheses are why tilde eligibility is a field of
//! `AssignCtx::Value` rather than a third flag: outside an assignment value it
//! is now unrepresentable rather than merely unused.

/// Where the scanner sits in the word it is currently building.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WordPos {
    /// A fresh word begins here: `#` opens a comment, `~` is tilde-special, and
    /// an assignment prefix (`name=`) may be peeled.
    Start,
    /// Inside a word already under construction; `#` and `~` are literal.
    Mid,
}

/// The enclosing word's assignment context — a different nesting level from
/// `WordPos`, which is why the two vary independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AssignCtx {
    /// Not scanning an assignment's right-hand side.
    No,
    /// Scanning the RHS of `name=…`. `after_sep` is true when the previous
    /// unquoted literal char was the assigning `=` or an embedded `:`, which is
    /// exactly when bash tilde-expands what follows. A second embedded `=` does
    /// NOT re-enable it (`h=HOME=~` stays literal, #294).
    Value { after_sep: bool },
}

/// The command-position state: one `WordPos`, one `AssignCtx`, changed only
/// through the transitions below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommandPos {
    word: WordPos,
    assign: AssignCtx,
}

impl Default for CommandPos {
    /// Input begins at a fresh word, outside any assignment.
    fn default() -> Self {
        CommandPos {
            word: WordPos::Start,
            assign: AssignCtx::No,
        }
    }
}

impl CommandPos {
    // ── queries ──────────────────────────────────────────────────────────────

    pub(crate) fn at_word_start(self) -> bool {
        matches!(self.word, WordPos::Start)
    }

    pub(crate) fn in_assignment_value(self) -> bool {
        matches!(self.assign, AssignCtx::Value { .. })
    }

    /// True when a `~` here would be tilde-expanded as an assignment value's
    /// prefix — i.e. inside a value, directly after the `=` or an embedded `:`.
    pub(crate) fn tilde_eligible_in_value(self) -> bool {
        matches!(self.assign, AssignCtx::Value { after_sep: true })
    }

    // ── transitions ──────────────────────────────────────────────────────────

    /// A blank, newline or operator was emitted: the next word-content atom
    /// starts a fresh word and is no longer in an assignment value.
    pub(crate) fn boundary(self) -> Self {
        CommandPos {
            word: WordPos::Start,
            assign: AssignCtx::No,
        }
    }

    /// Beginning to scan one atom of word content. Tilde eligibility is
    /// default-cleared here; only `end_literal_run` re-establishes it, because a
    /// non-literal part (an expansion, a quoted run) ends the unquoted `=`/`:`
    /// run that made a following `~` eligible.
    pub(crate) fn begin_atom(self) -> Self {
        CommandPos {
            word: self.word,
            assign: match self.assign {
                AssignCtx::No => AssignCtx::No,
                AssignCtx::Value { .. } => AssignCtx::Value { after_sep: false },
            },
        }
    }

    /// This atom is word content: whatever follows is mid-word.
    pub(crate) fn enter_word_content(self) -> Self {
        CommandPos {
            word: WordPos::Mid,
            assign: self.assign,
        }
    }

    /// A literal run ended. Inside an assignment value, a following `~` is
    /// tilde-eligible iff the run's last unquoted char was `:`.
    pub(crate) fn end_literal_run(self, ended_on_colon: bool) -> Self {
        CommandPos {
            word: WordPos::Mid,
            assign: match self.assign {
                AssignCtx::No => AssignCtx::No,
                AssignCtx::Value { .. } => AssignCtx::Value {
                    after_sep: ended_on_colon,
                },
            },
        }
    }

    /// An assignment prefix (`name=`, `name+=`, `name[i]=`) was just consumed:
    /// what follows is the value, mid-word, and tilde-eligible right after `=`.
    pub(crate) fn begin_assignment_value(self, after_sep: bool) -> Self {
        CommandPos {
            word: WordPos::Mid,
            assign: AssignCtx::Value { after_sep },
        }
    }

    /// Entering a nested command context (a `$(…)` body, an injected alias
    /// body): its first atom is at a fresh word start. The enclosing assignment
    /// context is deliberately untouched — it belongs to the OUTER word, and
    /// leaving it alone is what keeps this re-encoding inert.
    pub(crate) fn enter_nested_command(self) -> Self {
        CommandPos {
            word: WordPos::Start,
            assign: self.assign,
        }
    }

    /// Resuming the outer word after a nested command context, which would
    /// otherwise leave its fresh word start behind (see the module docs).
    pub(crate) fn resume_outer_word(self) -> Self {
        CommandPos {
            word: WordPos::Mid,
            assign: self.assign,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_begins_at_a_fresh_word_outside_any_assignment() {
        let p = CommandPos::default();
        assert!(p.at_word_start());
        assert!(!p.in_assignment_value());
        assert!(!p.tilde_eligible_in_value());
    }

    #[test]
    fn a_boundary_resets_both_dimensions() {
        let p = CommandPos::default()
            .begin_assignment_value(true)
            .boundary();
        assert!(p.at_word_start());
        assert!(!p.in_assignment_value());
        assert!(!p.tilde_eligible_in_value());
    }

    #[test]
    fn word_content_leaves_the_assignment_context_alone() {
        // The two dimensions are independent: entering word content moves the
        // word position without leaving the value.
        let p = CommandPos::default()
            .begin_assignment_value(true)
            .enter_word_content();
        assert!(!p.at_word_start());
        assert!(p.in_assignment_value());
    }

    #[test]
    fn tilde_eligibility_survives_only_a_run_ending_on_a_colon() {
        let in_value = CommandPos::default().begin_assignment_value(true);
        assert!(in_value.tilde_eligible_in_value(), "seeded right after `=`");
        assert!(
            in_value.end_literal_run(true).tilde_eligible_in_value(),
            "a run ending on `:` keeps a following `~` eligible"
        );
        assert!(
            !in_value.end_literal_run(false).tilde_eligible_in_value(),
            "a run ending on anything else does not — `h=HOME=~` is literal (#294)"
        );
    }

    #[test]
    fn tilde_eligibility_cannot_exist_outside_an_assignment_value() {
        // Measured: 0 of 3103 real scripts ever had it. Here it is structural —
        // `after_sep` is a field of `Value`, so there is no way to express it.
        let outside = CommandPos::default();
        assert!(!outside.tilde_eligible_in_value());
        assert!(!outside.end_literal_run(true).tilde_eligible_in_value());
        assert!(!outside.begin_atom().tilde_eligible_in_value());
    }

    #[test]
    fn beginning_an_atom_clears_eligibility_but_not_the_value() {
        let p = CommandPos::default()
            .begin_assignment_value(true)
            .begin_atom();
        assert!(p.in_assignment_value());
        assert!(!p.tilde_eligible_in_value());
    }

    #[test]
    fn a_nested_command_starts_a_word_without_leaving_the_outer_value() {
        // The measured H1 case: `x=$(echo hi)` — 1684 of 3103 scripts.
        let p = CommandPos::default()
            .begin_assignment_value(true)
            .enter_word_content()
            .enter_nested_command();
        assert!(p.at_word_start(), "the body begins a fresh word");
        assert!(
            p.in_assignment_value(),
            "the OUTER word is still an assignment"
        );
    }

    #[test]
    fn resuming_the_outer_word_undoes_only_the_word_position() {
        let p = CommandPos::default()
            .begin_assignment_value(false)
            .enter_nested_command()
            .resume_outer_word();
        assert!(!p.at_word_start());
        assert!(p.in_assignment_value());
    }
}
