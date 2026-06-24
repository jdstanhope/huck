# v211: huck-syntax API polish for external publication

## Goal

Bring the `huck-syntax` public API up to idiomatic Rust standards for external
publication. The crate is solid as an internal frontend (consumed today only
by `huck-engine` and `huck-cli`), but the API has four concrete friction
points surfaced while writing the `examples/{tokenize_dump,list_assignments}.rs`
walkthrough demos:

1. Error types render via free functions, not `Display`/`std::error::Error`.
2. Free-function renderers take owned `LexError` / `ParseError` (forces clones).
3. AST enums (`Command`, `WordPart`, etc.) lack `#[non_exhaustive]` — any new
   variant is a SemVer-breaking change for downstream matches.
4. No curated root re-exports; users hunt through 6 modules to find common types.
5. Crate-level doc comment is one-line; no embedding example.

Plus the `try_split_assignment` helper consumes its `Word` input — useful as a
peek-first variant.

This iteration breaks the v204-v208 embedding-arc / v209-v210 bash-compat
pattern for a focused API-ergonomics polish. No behavior changes; no shell
semantics changes; no bash-divergences.md edits.

## Background

`huck-syntax` was extracted from the runtime crate in v202 (merge `5531dbe`,
2026-06-22). At that point it was treated as an internal "frontend" library
whose only consumers were guaranteed to be in-workspace. v211 turns that
crate into something a third party could `cargo add` and use for shell
parsing — without re-running every breakage when a new variant lands or
needing to wrap our error types in their own.

The trigger is the v210 follow-on conversation: the user asked to review the
exported API and build usage examples. Building `examples/tokenize_dump.rs`
and `examples/list_assignments.rs` surfaced concrete frictions. Each polish
item in this iteration is grounded in a real friction observed in that work.

## Scope

**In scope:**

- **Display + std::error::Error** on `LexError`, `ParseError`, `BraceError`.
- **By-ref signatures** on the free-function renderers:
  - `lex_error_message(&LexError) -> String`.
  - `parse_error_message(&ParseError) -> String`.
- **`try_split_assignment_ref(&Word) -> Option<Assignment>`** — peek variant
  of the existing `try_split_assignment(Word) -> Result<Assignment, Word>`.
  Old function stays (consuming form is still useful).
- **`#[non_exhaustive]`** on the breakage-prone enums:
  - `lexer::Token`, `lexer::WordPart`, `lexer::ParamModifier`,
    `lexer::TransformOp`, `lexer::LexError`.
  - `command::Command`, `command::ParseError`.
  - `brace_expand::BraceError`.
  - Excluded (smaller, stable shape): `Operator`, `CaseDirection`,
    `SubstAnchor`, `SubscriptKind`, `Connector`, `CaseTerminator`,
    `TestBinaryOp`, `TestUnaryOp`, `FileMode`, `RedirOp`, `RedirFd`,
    `AssignTarget`, `DeclArg`, `SimpleCommand`, `ProcDir`, `TildeSpec`,
    `CharCursor`. (These are stable today; revisit if they start churning.)
- **Curated root re-exports** in `lib.rs`:
  - From `lexer`: `tokenize`, `tokenize_with_opts`, `Token`, `Word`,
    `WordPart`, `LexerOptions`, `LexError`, `ParamModifier`, `TransformOp`.
  - From `command`: `parse`, `Sequence`, `Command`, `Pipeline`,
    `SimpleCommand`, `ExecCommand`, `Assignment`, `AssignTarget`,
    `ParseError`.
  - From `generate`: `command_to_source`, `function_to_source`.
  - From `brace_expand`: `expand` (renamed `brace_expand` at root),
    `BraceError`.
- **Module-level doc comment** on `lib.rs` with a short embedding example
  (5-10 lines) showing the lex→parse→walk loop.
- Update internal consumers (`huck-engine`, `huck-cli`) to compile against
  the new signatures.
- Update existing `examples/{tokenize_dump,list_assignments}.rs` to use the
  new ergonomics (e.g., drop the `format!("{e:?}")` workaround, use `?`).

**Out of scope:**

