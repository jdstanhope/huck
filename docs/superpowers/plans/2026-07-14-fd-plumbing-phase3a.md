# fd-plumbing Phase 3a — one redirect lowering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate the three copy-pasted redirect op-resolution producers (`RedirectScope::apply`, `apply_var`, `build_child_redir_plan`) into one behavior-preserving `lower_redirects() -> RedirPlan`, consumed by two thin appliers.

**Architecture:** `lower_redirects` walks the ordered redirect list once, resolving each into a neutral `PlanOp` (opening files as `OwnedFd`, spawning heredoc writers, resolving dup words to fd numbers, allocating `{var}` high fds). Two thin consumers apply it: the child path translates `RedirPlan` → the existing `ChildRedirPlan` (dup2/close replay); the in-process path adds `RedirectScope::apply_plan` (save/restore onto real fds). The `RedirectSlot` fast-path and `build_child_extra_ops` are untouched (retired in Phase 3b).

**Tech Stack:** Rust, `std::os::fd::OwnedFd`, libc fcntl/dup2. Crate: `huck-engine`. All work is in `crates/huck-engine/src/executor.rs` plus one harness file.

**Spec:** `docs/superpowers/specs/2026-07-14-fd-plumbing-phase3a-design.md`. **Issue:** #139.

## Global Constraints

- **Behavior-preserving.** No user-visible change; no bug closed. The bash-diff sweep (`tests/scripts/run_diff_checks.sh`, 188 harnesses) + `fd_torture_diff_check.sh` staying byte-green on BOTH debug and release binaries is the proof of correctness.
- **Commit trailer, verbatim on every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** run `cargo fmt --all` before every commit (CI enforces `--check`).
- **Test/build discipline (this box OOMs on `cargo test --workspace`):**
  - Build binary: `cargo build -p huck` (debug), `cargo build --release -p huck` (release, for the sweep).
  - Lib tests: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1806 tests).
  - Integration binary (fd-number-sensitive, run before any push): `cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1` under `ulimit -v 6000000`.
  - Sweep: `(ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh)` — each harness on its own default binary; never override `HUCK_BIN`.
- **Out of scope, DO NOT TOUCH:** `slots_for_simple_path`, `slot_consumes`, `stage_extra_redirects`, `build_child_extra_ops`, `run_multi_stage`'s slot reads, and the H7 software-sink layer (`redirs_write_stdout`, `final_dests_for_1_2`, `run_builtin_with_redirects`'s routing, `emit_exec_spawn_diag`). These are Phase 3b / orthogonal.

---

### Task 1: Add the in-process ⇄ child parity net to `fd_torture_diff_check.sh`

Establish the characterization net FIRST: cases that pass on today's code and must stay green through the refactor. They target the exact semantics the two appliers must preserve — lazy dup-source validation, `{var}` persistence in-process vs child inheritance, ordering, and #135.

**Files:**
- Modify: `tests/scripts/fd_torture_diff_check.sh` (add cases after the existing "dup / close / merge" block, before the `#128` block).

**Interfaces:**
- Consumes: the harness's `check "label" 'fragment'` helper (byte-compares `bash -c` vs `huck -c`, cwd `$WORK`, `$WORK/inA` contains `FA\n`).
- Produces: nothing consumed by later tasks; it is the regression net they run.

- [ ] **Step 1: Add the parity cases**

In `tests/scripts/fd_torture_diff_check.sh`, immediately after the line:
```bash
check "&> merge to file"          'echo hi &> f; cat f'
```
insert:
```bash
# --- Phase 3a parity net: semantics the unified lowering must preserve ---
# Lazy dup-source validation: 4>&3 is only valid because 3>file applied first.
check "3a lazy dup after file"     'exec 3>&-; { echo x; } 3>f 4>&3; cat f'
check "3a dup chain swap"          '{ echo o; echo e 1>&2; } 3>&1 1>&2 2>&3 2>/dev/null'
# {var} persists in-process (exec / compound) but is per-command in a child.
check "3a namedfd exec persists"   'exec {v}>f; echo hi >&"$v"; exec {v}>&-; cat f'
check "3a namedfd compound"        '{ echo hi; } {v}>f; cat f'
check "3a namedfd external child"  'echo hi {v}>f; cat f'
check "3a namedfd move"            'exec 5>f; { echo hi; } {v}>&5-; cat f'
# In-process ordering: 2>&1 >file (compound) vs a single external command.
check "3a order compound 2>&1>f"   '{ echo o; echo e 1>&2; } 2>&1 >f; cat f'
check "3a order external 2>&1>f"   '/bin/echo hi 2>&1 >f; cat f'
# fd>2 heredoc on a single (non-pipeline) external — full ordered path.
check "3a fd3 heredoc external"    $'exec 3>&-; /bin/cat <&3 3<<EOF\nbody\nEOF'
```

