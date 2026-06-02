# Working on huck

huck is a POSIX-ish shell written in Rust, built one numbered
iteration at a time. Before designing, planning, or listing work,
read these two docs — they're the project's authoritative source:

- **`docs/architecture.md`** — module map, key types
  (`Word`/`Sequence`/`Shell`/`VarValue`/`ExecOutcome`/etc.), the
  lex→parse→expand→execute pipeline, cross-cutting conventions
  (`process_line`, inline-assignment snapshot/restore, local-scope
  unwinding, `DeclArg` pre-parse, bash-diff harnesses), the
  iteration workflow, and a "where to add common features"
  cheatsheet (new builtin / modifier / `test` operator / control
  flow / `set -o` option / trap signal / array follow-on).

- **`docs/bash-divergences.md`** — every divergence from bash 5.x,
  grouped into Bugs / Missing features / Intentional / Low-impact
  tiers. Every entry has a status: `[fixed vNN]`, `[deferred]`,
  `[intentional]`, etc. Grep for `[deferred]` to find pending work,
  ranked by severity (`high`/`medium`/`low`). The change log at
  the end records every shipped iteration.

The README's iteration table indexes the v1–vNN history at a glance.
For per-iteration design context, `docs/superpowers/specs/` and
`docs/superpowers/plans/` hold the paper trail (one pair per `vNN`).

## When the user says "start vNN: <feature>"

Run the standard iteration loop without being asked:

1. **Brainstorm** via the `superpowers:brainstorming` skill — ask
   one question at a time, propose 2-3 approaches, present design
   in sections with per-section approval.
2. **Write spec** to `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`
   and commit on main.
3. **Write plan** to `docs/superpowers/plans/YYYY-MM-DD-<topic>.md`
   via `superpowers:writing-plans` and commit on main.
4. **Implement** via `superpowers:subagent-driven-development` on a
   `vNN-<topic>` branch: fresh subagent per task with spec + code
   quality review between tasks.
5. **Final review** of the whole branch diff before merge.
6. **Merge** with `--no-ff`, push to origin, delete the local
   branch. Update `docs/bash-divergences.md` (flip the relevant
   `M-*` from `[deferred]` to `[fixed vNN]`, add change-log entry),
   the README iteration table, and the long-running memory files.

## Conventions

- **Commit trailer**: every commit ends with
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.
  The "(1M context)" parenthetical is canonical; do not remove it.
- **Bash-diff harnesses** under `tests/scripts/*_diff_check.sh` run
  the same fragments through bash and huck and assert byte-identical
  output. Adding a `<feature>_diff_check.sh` is the gold standard
  for verifying bash compat on a new feature.
- **Don't push directly to main without confirmation.** Use
  `AskUserQuestion` before merging an iteration branch.
