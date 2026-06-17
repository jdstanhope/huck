# v176: redirect on a compound-command pipeline stage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a trailing redirect attach to a compound-command pipeline stage (`echo a | (cat) > f`, `grep … | { … } > "$out"`), matching bash.

**Architecture:** `parse_next_stage` (the pipeline-stage parser) returns compound stages without parsing a trailing redirect, unlike its sibling `parse_command_inner`. Mirror `parse_command_inner`: wrap each compound arm with `maybe_wrap_redirects`. Pure parser change — the executor already honors a `Redirected`-wrapped compound stage.

**Tech Stack:** Rust (the huck parser, `src/command.rs`); bash-diff harness; the parse-compat sweep (`tools/parse_sweep.sh`).

**Spec:** `docs/superpowers/specs/2026-06-17-pipeline-compound-redirect-design.md`

**Branch:** `v176-pipeline-compound-redirect`

**Background facts (verified):**
- huck rejects `echo a | (cat) > f`, `echo a | {cat;} > f`, `echo a | (cat) 2>&1`, `(echo a)|{cat;}>f`, and the same nested in `$( … )` with "unexpected token after command"; bash accepts all. ~18 real scripts (kernel `syscall*` family, lvmdump, setupcon, zgrep, xzgrep, nvm, pktgen/functions.sh) hit this.
- Works already (so the fix is specific): compound stage with NO redirect (`echo a | (cat)`); redirect on a plain stage (`echo a | cat > f`); standalone `(…) > f`.
- `maybe_wrap_redirects(cmd, iter)` (`command.rs:2097`) parses trailing redirects and returns `Command::Redirected { inner, redirects }` (or `cmd` unchanged if none).
- `parse_command_inner` (`command.rs:1030`) is the model: it wraps every compound arm with `maybe_wrap_redirects`.
- Execution already correct: `classify_stage` (`executor.rs:5880`) routes a non-bare-external stage to `InProcess` → `fork_and_run_in_subshell` → `run_command`, whose `Command::Redirected` arm (`executor.rs:527`) applies the redirect inside the forked stage over the pipe fd. NO executor change.
- `huck -n <file>` is parse-only (verified side-effect-free), rc 0 on a clean parse.

---

### Task 1: Wrap compound pipeline-stage arms with `maybe_wrap_redirects`

**Files:** Modify `src/command.rs` (function `parse_next_stage`, ~lines 2270–2325).

- [ ] **Step 1: Wrap the arith-block stage arm**

In `parse_next_stage`, the arith-block branch currently ends:
```rust
        return Ok((Command::Arith(body), false));
```
Change it to:
```rust
        return Ok((maybe_wrap_redirects(Command::Arith(body), iter)?, false));
```

- [ ] **Step 2: Wrap the keyword compound arms**

In the `match iter.peek().and_then(keyword_of)` block, replace these arms:
```rust
        Some(Keyword::If) => Ok((Command::If(Box::new(parse_if(iter)?)), false)),
        Some(Keyword::While) | Some(Keyword::Until) => {
            Ok((Command::While(Box::new(parse_while(iter)?)), false))
        }
        Some(Keyword::For) => Ok((parse_for_command(iter)?, false)),
        Some(Keyword::Select) => {
            iter.next(); // consume `select`
            Ok((parse_select_command(iter)?, false))
        }
        Some(Keyword::Case) => Ok((Command::Case(Box::new(parse_case(iter)?)), false)),
        Some(Keyword::LBrace) => {
            Ok((Command::BraceGroup(Box::new(parse_brace_group(iter)?)), false))
        }
        Some(Keyword::DoubleBracketOpen) => Ok((parse_double_bracket(iter)?, false)),
```
with (each compound now wrapped via `maybe_wrap_redirects(…, iter)?`):
```rust
        Some(Keyword::If) => Ok((
            maybe_wrap_redirects(Command::If(Box::new(parse_if(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::While) | Some(Keyword::Until) => Ok((
            maybe_wrap_redirects(Command::While(Box::new(parse_while(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::For) => {
            let cmd = parse_for_command(iter)?;
            Ok((maybe_wrap_redirects(cmd, iter)?, false))
        }
        Some(Keyword::Select) => {
            iter.next(); // consume `select`
            let cmd = parse_select_command(iter)?;
            Ok((maybe_wrap_redirects(cmd, iter)?, false))
        }
        Some(Keyword::Case) => Ok((
            maybe_wrap_redirects(Command::Case(Box::new(parse_case(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::LBrace) => Ok((
            maybe_wrap_redirects(Command::BraceGroup(Box::new(parse_brace_group(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::DoubleBracketOpen) => {
            let cmd = parse_double_bracket(iter)?;
            Ok((maybe_wrap_redirects(cmd, iter)?, false))
        }
```
(Leave the `Coproc`, `Some(other)`, and the function-def / simple-stage handling in the `None` arm UNCHANGED.)

