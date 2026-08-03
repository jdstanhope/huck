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

- **GitHub issues labelled [`divergence`](https://github.com/jdstanhope/huck/issues?q=is%3Aissue+label%3Adivergence)** —
  the live tracker for every ACTIONABLE divergence from bash 5.x (bugs and
  missing features we intend to address). Filter open work by `bug` /
  `enhancement` and `sev:high` / `sev:medium` / `sev:low`. Deliberate,
  kept-by-design divergences are the closed [`by-design`](https://github.com/jdstanhope/huck/issues?q=is%3Aissue+label%3Aby-design)
  issues. **Before starting new work, review the open `divergence` issues** and
  either take an existing one or open a new issue to capture the work.

- **`docs/bash-divergences.md`** — the INTENTIONAL (kept-by-design)
  divergences only, each linking to its closed `by-design` issue. Actionable
  divergences are NOT here — they live in the GitHub issue tracker (above).
  Resolved divergences and the per-iteration history are in git history and
  `docs/superpowers/` specs+plans.

The README's iteration table indexes the v1–vNN history at a glance.
For per-iteration design context, `docs/superpowers/specs/` and
`docs/superpowers/plans/` hold the paper trail (one pair per `vNN`).

## When the user says "start vNN: <feature>"

Run the standard iteration loop without being asked:

0. **Pick up (or open) an issue.** Review the open [`divergence`](https://github.com/jdstanhope/huck/issues?q=is%3Aissue+is%3Aopen+label%3Adivergence)
   issues. Take the existing issue that matches the work, or open a new one
   (`gh issue create`, labels `divergence` + `bug`/`enhancement` +
   `sev:*`) to capture it. Note the issue number — the PR will close it.
1. **Brainstorm** via the `superpowers:brainstorming` skill — ask
   one question at a time, propose 2-3 approaches, present design
   in sections with per-section approval.
2. **Write spec** to `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`
   and commit on main. The spec MUST reference the issue (`#N` with a link).
3. **Write plan** to `docs/superpowers/plans/YYYY-MM-DD-<topic>.md`
   via `superpowers:writing-plans` and commit on main. The plan MUST reference
   the issue (`#N`). Once both are committed, **comment on the issue with links
   to the committed spec and plan** (`gh issue comment N` with the
   `github.com/.../blob/main/...` URLs) so the issue tracks its design trail.
4. **Implement** via `superpowers:subagent-driven-development` on a
   `vNN-<topic>` branch: fresh subagent per task with spec + code
   quality review between tasks.
5. **Final review** of the whole branch diff before merge.
   - Run the bash-diff sweep before the PR: build both binaries
     (`cargo build --locked --bin huck` + `cargo build --release --locked
     --bin huck`) then `tests/scripts/run_diff_checks.sh`; it must be green.
     (CI runs it too, but catch regressions locally first.)
6. **Open a pull request** (`gh pr create`) targeting `main`, with the body
   referencing the issue via `Closes #N`, and hand it to the user to review
   and merge — do NOT merge a `vNN` iteration to main yourself (a bug-fix
   round is different; see the cascade section). Push the `vNN-<topic>` branch
   to origin so the PR has a head. Before opening the PR, update the docs +
   memory as part of the branch:
   - If the work resolved a divergence, the merged PR auto-closes its issue
     (`Closes #N`); no edit to `docs/bash-divergences.md` is needed unless the
     resolved item was **intentional** (then remove it there too). Open a new
     `divergence` issue for any follow-on gap discovered.
   - If the work adds a NEW intentional divergence, add it to
     `docs/bash-divergences.md` and open + close a `by-design` issue for it.
   - Record the iteration in the long-running memory files
     (`project_huck_iterations.md` + `MEMORY.md`). (The README no longer
     carries a per-version table.)

## Bug-fix rounds and the follow-on cascade

A **bug-fix round** — one issue, no `vNN`, no spec/plan — runs the short
loop: branch → fix → a `<feature>_diff_check.sh` harness → build both
binaries + full `tests/scripts/run_diff_checks.sh` sweep → PR
(`Closes #N`) → **wait for the GitHub run to FINISH and pass** →
squash-merge it yourself.

Fixing one bug almost always turns up neighbouring divergences. File each
as its own `divergence` issue (never smuggle it into the current PR),
then **keep going without being asked**:

1. Finish the current round and merge its PR (CI green — local green is
   not enough).
2. Take the issues you just filed, one round each, same short loop.
3. Repeat until the cascade is dry, then report the whole chain.

**Stop and hand back to the user** when the next issue is a *significant
change that may affect multiple features*: a shared table or chokepoint
several builtins read, a new subsystem, or a blast radius wider than the
builtin you were fixing — anything that wants a spec + plan. Say what you
found, why it is bigger, and let the user decide. Small, self-contained,
harness-provable fixes keep cascading; anything else waits.

**Never stack a PR on an unmerged branch.** Merge the parent to `main`
first, then branch again from `main`. A PR based on a branch that then
gets squash-merged is orphaned — it merges into the stale branch, not
`main`, and GitHub still reports it as merged. If the follow-on cannot
wait, branch from `main` and cherry-pick.

## Conventions

- **Commit trailer**: every commit ends with
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
  The "(1M context)" parenthetical is canonical; do not remove it. (Update
  the model version to match whichever Claude model is doing the work; was
  4.7 through v136, 4.8 from v137.)
- **Formatting**: run `cargo fmt --all` before committing — CI enforces
  `cargo fmt --all --check`, so an unformatted tree fails the build.
- **CI**: `.github/workflows/ci.yml` runs fmt-check + `cargo build`/`cargo test`
  (`--workspace --locked`) on every push and every PR to `main`, on
  `ubuntu-24.04` (bash 5.2.21, huck's compat target).
- **Bash-diff harnesses** under `tests/scripts/*_diff_check.sh` run
  the same fragments through bash and huck and assert byte-identical
  output. Adding a `<feature>_diff_check.sh` is the gold standard
  for verifying bash compat on a new feature.
- **Don't push directly to main.** Everything lands via a pull request.
  A `vNN` **iteration** PR is handed to the user to review and merge —
  push the branch, open the PR (`Closes #N`), leave the merge alone. A
  **bug-fix round** PR you squash-merge yourself once the GitHub run has
  finished and passed, then move on to the issues that round filed (see
  "Bug-fix rounds and the follow-on cascade" above).