- Behavior changes: lexing/parsing semantics stay byte-identical.
- Renaming exported types (`SimpleCommand`, `ExecCommand`, etc.) — names are
  load-bearing for in-workspace callers.
- Adding getters to replace `pub` fields. The fields are documented and
  load-bearing for in-workspace tests; field-access ergonomics matter less
  than the four items above. Defer.
- Field-access getters on `Word(pub Vec<WordPart>)`. Defer.
- `#[non_exhaustive]` on the smaller, stable enums (see exclusion list above).
- Publishing to crates.io — that's a separate decision; v211 just makes the
  crate ready for it.
- Versioning the crate up to 0.2.0 — defer to the publish decision.

## Behavioral / observable changes

None at runtime. Every change is at the API surface:

- New trait impls on existing types (`Display`, `Error`) — additive.
- New `#[non_exhaustive]` markers — forces internal `match` sites in
  `huck-engine` to use `_ =>` where they don't already. Plan addresses this
  per call site.
- Signature changes on two free functions and one helper — breaking for the
  2 known consumers; the v211 implementation updates them in lockstep.
- New `pub use` at crate root — additive (doesn't remove the module paths).

## Risk

1. **`#[non_exhaustive]` forces exhaustive-match call sites to use `_ =>`**.
   Internal call sites in `huck-engine` that match on `Command` /
   `WordPart` / etc. will fail to compile until updated. Plan: grep for
   every exhaustive match site and add `_ =>` arms (most arms already
   exist, so this is mechanical). Risk: silently miss a variant.
   Mitigation: cargo build is the source of truth; the lockstep updates
   in this iteration's commits exercise every site.

2. **Display impl byte-identical to the existing free function**. The
   `Display` body MUST produce the same string as today's
   `lex_error_message(LexError)` / `parse_error_message(ParseError)`,
   because user-facing error messages are tested via bash-diff harnesses.
   Plan: the new Display impls delegate to the existing free-function
   body; the free function becomes a thin wrapper. Zero
   user-facing change.

3. **Curated root re-exports might shadow** an existing user import path.
   Since `huck-syntax` has only 2 internal consumers and they import from
   the module paths (`huck_syntax::lexer::Token`), the root re-exports
   are purely additive at the public surface. Verify the 2 consumers
   continue to compile without changes.

4. **`try_split_assignment_ref` semantic parity with `try_split_assignment`**.
   The new helper must return `Some(_)` exactly when the consuming form
   returns `Ok(_)`, and `None` otherwise. Plan: implement the peek
   variant such that it shares the recognition logic with the consuming
   form (factor out a `recognize_assignment_shape(&Word) -> bool` or
   similar). Tests pin both shapes.

## Testing strategy

- Unit tests for each new `Display` impl: a representative `LexError`/
  `ParseError`/`BraceError` value produces the expected message.
- Unit test for `try_split_assignment_ref` parity with
  `try_split_assignment` on the same set of inputs (bare scalar,
  appended, indexed, compound array, dynamic-prefix non-assignment).
- Doctest in the `lib.rs` module-level comment exercising the lex →
  parse → walk example end-to-end.
- Full workspace test suite remains green: every existing test still
  passes after the signature updates ripple through huck-engine /
  huck-cli.
- Both `examples/*.rs` updated and `cargo build --examples -p
  huck-syntax` clean.

No bash-divergences.md change. No harness change.

## Documentation

- `lib.rs` module-level doc grows a 5-10 line embedding example.
- `docs/architecture.md` gains a sentence noting the crate's API
  readiness (no big restructure — just a pointer).
- `docs/bash-divergences.md` unchanged.

## Acceptance

- Full `cargo test --workspace` green.
- `cargo build --examples -p huck-syntax` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo doc --no-deps -p huck-syntax` builds without broken intra-doc
  links.
- Both v211-tail examples compile with the new ergonomics (no
  `format!("{e:?}")` workaround; no `LexError` clone).
- Each external-facing change is observable via `cargo doc` — Display
  + Error impls show on the error types; `#[non_exhaustive]` shows on
  the marked enums; root re-exports show on the crate page.
