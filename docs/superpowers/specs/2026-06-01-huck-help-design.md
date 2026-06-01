# huck v67 — `help` builtin (M-81)

## Goal

Add bash's `help` builtin so interactive users can discover
huck's builtins and their usage. Ships with help text authored
for all ~42 current builtins.

After v67:

- `help` (no args) — lists all builtins as `name: synopsis`
  lines, sorted alphabetically.
- `help cd` — prints synopsis + indented description.
- `help -s cd` — synopsis only.
- `help -d cd` — description only.
- `help -m cd` — man-page format with NAME/SYNOPSIS/DESCRIPTION
  sections.
- `help cd echo` — multi-arg: emit for each.
- `help bad_name` — "no help topics match" + exit 1.
- `help -X` (unknown flag) — exit 2 + stderr.

No glob patterns in v67 (Tier C deferred).

New tracked divergence: **M-81: `help`**.

## Scope decisions (locked via AskUserQuestion)

**Tier B** — synopsis + description + `-s` / `-d` / `-m` flags;
no glob patterns. Multi-arg supported.

## Out of scope (deferred)

- `help cd*` glob patterns (Tier C).
- The detailed long-form descriptions bash ships (would need to
  more-or-less copy/translate bash's help text — copyright
  concern). huck authors its own concise descriptions matching
  huck's actual behavior.
- Help entries for shell keywords (`if`, `while`, etc.). bash's
  `help` includes those; we focus on actual `BUILTIN_NAMES`
  entries for v67.

## Architecture

### `HelpEntry` struct + static table

```rust
struct HelpEntry {
    name: &'static str,
    synopsis: &'static str,
    description: &'static str,
}

/// Help entries for every builtin in BUILTIN_NAMES. Sorted by
/// name for the listing form. `.` and `source` share an entry
/// (point at the same struct). `[` and `test` share. `declare`
/// and `typeset` share.
static HELP_ENTRIES: &[HelpEntry] = &[
    HelpEntry {
        name: ".",
        synopsis: ". FILENAME [ARGUMENTS]",
        description: "Execute commands from a file in the current shell.\n\
                      Reads and executes commands from FILENAME in the current shell context.\n\
                      If FILENAME does not contain a slash, $PATH is searched.\n\
                      Equivalent to `source FILENAME`.",
    },
    HelpEntry {
        name: ":",
        synopsis: ":",
        description: "Null command. Always exits 0.\n\
                      Arguments are expanded normally; useful with parameter expansion side\n\
                      effects (e.g., `: ${VAR:=default}`).",
    },
    // ... ~40 more entries (full list in plan)
];
```

### Lookup

```rust
fn find_help(name: &str) -> Option<&'static HelpEntry> {
    HELP_ENTRIES.iter().find(|e| e.name == name)
}
```

Linear scan (~42 entries) is fine for human-typed `help X`.

### `builtin_help`

```rust
fn builtin_help(
    args: &[String],
    out: &mut dyn std::io::Write,
    _shell: &mut Shell,
) -> ExecOutcome {
    // Parse flags: -s, -d, -m. Cluster supported.
    let mut want_synopsis = false;
    let mut want_description = false;
    let mut want_man = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" { i += 1; break; }
        if !arg.starts_with('-') || arg.len() < 2 { break; }
        for &c in &arg.as_bytes()[1..] {
            match c {
                b's' => want_synopsis = true,
                b'd' => want_description = true,
                b'm' => want_man = true,
                other => {
                    eprintln!("huck: help: -{}: invalid option", other as char);
                    return ExecOutcome::Continue(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];

    if names.is_empty() {
        for entry in HELP_ENTRIES {
            let _ = writeln!(out, "{}: {}", entry.name, entry.synopsis);
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit = 0;
    for name in names {
        match find_help(name) {
            Some(entry) => emit_help_entry(
                entry, out, want_synopsis, want_description, want_man,
            ),
            None => {
                eprintln!("huck: help: no help topics match `{name}'");
                exit = 1;
            }
        }
    }
    ExecOutcome::Continue(exit)
}

