# v27: Here-Strings `<<<word` — Design Spec

## Goal

Support POSIX/bash here-string redirection: `cmd <<<word` feeds the
expansion of `word` (with a trailing newline) as `cmd`'s stdin. Closes
M-13 from `docs/bash-divergences.md`.

Pre-v27: `cmd <<<word` is a parse error (`<<<` lexes as `<<` + `<`
which the parser rejects).

After v27, the user can:
```sh
cat <<< "hello $USER"
```
…and `cat` sees `hello jdstanhope\n` on stdin.

## Scope

One redirect form, full POSIX/bash semantics. Reuses the v24 deferred-
expansion + stdin-pipe machinery wholesale; the new lexer/AST/executor
work is small.

## Semantics

**Single Word body**: `<<<` is followed by one Word (subject to normal
word-boundary rules — whitespace ends it). Unlike heredocs there's no
delimiter or multi-line collection.

**Full expansion**: the body Word undergoes the same expansion as a
variable-assignment RHS — `$var`, `${var}`, `$(cmd)`, `` `cmd` ``,
tilde, arithmetic, parameter expansion. **No** word-splitting, **no**
globbing. Implementation: `expand_assignment(word, shell)`.

**Trailing newline appended**: after expansion, a literal `\n` is
appended so the child's stdin always ends with a newline. Matches bash
exactly; useful for line-oriented tools.

**Quoting affects only escape semantics, not splitting**: `<<<"hi"` and
`<<<hi` produce identical bytes (`hi\n`). Quoting matters when
expansion-relevant characters need literal handling — e.g.
`<<< "$FOO"` (FOO with spaces preserved as one string) vs
`<<< $FOO`. Since neither expand-splits in this context, the behavior
is identical even there. Quoting matters mostly for things like
`<<< '\$LITERAL'` (escapes the dollar) vs `<<< "\$LITERAL"` (escapes
the dollar inside double quotes, same result via the v24 escape set).

**Last-wins for stdin**: `cat <file <<<word` overrides `<file` with the
here-string (parser overwrites `stdin` field; second-parsed redirect
wins). Composes the same way with `<<EOF` heredocs and other `<<<word`s.

**Empty Word**: `<<<""` and `<<< $EMPTY` produce `\n` on stdin (one
empty line). Bash-equivalent.

**Per-stage scoping**: in a pipeline, each stage's here-string expands
with that stage's inline assignments visible. v23 inline-assignment +
v24 deferred-expansion design carries straight through.

## Lexer changes

`src/lexer.rs`:

**New `Operator::HereString`** variant for `<<<`.

The existing `<` arm peeks for `<<` (Heredoc), `<<-` (HeredocStrip).
Extend the peek-chain to check for a third `<`:

```rust
'<' => {
    if chars.peek() == Some(&'<') {
        chars.next(); // second '<'
        if chars.peek() == Some(&'<') {
            chars.next(); // third '<' — here-string
            tokens.push(Token::Op(Operator::HereString));
            in_assignment_value = false;
        } else if chars.peek() == Some(&'-') {
            // existing HeredocStrip path
        } else {
            // existing Heredoc path
        }
    } else {
        // existing RedirIn path
    }
}
```

No interaction with the heredoc pending-body queue — here-strings have
no body-collection phase. The Word that follows `<<<` is a normal
Token::Word that comes on the same line, parsed by the existing
word-lexing machinery.

**No new `Token` variant** — `Operator::HereString` + the next
`Token::Word` is enough.

## AST changes

`src/command.rs`:

```rust
pub enum Redirect {
    Read(Word),
    Truncate(Word),
    Append(Word),
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    HereString(Word),     // NEW: <<<word
}
```

The new variant matches `Read(Word)`'s shape — just a Word. The
`expand: bool` flag isn't needed because here-strings are always
expanding (no literal-mode variant in bash).

## Parser changes