- [ ] **Step 2: Build the debug binary and run the harness on current code**

Run:
```bash
cargo build -p huck && tests/scripts/fd_torture_diff_check.sh
```
Expected: every line `PASS`, final `... passed, 0 failed`. (If any new case FAILs on current code, it is characterizing a pre-existing divergence, not a regression target — remove that case and note it; do NOT try to fix behavior here.)

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add tests/scripts/fd_torture_diff_check.sh
git commit -m "$(cat <<'EOF'
v292 T1: add in-process/child parity net to fd_torture (#139)

Characterization cases that pass on current behavior and must stay green
through the Phase 3a lowering consolidation: lazy dup-source validation,
{var} persistence (in-process) vs child inheritance, 2>&1 >file ordering,
fd>2 heredoc on a single external.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Replace `open_redirect_file`'s `relocate: bool` with an `FdPlacement` enum

The P2 leftover Minor. Pure rename; behavior-preserving. Done before the lowering refactor so `lower_redirects` uses the enum from the start.

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — `open_redirect_file` (~4294) and its 4 current call sites (in `RedirectScope::apply` ~1038, `apply_var` ~1224, `build_child_redir_plan` ~5675 and ~5802, `build_child_extra_ops` ~5945).

**Interfaces:**
- Produces: `enum FdPlacement { Relocated, RawLow }` and `fn open_redirect_file(mode: &FileMode, path: &str, noclobber: bool, placement: FdPlacement) -> io::Result<OwnedFd>`. Later tasks call it with `FdPlacement::Relocated` (redirect targets) or `FdPlacement::RawLow` (`{var}` sources).

- [ ] **Step 1: Define the enum and change the signature**

Above `fn open_redirect_file`, add:
```rust
/// Where a freshly-opened redirect-source fd should land.
enum FdPlacement {
    /// Relocate to >= 10 and set FD_CLOEXEC. Used for redirect *targets* on real
    /// fds so the source stays out of the 0..9 swap range that explicit targets
    /// (`3>&1 2>&3`) operate on.
    Relocated,
    /// Return the raw low File fd as opened (CLOEXEC). Used only by the `{var}`
    /// path, which relocates once itself via `dup_to_high_fd` — relocating here
    /// too double-relocates the named fd (fd 11 vs bash's 10; the #135 regression).
    RawLow,
}
```
Change the function to:
```rust
fn open_redirect_file(
    mode: &FileMode,
    path: &str,
    noclobber: bool,
    placement: FdPlacement,
) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    let file: File = match mode {
        FileMode::ReadOnly => File::open(path)?,
        FileMode::Truncate => open_writable(path, noclobber)?,
        FileMode::Clobber => open_writable(path, false)?,
        FileMode::Append => OpenOptions::new().create(true).append(true).open(path)?,
        FileMode::ReadWrite => OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?,
    };
    match placement {
        FdPlacement::RawLow => Ok(OwnedFd::from(file)),
        FdPlacement::Relocated => {
            let raw = relocate_high_cloexec(file.into_raw_fd());
            Ok(unsafe { OwnedFd::from_raw_fd(raw) })
        }
    }
}
```

- [ ] **Step 2: Update the 4 call sites**

Replace `, true)` with `, FdPlacement::Relocated)` at the two non-`{var}` File opens (in `RedirectScope::apply` and `build_child_redir_plan` non-`{var}` arm ~5802, and `build_child_extra_ops` ~5945 — 3 relocate=true sites). Replace `, false)` with `, FdPlacement::RawLow)` at the two `{var}` File opens (`apply_var` ~1224 and `build_child_redir_plan` `{var}` arm ~5675). Grep to confirm none remain:
```bash
grep -n 'open_redirect_file' crates/huck-engine/src/executor.rs
```
Expected: only the definition + calls passing `FdPlacement::Relocated` / `FdPlacement::RawLow`; no `, true)` / `, false)`.

- [ ] **Step 3: Build and run the nets**

Run:
```bash
cargo build -p huck && \
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3 && \
tests/scripts/fd_torture_diff_check.sh | tail -1
```
Expected: warning-clean build, lib `test result: ok. 1806 passed` (count may vary ±), fd_torture `... passed, 0 failed`.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v292 T2: open_redirect_file relocate:bool -> FdPlacement enum (#139)

Pure rename of the P2 boolean-trap into a 2-variant placement enum
(Relocated / RawLow). Behavior-preserving.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Introduce `lower_redirects` + rewire the child path

Create the single lowering and the neutral `RedirPlan`/`PlanOp`, then make the single-external-command child path consume it via a `RedirPlan` → `ChildRedirPlan` translation. `build_child_redir_plan` becomes a thin wrapper. The in-process path is unchanged this task (still uses `apply`/`apply_var`).

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — add `RedirPlan`, `PlanOp`, `lower_redirects`, `lower_named_fd`, `redir_plan_to_child`; replace the body of `build_child_redir_plan` (~5615) with a wrapper.

**Interfaces:**
- Consumes: `FdPlacement` (Task 2); existing `resolve_dup_source`, `validate_fd_open`, `check_restricted_redirect`, `expand_single`, `expand_assignment`, `spawn_heredoc_writer`, `relocate_high_cloexec`, `crate::child_fd::dup_to_high_fd`, `redir_open_error`; existing `ChildRedirPlan { ops: Vec<ChildRedirOp>, held: Vec<OwnedFd>, heredoc_writers: Vec<libc::pid_t> }` and `ChildRedirOp::{Dup{target,source}, Close{target}}`.
- Produces:
  - `struct RedirPlan { ops: Vec<PlanOp>, heredoc_writers: Vec<libc::pid_t> }` (ownership of temp fds lives INSIDE the ops, not a separate held vec).
  - `enum PlanOp { InstallOwned { target: RawFd, source: OwnedFd }, InstallDup { target: RawFd, source: RawFd }, Close { target: RawFd }, NamedFd { high: OwnedFd, name: String } }`.
  - `fn lower_redirects(redirects: &[Redirection], shell: &mut Shell, sink: &mut StdoutSink, err_sink: &mut StderrSink) -> Result<RedirPlan, i32>`.
  - `fn redir_plan_to_child(plan: RedirPlan) -> ChildRedirPlan`.
  - Task 4 will add `RedirectScope::apply_plan(&mut self, plan: RedirPlan, …)` consuming the SAME `RedirPlan`.

- [ ] **Step 1: Add the plan types**

Immediately above `struct ChildRedirPlan` (~5587), add:
```rust
/// The neutral result of lowering an ordered redirect list: what fds the
/// command will see, resolved but not yet installed. Consumed by exactly two
/// appliers — `redir_plan_to_child` (child dup2/close replay) and
/// `RedirectScope::apply_plan` (in-process save/restore). Ownership of every
/// parent-opened temp (files, heredoc read ends, `{var}` high fds) lives INSIDE
/// the ops, so a lowering error drops them (no leak; P1 discipline).
struct RedirPlan {
    ops: Vec<PlanOp>,
    heredoc_writers: Vec<libc::pid_t>,
}

/// One resolved, ordered redirect action. Source order is preserved.
enum PlanOp {
    /// A parent-opened temp (`>file`, heredoc/here-string read end) duped onto
    /// `target`. In-process: if `source`'s fd == `target` (a relocated file that
    /// landed on its own target, target >= 10) clear FD_CLOEXEC in place (#135);
    /// else dup2 + save/restore, then close the temp. Child: dup2 (replay's
    /// `source == target` arm clears CLOEXEC), temp held until spawn.
    InstallOwned { target: RawFd, source: std::os::fd::OwnedFd },
    /// A borrowed shell fd (`>&w` / `<&w`, and the dup half of a move). `source`
    /// is a resolved fd NUMBER. In-process: validate open, then dup2 + save/restore.
    /// Child: dup2 (no validation — the fd is inherited).
    InstallDup { target: RawFd, source: RawFd },
    /// `N>&-`, and the source-close half of a move (`>&w-`).
    Close { target: RawFd },
    /// `{var}` named-fd. `high` is the live descriptor the command sees, already
    /// allocated non-CLOEXEC (>= 10). In-process: assign `$name = high` and let it
    /// persist (take it out of the plan; do NOT save/restore). Child: keep `high`
    /// held (inherited, non-CLOEXEC), replay a defensive `dup2(high, high)`, and
    /// do NOT assign `$name` (bash doesn't for an external command).
    NamedFd { high: std::os::fd::OwnedFd, name: String },
}
```

- [ ] **Step 2: Add `lower_redirects` and `lower_named_fd`**

Immediately above `fn build_child_redir_plan` (~5615), add:
```rust
/// The single redirect lowering (Phase 3a). Walks `redirects` in source order,
/// resolving each into a neutral `PlanOp`: opens files (as OwnedFd), spawns
/// heredoc writers, resolves dup WORDS to fd NUMBERS, and allocates `{var}` high
/// fds. It does NOT apply anything and does NOT validate dup sources (validation
/// is apply-time for the in-process path: `3>file 4>&3`). On any error it closes
/// every fd opened so far (so heredoc writers hit EOF/EPIPE) then reaps those
/// writers, and returns Err(code) with the diagnostic already printed.
fn lower_redirects(
    redirects: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<RedirPlan, i32> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let mut plan = RedirPlan {
        ops: Vec::new(),
        heredoc_writers: Vec::new(),
    };
    // On error: drop opened fds first (close read ends -> writers get EOF/EPIPE),
    // then reap. Hang-free even for >64KB heredoc bodies. Both appliers converge
    // on this cleanup (child path previously leaked the zombie; benign).
    macro_rules! fail {
        ($code:expr) => {{
            plan.ops.clear();
            for pid in plan.heredoc_writers.drain(..) {
                let mut st = 0;
                unsafe { libc::waitpid(pid, &mut st, 0) };
            }
            return Err($code);
        }};
    }
    for redir in redirects {
        if let RedirFd::Var(name) = &redir.fd {
            if let Err(code) = lower_named_fd(name, redir, shell, sink, err_sink, &mut plan) {
                fail!(code);
            }
            continue;
        }
        let Some(target) = redir.target_fd() else {
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "ambiguous redirect");
            }
            fail!(1);
        };
        let target = target as RawFd;
        match &redir.op {
            RedirOp::File { mode, target: word } => {
                let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(p) => p,
                    Err(()) => fail!(1),
                };
                if check_restricted_redirect(mode, &path, shell, sink, err_sink).is_err() {
                    fail!(1);
                }
                let owned = match open_redirect_file(
                    mode,
                    &path,
                    shell.shell_options.noclobber,
                    FdPlacement::Relocated,
                ) {
                    Ok(fd) => fd,
                    Err(e) => {
                        redir_open_error(shell, err_sink, sink, &path, &e);
                        fail!(1);
                    }
                };
                plan.ops.push(PlanOp::InstallOwned {
                    target,
                    source: owned,
                });
            }
            RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
                let is_move = matches!(&redir.op, RedirOp::Move { .. });
                let src = match resolve_dup_source(source, shell, sink, err_sink) {
                    Ok(n) => n,
                    Err(()) => fail!(1),
                };
                // Degenerate `N>&N-` (source == target): bash no-op (redir.c's
                // `redir_fd != redirector` guard). Contributes nothing.
                if !(is_move && src == target) {
                    plan.ops.push(PlanOp::InstallDup { target, source: src });
                    if is_move {
                        plan.ops.push(PlanOp::Close { target: src });
                    }
                }
            }
            RedirOp::Close => plan.ops.push(PlanOp::Close { target }),
            RedirOp::Heredoc { body, .. } => {
                let bytes = expand_assignment(body, shell).into_bytes();
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        plan.heredoc_writers.push(pid);
                        let rfd = relocate_high_cloexec(rfd);
                        let owned = unsafe { OwnedFd::from_raw_fd(rfd) };
                        plan.ops.push(PlanOp::InstallOwned {
                            target,
                            source: owned,
                        });
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        fail!(1);
                    }
                }
            }
            RedirOp::HereString(w) => {
                let mut bytes = expand_assignment(w, shell).into_bytes();
                bytes.push(b'\n');
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        plan.heredoc_writers.push(pid);
                        let rfd = relocate_high_cloexec(rfd);
                        let owned = unsafe { OwnedFd::from_raw_fd(rfd) };
                        plan.ops.push(PlanOp::InstallOwned {
                            target,
                            source: owned,
                        });
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        fail!(1);
                    }
                }
            }
        }
    }
    Ok(plan)
}

/// Lower a `{var}` named-fd redirection. Allocates a free high fd (>= 10,
/// non-CLOEXEC) duped from the resolved source and emits a `NamedFd` op; a move
/// also emits a `Close` of the original source. `{var}>&-` emits a `Close` of the
/// fd currently named by `$name`. Mirrors the old `apply_var` /
/// `build_child_redir_plan` `{var}` arms exactly.
fn lower_named_fd(
    name: &str,
    redir: &Redirection,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    plan: &mut RedirPlan,
) -> Result<(), i32> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    if matches!(&redir.op, RedirOp::Close) {
        let cur = shell.lookup_var(name).unwrap_or_default();
        let fd: RawFd = match cur.trim().parse::<i32>() {
            Ok(n) if n >= 0 => n,
            _ => {
                {
                    let mut err = err_writer(err_sink, sink);
                    crate::sh_error_to!(shell, &mut *err, None, "{name}: ambiguous redirect");
                }
                return Err(1);
            }
        };
        plan.ops.push(PlanOp::Close { target: fd });
        return Ok(());
    }
    let is_move = matches!(&redir.op, RedirOp::Move { .. });
    // Resolve the source fd. `owned_src` holds an fd we opened (File / heredoc /
    // here-string read end) that must be closed after duping to `high`; `dup_src`
    // is a borrowed shell fd number (a Dup/Move source) left alone.
    let mut owned_src: Option<OwnedFd> = None;
    let mut dup_src: Option<RawFd> = None;
    match &redir.op {
        RedirOp::File { mode, target: word } => {
            let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                Ok(p) => p,
                Err(()) => return Err(1),
            };
            if check_restricted_redirect(mode, &path, shell, sink, err_sink).is_err() {
                return Err(1);
            }
            // RawLow: the {var}-fd relocation happens once below via dup_to_high_fd.
            match open_redirect_file(mode, &path, shell.shell_options.noclobber, FdPlacement::RawLow)
            {
                Ok(fd) => owned_src = Some(fd),
                Err(e) => {
                    redir_open_error(shell, err_sink, sink, &path, &e);
                    return Err(1);
                }
            }
        }
        RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
            let src = resolve_dup_source(source, shell, sink, err_sink).map_err(|()| 1)?;
            dup_src = Some(src);
        }
        RedirOp::Heredoc { body, .. } => {
            let bytes = expand_assignment(body, shell).into_bytes();
            match spawn_heredoc_writer(&bytes) {
                Ok((rfd, pid)) => {
                    plan.heredoc_writers.push(pid);
                    owned_src = Some(unsafe { OwnedFd::from_raw_fd(rfd) });
                }
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "heredoc: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    return Err(1);
                }
            }
        }
        RedirOp::HereString(w) => {
            let mut bytes = expand_assignment(w, shell).into_bytes();
            bytes.push(b'\n');
            match spawn_heredoc_writer(&bytes) {
                Ok((rfd, pid)) => {
                    plan.heredoc_writers.push(pid);
                    owned_src = Some(unsafe { OwnedFd::from_raw_fd(rfd) });
                }
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "heredoc: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    return Err(1);
                }
            }
        }
        RedirOp::Close => unreachable!("Close handled above"),
    }
    let raw_src: RawFd = match (&owned_src, dup_src) {
        (Some(o), _) => o.as_raw_fd(),
        (None, Some(s)) => s,
        _ => unreachable!("resolved exactly one source"),
    };
    let high = match crate::child_fd::dup_to_high_fd(raw_src, 10, false) {
        Ok(h) => h,
        Err(e) => {
            // owned_src drops here (closes it); a dup_src is the shell's, left open.
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "{name}: {}", crate::bash_io_error(&e));
            }
            return Err(1);
        }
    };
    // Close the owned source now that it's been duped to `high`.
    drop(owned_src);
    plan.ops.push(PlanOp::NamedFd {
        high: unsafe { OwnedFd::from_raw_fd(high) },
        name: name.to_string(),
    });
    if is_move {
        // Move: close the original source (a shell fd) after the dup. Only a
        // Dup/Move source reaches here (owned sources aren't moves).
        if let Some(s) = dup_src {
            plan.ops.push(PlanOp::Close { target: s });
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Add the child translation and make `build_child_redir_plan` a wrapper**

Replace the ENTIRE body of `fn build_child_redir_plan` (from its `{` at ~5615 through its closing `}` at ~5900) with:
```rust
fn build_child_redir_plan(
    redirects: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<ChildRedirPlan, i32> {
    Ok(redir_plan_to_child(lower_redirects(
        redirects, shell, sink, err_sink,
    )?))
}

/// Translate a neutral `RedirPlan` into the child dup2/close replay plan. Owned
/// temps move into `held` (kept alive until the fork); `{var}` high fds replay a
/// defensive same-fd op and are held (the child inherits them non-CLOEXEC).
/// `$var` is NOT assigned — bash doesn't for an external command.
fn redir_plan_to_child(plan: RedirPlan) -> ChildRedirPlan {
    use std::os::fd::AsRawFd;
    let mut child = ChildRedirPlan {
        ops: Vec::new(),
        held: Vec::new(),
        heredoc_writers: plan.heredoc_writers,
    };
    for op in plan.ops {
        match op {
            PlanOp::InstallOwned { target, source } => {
                let raw = source.as_raw_fd();
                child.ops.push(ChildRedirOp::Dup { target, source: raw });
                child.held.push(source);
            }
            PlanOp::InstallDup { target, source } => {
                child.ops.push(ChildRedirOp::Dup { target, source });
            }
            PlanOp::Close { target } => child.ops.push(ChildRedirOp::Close { target }),
            PlanOp::NamedFd { high, name: _ } => {
                let raw = high.as_raw_fd();
                child.ops.push(ChildRedirOp::Dup {
                    target: raw,
                    source: raw,
                });
                child.held.push(high);
            }
        }
    }
    child
}
```

- [ ] **Step 4: Build — expect warning-clean (all new items used by the child path)**

Run:
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. `RedirPlan`, `PlanOp` (all variants), `lower_redirects`, `lower_named_fd`, and `redir_plan_to_child` are all reachable from the `build_child_redir_plan` wrapper. The in-process `apply`/`apply_var` still exist and are still called (they are deleted in Task 4), so nothing is dead. If a real unused-import warning appears, remove the specific unused import.

- [ ] **Step 5: Run the full regression nets (child path is exercised by every external command with redirects)**

Run:
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
tests/scripts/fd_torture_diff_check.sh | tail -1
ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3
```
Expected: lib `test result: ok`, fd_torture `... passed, 0 failed`, named_fd `test result: ok. 7 passed`.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v292 T3: introduce lower_redirects + rewire the child path (#139)

New neutral RedirPlan/PlanOp + single lower_redirects() lowering; the
single-external-command child path now consumes it via redir_plan_to_child
(RedirPlan -> ChildRedirPlan). build_child_redir_plan is a thin wrapper.
In-process path unchanged this task. Behavior-preserving.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Add `RedirectScope::apply_plan`, rewire the 3 in-process call sites, delete `apply`/`apply_var`

Point the in-process paths at `lower_redirects` too, then delete the now-dead duplicated resolvers. After this task there is exactly one redirect lowering.

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — add `RedirectScope::apply_plan`; rewrite the 3 loops (`with_redirect_scope` ~1529, the builtin-redirect helper ~1640, `apply_redirects_permanently` ~5385); delete `fn apply` (~1008–1168) and `fn apply_var` (~1177–1326).

**Interfaces:**
- Consumes: `lower_redirects`, `RedirPlan`, `PlanOp` (Task 3); existing `RedirectScope::{redirect, close_target, reap_heredoc_writers, saved, heredoc_writers}`, `validate_fd_open`.
- Produces: `RedirectScope::apply_plan`. Removes `RedirectScope::apply` / `apply_var` from the API.

- [ ] **Step 1: Add `apply_plan`**

In `impl RedirectScope`, replacing the deleted `apply`/`apply_var` (keep `new`, `redirect`, `close_target`, `reap_heredoc_writers`), add:
```rust
    /// Apply a lowered `RedirPlan` to the real fds in source order, save/restore
    /// aware (Drop rolls back). Replaces the old per-`Redirection` `apply`.
    fn apply_plan(
        &mut self,
        plan: RedirPlan,
        shell: &mut Shell,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ExecOutcome> {
        use std::os::fd::{AsRawFd, IntoRawFd};
        self.heredoc_writers.extend(plan.heredoc_writers);
        for op in plan.ops {
            match op {
                PlanOp::InstallOwned { target, source } => {
                    let raw = source.as_raw_fd();
                    if raw == target {
                        // #135: a relocated file landed on its own target (target
                        // >= 10). Keep it, clear FD_CLOEXEC in place (a no-op dup2
                        // would NOT), record a was-closed restore. Persist by
                        // taking it out of the OwnedFd so Drop doesn't close it.
                        let fd = source.into_raw_fd();
                        unsafe {
                            let flags = libc::fcntl(fd, libc::F_GETFD);
                            if flags >= 0 {
                                let _ =
                                    libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                            }
                        }
                        self.saved.push((target, -1));
                    } else if self.redirect(shell, raw, target, sink, err_sink).is_err() {
                        // `source` drops here (closes the temp) on the error path.
                        return Err(ExecOutcome::Continue(1));
                    } else {
                        // dup2 succeeded; close the temp now (matches the old
                        // close-after-dup2 so a later {var}/open reuses its number).
                        drop(source);
                    }
                }
                PlanOp::InstallDup { target, source } => {
                    validate_fd_open(source, shell, sink, err_sink)
                        .map_err(|()| ExecOutcome::Continue(1))?;
                    if self.redirect(shell, source, target, sink, err_sink).is_err() {
                        return Err(ExecOutcome::Continue(1));
                    }
                }
                PlanOp::Close { target } => self.close_target(target),
                PlanOp::NamedFd { high, name } => {
                    // Assign $var and leave `high` OPEN (persists past the command);
                    // NOT registered in `saved`, so Drop must not close it.
                    let fd = high.into_raw_fd();
                    shell.set(&name, fd.to_string());
                }
            }
        }
        Ok(())
    }
```
Then delete `fn apply` and `fn apply_var` in their entirety.

- [ ] **Step 2: Rewire `with_redirect_scope` (~1529)**

Replace:
```rust
    let force_terminal = redirs_write_stdout(redirs);
    for r in redirs {
        if let Err(outcome) = scope.apply(r, shell, sink, err_sink) {
            scope.reap_heredoc_writers();
            drop(scope);
            drain_procsubs(shell, procsub_base);
            return outcome;
        }
    }
```
with:
```rust
    let force_terminal = redirs_write_stdout(redirs);
    match lower_redirects(redirs, shell, sink, err_sink) {
        Ok(plan) => {
            if let Err(outcome) = scope.apply_plan(plan, shell, sink, err_sink) {
                scope.reap_heredoc_writers();
                drop(scope);
                drain_procsubs(shell, procsub_base);
                return outcome;
            }
        }
        Err(code) => {
            scope.reap_heredoc_writers();
            drop(scope);
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(code);
        }
    }
```

- [ ] **Step 3: Rewire the builtin-redirect helper loop (~1640)**

Replace:
```rust
    let mut scope = RedirectScope::new();
    for r in redirs {
        if let Err(outcome) = scope.apply(r, shell, sink, err_sink) {
            scope.reap_heredoc_writers();
            drop(scope);
            drain_procsubs(shell, procsub_base);
            return outcome;
        }
    }
```
with:
```rust
    let mut scope = RedirectScope::new();
    match lower_redirects(redirs, shell, sink, err_sink) {
        Ok(plan) => {
            if let Err(outcome) = scope.apply_plan(plan, shell, sink, err_sink) {
                scope.reap_heredoc_writers();
                drop(scope);
                drain_procsubs(shell, procsub_base);
                return outcome;
            }
        }
        Err(code) => {
            scope.reap_heredoc_writers();
            drop(scope);
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(code);
        }
    }
```

- [ ] **Step 4: Rewire `apply_redirects_permanently` (~5385)**

Replace:
```rust
    for redir in &cmd.redirects {
        if scope.apply(redir, shell, sink, err_sink).is_err() {
            scope.reap_heredoc_writers();
            return Err(()); // scope Drop restores partial → atomic rollback
        }
    }
```
with:
```rust
    match lower_redirects(&cmd.redirects, shell, sink, err_sink) {
        Ok(plan) => {
            if scope.apply_plan(plan, shell, sink, err_sink).is_err() {
                scope.reap_heredoc_writers();
                return Err(()); // scope Drop restores partial → atomic rollback
            }
        }
        Err(_) => {
            scope.reap_heredoc_writers();
            return Err(());
        }
    }
```

- [ ] **Step 5: Build — expect it to be warning-clean (nothing dead now)**

Run:
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. `apply`/`apply_var` are gone; `lower_redirects`/`apply_plan`/`redir_plan_to_child` all have live callers. Grep to confirm the old methods are gone:
```bash
grep -n 'fn apply\b\|fn apply_var\|scope.apply(' crates/huck-engine/src/executor.rs
```
Expected: no matches.

- [ ] **Step 6: Run the full regression suite (both binaries)**

Run:
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3
cargo build --release -p huck 2>&1 | tail -1
( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )
```
Expected: lib `ok`, named_fd `7 passed`, sweep `188 passed, 0 failed`. Run the sweep once more selecting the release binary if `run_diff_checks.sh` doesn't already build both — confirm fd_torture passes on both debug and release.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v292 T4: apply_plan + rewire in-process paths; delete apply/apply_var (#139)

The three in-process redirect call sites (with_redirect_scope, the builtin
helper, apply_redirects_permanently) now lower via lower_redirects and apply
via RedirectScope::apply_plan. The duplicated apply/apply_var resolvers are
deleted — one redirect lowering remains. Behavior-preserving.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- **Behavior-preservation is the contract.** The proof is the sweep + fd_torture + named_fd staying byte-green on both binaries. There is no new feature to test.
- **The one deliberate convergence:** `lower_redirects` reaps its own heredoc writers on an error path (after closing their read ends). The old child path leaked that zombie; the old in-process path reaped before closing (a latent large-body hang). The new path is strictly safer and not observable in the sweep — flag for confirmation, not a defect.
- **Ownership timing to verify:** in-process `InstallOwned` (non-#135) closes its temp immediately after dup2 (via `drop(source)`), NOT at end-of-plan, so a subsequent `{var}`/open reuses the freed low number exactly as before (the `3a namedfd` cases guard this). The `#135` and `NamedFd` arms `into_raw_fd()` to persist.
- **Async-signal safety:** `PlanOp::NamedFd`'s `String` never reaches `pre_exec` — the child path translates to `ChildRedirOp` (no `String`) in `redir_plan_to_child`, before the fork. `replay_redir_ops` is untouched.
- **Untouched by design:** `build_child_extra_ops`, `stage_extra_redirects`, `slot_consumes`, the `run_multi_stage` slot reads, and the H7 sink layer. Confirm the diff touches none of them.