fn emit_help_entry(
    entry: &HelpEntry,
    out: &mut dyn std::io::Write,
    want_synopsis: bool,
    want_description: bool,
    want_man: bool,
) {
    if want_man {
        let _ = writeln!(out, "NAME");
        let _ = writeln!(out, "    {}", entry.name);
        let _ = writeln!(out);
        let _ = writeln!(out, "SYNOPSIS");
        let _ = writeln!(out, "    {}", entry.synopsis);
        let _ = writeln!(out);
        let _ = writeln!(out, "DESCRIPTION");
        for line in entry.description.lines() {
            let _ = writeln!(out, "    {}", line);
        }
        return;
    }
    // -s alone: synopsis-only line.
    if want_synopsis && !want_description {
        let _ = writeln!(out, "{}: {}", entry.name, entry.synopsis);
        return;
    }
    // -d alone: description-only block.
    if want_description && !want_synopsis {
        for line in entry.description.lines() {
            let _ = writeln!(out, "{}", line);
        }
        return;
    }
    // Default (or -sd combined): synopsis + indented description.
    let _ = writeln!(out, "{}: {}", entry.name, entry.synopsis);
    for line in entry.description.lines() {
        let _ = writeln!(out, "    {}", line);
    }
}
```

### Dispatch + `BUILTIN_NAMES`

- `"help"` added to `BUILTIN_NAMES`.
- NOT in `is_special_builtin` (bash-specific, regular).
- `"help" => builtin_help(args, out, shell)` dispatch arm.

## Content authoring guidelines

Each `HelpEntry` should have:
- **Synopsis**: bash-style one-liner with `[OPTIONAL]` brackets,
  uppercase placeholders. Match bash's `help -s NAME` style.
- **Description**: 1–4 lines. Concise prose describing what the
  builtin does in huck. Capture huck-specific divergences when
  significant. Do NOT copy bash's help text verbatim
  (copyright); paraphrase.

Tone: match bash's clipped style — declarative present tense,
no marketing.

### Examples

```rust
HelpEntry {
    name: "alias",
    synopsis: "alias [-p] [NAME[=VALUE] ...]",
    description: "Define or display aliases.\n\
                  With no arguments, print all defined aliases. With NAME but no value,\n\
                  print that alias's value. With NAME=VALUE, define the alias.\n\
                  Aliases expand at command-name position in interactive input.",
},
HelpEntry {
    name: "cd",
    synopsis: "cd [DIR]",
    description: "Change the shell working directory.\n\
                  With no argument, cd to $HOME.\n\
                  Updates $PWD and $OLDPWD.\n\
                  `cd -` cd's to $OLDPWD (the previous directory) and prints the new PWD.",
},
HelpEntry {
    name: "declare",
    synopsis: "declare [-rxifFp] [+rxi] [NAME[=VALUE] ...]",
    description: "Declare variables and set attributes.\n\
                  -r mark readonly; -x mark exported; -i mark integer (RHS arith-evaluated);\n\
                  +x un-export; +i unmark integer; +r errors (readonly cannot be removed).\n\
                  -f / -F list shell functions; -p print declarations of named vars.\n\
                  Inside a function (and without -g), declarations are local-scoped.\n\
                  Synonym: typeset.",
},
```

## Behavior table

| Input | Behavior |
|---|---|
| `help` | List all entries `name: synopsis`, sorted, exit 0 |
| `help cd` | "cd: cd [DIR]\n    <description lines>" |
| `help -s cd` | "cd: cd [DIR]" only |
| `help -d cd` | description block only (no synopsis prefix) |
| `help -m cd` | NAME/SYNOPSIS/DESCRIPTION sections |
| `help cd echo` | both entries shown in default form |
| `help -s cd echo` | both as synopsis lines |
| `help notabuiltin` | stderr "no help topics match" + exit 1 |
| `help cd notabuiltin` | cd shown + stderr for notabuiltin + exit 1 |
| `help -X` | stderr "invalid option" + exit 2 |
| `help -sd cd` | both flags set → default form (synopsis + description) |

## Test plan

### Unit tests in `src/builtins.rs::mod help_tests` (8 tests)

1. `help_no_args_lists_all` — `run_builtin("help", &[], …)` →
   stdout has lines for at least `cd`, `echo`, `eval`. (Don't
   require an exact full-list match — content is fluid.)
2. `help_named_builtin_default_form` — `help cd` → output
   contains "cd: " and at least one indented "    " line.
3. `help_synopsis_only` — `help -s echo` → exactly "echo: ..."
   single line; no indentation.
4. `help_description_only` — `help -d echo` → output does NOT
   start with "echo: " (description only, no synopsis prefix).
5. `help_man_format` — `help -m echo` → output contains
   "NAME", "SYNOPSIS", "DESCRIPTION" sections.
6. `help_invalid_option` — `help -X` → exit 2.
7. `help_not_found` — `help __no_such_builtin__` → exit 1 + (we
   can't easily capture stderr via run_builtin helper; just
   assert exit 1).
8. `help_multi_name_partial_miss` — `help cd __no_such__` →
   exit 1 (overall) but cd's content is in stdout.

### Integration tests in `tests/help_integration.rs` (5 tests)

1. `help_lists_known_builtin` — `help` → stdout contains "cd:"
   and "echo:".
2. `help_named_includes_synopsis_and_description` — `help cd` →
   stdout has "cd:" and at least one indented continuation line.
3. `help_s_synopsis_only` — `help -s echo` → exactly one line
   like "echo: echo …".
4. `help_unknown_errors` — `help __no_such_builtin__` → stderr
   "no help topics match", rc=1.
5. `help_man_format_has_sections` — `help -m cd` → stdout has
   "NAME", "SYNOPSIS", "DESCRIPTION" lines.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **builtin_help + HELP_ENTRIES content + 8 unit tests** —
   `src/builtins.rs`. Authoring ~39 unique `HelpEntry` literals
   for the current 42-name `BUILTIN_NAMES` (`.`/`source`,
   `[`/`test`, `declare`/`typeset` share content but each needs
   its own entry keyed by name).

2. **Integration tests** — `tests/help_integration.rs` (5
   scenarios).

3. **Docs** — M-81 entry; change-log; README v67 row.

## Acceptance criteria

- 8 unit tests pass.
- 5 integration tests pass.
- `cargo test --all-targets` green.
- `cargo clippy --all-targets -- -D warnings` clean.
- `help` is regular (NOT in `is_special_builtin`).
- All current `BUILTIN_NAMES` entries (42) have a `HelpEntry`.
  `help BUILTIN_NAME` works for every one.
- Tier B flags work: `-s`, `-d`, `-m`. Combinations: `-sd` →
  default form.
- M-81 doc entry added.
