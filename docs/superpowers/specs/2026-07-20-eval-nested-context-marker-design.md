# v315 — `eval:` nested syntax-error context marker + eval line base

**Issue:** [#209](https://github.com/jdstanhope/huck/issues/209) — posix2 near-miss.
Phase 2 of the syntax-error-diagnostic work (phase 1 = v314 top-level shapes,
[#211](https://github.com/jdstanhope/huck/issues/211)). This is the piece posix2
needs to flip.

**Goal:** a syntax error raised while parsing an `eval` string is reported with
bash's `eval:` source marker and the outer line number where `eval` was
invoked, e.g. `<name>: eval: line 199: syntax error near unexpected token \`)'`.
The same root fix (an eval **line base**) also corrects `$LINENO` inside `eval`
(huck reports 1 where bash reports the outer line).

---

## The measured bash model

huck (post-v314) already matches bash's *wording* and its Shape 1/2/3
classification for nested errors; only the `eval:` **marker** and the **line
number** diverge. Command substitution (`$(…)`) already matches and is out of
scope (see below).

| input | bash | huck (post-v314) |
|---|---|---|
| `eval "case esac in esac) ;; esac"` (`-c`) | `eval: line 1: syntax error near unexpected token \`)'` + echo | `-c: line 1: …` |
| script line 3 = `eval "case esac in esac) ;; esac"` | `<name>: eval: line 3: …` | `<name>: line 1: …` |
| `eval "echo \$LINENO"` on script line 3 | `LINENO=3` | `LINENO=1` |
| posix2.tests:199 `eval 'case esac in esac) …'` | `<name>: eval: line 199: …` | `<name>: line 1: …` |

Two observations that shape the design:

1. **The `eval:` marker REPLACES `-c:`.** In `-c` mode bash prints `bash: eval:
   line N:` (no `-c:` segment); in script mode `<name>: eval: line N:`. So inside
   an eval, the marker is `eval:` and the `-c:` segment is suppressed.
2. **The line is the OUTER line where `eval` sits**, not the line within the
   eval string. posix2's eval is on script line 199 → `eval: line 199` (the eval
   string is a single line). This is the same number `$LINENO` should report
   inside that eval (bash: `LINENO=3` for eval on line 3).

huck's root problem: `eval_in_sink` (builtins.rs) reparses the joined string via
`process_line_in_sinks`, which numbers lines from 1 *within the string* and has
no notion of the outer line — so both the error line and the `current_lineno`
(`$LINENO`) stamp lose the outer line, and no `eval:` marker is emitted.

### Scope

- **In scope:** the `eval:` marker + an eval **line base** so single-line eval
  strings report the outer line EXACTLY (posix2 + the common `$LINENO` case),
  and `$LINENO` inside eval is corrected by the same base.
- **Multi-line eval strings:** get the base offset but are NOT guaranteed
  byte-exact — bash's multi-line eval line arithmetic has an off-by-one quirk
  (measured: eval on line 3, error on the eval string's 2nd physical line →
  bash reports line 5, huck will report 4). Documented as a known approximation;
  posix2 does not exercise it.
- **Out of scope:** the `command substitution:` marker. Command substitution
  `$(…)` already matches bash (a parse-time `$(case x in)` error is reported
  `-c:`, not `command substitution:`); bash only emits `command substitution:`
  when it reparses a comsub body at *expansion* time (e.g. an unterminated
  backtick body). Replicating that parse-vs-expansion-time distinction is a
  separate divergence posix2 does not need — filed as a follow-on issue.

---

## Design

One new field carries both signals (the marker and the base):

```rust
// Shell (shell_state.rs)
/// Set to Some(outer_line) while executing an `eval` string: the line where the
/// `eval` command was invoked. Drives the `eval:` syntax-error marker and the
/// line base for inner error lines and `$LINENO`. None at top level. v315 (#209).
pub eval_frame: Option<u32>,
```

Helper:

```rust
impl Shell {
    /// Line offset to add to an eval string's local (1-based) line numbers so
    /// they reflect the outer line where `eval` sits. 0 outside eval.
    pub fn line_base(&self) -> u32 {
        self.eval_frame.map_or(0, |n| n.saturating_sub(1))
    }
}
```

### Read site 1 — `eval_in_sink` sets the frame (builtins.rs)

`eval_in_sink` already saves/restores `xtrace_depth` around the inner
`process_line_in_sinks(&joined, …)`. Add the identical save/set/restore for
`eval_frame`, set to the outer eval line (already stamped into
`current_lineno` before the `eval` builtin runs):

```rust
    let saved_frame = shell.eval_frame;
    shell.eval_frame = Some(shell.current_lineno.max(1));
    let saved = shell.xtrace_depth;
    shell.xtrace_depth += 1;
    let r = crate::shell::process_line_in_sinks(&joined, shell, true, sink, err_sink);
    shell.xtrace_depth = saved;
    shell.eval_frame = saved_frame;
```

(`.max(1)` guards the top-level `current_lineno == 0` initial state so
`line_base()` stays 0 → local line 1 → reported line 1, matching bash's
`eval: line 1` in `-c 'eval "…"'`.)

### Read site 2 — the syntax-error line (shell.rs ~474)

The `Err(e)` arm computes `ln = 1 + <newline count to cursor>`. Add the base:

```rust
    let ln = shell.line_base() + 1 + line.as_bytes()[..off]
        .iter().filter(|&&b| b == b'\n').count() as u32;
```

Single-line eval string: `line_base()` = 199−1 = 198, newline count 0 →
`ln = 199`. ✓

### Read site 3 — the `$LINENO` stamp (executor.rs ~4191)

```rust
    shell.current_lineno = shell.line_base() + cmd.line;
```

Single-line eval string, `cmd.line == 1`: `198 + 1 = 199`. ✓ (Apply the same
`line_base()` addition at the sibling stamp sites that set `current_lineno` from
a parse-time `cmd.line` — executor.rs ~3837 assignment RHS and ~6735 pipeline
stage — so `$LINENO` is consistent across command kinds inside eval. Verify each
against `bash`.)

### Read site 4 — the marker (shell_state.rs `Diag::Syntax` arm, ~1244)

```rust
    Diag::Syntax { line } => {
        if !self.is_interactive {
            if self.eval_frame.is_some() {
                out.push_str("eval: ");
            } else if self.source_depth == 0 && self.is_command_string {
                out.push_str("-c: ");
            }
            out.push_str(&format!("line {line}: "));
        }
    }
```

The `eval:` marker replaces `-c:` (bash suppresses `-c:` inside eval), and works
in script mode too (`<name>: eval: line N:`, no `-c:` there anyway). The Shape-1
source-echo second line reuses the same prefix, so it inherits `eval:` for free.

---

## Testing

- **posix2 re-sweep MUST flip to PASS** — the #209 payoff. Run
  `HUCK_BASH_TEST_CATEGORY=posix2` and confirm.
- **New `tests/scripts/eval_line_diag_diff_check.sh`** (byte-diff huck vs bash,
  normalize only each shell's own `<name>:` prefix, compare stderr + rc):
  - `-c` eval error: `eval "case esac in esac) ;; esac"` → `eval: line 1: …` + echo.
  - Script eval error on line 3: a temp-file script whose line-3 `eval "case esac
    in esac)"` reports `eval: line 3: …`.
  - `$LINENO` inside eval on script line 3 → `3` (single-line eval string).
  - `-c` `eval "echo \$LINENO"` → `1`.
  - A NON-eval control (`case esac in esac)` at top level) still reports `-c:`
    (marker only appears inside eval).
- **`syntax_error_diag_diff_check.sh` stays green** — non-eval paths unaffected
  (`eval_frame` is `None`).
- **`$LINENO` regression guard:** run the existing LINENO tests
  (`/usr/bin/grep -rln 'LINENO' crates/huck-engine/src tests/`) — top-level and
  in-function `$LINENO` must be unchanged (`line_base()` is 0 outside eval).
- **File the follow-on issue** for the deferred `command substitution:` marker
  (parse-vs-expansion-time), referencing #209.

## Rejected alternatives

- **Fix only the error-message line, not `$LINENO`.** Same root (eval loses the
  outer line); fixing the line base corrects both, and a special-cased
  error-only path would be a hack that leaves `$LINENO` wrong.
- **A separate boolean `in_eval` flag + a separate `eval_line_base` field.** Two
  fields that must stay in sync; `Option<u32>` carries both the marker signal
  (`is_some()`) and the base in one field.
- **Take on the `command substitution:` marker now.** Deep parse-vs-expansion
  bash-internals distinction, no posix2 benefit, `$(…)` already matches —
  separate follow-on.
- **Make eval's inner parse carry an AST line offset.** Larger change to the
  parser's line assignment; the `line_base()` addition at the three stamp/emit
  sites is smaller and localized.
