# v187: function-definition trailing redirects (resolves M-09b) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accept a trailing redirect on a function definition (`name() { … } >f` and `function name { … } >f`), applied at every call with call-time filename expansion — resolving the tracked divergence M-09b.

**Architecture:** A one-line recursive guard in `is_function_body_shape` (`src/command.rs`) to accept a `Command::Redirected` whose inner is a valid function-body shape. `parse_command` already parses `{ … } >f` into a `Redirected`; the function table stores the body AST and re-executes it per call; the executor already applies `Redirected` redirects with call-time expansion. So the correct bash semantics fall out with no AST/parser/executor change. Validated end-to-end (define+call, each-call truncation, `>>`, `>&2`, call-time expansion, both forms, and `declare -f` round-trip — `declare -f` even renders `} 1>&2` like bash).

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh`, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-18-func-def-redirects-design.md`

**Branch:** `v187-func-def-redirects` (from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract; all via stdout)

| Fragment | bash | huck (today) |
|---|---|---|
| `f() { echo "[$1]"; } >/tmp/h1; f A; f B; cat /tmp/h1` | `[B]` | ★ function-def error |
| `g() { echo L; } >>/tmp/h2; rm -f /tmp/h2; g; g; cat /tmp/h2` | `L`⏎`L` | ★ |
| `e() { echo err; } >&2; e 2>/tmp/h3; cat /tmp/h3` | `err` | ★ |
| `D=/tmp/ha; h() { echo hi; } >"$D"; D=/tmp/hb; …; h; cat ha; cat hb` | `ha:`⏎`hb:hi` | ★ |
| `function k { echo K; } >/tmp/h4; k; cat /tmp/h4` | `K` | ★ |
| `c() { echo plain; }; c` | `plain` | `plain` (unchanged) |

`★` = currently `syntax error: function definition: expected '()' and a compound-command body`.

---

### Task 1: accept a `Redirected` function body

**Files:**
- Modify: `src/command.rs` (`is_function_body_shape`, ~`:1161`)
- Test: `src/command.rs` (`mod tests`) + `tests/declare_f_integration.rs`

