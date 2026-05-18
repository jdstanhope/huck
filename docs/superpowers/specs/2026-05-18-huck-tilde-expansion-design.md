# huck Tilde Expansion Design (v9)

**Status:** Approved 2026-05-18.

## Overview

Add bash-style tilde expansion: `~`, `~/path`, `~user`, `~+` (= `$PWD`),
`~-` (= `$OLDPWD`), and their `/path` suffix forms. Tilde is also
expanded after unquoted `:` and `=` inside assignment-context words
(`PATH=~/bin:~/lib`). A successful `cd` now maintains both `PWD` and
`OLDPWD` so the `~+` / `~-` forms have something to resolve.

This is the first of three iterations covering the "essential" bash
expansion features. v10 will add pathname expansion (globbing) and v11
will add arithmetic expansion. Parameter-expansion modifiers
(`${var:-default}`, etc.) are explicitly deferred — see "Future" below.

## Goals

- `cd ~`, `cd ~/Downloads`, `cat ~alice/notes.txt`, `cd ~-` all work.
- `PATH=~/bin:~/lib:$PATH` expands every `~`.
- Unknown user / missing `HOME`/`PWD`/`OLDPWD` → leave the tilde
  construct as literal text (matches bash / POSIX).
- Tildes inside `'…'` or `"…"` stay literal (already true in v1).
- `cd` keeps `PWD` and `OLDPWD` exported and current.

## Non-goals (deferred or out of scope)

- `~N`, `~+N`, `~-N` — csh-style directory-stack indices. No `dirs`/
  `pushd`/`popd` today, so these can't resolve to anything meaningful.