- [ ] **Step 3: Wrap the bare-`(` subshell stage**

In the `None` arm, the subshell branch currently is:
```rust
            // Bare `(` at pipeline-stage position → subshell.
            if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
                return Ok((parse_subshell(iter)?, false));
            }
```
Change the return to wrap with `maybe_wrap_redirects`:
```rust
            // Bare `(` at pipeline-stage position → subshell.
            if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
                let cmd = parse_subshell(iter)?;
                return Ok((maybe_wrap_redirects(cmd, iter)?, false));
            }
```
(Do NOT touch the function-def branch `parse_function_def(w, iter)?` or the two `parse_simple_stage(...)` calls below it — simple stages parse their own redirects.)

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`. (`maybe_wrap_redirects` is already defined in `command.rs`; no imports.)

- [ ] **Step 5: Verify the fix + execution + regression (vs bash)**

Run:
```bash
H="$(pwd)/target/debug/huck"
echo "--- parse-only: these must now parse (no output = OK) ---"
for frag in \
  'echo a | ( cat ) > /tmp/o' \
  'echo a | { cat; } > /tmp/o' \
  'echo a | ( cat ) 2>&1' \
  '( echo a ) | { cat; } > /tmp/o' \
  'x=$( echo a | { cat; } > /dev/null )' \
  'seq 3 | while read x; do echo "$x"; done > /tmp/o' \
  'printf "1\n2\n" | case x in x) cat;; esac > /tmp/o'; do
  printf '%s\n' "$frag" | "$H" -n /dev/stdin 2>&1 | sed "s|^|  FAIL($frag): |"
done
echo "--- execute: redirect goes to the file, output matches bash ---"
for frag in \
  'echo hi | ( cat ) > /tmp/v176a; cat /tmp/v176a' \
  'printf "1 a\n2 b\n" | tail -n1 | ( read n x; echo "$n=$x" ) > /tmp/v176b; cat /tmp/v176b' \
  'seq 3 | { tr "\n" "-"; } > /tmp/v176c; cat /tmp/v176c; echo' \
  'echo keep | ( cat ); echo also'; do
  b=$(bash -c "$frag" 2>&1); h=$("$H" -c "$frag" 2>&1)
  [ "$b" = "$h" ] && echo "  OK: [$h]" || echo "  MISMATCH bash=[$b] huck=[$h]  <= $frag"
done
rm -f /tmp/v176a /tmp/v176b /tmp/v176c /tmp/o
```
Expected: the parse-only loop prints NOTHING (all seven parse); the execute loop prints `OK:` for all four (`hi`, `2=b`, `1-2-3-`, and `keep`+`also` — the last is the no-redirect regression).

- [ ] **Step 6: Commit**

```bash
git add src/command.rs
git commit -m "v176: allow a redirect on a compound-command pipeline stage

parse_next_stage returned compound stages (subshell/{ }/if/while/for/select/case/
[[ ]]/arith) without parsing a trailing redirect, unlike its sibling
parse_command_inner. Mirror that path: wrap each compound arm with
maybe_wrap_redirects, so \`echo a | ( cat ) > f\` and \`grep … | { … } > \"\$out\"\`
parse like bash. Parser-only — classify_stage already routes the resulting
Command::Redirected compound stage to the forked InProcess path, where
run_command applies the redirect over the pipe fd.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Bash-diff harness for redirects on compound pipeline stages

**Files:** Create `tests/scripts/pipe_compound_redirect_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/pipe_compound_redirect_diff_check.sh` with exactly:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v176: a redirect on a compound-command
# pipeline stage ( subshell / { } group / if / while / case / arith ). Each case
# EXECUTES the construct (writing to a per-run temp file under our control, then
# printing its contents) and asserts identical stdout+exit under bash and huck.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

check "subshell stage > file"   "echo hi | ( cat ) > $T/a; cat $T/a"
check "group stage > file"      "printf 'x\ny\n' | { cat; } > $T/b; cat $T/b"
check "compound|compound > file" "( echo z ) | { cat; } > $T/c; cat $T/c"
check "stage 2>&1 redirect"     "echo e | ( cat ) 2>&1"
check "redirect nested in sub"  "v=\$( echo n | { cat; } > $T/d ); cat $T/d; echo \"v=[\$v]\""
check "syscall-style read"      "printf '1 a\n2 b\n' | tail -n1 | ( read n x; echo \"\$n=\$x\" ) > $T/e; cat $T/e"
check "while stage > file"      "seq 3 | while read x; do echo \"r\$x\"; done > $T/f; cat $T/f"
check "case stage > file"       "printf 'P\n' | case x in x) cat;; esac > $T/g; cat $T/g"
check "append on group stage"   "echo one > $T/h; echo two | { cat; } >> $T/h; cat $T/h"
check "regression no redirect"  "echo keep | ( cat ); echo also"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run it**

Run: `chmod +x tests/scripts/pipe_compound_redirect_diff_check.sh && cargo build --quiet && bash tests/scripts/pipe_compound_redirect_diff_check.sh`
Expected: `Total: 10, Pass: 10, Fail: 0`.
If a case FAILs, the redirect isn't being honored for that stage shape — investigate (parser missed an arm, or an execution issue); do NOT weaken the assertion.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/pipe_compound_redirect_diff_check.sh
git commit -m "test: v176 bash-diff harness for redirects on compound pipeline stages

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Parse-sweep payoff + full regression

**Files:** none (verification only).

- [ ] **Step 1: Confirm the payoff via the parse sweep**

```bash
cargo build --quiet
PARSE_TIMEOUT=10 bash tools/parse_sweep.sh tools/scripts.tsv /tmp/v176_sweep.tsv | tail -10
echo "--- remaining 'unexpected token after command' HUCK_GAPs (was ~18) ---"
awk -F'\t' '$3=="HUCK_GAP" && index($7,"unexpected token after command")' /tmp/v176_sweep.tsv | wc -l
echo "--- total HUCK_GAP now (was 49 after v175) ---"
awk -F'\t' '$3=="HUCK_GAP"' /tmp/v176_sweep.tsv | wc -l
echo "--- HUCK_LENIENT / HUCK_CRASH (must stay 0) ---"
awk -F'\t' '$3=="HUCK_LENIENT"' /tmp/v176_sweep.tsv | wc -l
awk -F'\t' '$3=="HUCK_CRASH"' /tmp/v176_sweep.tsv | wc -l
echo "--- what 'unexpected token' scripts remain? (expect ~zdiff only) ---"
awk -F'\t' '$3=="HUCK_GAP" && index($7,"unexpected token after command"){print $6}' /tmp/v176_sweep.tsv | sort -u
```
Expected: "unexpected token after command" drops to ~1–2 (the `zdiff` outlier and possibly a straggler), total `HUCK_GAP` falls by ~16 (≈49 → ≈33), `HUCK_LENIENT`/`HUCK_CRASH` stay `0`. If a non-`zdiff` "unexpected token" gap remains, inspect its failing line (`huck -n <path>` then `sed -n 'LINEp'`) and report whether it's a different construct (out of scope) or a missed compound arm.

- [ ] **Step 2: Full regression**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN` → `CLEAN`.
Run: `cargo test >/tmp/v176.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v176.log` → `exit: 0`, `0`.
Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"` → `0 failed` (count is now 98 with the new harness).

- [ ] **Step 3: No commit (verification task).** Report the before/after `HUCK_GAP` numbers, which "unexpected token" scripts remain, and the regression results.

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `src/command.rs` (the wrapped compound arms in `parse_next_stage` only) + the new `tests/scripts/pipe_compound_redirect_diff_check.sh`. Confirm `parse_command_inner`, the function-def/simple-stage arms, and the executor are untouched.
- Re-run `pipe_compound_redirect_diff_check.sh` (10/10) and the full harness suite (98/98); spot-check a real script: `./target/debug/huck -n /usr/src/linux-headers-*/scripts/syscallhdr.sh && echo "syscallhdr parses"` (use a glob that resolves to a present version).
- Merge `v176-pipeline-compound-redirect` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the branch.
- Record in `project_huck_iterations.md` + `MEMORY.md`; update the parse-sweep backlog note (HUCK_GAP ~33 left; "unexpected token" cluster cleared down to the zdiff outlier). No `bash-divergences.md` change.

---

## Self-review (plan vs spec)

- **Spec coverage:** wrap the 8 compound arms (arith, if, while/until, for, select, case, lbrace, doublebracket, subshell) — Task 1 Steps 1–3 (note: while/until is one arm, so that's the 8 listed in the spec) ✓; function-def/simple/coproc unchanged (Task 1 notes + final review) ✓; executor untouched (background facts + final review) ✓; new executing harness with the listed shapes (Task 2) ✓; parse-sweep payoff ~18→~1 (Task 3 Step 1) ✓; full regression + clippy (Task 3 Step 2) ✓; iteration record, no divergence doc (final review) ✓.
- **Placeholder scan:** none — exact before/after for every arm, full harness content, exact verification commands with expected output.
- **Type/name consistency:** `maybe_wrap_redirects(cmd, iter)?` matches the real signature (`command.rs:2097`, returns `Result<Command, ParseError>`); each arm still returns the `(Command, bool)` tuple `parse_next_stage` requires, with `false` (no `|` consumed); `Command::If/While/Case/BraceGroup/Arith`, `parse_for_command`, `parse_select_command`, `parse_double_bracket`, `parse_subshell` are the existing names used unchanged; harness filename and `target/debug/huck` consistent across tasks.