**Context:** `is_function_body_shape` is called only at `:1196` (`parse_function_def`, the `name()` form) and `:1236` (`parse_function_keyword_def`, the `function` form). It currently accepts the compound shapes but not `Command::Redirected`. `parse_command` already produces `Redirected { inner: <compound>, redirects }` for `{ … } >f`. Tests in `mod tests` use `crate::lexer::tokenize(src).unwrap()` then `parse(tokens).unwrap().unwrap()` and a `first_function(&seq) -> (name, &Command)` helper.

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v187-func-def-redirects
```

- [ ] **Step 2: Add the recursive guard**

In `src/command.rs`, `is_function_body_shape` currently reads:

```rust
fn is_function_body_shape(body: &Command) -> bool {
    matches!(
        body,
        Command::If(_)
```

Insert the guard at the top of the function body (before the `matches!`):

```rust
fn is_function_body_shape(body: &Command) -> bool {
    // A redirected compound (`{ … } >file`) is a valid function body — the
    // redirect attaches to the definition and is applied (with call-time
    // filename expansion) on every call. The Redirected body is stored and
    // re-executed per call, giving bash's semantics with no executor change
    // (M-09b). A Redirected wrapping a non-compound is still rejected.
    if let Command::Redirected { inner, .. } = body {
        return is_function_body_shape(inner);
    }
    matches!(
        body,
        Command::If(_)
```

(The rest of the `matches!` is unchanged.)

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 4: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
chk() { printf '%b\n' "$2" > /tmp/v187.sh
  printf '%-22s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v187.sh 2>&1 | tr '\n' '|')" "$(bash --norc /tmp/v187.sh 2>&1 | tr '\n' '|')"; }
chk 'define+call >file'  'f() { echo "[$1]"; } >/tmp/h1; f A; f B; cat /tmp/h1'
chk 'append >>file'      'rm -f /tmp/h2; g() { echo L; } >>/tmp/h2; g; g; cat /tmp/h2'
chk 'dup >&2'            'e() { echo err; } >&2; e 2>/tmp/h3; cat /tmp/h3'
chk 'call-time expand'   'rm -f /tmp/ha /tmp/hb; D=/tmp/ha; h() { echo hi; } >"$D"; D=/tmp/hb; h; echo "ha:$(cat /tmp/ha 2>/dev/null)"; echo "hb:$(cat /tmp/hb)"'
chk 'keyword form'       'function k { echo K; } >/tmp/h4; k; cat /tmp/h4'
chk 'control no-redir'   'c() { echo plain; }; c'
chk 'declare -f renders' 'f() { echo hi; } >&2; declare -f f'
```

Expected (huck == bash): `[B]`; `L|L`; `err`; `ha:|hb:hi`; `K`; `plain`; and `declare -f` renders the body ending `} 1>&2`.

- [ ] **Step 5: Add parser unit tests**

In `src/command.rs` `mod tests`, near `parse_function_with_if_body`, add:

```rust
    #[test]
    fn function_def_accepts_trailing_redirect() {
        // v187 (M-09b): a trailing redirect makes the body a Redirected wrapping
        // the compound; accepted for BOTH definition forms.
        for src in ["f() { :; } >&2", "function f { :; } >&2"] {
            let toks = crate::lexer::tokenize(src).unwrap();
            let seq = parse(toks).unwrap().unwrap();
            let (name, body) = first_function(&seq);
            assert_eq!(name, "f", "src={src:?}");
            let Command::Redirected { inner, redirects } = body else {
                panic!("expected Redirected body for {src:?}, got {body:?}");
            };
            assert!(matches!(**inner, Command::BraceGroup(_)), "src={src:?}");
            assert_eq!(redirects.len(), 1, "src={src:?}");
        }
    }

    #[test]
    fn function_def_rejects_redirected_non_compound_body() {
        // A redirected NON-compound (`f() echo hi >f`) is still not a valid
        // function body (the recursion bottoms out at a Simple command).
        let toks = crate::lexer::tokenize("f() echo hi >/tmp/zz").unwrap();
        assert!(matches!(parse(toks), Err(ParseError::FunctionBody)));
    }
```

- [ ] **Step 6: Add a `declare -f` round-trip integration test**

In `tests/declare_f_integration.rs`, add (using the file's existing `huck_c` helper — match its signature):

```rust
#[test]
fn declare_f_preserves_definition_redirect() {
    // v187 (M-09b): a definition-attached redirect renders in declare -f and
    // round-trips through eval.
    let d = huck_c("f() { echo hi; } >&2; declare -f f");
    assert!(d.contains("&2"), "declare -f dropped the redirect: {d:?}");
    let out = huck_c("f() { echo hi; } >&2; eval \"$(declare -f f)\"; f 2>&1");
    assert!(out.contains("hi"), "redirect not preserved through round-trip: {out:?}");
}
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test --lib function_def_accepts function_def_rejects 2>&1 | tail -8 && cargo test --test declare_f_integration 2>&1 | tail -8`
Expected: all pass. (If `cargo test --lib` rejects two filters, run them one at a time.)

- [ ] **Step 8: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 9: Commit**

```bash
git add src/command.rs tests/declare_f_integration.rs
git commit -m "$(cat <<'EOF'
v187: accept trailing redirects on function definitions (resolves M-09b)

is_function_body_shape now accepts a Command::Redirected whose inner is a valid
function-body shape. parse_command already wraps `{ … } >f` in Redirected, the
function table re-executes the body per call, and the executor applies the
redirects with call-time filename expansion — so `name() { … } >f` and
`function name { … } >f` now define + apply the redirect at every call, matching
bash, with no AST/executor change. declare -f round-trips (renders `} 1>&2`).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 4 table (all must match), Step 7 result, any concerns. If a code site doesn't match the line numbers above, find it by the surrounding code and report.

---

### Task 2: Bash-diff harness `func_redirect_diff_check.sh`

**Files:**
- Create: `tests/scripts/func_redirect_diff_check.sh`

**Context:** Harnesses pipe each fragment to bash AND huck and assert byte-identical output. Cases that write a file `cat` it back so stdout carries the result. Model: `tests/scripts/dollar_quote_forms_diff_check.sh`. Use UNIQUE temp paths so concurrent harness runs don't collide.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/func_redirect_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v187 (M-09b): a trailing redirect on a
# function DEFINITION is applied at every call, with call-time filename
# expansion. Both forms (`name() …` / `function name …`). Cases write a temp
# file then cat it so the result is on stdout. rc 0 in bash → compare full
# stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
D=$(mktemp -d)

check "define+call truncate" "f() { echo \"[\$1]\"; } >$D/a; f A; f B; cat $D/a"
check "append >> two calls"  "g() { echo L; } >>$D/b; g; g; cat $D/b"
check "dup >&2 captured"      "e() { echo err; } >&2; e 2>$D/c; cat $D/c"
check "call-time filename"    "Z=$D/x; h() { echo hi; } >\"\$Z\"; Z=$D/y; h; echo \"x:\$(cat $D/x 2>/dev/null)\"; echo \"y:\$(cat $D/y)\""
check "function keyword form" "function k { echo K; } >$D/d; k; cat $D/d"
check "redir + arg redir"     "m() { echo M; } >$D/e; m >$D/f; echo \"e:\$(cat $D/e 2>/dev/null)\"; echo \"f:\$(cat $D/f)\""
check "control no redirect"   'c() { echo plain; }; c'

rm -rf "$D"
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, build, run**

Run:

```bash
chmod +x tests/scripts/func_redirect_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/func_redirect_diff_check.sh
```

Expected: `Total: 7, Pass: 7, Fail: 0`, exit 0. NOTE the "redir + arg redir" case: bash's precedence when a CALL also has a redirect (`m >file2` where `m` is defined `>file1`) — confirm bash's actual output and that huck matches; if the case's own semantics are surprising, report the bash output rather than weakening it. If any other case FAILs, STOP and report.

- [ ] **Step 3: Prove the harness is non-tautological**

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/func_redirect_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: the redirect-bearing cases FAIL against the pre-fix binary (function-def error); the `control no redirect` case passes pre-fix; prefix-exit=1. If it PASSES pre-fix, the harness is tautological — STOP and report. (`main` is the pre-v187 baseline.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/func_redirect_diff_check.sh
git commit -m "$(cat <<'EOF'
v187: bash-diff harness for function-definition trailing redirects

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, Step 2 Total/Pass/Fail line (+ the `redir + arg redir` bash output if notable), Step 3 prefix-exit + which cases failed pre-fix (proving non-tautology).

---

### Task 3: Parse-sweep payoff, regression, and M-09b deletion

**Files:**
- Modify: `docs/bash-divergences.md` (DELETE M-09b, decrement Tier-2 count)
- Modify (only if found): old-behavior tests from the up-front grep
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn "is_function_body_shape\|FunctionBody\|function definition" src/ tests/ | grep -iv "fn is_function_body_shape" | head -30
```

Classify any hit UPDATE (encodes the old rejection of a redirected function body) vs LEAVE (unrelated — e.g. a test that a non-compound body IS rejected, which still holds). Report each. (None expected — the change only ADDS acceptance.)

- [ ] **Step 2: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. A failure on a function-def / declare-f assertion: verify against bash; if it encoded the old rejection, UPDATE; else STOP and report.

- [ ] **Step 3: Whole bash-diff harness suite**

Run:

```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```

Expected: `ALL HARNESSES GREEN`.

- [ ] **Step 4: Parse-sweep payoff**

```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -15
F='/home/john/go/pkg/mod/golang.org/x/tools@v0.39.1-0.20260109155911-b69ac100ecb7/internal/mcp/create-repo.sh'
echo "=== create-repo.sh ==="; ./target/debug/huck -n "$F"; echo "huck-n rc=$?"; bash -n "$F"; echo "bash-n rc=$?"
echo "=== remaining function-definition gaps? ==="
awk -F'\t' '$3=="HUCK_GAP" && $7 ~ /function definition/{print $6}' tools/parse_results.tsv || echo "(none)"
```

Expected: `create-repo.sh` `huck -n` SILENT rc 0 = bash. Report the new HUCK_GAP vs the 6 baseline; LENIENT/CRASH/TIMEOUT stay 0. byobu-ulevel REMAINS a gap (the separate `\<NL>`-array bug, v188) — confirm it's still the function-def error (a different construct) and note it.

- [ ] **Step 5: Delete the M-09b entry**

In `docs/bash-divergences.md`, delete the entire `M-09b` bullet (`- **M-09b: Definition-attached redirections** …`). Do not disturb the adjacent `M-09a` entry. Then decrement the Tier-2 count in the Summary table (find `| Missing features (Tier 2) | N |` and decrement N by 1). Verify:

```bash
grep -n "M-09b" docs/bash-divergences.md || echo "no M-09b remains"
grep "Missing features (Tier 2)" docs/bash-divergences.md
```

- [ ] **Step 6: Commit**

```bash
git add docs/bash-divergences.md
# add any test files updated in Step 1/2
git commit -m "$(cat <<'EOF'
v187: resolve M-09b (delete entry); regression + parse-sweep pass

create-repo.sh now parses (huck -n rc 0 = bash). Full cargo test + all
bash-diff harnesses green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 1 grep classification, Step 2 cargo test summary, Step 3 result, Step 4 create-repo rc + new HUCK_GAP vs 6, confirmation M-09b is gone with the count decremented, and that byobu-ulevel remains (the v188 array bug).

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm only the `is_function_body_shape` guard + the parser/declare-f tests, the new harness, and the M-09b deletion changed.
- [ ] **Step 2: Scope boundary** — no AST/executor/`parse_command` change; plain function definitions unchanged; the `\<NL>`-array bug (v188) and M-09a untouched.
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v187 in `project_huck_iterations.md` + `MEMORY.md`, update the backlog note (byobu-ulevel `\<NL>`-array → v188).

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- `Redirected`-body acceptance in `is_function_body_shape` → Task 1 Step 2. ✓
- Parser unit tests (both forms → Redirected body; non-compound rejected) → Task 1 Step 5. ✓
- `declare -f` round-trip test → Task 1 Step 6. ✓
- New harness (define+call each-call, `>>`, `>&2`, call-time expansion, keyword form, control) → Task 2. ✓
- Parse-sweep: create-repo clears + HUCK_GAP report + byobu-ulevel-remains note → Task 3 Step 4. ✓
- Full `cargo test` + up-front grep + existing function/declare-f tests stay green → Task 3 Steps 1-2. ✓
- Delete M-09b + decrement Tier-2 → Task 3 Step 5. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 3 + Task 1 Step 8. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows complete before/after code; every run step shows command + expected output. The harness's `huck_c` helper is referenced as "match its signature" because it is an existing helper in `declare_f_integration.rs`.

**3. Type consistency:** `is_function_body_shape(&Command) -> bool` signature unchanged (one guard added). `first_function(&seq) -> (name, &Command)` and the `Command::Redirected { inner, redirects }` shape (inner: `Box<Command>`, redirects: `Vec<Redirection>`) match the AST. `crate::lexer::tokenize` is the established string→tokens helper used across `command.rs` tests.

**Resolved during planning:** validated the one-line change end-to-end (define+call, each-call truncation, `>>`, `>&2`, call-time filename expansion all match bash); confirmed `declare -f` renders `} 1>&2` (matches bash) and round-trips; confirmed create-repo.sh parses; confirmed `is_function_body_shape`'s only two callers are the two function-def parsers.