The existing per-stage redirect-consuming code recognises `Token::Op(RedirIn)`
+ next-word → `Redirect::Read`. Add an arm:

```rust
Token::Op(Operator::HereString) => {
    let target_word = match iter.next() {
        Some(Token::Word(w)) => w,
        _ => return Err(ParseError::MissingRedirectTarget),
    };
    cmd.stdin = Some(Redirect::HereString(target_word));
}
```

(Reuse whatever the existing `MissingRedirectTarget`-style error is.)

Last-wins: parser overwrites `stdin` on each redirect. `cat <file <<<word`
→ `stdin = Some(Redirect::HereString(...))`. Matches bash.

## Executor changes

`src/executor.rs`:

**New variants** in the deferred-stdin enums (introduced in v24):

```rust
enum ResolvedStdin {
    File(String),
    Heredoc(Word),
    HereString(Word),       // NEW
}

enum StdinInput {
    File(File),
    DeferredHeredoc(Word),
    DeferredHereString(Word),  // NEW
}
```

**`resolve()`** translates `Redirect::HereString(w)` → `ResolvedStdin::HereString(w.clone())`.

**`open_stage_files`** stores `ResolvedStdin::HereString(body)` as
`StdinInput::DeferredHereString(body)`. (Defer expansion until per-stage
inline assignments are applied, exactly like v24's heredoc plumbing.)

**Expansion site** (in `run_multi_stage`, `run_subprocess`, and
`run_background_sequence` — wherever `DeferredHeredoc(body)` is
currently expanded to bytes):

```rust
StdinInput::DeferredHereString(body) => {
    let mut bytes = expand_assignment(&body, shell).into_bytes();
    bytes.push(b'\n');
    bytes
}
```

The bytes flow through the same `pending_input` → write-to-child-stdin
pipe machinery as heredocs.

**No new infrastructure**. Three small enum-arm additions in three
functions; one new expansion site.

## Edge cases

- **Empty word** (`cat <<<""` or `cat <<< $EMPTY`): body expands to `""`;
  trailing `\n` makes it `"\n"`. Child gets one empty line. Per bash.
- **Word with leading/trailing whitespace** (`cat <<< " hi "`): preserved
  literally because expand_assignment doesn't split. Body bytes are
  ` hi \n`.
- **Composition with heredoc** (`cat <<EOF <<<override\nignored\nEOF`):
  last-wins; here-string takes stdin. The heredoc body is still
  collected by the lexer (and discarded by the parser when stdin gets
  overwritten). Matches bash.
- **Pipeline stages** (`cat <<< first | grep first`): the here-string
  attaches to stage 0; stage 1 reads from the pipe. Per-stage as
  expected.
- **Inline assignment + here-string** (`FOO=hi cat <<< $FOO`): v23
  per-stage scoping + v24 deferred expansion → child sees `hi\n`.
- **Backgrounded** (`cat <<< body > /tmp/out &`): v25's
  `run_background_sequence` already supports DeferredHeredoc; the new
  DeferredHereString case lives next to it.
- **Single-quoted body** (`cat <<< '$FOO'`): the lexer's single-quote
  scanner produces a `Literal { quoted: true, text: "$FOO" }`;
  expand_assignment treats it as literal text; child gets `$FOO\n`.
- **`$?` snapshot** (`false; cat <<< $?`): B-07's snapshot semantics
  flow through expand_assignment; child gets `1\n` (pre-here-string `$?`).
- **Tilde in here-string** (`cat <<< ~`): tilde-expansion via
  expand_assignment → child gets the user's HOME followed by `\n`.

## Out of scope

- `&>` / `&>>` combined stdout+stderr redirects — separate audit entry M-19.
- Nested redirects beyond stdin (none exist for `<<<` — it's stdin-only by
  definition).

## Tests

### Lexer (`src/lexer.rs::tests`)

| Test | Covers |
| --- | --- |
| `tokenize_here_string_op_alone` | `<<<` → `Token::Op(Operator::HereString)` |
| `tokenize_here_string_with_unquoted_word` | `cat <<< hello` → `[Word("cat"), Op(HereString), Word("hello")]` |
| `tokenize_here_string_with_quoted_word` | `cat <<< "hi there"` → quoted Word body |
| `tokenize_here_string_with_var_in_body` | `cat <<< $FOO` → Var part in body |
| `tokenize_here_string_with_command_sub_in_body` | `cat <<< $(date)` → CommandSub part in body |
| `tokenize_double_less_still_heredoc` | regression: `cat <<EOF` stays Heredoc, not split into `<<` + `<EOF` |

### Parser (`src/command.rs::tests`)

| Test | Covers |
| --- | --- |
| `parse_here_string_attaches_to_stdin` | `cat <<< hi` → `ExecCommand.stdin = Some(Redirect::HereString(...))` |
| `parse_here_string_last_wins_over_file` | `cat <file <<< hi` → stdin is the here-string |
| `parse_here_string_last_wins_over_heredoc` | `cat <<EOF <<< override\nignored\nEOF` → stdin is the here-string |
| `parse_here_string_missing_word_errors` | `cat <<<` (no following word) → parse error |
| `parse_here_string_in_pipeline_stage` | `cat <<< x \| grep x` → stage 0 has HereString stdin; stage 1 doesn't |

### Integration (new `tests/here_string_integration.rs`)

| Test | Script | Expected |
| --- | --- | --- |
| `here_string_simple_word` | `cat <<< hello\nexit\n` | `hello\n` |
| `here_string_quoted_word` | `cat <<< "hello world"\nexit\n` | `hello world\n` |
| `here_string_expands_var` | `FOO=hi\ncat <<< $FOO\nexit\n` | `hi\n` |
| `here_string_expands_command_sub` | `cat <<< $(echo via-sub)\nexit\n` | `via-sub\n` |
| `here_string_with_inline_assignment` | `FOO=val cat <<< $FOO\nexit\n` | `val\n` |
| `here_string_in_pipeline_stage` | `cat <<< marker \| grep marker\nexit\n` | `marker\n` |
| `here_string_empty_word` | `cat <<< ""\nexit\n` | empty line (`\n`) |
| `here_string_no_split_with_spaces` | `FOO="a b c"\ncat <<< $FOO\nexit\n` | `a b c\n` (single line) |
| `here_string_last_wins_over_file` | `echo wrong > /tmp/huck_v27_test\ncat </tmp/huck_v27_test <<< right\nrm /tmp/huck_v27_test\nexit\n` | `right\n` |
| `here_string_trailing_newline_present` | `cat <<< hi \| wc -l\nexit\n` | `1` (one line, due to trailing \n) |
| `here_string_dollar_question_snapshot` | `false\ncat <<< $?\nexit\n` | `1\n` (B-07 snapshot) |
| `here_string_single_quoted_no_expand` | `FOO=hi\ncat <<< '$FOO'\nexit\n` | `$FOO\n` (single quotes prevent expansion) |
| `here_string_backgrounded` | `cat <<< body > /tmp/huck_v27_bg_$$ &\nwait\ncat /tmp/huck_v27_bg_$$\nrm -f /tmp/huck_v27_bg_$$\nexit\n` | `body\n` |

That's 6 lexer + 5 parser + 13 integration = 24 new tests.

### Doc updates

- `docs/bash-divergences.md`: M-13 → `[fixed (2026-05-26)]`; Tier 2 count adjusted; change-log entry.
- `README.md`: v27 row.

## Change log

- **2026-05-26**: Spec drafted; scope = `<<<word` with bash semantics
  (full expansion, no split/glob, trailing `\n`); new `Redirect::HereString`
  variant; reuses v24 deferred-expansion + stdin-pipe machinery.
