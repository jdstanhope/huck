# v193: `**` globstar respects `shopt globstar` — Design

**Status:** approved 2026-06-19
**Iteration:** v193
**Origin:** Tier-2 backlog **M-53** (mischaracterized as "huck `**` ≡ `*`").
Reality: huck's regular glob path delegates to the Rust `glob` crate, whose `**`
is recursive **unconditionally** — `GlobOpts` has no `globstar` field and
`glob_opts()` never reads the shopt. So huck does globstar even when
`shopt -u globstar` (bash's default), over-matching in scripts that don't opt in.

## bash contract (verified)

`shopt globstar` defaults **off**. When **off**, `**` is just two `*` (≡ `*`,
single-level, never crosses `/`). When **on**, `**` matches across `/` (recursive):

| pattern (in a tree `r.txt a/x.txt a/b/y.txt a/b/c/z.txt`) | globstar OFF | globstar ON |
|---|---|---|
| `**/*.txt` | `a/x.txt` (= `*/*.txt`, one level) | `r.txt a/x.txt a/b/y.txt a/b/c/z.txt` (recursive — `**` matches zero+ dirs incl. cwd) |
| `**` (bare) | the cwd's non-dot entries (= `*`) | every file AND dir at all depths incl. cwd |

huck today: globstar OFF still recurses (bug). ON `**/*.txt` already matches bash
(the crate recurses). ON bare `**` matches dirs-only (crate quirk) — the residual.

## Design

A glob-engine change (`src/expand.rs` + `src/shell_state.rs`). Gate `**`
recursion on the shopt by collapsing `**`→`*` in the pattern when globstar is off
(so the `glob` crate sees single-level `*`); leave `**` when on.

### 1. Thread the shopt

- Add `pub globstar: bool` to `GlobOpts` (`src/expand.rs:10`).
- In `Shell::glob_opts()` (`src/shell_state.rs:815`): add
  `globstar: self.shopt_options.get("globstar").unwrap_or(false),`.
- `GlobOpts::default()` gets `globstar: false` (matches bash default; the
  `#[derive(Default)]` or manual default already covers a new `bool` field as
  `false` — verify it's `#[derive(Default)]`, else add the field).

### 2. `collapse_globstar` helper (`src/expand.rs`)

```rust
/// Collapses a run of consecutive `*` to a single `*` (`**`→`*`, `***`→`*`),
/// matching bash's behavior when `shopt globstar` is OFF (two `*` are just one
/// `*`). Skips `*` inside a `[…]` bracket class and honors `\`-escapes, so
/// `[**]` and `\*\*` are untouched.
fn collapse_globstar(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len());
    let mut chars = pat.chars().peekable();
    let mut in_bracket = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => { out.push('\\'); if let Some(n) = chars.next() { out.push(n); } }
            '[' if !in_bracket => { in_bracket = true; out.push('['); }
            ']' if in_bracket => { in_bracket = false; out.push(']'); }
            '*' if !in_bracket => {
                out.push('*');
                while chars.peek() == Some(&'*') { chars.next(); }
            }
            other => out.push(other),
        }
    }
    out
}
```

### 3. Gate the non-extglob glob path

In `glob_expand_fields_opts`, the `else` (non-extglob) branch builds
`let npat = crate::glob_match::translate_bracket_negation(&pattern);` then calls
`glob_with(&npat, …)`. Replace with:

```rust
            let npat = crate::glob_match::translate_bracket_negation(&pattern);
            // `**` is recursive only when `shopt -s globstar`; otherwise it is
            // two ordinary `*` (≡ `*`). The `glob` crate always treats `**` as
            // recursive, so collapse it when globstar is off.
            let npat = if opts.globstar { npat } else { collapse_globstar(&npat) };
            match glob_with(&npat, match_opts) { … }
```

(The extglob path's `walk_components` already treats a `**` component like a
single-level `*` — no recursion — so it is already globstar-OFF-correct; a
globstar-ON + extglob combination not recursing is a rare residual, noted below.)

## Verification

- **New bash-diff harness** `tests/scripts/globstar_diff_check.sh` (byte-identical
  stdout): builds a private temp tree (`mktemp -d`, `r.txt a/x.txt a/b/y.txt
  a/b/c/z.txt`), `cd`s in, and compares sorted output of:
  - **off (default)**: `**/*.txt` (= one level), bare `**`, `a/**` , `**/*` —
    each byte-identical to bash with globstar off.
  - **on**: `shopt -s globstar; **/*.txt` (recursive), `**/y.txt`, `a/**/*.txt` —
    byte-identical to bash with globstar on.
  - a control with no `**` (`a/*` , `*.txt`) — unchanged.
  - The bare-`**`-when-ON case is NOT byte-compared (documented residual); a
    comment in the harness records why.
  (Sort both sides — `printf '%s\n' <pat> | sort` — so directory-read order
  doesn't matter.)
- **Unit tests** (`src/expand.rs` `mod tests`): `collapse_globstar("**")=="*"`,
  `collapse_globstar("a/**/b")=="a/*/b"`, `collapse_globstar("**/*.txt")=="*/*.txt"`,
  `collapse_globstar("[**]")=="[**]"` (bracket untouched),
  `collapse_globstar("\\*\\*")=="\\*\\*"` (escapes untouched),
  `collapse_globstar("a*b")=="a*b"` (single `*` unchanged); plus a `glob_opts`
  test that the `globstar` shopt flows into `GlobOpts.globstar`.
- **Up-front grep** `src/` + `tests/` for tests asserting huck's CURRENT
  unconditional-`**`-recursion (default-on) behavior; update them to the gated
  behavior (globstar off → single-level). Verify each against bash.
- **Parse/runtime sweeps unaffected** (globbing is runtime). Full `cargo test`
  (0 failures); all harnesses + clippy green.

## Docs / close-out

**M-53** is no longer accurate. REWRITE it (or replace with a new entry): the
`shopt -u globstar` (default) behavior is now correct (`**` ≡ `*`); the remaining
divergence is the bare-`**`-when-globstar-ON set — huck (via the `glob` crate)
matches directories at all depths but NOT files at the cwd/leaf the way bash's
bare `**` does — keep as `[deferred]` low (needs huck's own recursive walker;
the common `**/*.ext` form is correct). Adjust the Tier-2 count if M-53 changes
tier (it stays Tier 2, count unchanged — reworded, not removed). Also note the
globstar-ON + extglob-pattern combo doesn't recurse (rare). Record v193 in
`project_huck_iterations.md` + `MEMORY.md`.

## Scope boundary

In scope: `GlobOpts.globstar` + plumbing; `collapse_globstar` gating the
non-extglob path; the harness + unit tests; rewording M-53. **Not** in scope:
exact bare-`**`-ON parity (the glob-crate dirs-vs-dirs+files quirk — documented
residual, would need a custom recursive walker); globstar-ON inside the extglob
path; symlink-loop protection; any change to `*`/`?`/`[…]`/extglob matching.
