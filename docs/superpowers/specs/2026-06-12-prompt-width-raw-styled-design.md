# huck v149 — correct prompt width via rustyline's (raw, styled) Prompt (B-01)

**Status:** approved design (short iteration).
**Fixes:** B-01 — with a powerline prompt (oh-my-posh), the input cursor lands
several columns INSIDE the rendered prompt; typed text overwrites the prompt tail.
**Branch (impl):** `v149-prompt-width`.

## Root cause (diagnosed, with evidence)

rustyline 18's prompt-width function (`tty/mod.rs::width`, via `calculate_position`)
zeroes only **CSI** escape sequences (`\x1b[…m` — the colors) through its `esc_seq`
state machine, and **ignores the `\x01`/`\x02` non-printing markers entirely**.
oh-my-posh's rendered prompt also contains **OSC** sequences — an OSC-8 hyperlink
around the path (`\x1b]8;;file://…\x1b\\`) and the OSC-0 window title
(`\x1b]0;bash in shuck\x07`), each wrapped in `\[ \]` (→ `\x01…\x02` after v148).
rustyline's `\x1b]` case falls through its CSI-only machine and **counts the entire
OSC body (URL, title text) as visible width**.

Measured on the real prompt (faithful reimplementation of rustyline's algorithm with
the same `unicode-width` 0.2.2 / `unicode-segmentation` 1.13.2 crates): rustyline
computes **135 columns**; true visible width is **39**. The 96-column phantom corrupts
rustyline's internal row/col model so the cursor is mispositioned. (The earlier "big
gap after the prompt" and this "cursor mid-prompt" are the same overcount at different
terminal widths.)

## Fix

rustyline's `Editor::readline<P: Prompt>` accepts any `Prompt`, and rustyline ships a
blanket impl `Prompt for (Raw, Styled)` where:
- `raw()` is used for **width measurement** (`calculate_position` at edit.rs:61/113),
- `styled()` is used for **display** (`highlight_prompt(prompt.styled(), …)` at
  unix.rs:1060). `HuckHelper`'s Highlighter is the default (empty), so `styled()` is
  printed unchanged.

So huck passes a tuple instead of a bare `&str`:
- `styled` = the current fully-expanded prompt (colors + glyphs + OSC title/hyperlinks).
- `raw` = the same prompt with the `\x01…\x02` non-printing spans (and the marker
  chars) removed → pure visible text, which measures to exactly the visible width.

rustyline's contract ("the styled version *must* have the same display width as the
raw version") holds: the stripped escapes/OSC are zero-width, so both render the same
visible glyphs.

This is the **intended** readline mechanism for non-printing prompt regions — not a
workaround. It preserves colors, powerline glyphs, the clickable-path hyperlink, AND
the window title, with a correctly-placed cursor.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/prompt.rs` | New `pub fn prompt_raw(s: &str) -> String` — drop `\x01…\x02` spans and the marker chars, returning the visible-only string. Unit tests. |
| `src/shell.rs` | `read_logical_command`: replace `editor.readline(&expanded)` with `editor.readline(&(prompt_raw(&expanded), expanded))` (name the measured string distinctly from the `Ok(raw)` input binding). |
| `tests/...` | A test asserting `prompt_raw` strips marker spans to visible text; optionally a width-regression check. |
| `docs/bash-divergences.md` | DELETE the B-01 entry. |

## `prompt_raw` semantics

Iterate chars; `\x01` enters skip, `\x02` exits skip, marker chars are never emitted,
non-marker chars are emitted only when not skipping. An unbalanced `\x01` (no closing
`\x02`) skips to end (matches readline's START_IGNORE-to-end behavior); a stray `\x02`
is dropped. Prompts with no markers (e.g. `huck> `) return unchanged → `raw == styled`
→ zero behavior change.

## Testing

1. **Unit** (`src/prompt.rs`): `prompt_raw("\x01\x1b[31m\x02X\x01\x1b[0m\x02") == "X"`;
   no-marker prompt unchanged; unbalanced `\x01` skips to end; bare `\x02` dropped.
2. **Width regression**: with the captured oh-my-posh prompt bytes, `prompt_raw` output
   contains no `\x1b`/`\x01`/`\x02` and its `unicode-width` equals the visible count.
3. **Full regression**: suite + all 67 harnesses green; clippy clean.
4. **Payoff (manual/PTY):** after `source ~/.bashrc`, typing at the oh-my-posh prompt
   lands the cursor at the end of the prompt (no overwrite).

## Notes
- Non-tty / `NO_COLOR`: rustyline prints `raw()` (visible-only, no color) — correct.
- Git safety: stay on `v149-prompt-width`; commit trailer
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