- `cd -` shorthand. Worth doing later; not required for v9.
- Logical-vs-physical path bookkeeping (bash preserves the path the
  user `cd`'d through even across symlinks). `huck` uses
  `env::current_dir()`, which canonicalizes — documented divergence.

## Architecture

Tilde expansion is *lexical*: it depends on the position of `~` in the
input text and on the absence of surrounding quotes, both of which the
lexer already knows. We extend the existing `WordPart::Tilde` variant
(currently a unit) to carry a `TildeSpec` enum and teach the lexer to
recognize all four shapes plus the assignment-position rule. The
`expand`/`expand_assignment` paths resolve each spec against `Shell`
state; on failure they emit the original literal text. `cd` is updated
to set `PWD`/`OLDPWD` so the `+`/`-` forms have a source of truth.

No new dependencies.

## 1. Lexer changes

### `TildeSpec` and `WordPart::Tilde`

`src/lexer.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum TildeSpec {
    Home,            // ~
    User(String),    // ~alice
    Pwd,             // ~+
    OldPwd,          // ~-
}

pub enum WordPart {
    Literal(String),
    Tilde(TildeSpec),   // CHANGED: was unit
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
}
```

### Recognition

A `~` becomes a `Tilde(spec)` part when both:

**Position-eligible:**
- At the start of a word (current rule), OR
- Inside an *assignment-context* word, immediately after an unquoted
  `:` or `=`.

An "assignment-context word" is a word whose unquoted prefix matches
`[A-Za-z_][A-Za-z0-9_]*=`. The lexer detects this with a one-shot
lookahead at word start; if matched, a flag stays on for the rest of
the word.

**Tail matches one of:**

| Tail                           | Spec        |
| ------------------------------ | ----------- |
| empty / `/` / whitespace / op  | `Home`      |
| `+` then end / `/`             | `Pwd`       |
| `-` then end / `/`             | `OldPwd`    |
| `[A-Za-z_]\w*` then end / `/`  | `User(name)`|

Operator characters considered terminators: `|`, `<`, `>`, `&`, `;`.

If neither rule matches, the `~` (and any tentatively-consumed
following characters) are written back as ordinary literal text.

### Quoting

`'~'`, `"~"`, `'~alice'`, `"~+"`, etc. all stay literal. The lexer
already routes quoted text through paths that don't emit `Tilde`
parts; no change needed there.

### Examples

| Input                        | Word parts                                      |
| ---------------------------- | ----------------------------------------------- |
| `~`                          | `Tilde(Home)`                                   |
| `~/foo`                      | `Tilde(Home)`, `Literal("/foo")`                |
| `~alice/bin`                 | `Tilde(User("alice"))`, `Literal("/bin")`       |
| `~+`                         | `Tilde(Pwd)`                                    |
| `~-/old`                     | `Tilde(OldPwd)`, `Literal("/old")`              |
| `PATH=~/bin:~/lib`           | `Literal("PATH="), Tilde(Home), Literal("/bin:"), Tilde(Home), Literal("/lib")` |
| `echo a:~/b`                 | `Literal("echo")`, then `Literal("a:~/b")` (not an assignment, no expansion) |
| `'~'`                        | `Literal("~")`                                  |
| `"~alice"`                   | `Literal("~alice")`                             |
| `~?`                         | `Literal("~?")` (tail doesn't match any form)    |
| `~`                          | (already covered above)                         |

## 2. Expansion

`src/expand.rs` gains three helpers:

```rust
fn resolve_tilde(spec: &TildeSpec, shell: &Shell) -> Option<String> {
    match spec {
        TildeSpec::Home   => shell.get("HOME").map(str::to_string),
        TildeSpec::Pwd    => shell.get("PWD").map(str::to_string),
        TildeSpec::OldPwd => shell.get("OLDPWD").map(str::to_string),
        TildeSpec::User(name) => lookup_home_for_user(name),
    }
}

fn render_tilde_literal(spec: &TildeSpec) -> String {
    match spec {
        TildeSpec::Home        => "~".to_string(),
        TildeSpec::Pwd         => "~+".to_string(),
        TildeSpec::OldPwd      => "~-".to_string(),
        TildeSpec::User(name)  => format!("~{name}"),
    }
}

fn lookup_home_for_user(name: &str) -> Option<String> {
    // libc::getpwnam_r with a stack buffer (~1 KiB), one heap retry on ERANGE.
    // Returns Some(home) on success, None on missing user / any other
    // failure. Pseudocode:
    //   let cname = CString::new(name).ok()?;
    //   let mut buf = [0u8; 1024];
    //   let mut pwd: libc::passwd = zeroed();
    //   let mut result: *mut libc::passwd = ptr::null_mut();
    //   loop {
    //       let rc = unsafe { libc::getpwnam_r(cname.as_ptr(), &mut pwd,
    //           buf.as_mut_ptr() as *mut _, buf.len(), &mut result) };
    //       if rc == 0 && !result.is_null() {
    //           return unsafe { CStr::from_ptr(pwd.pw_dir) }
    //               .to_str().ok().map(str::to_string);
    //       }
    //       if rc == libc::ERANGE && buf.len() < 16384 {
    //           // grow once on a heap Vec and retry.
    //       }
    //       return None;
    //   }
}
```

Both existing `WordPart::Tilde` arms in `expand` and `expand_assignment`
become:

```rust
WordPart::Tilde(spec) => {
    let text = resolve_tilde(spec, shell)
        .unwrap_or_else(|| render_tilde_literal(spec));
    // expand path: push to current buffer, mark emitted.
    // expand_assignment path: push to single result string.
    current.push_str(&text);
    has_emitted = true;
}
```

(The `expand` path also keeps the snapshot-status pattern; the
`expand_assignment` path doesn't have `has_emitted`.)

### Fallback rationale

POSIX and bash both leave an unresolved tilde construct unchanged:
`echo ~nosuchuser` prints `~nosuchuser`. `render_tilde_literal`
reconstructs the original text from the spec — no need to thread the
raw source through the lexer.

## 3. `cd` maintenance of `PWD` and `OLDPWD`

`src/builtins.rs::builtin_cd`:

1. Resolve `target` (current logic).
2. `env::set_current_dir(target)`. On failure, return as today.
3. On success:
   - Compute `new_pwd = env::current_dir()?` (canonical absolute path).
     If this fails, fall back to skipping the PWD update with a warning;
     don't error the `cd` itself (the chdir succeeded).
   - If `shell.get("PWD")` is `Some(prev)`, call
     `shell.export_set("OLDPWD", prev.to_string())`.
   - `shell.export_set("PWD", new_pwd.to_string_lossy().to_string())`.
4. Return `Continue(0)`.

Both vars are written via `export_set` so subprocesses inherit them.

**On startup:** huck inherits `PWD`/`OLDPWD` from the parent shell (no
special handling needed — they're already in the environment huck
captures in `Shell::new`).

## 4. Edge cases & errors

| Scenario                                                       | Behavior                                                |
| -------------------------------------------------------------- | ------------------------------------------------------- |
| `~` with `HOME` unset                                          | Literal `~`                                              |
| `~+` with `PWD` unset                                          | Literal `~+`                                             |
| `~-` with `OLDPWD` unset (e.g., first cd of session)           | Literal `~-`                                             |
| `~nosuchuser`                                                  | Literal `~nosuchuser`                                    |
| `~alice` (passwd lookup fails for any reason)                  | Literal `~alice`                                         |
| `'~'` / `"~"` / `~` inside `${VAR}`                            | Literal (no `Tilde` part emitted)                       |
| `~` mid-word (e.g., `a~b`)                                     | Literal (no expansion)                                  |
| `echo a:~/b` (colon in non-assignment word)                    | Literal `a:~/b` (no expansion after `:`)                |
| `PATH=~/bin:~/lib` (assignment-context word)                   | Both tildes expand                                       |
| `cd` to a directory where `env::current_dir()` fails           | chdir succeeded; emit warning; skip PWD update           |
| `~` followed by junk (`~?`, `~$abc`)                           | Literal                                                  |

## 5. Testing

**Lexer (`src/lexer.rs::tests`):**
- All eight examples in the §1 table.
- Quoted variants: `'~'`, `"~"`, `'~alice'`, `"~+"`, `"~-"`.
- Assignment context true positives: `PATH=~/bin`, `PATH=~/bin:~/lib`,
  `X=~`, `X=~alice/b`.
- Assignment context false positives: `1ABC=~/x` (name starts with
  digit — not an assignment), `echo a:~/b` (no `=`).
- Mid-word `~`: `a~b`, `~/a~b`.
- Junk tails: `~?`, `~,`, `~$abc`.

**Expand (`src/expand.rs::tests`):**
- Each `TildeSpec` resolves correctly when its source var is set.
- Each falls back to its literal form when the source var is unset.
- `Tilde(User(_))` with a known user (use a real user — `root` is
  almost always present) — tagged `#[ignore]` or use a feature flag if
  we want CI hermeticity; otherwise inline-tested for the unknown-user
  case only.
- `Tilde(Home)` followed by `Literal("/x")` joins cleanly: `<home>/x`.
- `expand_assignment` with `Tilde(Home)` produces the joined string
  without word splitting.

**Builtins (`src/builtins.rs::tests`):**
- After `builtin_cd(&["/tmp"], &mut shell)`, `shell.get("PWD")` is
  `"/tmp"` and is exported.
- After two successive `cd` calls, `shell.get("OLDPWD")` matches the
  previous `PWD`.
- `cd` with `PWD` initially unset: `PWD` is set; `OLDPWD` remains unset.

**Manual smoke (real terminal, post-merge):**
1. `cd ~` → home dir.
2. `cd /tmp; cd ~-` → back to home.
3. `cd /var; cd /etc; cd ~-` → back to /var.
4. `ls ~/Downloads` (if it exists).
5. `cd ~root` if you have a `root` user (most systems).
6. `PATH=~/bin:~/lib; echo $PATH` → two real paths joined by colon.
7. `echo '~'` → prints literal `~`.
8. `echo a:~/b` → prints literal `a:~/b` (no expansion).

## 6. Files

| File              | Changes                                                                  |
| ----------------- | ------------------------------------------------------------------------ |
| `src/lexer.rs`    | `TildeSpec`, `WordPart::Tilde(TildeSpec)`, recognition for all four forms, assignment-context tracking, tests. |
| `src/expand.rs`   | `resolve_tilde`, `render_tilde_literal`, `lookup_home_for_user`, update both `expand` arms, tests. |
| `src/builtins.rs::builtin_cd` | Maintain `PWD`/`OLDPWD` via `export_set`; tests. |

No new crates. `libc` already in `Cargo.toml`.

## 7. Future (separate iterations)

- **v10:** Pathname expansion (globbing): `*`, `?`, `[abc]`.
- **v11:** Arithmetic expansion: `$((expr))`.
- **Later:** parameter-expansion modifiers (`${var:-x}`, `${var/pat/repl}`, etc.).
- **Later:** brace expansion (`{a,b,c}`, `{1..10}`).
- **Later:** `cd -` shorthand, directory stack (`pushd`/`popd`/`dirs`).
