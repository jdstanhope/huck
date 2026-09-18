//! Job table for tracking background pipelines.
//!
//! A `Job` represents one background pipeline. Its `pids` are the PIDs of
//! the pipeline stages in order; its `pgid` is the process group ID
//! (always equal to the first stage's PID). `reap` updates per-pid state
//! when a child is reaped; when all pids are reaped, the job's overall
//! state transitions to `Done` or `Signaled` based on the LAST stage's
//! status (matching bash's pipeline exit-status rule without `pipefail`).

use crate::err_thread_local::with_err;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped(i32),
    Done(i32),
    Signaled(i32),
}

#[derive(Debug, PartialEq, Eq)]
pub enum JobSpecResolveError {
    NotFound,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    #[allow(dead_code)]
    pub pgid: i32,
    pub pids: Vec<i32>,
    pub reaped: Vec<bool>,
    pub last_status: Option<i32>,
    pub command: String,
    pub state: JobState,
    pub notified: bool,
    pub created_at: u64,
    pub marked_for_nohup: bool,
    /// True when this job has its OWN process group (`setpgid`'d at spawn —
    /// interactive job control, or any stopped/own-group job). False when the
    /// job shares the shell's process group (a non-interactive background job,
    /// since v173): signal it per-pid, never `killpg`. Bash's `J_JOBCONTROL`.
    pub own_pgroup: bool,
    /// Set when the job died from a signal that dumped core, so the notice can
    /// carry bash's ` (core dumped)` suffix (#420).
    pub core_dumped: bool,
}

/// Cap on the saved terminal-status ring (`last_statuses`). Bounded so that
/// #175's whole point — no unbounded job-table growth — is preserved: a script
/// that backgrounds millions of jobs without `wait`ing them cannot leak memory
/// through the saved-status side table either. On overflow the oldest entry is
/// dropped. bash keeps completed statuses waitable until they age out; 4096 is
/// far more than any realistic `wait $pid`-after-the-fact working set.
const SAVED_STATUS_CAP: usize = 4096;

/// bash's `js.c_childmax`: `sysconf(_SC_CHILD_MAX)` clamped to
/// [`DEFAULT_CHILD_MAX`, `MAX_CHILD_MAX`] (4096..=32768). Only read by the
/// no-force arm of `mark_dead_jobs_as_notified`.
fn child_max() -> usize {
    let raw = unsafe { libc::sysconf(libc::_SC_CHILD_MAX) };
    let n = if raw < 0 { 32768 } else { raw as usize };
    n.clamp(4096, 32768)
}

#[derive(Debug, Clone, Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    next_created_at: u64,
    /// bash's `js.j_current` / `js.j_previous` — the `+` and `-` jobs. Stored,
    /// not derived: a job KEEPS `+` after it dies until it is deleted, and a
    /// stop takes `+` from a newer running job. Maintained by `set_current_job`
    /// / `reset_current` at the same points bash calls them (a new job, a
    /// deletion, a stop, a continue).
    current: Option<u32>,
    previous: Option<u32>,
    /// Terminal exit statuses of jobs that have already been pruned from
    /// `jobs`, keyed by (pid, decoded-status). bash prunes completed jobs from
    /// the visible `jobs` list but RETAINS their exit status so a later
    /// `wait $pid` still resolves (repeatedly, until it ages out). Populated by
    /// every prune path (`remove_notified`, `remove_job_recording_status`);
    /// consulted by `wait`'s ECHILD fallback. Drop-oldest bounded at
    /// `SAVED_STATUS_CAP`.
    last_statuses: Vec<(i32, i32)>,
    /// #183: pids of LIVE children this shell forked, tracked independently of
    /// the visible `jobs` list. `reap_completed` walks this set instead of
    /// calling `waitpid(-1)`, which reaps ANY child of the process — fine for a
    /// standalone shell that owns its process, but huck-engine is a LIBRARY, so
    /// a wildcard wait steals children the EMBEDDER spawned (and, in the
    /// multithreaded cargo test binary, children of concurrently running tests).
    ///
    /// Deliberately NOT derived from `jobs`: a bare `disown` removes the job
    /// (`builtins::builtin_disown`) while its child lives on, so a set keyed on
    /// table membership would leave disowned children as zombies — trading one
    /// leak for another. Entries are released on terminal reap (or on ECHILD,
    /// when something else got there first), so this stays bounded by the number
    /// of LIVE children and cannot re-introduce a #175-style leak.
    owned_pids: Vec<i32>,
}

impl JobTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only view of the current jobs. Used by `compgen -A job/running/stopped`.
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    /// #183: pids of live children this shell forked — the reap set walked by
    /// `reap_completed` in place of `waitpid(-1)`. Survives `disown` (which drops
    /// the visible job but not our duty to reap its child).
    pub fn owned_pids(&self) -> &[i32] {
        &self.owned_pids
    }

    /// #183: forget `pid` — it has been reaped (by us or by someone else, e.g.
    /// the `wait` builtin), so it is no longer a live child. Keeps `owned_pids`
    /// bounded by the number of LIVE children.
    pub fn release_owned_pid(&mut self, pid: i32) {
        self.owned_pids.retain(|&p| p != pid);
    }

    /// Inserts a new Running job that owns its process group (the common case:
    /// interactive job control). Numbers it after the last live job (see
    /// `next_id`), having first dropped the reported dead ones. Returns it.
    pub fn add(&mut self, pgid: i32, pids: Vec<i32>, command: String) -> u32 {
        self.remove_notified();
        self.add_with_pgroup(pgid, pids, command, true)
    }

    /// Like `add`, but records whether the job owns its process group. A
    /// non-interactive background job shares the shell's group (`own_pgroup =
    /// false`) and must be signalled per-pid.
    pub fn add_with_pgroup(
        &mut self,
        pgid: i32,
        pids: Vec<i32>,
        command: String,
        own_pgroup: bool,
    ) -> u32 {
        self.remove_notified();
        let id = self.next_id();
        let n = pids.len();
        // #183: every registered pid is a live child we own and must reap
        // ourselves. This is the single registration choke point (`add` delegates
        // here), so it is the only place ownership needs recording.
        for &p in &pids {
            if p > 0 && !self.owned_pids.contains(&p) {
                self.owned_pids.push(p);
            }
        }
        let job = Job {
            id,
            pgid,
            pids,
            reaped: vec![false; n],
            last_status: None,
            command,
            state: JobState::Running,
            notified: false,
            created_at: self.next_created_at,
            marked_for_nohup: false,
            own_pgroup,
            core_dumped: false,
        };
        self.insert_job(job)
    }

    /// Inserts a synthetic already-Done job — used for pure-builtin
    /// pipelines that ran synchronously in the parent shell.
    pub fn add_synthetic_done(&mut self, command: String, exit: i32) -> u32 {
        self.remove_notified();
        let id = self.next_id();
        let job = Job {
            id,
            pgid: 0,
            pids: Vec::new(),
            reaped: Vec::new(),
            // Encode `exit` as a normal-exit raw waitpid status so any
            // future reader of `last_status` can decode it consistently.
            last_status: Some(exit << 8),
            command,
            state: JobState::Done(exit),
            notified: false,
            created_at: self.next_created_at,
            marked_for_nohup: false,
            own_pgroup: true,
            core_dumped: false,
        };
        self.insert_job(job)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter()
    }

    /// Marks `pid` as reaped with the given raw waitpid status. If the pid
    /// is the LAST stage of its job, records the status; when all pids of
    /// the job are reaped, transitions its overall state. No-op if `pid`
    /// isn't owned by any job in the table.
    pub fn reap(&mut self, pid: i32, raw_status: i32) {
        for job in self.jobs.iter_mut() {
            if let Some(idx) = job.pids.iter().position(|&p| p == pid) {
                if libc::WIFSTOPPED(raw_status) {
                    let new_sig = libc::WSTOPSIG(raw_status);
                    // Idempotent: the synchronous waiter in the executor / `fg` already
                    // handled this stop event for one stage; later WUNTRACED reports for
                    // sibling stages of the same pipeline must not re-fire the
                    // notification. Only update + re-notify if the state actually changes.
                    let already_in_this_state =
                        matches!(job.state, JobState::Stopped(s) if s == new_sig);
                    if !already_in_this_state {
                        job.state = JobState::Stopped(new_sig);
                        job.notified = false;
                        let id = job.id;
                        // bash's `waitchld`: a job that just stopped becomes current.
                        self.set_current_job(id);
                    }
                    return;
                }
                if libc::WIFCONTINUED(raw_status) {
                    // A WCONTINUED report: a previously-Stopped job resumed
                    // (e.g. `kill -s CONT` / `bg`). Flip it back to Running. A
                    // continue is NOT a terminal reap, so do not touch
                    // `job.reaped[idx]`. Idempotent: no-op if already Running.
                    if matches!(job.state, JobState::Stopped(_)) {
                        job.state = JobState::Running;
                        job.notified = false;
                        // bash's `waitchld`: a continue re-picks the current job.
                        self.reset_current();
                    }
                    return;
                }
                if job.reaped[idx] {
                    return;
                }
                job.reaped[idx] = true;
                // Record the status if this is the last stage.
                if idx == job.pids.len() - 1 {
                    job.last_status = Some(raw_status);
                }
                if job.reaped.iter().all(|&b| b) {
                    let raw = job.last_status.unwrap_or(0);
                    job.state = decode_status(raw);
                    job.core_dumped = libc::WIFSIGNALED(raw) && core_dumped(raw);
                }
                return;
            }
        }
        // pid not in any job — silently ignore (it could be a long-dead
        // child or one not tracked in the job table).
    }

    /// Returns every job whose state has changed since it was last reported
    /// (non-Running and not yet notified), in id order. Pure: which of these
    /// get reported — and which get marked — is `notify_of_job_status`'s
    /// decision, ported rule for rule from bash.
    pub fn pending_notifications(&self) -> Vec<Job> {
        let mut out: Vec<Job> = self
            .jobs
            .iter()
            .filter(|j| !matches!(j.state, JobState::Running) && !j.notified)
            .cloned()
            .collect();
        out.sort_by_key(|j| j.id);
        out
    }

    /// Marks one job notified. No-op if the id doesn't exist.
    pub fn mark_one_notified(&mut self, id: u32) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.notified = true;
        }
    }

    /// bash's `mark_dead_jobs_as_notified`. With `force`, every dead job is
    /// marked (`wait` with no operands: POSIX lets the shell discard every
    /// collected status) — except, in a non-interactive shell, the job that
    /// owns `$!`, which POSIX says must stay waitable until reported.
    /// Without `force` (a loop iteration's `REAP()`), nothing is marked unless
    /// the dead processes exceed CHILD_MAX, and then only the oldest, down to
    /// the cap — a bound, not a behaviour, in any real script.
    pub fn mark_dead_jobs_as_notified(
        &mut self,
        force: bool,
        interactive: bool,
        last_async_pid: Option<i32>,
    ) {
        let exempt = |j: &Job| !interactive && j.pids.last().copied() == last_async_pid;
        if force {
            for job in self.jobs.iter_mut() {
                if Self::terminal_code(&job.state).is_some() && !exempt(job) {
                    job.notified = true;
                }
            }
            return;
        }
        let mut ndeadproc: usize = self
            .jobs
            .iter()
            .filter(|j| Self::terminal_code(&j.state).is_some())
            .map(|j| j.pids.len().max(1))
            .sum();
        let childmax = child_max();
        if ndeadproc <= childmax {
            return;
        }
        for job in self.jobs.iter_mut() {
            if Self::terminal_code(&job.state).is_some() && !exempt(job) {
                ndeadproc -= job.pids.len().max(1);
                if ndeadproc <= childmax {
                    break;
                }
                job.notified = true;
            }
        }
    }

    /// Drops all jobs that are non-Running AND notified. Before dropping each
    /// one, its terminal exit status is recorded in the saved-status ring so a
    /// later `wait $pid` still resolves it (bash retains completed statuses even
    /// after pruning the visible `jobs` entry).
    pub fn remove_notified(&mut self) {
        // Clone the to-be-pruned jobs first so we can reuse `record_pruned_job`
        // (which borrows `self` mutably) without holding an immutable borrow of
        // `self.jobs` across it; these jobs are about to be dropped anyway.
        let pruned: Vec<Job> = self
            .jobs
            .iter()
            .filter(|j| !matches!(j.state, JobState::Running | JobState::Stopped(_)) && j.notified)
            .cloned()
            .collect();
        for job in &pruned {
            self.record_pruned_job(job);
        }
        self.delete_where(|j| {
            !matches!(j.state, JobState::Running | JobState::Stopped(_)) && j.notified
        });
    }

    /// The decoded terminal exit code for a completed job: `Done(c)` → `c`,
    /// `Signaled(s)` → `128 + s`. `None` for a still-live (Running/Stopped) job.
    fn terminal_code(state: &JobState) -> Option<i32> {
        match state {
            JobState::Done(c) => Some(*c),
            JobState::Signaled(s) => Some(128 + *s),
            JobState::Running | JobState::Stopped(_) => None,
        }
    }

    /// Records a to-be-pruned job's terminal status against each of its pids
    /// (so `$!` — the leader pid — resolves via `wait`). No-op if the job is not
    /// terminal. A synthetic Done job has no pids, so nothing is recorded — it
    /// was never a real child, so `wait $pid` on it could not resolve anyway.
    fn record_pruned_job(&mut self, job: &Job) {
        if let Some(code) = Self::terminal_code(&job.state) {
            for &pid in &job.pids {
                self.record_terminal_status(pid, code);
            }
        }
    }

    /// Records one `(pid, code)` in the bounded saved-status ring. If `pid` is
    /// already present its code is refreshed in place; otherwise it is appended,
    /// evicting the oldest entry when the cap is exceeded.
    pub fn record_terminal_status(&mut self, pid: i32, code: i32) {
        if let Some(slot) = self.last_statuses.iter_mut().find(|(p, _)| *p == pid) {
            slot.1 = code;
            return;
        }
        if self.last_statuses.len() >= SAVED_STATUS_CAP {
            self.last_statuses.remove(0);
        }
        self.last_statuses.push((pid, code));
    }

    /// Looks up a saved terminal status by pid. Does NOT remove it — bash
    /// resolves `wait $pid` repeatedly until the entry ages out.
    pub fn saved_status(&self, pid: i32) -> Option<i32> {
        self.last_statuses
            .iter()
            .rev()
            .find(|(p, _)| *p == pid)
            .map(|(_, code)| *code)
    }

    /// The `+` and `-` jobs (bash's `js.j_current` / `js.j_previous`). A dead
    /// job keeps its marker until it is deleted, so `jobs` shows `[2]+  Done`
    /// for the job the user just saw finish.
    pub fn current_and_previous(&self) -> (Option<u32>, Option<u32>) {
        (self.current, self.previous)
    }

    /// Most-recent Running or Stopped job id (the `+` job for fg/bg/jobs).
    pub fn current_id(&self) -> Option<u32> {
        self.jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Running | JobState::Stopped(_)))
            .max_by_key(|j| j.created_at)
            .map(|j| j.id)
    }

    /// Most-recent Stopped job id, ignoring Running jobs. Used by `bg`.
    pub fn current_stopped_id(&self) -> Option<u32> {
        self.jobs
            .iter()
            .filter(|j| matches!(j.state, JobState::Stopped(_)))
            .max_by_key(|j| j.created_at)
            .map(|j| j.id)
    }

    /// True if any job is Running or Stopped (i.e., `wait` should block).
    pub fn has_pending(&self) -> bool {
        self.jobs
            .iter()
            .any(|j| matches!(j.state, JobState::Running | JobState::Stopped(_)))
    }

    /// Resolves a JobSpec to a job id, if any matching job exists.
    pub fn resolve(&self, spec: &crate::job_spec::JobSpec) -> Result<u32, JobSpecResolveError> {
        use crate::job_spec::JobSpec;
        match spec {
            JobSpec::Id(id) => self
                .jobs
                .iter()
                .find(|j| j.id == *id)
                .map(|j| j.id)
                .ok_or(JobSpecResolveError::NotFound),
            // #758: bash's `%+` is `js.j_current`, which a job KEEPS after it dies
            // until it is deleted at a cleanup point — so `kill -KILL %+; wait %`
            // still finds the job and reports 137. `current_id` (Running/Stopped
            // only) is the right question for `fg`/`bg`, not for resolving a spec.
            JobSpec::Current => {
                let (cur, _) = self.current_and_previous();
                cur.ok_or(JobSpecResolveError::NotFound)
            }
            JobSpec::Previous => {
                let (_, prev) = self.current_and_previous();
                prev.ok_or(JobSpecResolveError::NotFound)
            }
            JobSpec::Prefix(p) => self.resolve_by_command(|cmd| cmd.starts_with(p.as_str())),
            JobSpec::Substring(p) => self.resolve_by_command(|cmd| cmd.contains(p.as_str())),
        }
    }

    pub fn jobs_mut(&mut self) -> &mut Vec<Job> {
        &mut self.jobs
    }

    /// Deletes the jobs in `ids` outright (disown, a terminal job `fg`/`bg`
    /// found), re-picking `+`/`-` if one of them held a marker.
    pub fn remove_ids(&mut self, ids: &[u32]) {
        self.delete_where(|j| ids.contains(&j.id));
    }

    /// Marks the job with id `id` as exempt from the shell's
    /// SIGHUP-on-exit broadcast. No-op if the id doesn't exist.
    pub fn mark_for_nohup(&mut self, id: u32) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.marked_for_nohup = true;
        }
    }

    /// Marks every job in `ids` as notified. Used by `jobs -n` to
    /// consume the state-change flag after printing.
    pub fn mark_notified(&mut self, ids: &[u32]) {
        for job in self.jobs.iter_mut() {
            if ids.contains(&job.id) {
                job.notified = true;
            }
        }
    }

    /// bash's `stop_pipeline` slot rule: the slot after the LAST occupied one
    /// (`js.j_lastj + 1`), so a freed lower number is not reused while a higher
    /// job lives — `sleep 5 & sleep 5 & sleep 5 & kill %2; …; sleep 5 &` is
    /// `%4`, not `%2`. Only an empty table restarts at 1. Callers run
    /// `cleanup_dead_jobs` first, as `stop_pipeline` does.
    fn next_id(&self) -> u32 {
        self.jobs.iter().map(|j| j.id).max().map_or(1, |m| m + 1)
    }

    fn insert_job(&mut self, job: Job) -> u32 {
        let id = job.id;
        self.next_created_at += 1;
        self.jobs.push(job);
        self.jobs.sort_by_key(|j| j.id);
        // `stop_pipeline` ends an async job's registration with `reset_current`.
        self.reset_current();
        id
    }

    /// The newest job numbered below `limit` that satisfies `pred` — bash's
    /// `most_recent_job_in_state`, which walks slot indices downward.
    fn most_recent_below<F: Fn(&Job) -> bool>(&self, limit: u32, pred: F) -> Option<u32> {
        self.jobs
            .iter()
            .filter(|j| j.id < limit && pred(j))
            .map(|j| j.id)
            .max()
    }

    fn is_stopped(&self, id: Option<u32>) -> bool {
        id.and_then(|id| self.jobs.iter().find(|j| j.id == id))
            .is_some_and(|j| matches!(j.state, JobState::Stopped(_)))
    }

    fn is_running(&self, id: Option<u32>) -> bool {
        id.and_then(|id| self.jobs.iter().find(|j| j.id == id))
            .is_some_and(|j| matches!(j.state, JobState::Running))
    }

    /// bash's `set_current_job`: make `id` the `+` job and pick a useful `-`:
    /// the old current if it is stopped; else the newest stopped job older than
    /// the current (when the current is itself stopped); else the newest
    /// running job older than the current (or the newest running job at all
    /// when the current is not running).
    fn set_current_job(&mut self, id: u32) {
        if self.current != Some(id) {
            self.previous = self.current;
            self.current = Some(id);
        }
        if self.previous != self.current && self.is_stopped(self.previous) {
            return;
        }
        let stopped = |j: &Job| matches!(j.state, JobState::Stopped(_));
        let running = |j: &Job| matches!(j.state, JobState::Running);
        if self.is_stopped(self.current)
            && let Some(c) = self.most_recent_below(id, stopped)
        {
            self.previous = Some(c);
            return;
        }
        let limit = if self.is_running(self.current) {
            id
        } else {
            u32::MAX
        };
        self.previous = self.most_recent_below(limit, running);
    }

    /// bash's `reset_current`: keep a stopped current job; otherwise prefer a
    /// stopped previous, then the newest stopped job, then the newest running
    /// job; with none of those there is no current job at all.
    fn reset_current(&mut self) {
        let candidate = if self.is_stopped(self.current) {
            self.current
        } else {
            let stopped = |j: &Job| matches!(j.state, JobState::Stopped(_));
            let running = |j: &Job| matches!(j.state, JobState::Running);
            if self.is_stopped(self.previous) {
                self.previous
            } else {
                self.most_recent_below(u32::MAX, stopped)
                    .or_else(|| self.most_recent_below(u32::MAX, running))
            }
        };
        match candidate {
            Some(id) => self.set_current_job(id),
            None => {
                self.current = None;
                self.previous = None;
            }
        }
    }

    /// Drops the jobs `pred` selects, then re-picks `+`/`-` if either was
    /// among them (bash's `delete_job` → `reset_current`).
    fn delete_where<F: Fn(&Job) -> bool>(&mut self, pred: F) {
        let lost_marker = self
            .jobs
            .iter()
            .any(|j| pred(j) && (Some(j.id) == self.current || Some(j.id) == self.previous));
        self.jobs.retain(|j| !pred(j));
        if lost_marker {
            self.reset_current();
        }
    }

    fn resolve_by_command<F: Fn(&str) -> bool>(&self, pred: F) -> Result<u32, JobSpecResolveError> {
        let matches: Vec<u32> = self
            .jobs
            .iter()
            .filter(|j| pred(j.command.as_str()))
            .map(|j| j.id)
            .collect();
        match matches.len() {
            0 => Err(JobSpecResolveError::NotFound),
            1 => Ok(matches[0]),
            _ => Err(JobSpecResolveError::Ambiguous),
        }
    }
}

/// Reaps this shell's OWN reapable children via non-blocking `waitpid(WNOHANG)`,
/// feeding each into the shell's job table. Also resets the SIGCHLD flag.
///
/// #183: walks `jobs.owned_pids()` + `shell.coprocs` rather than calling
/// `waitpid(-1)`. A wildcard wait reaps ANY child of the process, which is right
/// for a standalone shell that owns its process but WRONG for huck-engine, which
/// is a library: it silently steals children the embedder spawned, taking their
/// exit status with it. The same theft breaks the multithreaded cargo test binary
/// (tests steal each other's children), where it surfaces either as ECHILD from a
/// one-shot `waitpid(pid)` or as an infinite hang in `stream_loop`'s poll loop.
pub fn reap_completed(shell: &mut crate::shell_state::Shell) {
    shell
        .sigchld_flag
        .store(false, std::sync::atomic::Ordering::Relaxed);
    reap_owned_once(shell);
}

/// One bounded reap pass over this shell's OWN children. Returns true if any
/// reported a state change (so a polling caller knows whether to sleep).
///
/// #183: the single implementation of "reap without `waitpid(-1)`". The `wait`
/// builtin's poll loops each had their own copy of a `waitpid(-1)` +
/// sleep-50ms block; they all call this instead, so the no-wildcard rule holds
/// by construction rather than per-site vigilance.
pub fn reap_owned_once(shell: &mut crate::shell_state::Shell) -> bool {
    // Snapshot the reap set first: the loop below mutates the job table (and the
    // coproc list) as it reaps. Coproc pids are tracked on the Shell, not the job
    // table, so they are unioned in here.
    let mut targets: Vec<i32> = shell.jobs.owned_pids().to_vec();
    targets.extend(shell.coprocs.iter().filter(|c| !c.dead).map(|c| c.pid));
    targets.sort_unstable();
    targets.dedup();

    // bash's `waitchld` asks for stop/continue reports only under job control
    // (`WUNTRACED|WCONTINUED` when `job_control && subshell_environment == 0`);
    // without it a stopped background job is still `Running` to the shell, and
    // `jobs` says so.
    let stop_flags = if (shell.is_interactive || shell.shell_options.monitor) && !shell.in_subshell
    {
        libc::WUNTRACED | libc::WCONTINUED
    } else {
        0
    };
    let mut reaped_any = false;
    for pid in targets {
        let mut raw_status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(pid, &mut raw_status, libc::WNOHANG | stop_flags) };
        if r == 0 {
            // Still running, no state change to report.
            continue;
        }
        if r < 0 {
            // ECHILD: already reaped by someone else (e.g. the `wait` builtin's
            // targeted wait, or a synchronous executor waiter). It is no longer a
            // live child, so drop it from the reap set to keep that set bounded.
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                shell.jobs.release_owned_pid(pid);
            }
            continue;
        }
        reaped_any = true;
        shell.jobs.reap(r, raw_status);
        // If the reaped child is a live coproc that actually exited, mark it
        // dead; its fds and variables go at the next cleanup point (#185). A
        // WIFSTOPPED (WUNTRACED) report means the coproc is merely stopped, and
        // a WIFCONTINUED (WCONTINUED) report means it just resumed — in BOTH
        // cases it is still alive.
        if !libc::WIFSTOPPED(raw_status) && !libc::WIFCONTINUED(raw_status) {
            // Terminal: no longer a live child.
            shell.jobs.release_owned_pid(r);
            shell.mark_coproc_dead(r);
        }
    }
    reaped_any
}

/// What the shell should say about one job whose state just changed (#418,
/// #420). `SignalLine` is bash's non-interactive form for a signal death.
#[derive(Debug, PartialEq, Eq)]
pub enum Notice {
    /// `[1]+  Terminated              sleep 5`
    JobLine(String),
    /// `huck: line 3: 4179740 Killed                  sleep 5` — the prologue
    /// is added by the caller, which owns the program name and line number.
    SignalLine(String),
}

/// The shell facts `notify_of_job_status` reads.
#[derive(Debug, Clone, Copy)]
pub struct NoticeCtx {
    pub interactive: bool,
    /// `is_interactive || set -m` (and not inside a subshell or a completion
    /// function): the `[N]+` job line needs it; the pid-form signal line does not.
    pub job_control: bool,
    /// Whether the signal that killed this job has a trap installed.
    pub trapped: bool,
    /// bash's `startup_state == 0` (`Shell::reads_script_input`): a script
    /// file or piped stdin — not `-c`, not an embedder's string, not a
    /// terminal. Such a shell reports a background job only when
    /// a signal killed it; a normal exit or a stop is left UNREPORTED — and so
    /// unmarked — until `jobs` lists it or `wait` collects it.
    pub script_mode: bool,
}

/// What one pending job gets: a notice (or silence) and whether that counts
/// as having reported it. bash's `notify_of_job_status`, one job at a time.
#[derive(Debug, PartialEq, Eq)]
pub struct Verdict {
    pub notice: Option<Notice>,
    pub mark_notified: bool,
}

/// Decides what to announce for `job`. Pure: the whole per-signal matrix is
/// unit-testable without a Shell or a child process.
///
/// Ported from bash's `notify_of_job_status`:
/// - a script-mode shell skips every job that did not die from a signal;
/// - a dead job takes the pid form when ALL of: non-interactive, killed by a
///   signal outside bash's quiet set (INT/TERM/PIPE), and that signal is
///   untrapped — WITH OR WITHOUT job control (`sleep 5 & kill -KILL %1;
///   sleep 0.3` prints `Killed` in a plain `-c` shell);
/// - otherwise the `[N]+` line, only under job control;
/// - a stop is only ever observed under job control (bash's `waitchld` passes
///   `WUNTRACED` only then), so without it the stop is neither reported nor
///   marked;
/// - whatever was printed or deliberately kept silent is marked notified, so
///   the next cleanup point may prune it.
pub fn job_notice(job: &Job, flag: char, ctx: NoticeCtx) -> Verdict {
    let silent = Verdict {
        notice: None,
        mark_notified: false,
    };
    let signaled = matches!(job.state, JobState::Signaled(_));
    match job.state {
        JobState::Running => return silent,
        JobState::Stopped(_) if !ctx.job_control => return silent,
        _ => {}
    }
    if ctx.script_mode && !signaled {
        return silent;
    }
    if let JobState::Signaled(sig) = job.state {
        let quiet_signal = sig == libc::SIGINT || sig == libc::SIGTERM || sig == libc::SIGPIPE;
        if !ctx.interactive && !quiet_signal && !ctx.trapped {
            let (state, _) = job_state_and_suffix(job);
            let pid = job.pids.first().copied().unwrap_or(job.pgid);
            return Verdict {
                notice: Some(Notice::SignalLine(format!(
                    "{pid} {state:<24}{}",
                    job.command
                ))),
                mark_notified: true,
            };
        }
    }
    if !ctx.job_control {
        // Dead, no job control: nothing to say, but it HAS been considered —
        // bash marks it here so the cleanup pass can drop it.
        return Verdict {
            notice: None,
            mark_notified: true,
        };
    }
    // #418: bash precedes a STOP notice with a bare newline, and only that one.
    let lead = if matches!(job.state, JobState::Stopped(_)) {
        "\n"
    } else {
        ""
    };
    Verdict {
        notice: Some(Notice::JobLine(format!(
            "{lead}{}",
            notification_line(job, flag)
        ))),
        mark_notified: true,
    }
}

/// True when this shell may announce with the `[N]+` job line: job control is
/// on (interactive, or `set -m`) and we are neither a forked subshell nor
/// inside a completion function.
fn job_control_for_notices(shell: &crate::shell_state::Shell) -> bool {
    (shell.is_interactive || shell.shell_options.monitor)
        && !shell.in_subshell
        && !shell.in_completion
}

/// Reaps, then — at a CLEANUP POINT — announces and prunes. See
/// [`reap_and_notify_ex`].
pub fn reap_and_notify(shell: &mut crate::shell_state::Shell) {
    reap_and_notify_ex(shell, true)
}

/// `announce = false` only REAPS. bash reaps a child whenever SIGCHLD arrives
/// but reports and prunes at exactly four points — the end of a foreground
/// wait, the `jobs` builtin, each loop iteration, and `wait` — so a run of
/// builtins leaves a background death unreported AND still in the table:
/// `sleep 3 & kill -TERM %1; echo A; echo B` says nothing, and `trap … USR1;
/// … & wait; jobs` (the wait interrupted by the trap) still lists the job
/// that died meanwhile (#475). The between-command pass therefore passes
/// `announce` = "the group just blocked on a foreground child" (#418), and
/// with it false does nothing beyond the reap — under job control or not.
/// Draining silently here was what emptied `jobs` in a script and swallowed
/// the non-job-control `Killed` line.
pub fn reap_and_notify_ex(shell: &mut crate::shell_state::Shell, announce: bool) {
    reap_completed(shell);
    if announce {
        notify_and_cleanup(shell);
    }
}

/// bash's `notify_and_cleanup`: report what is pending, then prune.
pub fn notify_and_cleanup(shell: &mut crate::shell_state::Shell) {
    notify_of_job_status(shell);
    cleanup_dead_jobs(shell);
}

/// bash's `reap_dead_jobs`, run by every loop iteration (`REAP()`) in a
/// non-interactive or job-control-less shell: no reporting; mark only what
/// the CHILD_MAX bound requires, then prune what is already reported.
pub fn reap_dead_jobs(shell: &mut crate::shell_state::Shell) {
    if shell.is_interactive && shell.shell_options.monitor {
        return;
    }
    reap_completed(shell);
    shell
        .jobs
        .mark_dead_jobs_as_notified(false, shell.is_interactive, shell.last_bg_pid);
    cleanup_dead_jobs(shell);
}

/// bash's `cleanup_dead_jobs`: drop every dead job that has been reported,
/// and dispose of every coproc whose child has been reaped (`coproc_reap`
/// lives here in bash too — a dead coproc's fds and `NAME`/`NAME_PID` stay
/// usable until one of the cleanup points, #185).
pub fn cleanup_dead_jobs(shell: &mut crate::shell_state::Shell) {
    shell.jobs.remove_notified();
    shell.dispose_dead_coprocs();
}

/// bash's `notify_of_job_status`: one verdict per pending job, printed in id
/// order; each job the verdict considered reported is marked so.
pub fn notify_of_job_status(shell: &mut crate::shell_state::Shell) {
    let job_control = job_control_for_notices(shell);
    let (current, previous) = shell.jobs.current_and_previous();
    let ctx_base = NoticeCtx {
        interactive: shell.is_interactive,
        job_control,
        trapped: false,
        script_mode: shell.reads_script_input(),
    };
    for job in shell.jobs.pending_notifications() {
        let flag = if Some(job.id) == current {
            '+'
        } else if Some(job.id) == previous {
            '-'
        } else {
            ' '
        };
        let trapped = match job.state {
            // bash's `signal_is_trapped()`: a trap set to ignore ("") counts.
            JobState::Signaled(sig) => shell
                .traps
                .contains_key(&crate::traps::TrapSignal::Real(sig)),
            _ => false,
        };
        let verdict = job_notice(
            &job,
            flag,
            NoticeCtx {
                trapped,
                ..ctx_base
            },
        );
        match verdict.notice {
            Some(Notice::JobLine(line)) => with_err(|err| e!(err, "{line}")),
            Some(Notice::SignalLine(body)) => {
                crate::sh_error!(shell, None, "{}", body);
            }
            None => {}
        }
        if verdict.mark_notified {
            shell.jobs.mark_one_notified(job.id);
        }
    }
}

/// bash's wording for a signal, taken from the SAME source bash uses — the
/// system's signal-description list (#420). `strsignal(3)` yields `Hangup`,
/// `Killed`, `Terminated`, `Broken pipe`, `User defined signal 1` and the rest
/// verbatim, so there is no table to transcribe or keep in sync per platform.
fn signal_description(sig: i32) -> String {
    // SAFETY: strsignal returns a pointer to a static (or thread-local) string
    // for any int; it is never null on glibc/macOS for the values we pass.
    let p = unsafe { libc::strsignal(sig) };
    if p.is_null() {
        return format!("Signal {sig}");
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

pub fn render_state(state: &JobState) -> String {
    match state {
        JobState::Running => "Running".to_string(),
        // #420: bash prints a plain `Stopped` for EVERY stop signal — SIGSTOP,
        // SIGTSTP and SIGTTIN alike, verified non-interactively and under a
        // PTY. It does NOT use the system description here, which would say
        // `Stopped (signal)` / `Stopped (tty input)`.
        JobState::Stopped(_) => "Stopped".to_string(),
        JobState::Done(0) => "Done".to_string(),
        JobState::Done(n) => format!("Exit {n}"),
        JobState::Signaled(s) => signal_description(*s),
    }
}

fn job_state_and_suffix(job: &Job) -> (String, &'static str) {
    let state = render_state(&job.state);
    // #420: the trailing `&` marks a job that is running in the background —
    // bash puts it on Running lines only, not on Done / Exit n / Stopped / a
    // signal death.
    let suffix = if matches!(job.state, JobState::Running) {
        " &"
    } else {
        ""
    };
    let mut state = state;
    if job.core_dumped {
        state.push_str(" (core dumped)");
    }
    (state, suffix)
}

/// Renders one notification/listing line for a job. The trailing `&` is
/// included for Running and Done/Signaled jobs — Stopped jobs are not
/// "running in the background" so the suffix would be misleading.
///
/// #410: bash's layout is `[N]<flag>` + TWO spaces + the state in a 24-column
/// left-justified field + the command IMMEDIATELY after it (no separator).
/// huck used one space and a trailing one, which lands the command in the same
/// column but shifts every state string one place left.
pub fn notification_line(job: &Job, flag: char) -> String {
    let (state, suffix) = job_state_and_suffix(job);
    format!(
        "[{}]{}  {:<24}{}{}",
        job.id, flag, state, job.command, suffix
    )
}

/// Bash-faithful `jobs -l` output for a single job. Returns one
/// String per pipeline stage. First stage carries the `[N]<flag>`
/// prefix, state, command, and trailing `&`. Subsequent stages are
/// indented 5 spaces and carry only the PID.
pub fn notification_line_long(job: &Job, flag: char) -> Vec<String> {
    let (state, suffix) = job_state_and_suffix(job);
    let mut lines = Vec::with_capacity(job.pids.len().max(1));
    let first_pid = job.pids.first().copied().unwrap_or(job.pgid);
    // #410: one space after the flag here (the pid takes the second column),
    // then the same 24-wide state field butted against the command.
    lines.push(format!(
        "[{}]{} {} {:<24}{}{}",
        job.id, flag, first_pid, state, job.command, suffix
    ));
    for pid in job.pids.iter().skip(1) {
        lines.push(format!("     {}", pid));
    }
    lines
}

/// WCOREDUMP is not exposed by the `libc` crate on every target; the bit is
/// 0x80 in the low byte of the wait status on Linux and the BSDs alike.
fn core_dumped(raw: libc::c_int) -> bool {
    raw & 0x80 != 0
}

/// Decodes a raw waitpid status into a JobState terminal variant.
fn decode_status(raw: libc::c_int) -> JobState {
    if libc::WIFEXITED(raw) {
        JobState::Done(libc::WEXITSTATUS(raw))
    } else if libc::WIFSIGNALED(raw) {
        JobState::Signaled(libc::WTERMSIG(raw))
    } else if libc::WIFSTOPPED(raw) {
        JobState::Stopped(libc::WSTOPSIG(raw))
    } else {
        JobState::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_done_raw(exit: i32) -> libc::c_int {
        // WIFEXITED is true when the low 7 bits are 0; the high 8 bits
        // hold the exit code. Construct that directly.
        exit << 8
    }

    fn fake_signaled_raw(signum: i32) -> libc::c_int {
        // WIFSIGNALED is true when the low 7 bits are 1..0x7E. The signum
        // lives in those low 7 bits.
        signum
    }

    fn fake_stopped_raw(signum: i32) -> libc::c_int {
        // POSIX: WIFSTOPPED true when low byte == 0x7f; stop signal in second byte.
        (signum << 8) | 0x7f
    }

    #[test]
    fn add_allocates_id_one_first() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        assert_eq!(id, 1);
    }

    /// bash's `stop_pipeline` takes the slot after the LAST job: a freed lower
    /// number is not reused while a higher job lives (`%4`, not `%2`), and only
    /// an empty table restarts at 1.
    #[test]
    fn add_after_remove_takes_the_slot_after_the_last_job() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string()); // id 1
        let _ = t.add(101, vec![101], "b".to_string()); // id 2
        let _ = t.add(102, vec![102], "c".to_string()); // id 3
        // Reap b fully so it can be removed.
        t.reap(101, fake_done_raw(0));
        let _ = drain_all(&mut t);
        t.remove_notified();
        let new_id = t.add(200, vec![200], "d".to_string());
        assert_eq!(new_id, 4);
        // Everything gone: the numbering restarts.
        t.remove_ids(&[1, 3, 4]);
        assert_eq!(t.add(300, vec![300], "e".to_string()), 1);
    }

    /// `add` is a cleanup point (`stop_pipeline` calls `cleanup_dead_jobs`
    /// first): a dead, reported job is gone before the new one is numbered.
    #[test]
    fn add_prunes_reported_dead_jobs_first() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string()); // id 1
        t.reap(100, fake_done_raw(0));
        let _ = drain_all(&mut t);
        assert_eq!(t.add(200, vec![200], "b".to_string()), 1);
        assert_eq!(t.iter().count(), 1);
    }

    /// The `+`/`-` markers are bash's stored `j_current`/`j_previous`: a dead
    /// job keeps `+` until it is deleted, a stop takes `+`, and a deletion
    /// re-picks from the stopped-then-running candidates.
    #[test]
    fn current_and_previous_follow_bash() {
        let mut t = JobTable::new();
        let a = t.add(100, vec![100], "a".to_string());
        let b = t.add(101, vec![101], "b".to_string());
        assert_eq!(t.current_and_previous(), (Some(b), Some(a)));
        // b dies: still `+`.
        t.reap(101, fake_done_raw(0));
        assert_eq!(t.current_and_previous(), (Some(b), Some(a)));
        // a stops: it becomes `+`, b (dead) is no candidate for `-`.
        t.reap(100, fake_stopped_raw(libc::SIGTSTP));
        assert_eq!(t.current_and_previous(), (Some(a), None));
        // b is deleted: a stopped current stays.
        let _ = drain_all(&mut t);
        t.remove_notified();
        assert_eq!(t.current_and_previous(), (Some(a), None));
        // A new running job: the stopped job stays current, the new one is `-`.
        let c = t.add(102, vec![102], "c".to_string());
        assert_eq!(t.current_and_previous(), (Some(a), Some(c)));
        // a continues: newest running job is current again.
        t.reap(100, fake_continued_raw());
        assert_eq!(t.current_and_previous(), (Some(c), Some(a)));
    }

    #[test]
    fn reap_single_pid_transitions_to_done() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        t.reap(100, fake_done_raw(0));
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Done(0)));
    }

    #[test]
    fn reap_pipeline_uses_last_stage_status() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100, 101], "a | b".to_string());
        // Reap first stage with exit 1 — should NOT be the final status.
        t.reap(100, fake_done_raw(1));
        // Job not yet fully reaped.
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Running));
        // Reap last stage with exit 0 — final status comes from this.
        t.reap(101, fake_done_raw(0));
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Done(0)));
    }

    #[test]
    fn reap_signaled_transitions_to_signaled() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        t.reap(100, fake_signaled_raw(15));
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Signaled(15)));
    }

    #[test]
    fn reap_unknown_pid_is_silent_no_op() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "cmd".to_string());
        t.reap(999, fake_done_raw(0));
        let job = t.iter().next().unwrap();
        assert!(matches!(job.state, JobState::Running));
    }

    #[test]
    fn pending_notifications_returns_completed_unnotified() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        t.reap(100, fake_done_raw(0));
        let notifs = drain_all(&mut t);
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].id, id);
        // Second call should be empty (notified flag set).
        let notifs2 = drain_all(&mut t);
        assert!(notifs2.is_empty());
    }

    #[test]
    fn pending_notifications_skips_running() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "running".to_string());
        let notifs = drain_all(&mut t);
        assert!(notifs.is_empty());
    }

    #[test]
    fn remove_notified_drops_only_notified_completed() {
        let mut t = JobTable::new();
        let id_a = t.add(100, vec![100], "a".to_string()); // 1, running
        let id_b = t.add(101, vec![101], "b".to_string()); // 2, running
        t.reap(100, fake_done_raw(0));
        let _ = drain_all(&mut t); // marks id_a notified
        t.remove_notified();
        let remaining: Vec<u32> = t.iter().map(|j| j.id).collect();
        assert_eq!(remaining, vec![id_b]);
        let _ = id_a;
    }

    #[test]
    fn has_pending_tracks_state() {
        let mut t = JobTable::new();
        assert!(!t.has_pending());
        let _ = t.add(100, vec![100], "x".to_string());
        assert!(t.has_pending());
        t.reap(100, fake_done_raw(0));
        assert!(!t.has_pending());
    }

    #[test]
    fn current_id_returns_most_recent_running_or_stopped() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string()); // id 1
        let _ = t.add(200, vec![200], "b".to_string()); // id 2 — more recent
        assert_eq!(t.current_id(), Some(2));
    }

    #[test]
    fn current_id_includes_stopped_jobs() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        let _ = t.add(200, vec![200], "b".to_string());
        t.jobs_mut()[1].state = JobState::Stopped(libc::SIGTSTP);
        assert_eq!(t.current_id(), Some(2));
    }

    #[test]
    fn current_id_returns_none_when_only_done_jobs() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "a".to_string());
        t.jobs_mut()[0].state = JobState::Done(0);
        assert_eq!(t.current_id(), None);
        let _ = id;
    }

    #[test]
    fn current_stopped_id_skips_running_jobs() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string()); // Running, id 1
        let _ = t.add(200, vec![200], "b".to_string()); // Running, id 2 (more recent)
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
        // Most-recent is id 2 (Running); current_stopped should skip it and return id 1.
        assert_eq!(t.current_stopped_id(), Some(1));
    }

    #[test]
    fn has_pending_true_when_any_stopped() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
        assert!(t.has_pending());
    }

    #[test]
    fn has_pending_false_when_all_done() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        t.jobs_mut()[0].state = JobState::Done(0);
        assert!(!t.has_pending());
    }

    #[test]
    fn add_synthetic_done_immediate() {
        let mut t = JobTable::new();
        let id = t.add_synthetic_done("echo hi".to_string(), 0);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Done(0)));
        assert!(job.pids.is_empty());
    }

    #[test]
    fn current_and_previous_tracks_insertion_order() {
        let mut t = JobTable::new();
        let id_a = t.add(100, vec![100], "a".to_string()); // 1
        let id_b = t.add(101, vec![101], "b".to_string()); // 2
        let id_c = t.add(102, vec![102], "c".to_string()); // 3
        let (cur, prev) = t.current_and_previous();
        assert_eq!(cur, Some(id_c));
        assert_eq!(prev, Some(id_b));
        let _ = id_a;
    }

    #[test]
    fn render_state_stopped_sigtstp_is_plain_stopped() {
        assert_eq!(render_state(&JobState::Stopped(libc::SIGTSTP)), "Stopped");
    }

    /// Test stand-in for the old side-effecting drain: every pending job is
    /// marked notified, as a `-c` shell without job control would.
    fn drain_all(t: &mut JobTable) -> Vec<Job> {
        let pending = t.pending_notifications();
        for j in &pending {
            t.mark_one_notified(j.id);
        }
        pending
    }

    fn signaled_job(sig: i32) -> Job {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "sleep 5".to_string());
        t.jobs_mut()[0].state = JobState::Signaled(sig);
        t.jobs_mut().remove(0)
    }

    fn ctx(interactive: bool, job_control: bool, trapped: bool) -> NoticeCtx {
        NoticeCtx {
            interactive,
            job_control,
            trapped,
            script_mode: false,
        }
    }

    fn script_ctx() -> NoticeCtx {
        NoticeCtx {
            interactive: false,
            job_control: false,
            trapped: false,
            script_mode: true,
        }
    }

    /// Without job control the `[N]+` line is never printed, but the pid-form
    /// signal line IS — `sleep 5 & kill -KILL %1; sleep 0.3` prints `Killed`
    /// in a plain `-c` shell. A normal exit is silent and counts as reported.
    #[test]
    fn job_notice_without_job_control() {
        let job = signaled_job(libc::SIGKILL);
        let v = job_notice(&job, '+', ctx(false, false, false));
        assert!(matches!(v.notice, Some(Notice::SignalLine(_))));
        assert!(v.mark_notified);
        let mut t = JobTable::new();
        t.add(1, vec![1], "x".to_string());
        t.jobs_mut()[0].state = JobState::Done(0);
        let v = job_notice(&t.jobs_mut()[0], '+', ctx(true, false, false));
        assert_eq!(v.notice, None);
        assert!(v.mark_notified);
        // A stop is only observed under job control: neither said nor marked.
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
        let v = job_notice(&t.jobs_mut()[0], '+', ctx(false, false, false));
        assert_eq!(v.notice, None);
        assert!(!v.mark_notified);
    }

    /// A script-file shell reports a background job only when a signal killed
    /// it; a normal exit stays pending — visible to `jobs` — until listed.
    #[test]
    fn job_notice_script_mode_skips_normal_exits() {
        let mut t = JobTable::new();
        t.add(1, vec![1], "x".to_string());
        t.jobs_mut()[0].state = JobState::Done(0);
        let v = job_notice(&t.jobs_mut()[0], '+', script_ctx());
        assert_eq!(v.notice, None);
        assert!(!v.mark_notified);
        let job = signaled_job(libc::SIGKILL);
        let v = job_notice(&job, '+', script_ctx());
        assert!(matches!(v.notice, Some(Notice::SignalLine(_))));
        assert!(v.mark_notified);
        // TERM is in the quiet set: no line, but reported all the same.
        let job = signaled_job(libc::SIGTERM);
        let v = job_notice(&job, '+', script_ctx());
        assert_eq!(v.notice, None);
        assert!(v.mark_notified);
    }

    /// A still-running job is not news.
    #[test]
    fn job_notice_is_silent_for_a_running_job() {
        let mut t = JobTable::new();
        t.add(1, vec![1], "x".to_string());
        assert_eq!(
            job_notice(&t.jobs_mut()[0], '+', ctx(false, true, false)).notice,
            None
        );
    }

    /// #420: the pid form is for a NON-interactive shell, an untrapped signal,
    /// and a signal outside bash's quiet set.
    #[test]
    fn job_notice_form_matrix() {
        for sig in [libc::SIGKILL, libc::SIGHUP, libc::SIGUSR1, libc::SIGALRM] {
            let job = signaled_job(sig);
            assert!(
                matches!(
                    job_notice(&job, '+', ctx(false, true, false)).notice,
                    Some(Notice::SignalLine(_))
                ),
                "signal {sig} non-interactive untrapped should take the pid form"
            );
            // Interactive, or trapped, flips it back to the job line.
            assert!(matches!(
                job_notice(&job, '+', ctx(true, true, false)).notice,
                Some(Notice::JobLine(_))
            ));
            assert!(matches!(
                job_notice(&job, '+', ctx(false, true, true)).notice,
                Some(Notice::JobLine(_))
            ));
        }
        // bash's quiet set keeps the job-line form even non-interactively.
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGPIPE] {
            let job = signaled_job(sig);
            assert!(
                matches!(
                    job_notice(&job, '+', ctx(false, true, false)).notice,
                    Some(Notice::JobLine(_))
                ),
                "signal {sig} should stay in the job-line form"
            );
        }
    }

    /// The pid form is `<pid> <state padded to 24><command>`, with the
    /// `prog: line N:` prologue left to the caller.
    ///
    /// #456: the wording comes from the system signal list, which differs off
    /// the compat target (macOS says `Killed: 9`), so the expectation is built
    /// from it and only Linux pins the literal.
    #[test]
    fn job_notice_signal_line_layout() {
        let job = signaled_job(libc::SIGKILL);
        let killed = render_state(&JobState::Signaled(libc::SIGKILL));
        let expected = format!("4242 {killed:<24}sleep 5");
        #[cfg(target_os = "linux")]
        assert_eq!(expected, "4242 Killed                  sleep 5");
        match job_notice(&job, '+', ctx(false, true, false)).notice {
            Some(Notice::SignalLine(body)) => {
                assert_eq!(body, expected);
            }
            other => panic!("expected a SignalLine, got {other:?}"),
        }
    }

    /// #418: a stop notice carries a leading blank line; nothing else does.
    #[test]
    fn job_notice_stop_line_leads_with_a_newline() {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "sleep 5".to_string());
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
        match job_notice(&t.jobs_mut()[0], '+', ctx(false, true, false)).notice {
            Some(Notice::JobLine(line)) => {
                assert_eq!(line, "\n[1]+  Stopped                 sleep 5")
            }
            other => panic!("expected a JobLine, got {other:?}"),
        }
        t.jobs_mut()[0].state = JobState::Done(0);
        t.jobs_mut()[0].notified = false;
        match job_notice(&t.jobs_mut()[0], '+', ctx(false, true, false)).notice {
            Some(Notice::JobLine(line)) => {
                assert_eq!(line, "[1]+  Done                    sleep 5")
            }
            other => panic!("expected a JobLine, got {other:?}"),
        }
    }

    /// #420: a core-dumping death carries bash's suffix. Probed out of the
    /// harness because apport makes those signals nondeterministic here.
    ///
    /// #456: the suffix goes INSIDE the 24-column state field, after the
    /// system's wording for the signal — which is `Quit` on the compat target
    /// and `Quit: 3` on macOS, so only Linux pins the literal.
    #[test]
    fn core_dumped_job_carries_the_suffix() {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "crash".to_string());
        t.jobs_mut()[0].state = JobState::Signaled(libc::SIGQUIT);
        t.jobs_mut()[0].core_dumped = true;
        let quit = render_state(&JobState::Signaled(libc::SIGQUIT));
        let field = format!("{quit} (core dumped)");
        let expected = format!("[1]+  {field:<24}crash");
        #[cfg(target_os = "linux")]
        assert_eq!(expected, "[1]+  Quit (core dumped)      crash");
        assert_eq!(notification_line(&t.jobs_mut()[0], '+'), expected);
    }

    /// #420: bash prints a plain `Stopped` for EVERY stop signal — probed for
    /// SIGSTOP, SIGTSTP and SIGTTIN, non-interactively and under a PTY. These
    /// used to assert glibc's `strsignal` wording (`Stopped (tty input)`,
    /// `Stopped (tty output)`, `Stopped (signal N)`), which bash does not use
    /// on this path.
    #[test]
    fn render_state_stopped_is_plain_for_every_stop_signal() {
        for sig in [
            libc::SIGSTOP,
            libc::SIGTSTP,
            libc::SIGTTIN,
            libc::SIGTTOU,
            99,
        ] {
            assert_eq!(
                render_state(&JobState::Stopped(sig)),
                "Stopped",
                "stop signal {sig} should render plain"
            );
        }
    }

    /// #420: a terminated job takes its wording from the system signal list,
    /// which is where bash's table comes from too.
    ///
    /// #456: that list is the platform's, so the exact strings hold only on
    /// the compat target (ubuntu-24.04 / bash 5.2.21). macOS's `strsignal`
    /// appends the number — `Killed: 9`, `Terminated: 15` — and real bash on
    /// macOS prints that too, so everywhere else asserts the prefix.
    #[test]
    fn render_state_signaled_uses_the_system_description() {
        for (sig, word) in [
            (libc::SIGTERM, "Terminated"),
            (libc::SIGKILL, "Killed"),
            (libc::SIGHUP, "Hangup"),
            (libc::SIGINT, "Interrupt"),
            (libc::SIGPIPE, "Broken pipe"),
        ] {
            let got = render_state(&JobState::Signaled(sig));
            #[cfg(target_os = "linux")]
            assert_eq!(got, word, "signal {sig}");
            #[cfg(not(target_os = "linux"))]
            assert!(got.starts_with(word), "signal {sig}: {got:?}");
        }
    }

    #[test]
    fn notification_line_for_stopped_omits_ampersand() {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "sleep 100".to_string());
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
        let line = notification_line(&t.jobs_mut()[0], '+');
        assert_eq!(line, "[1]+  Stopped                 sleep 100");
    }

    #[test]
    fn notification_line_for_done_omits_ampersand() {
        let mut t = JobTable::new();
        t.add_synthetic_done("echo hi".to_string(), 0);
        let line = notification_line(&t.jobs_mut()[0], ' ');
        // #420: no trailing `&` — bash marks only a RUNNING job that way.
        assert_eq!(line, "[1]   Done                    echo hi");
    }

    #[test]
    fn notification_line_for_nonzero_exit_shows_exit_n() {
        let mut t = JobTable::new();
        t.add_synthetic_done("test -z hi".to_string(), 1);
        let line = notification_line(&t.jobs_mut()[0], ' ');
        assert_eq!(line, "[1]   Exit 1                  test -z hi");
    }

    #[test]
    fn notification_line_for_stopped_tty_input_is_plain() {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "cat".to_string());
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTTIN);
        let line = notification_line(&t.jobs_mut()[0], '+');
        assert_eq!(line, "[1]+  Stopped                 cat");
    }

    #[test]
    fn reap_with_stopped_status_transitions_job_to_stopped_state() {
        let mut t = JobTable::new();
        let _ = t.add(4242, vec![4242], "sleep 100".to_string());
        // POSIX: WIFSTOPPED true when low byte == 0x7f; stop signal in second byte.
        let raw_status: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        t.reap(4242, raw_status);
        let j = &t.jobs_mut()[0];
        assert!(
            matches!(j.state, JobState::Stopped(s) if s == libc::SIGTSTP),
            "got state {:?}",
            j.state
        );
        assert!(!j.reaped[0], "stopped is not reaped — child still exists");
        assert!(
            !j.notified,
            "stopped jobs must be visible to the next notification pass"
        );
    }

    // A raw waitpid status for which WIFCONTINUED is true. The encoding is
    // libc-specific (#297):
    //   - Linux/glibc: the __W_CONTINUED sentinel, `status == 0xffff`.
    //   - BSD/macOS:   `_WSTATUS(x) == _WSTOPPED && WSTOPSIG(x) == SIGCONT`,
    //     i.e. the stopped encoding carrying SIGCONT. (0xffff decodes there as
    //     WIFSTOPPED with stop signal 255, which is why the glibc sentinel
    //     landed these jobs in Stopped(255) instead of Running.)
    // `reap` itself uses libc::WIFCONTINUED, so only this fixture is
    // platform-sensitive; the assert keeps it honest on a new target rather
    // than letting a wrong constant quietly retarget the test.
    fn fake_continued_raw() -> libc::c_int {
        #[cfg(target_os = "linux")]
        let raw: libc::c_int = 0xffff;
        #[cfg(not(target_os = "linux"))]
        let raw: libc::c_int = (libc::SIGCONT << 8) | 0x7f;
        assert!(
            libc::WIFCONTINUED(raw),
            "fixture {raw:#x} is not WIFCONTINUED on this platform"
        );
        raw
    }

    #[test]
    fn reap_continued_transitions_stopped_job_to_running() {
        let mut t = JobTable::new();
        let _ = t.add(4242, vec![4242], "sleep 100".to_string());
        // First stop it.
        let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        t.reap(4242, stopped);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(_)));
        // A continued report flips it back to Running (not reaped/Done).
        t.reap(4242, fake_continued_raw());
        let j = &t.jobs_mut()[0];
        assert!(
            matches!(j.state, JobState::Running),
            "continued job must be Running, got {:?}",
            j.state
        );
        assert!(!j.reaped[0], "a continue is not a terminal reap");
        assert!(
            !j.notified,
            "a resumed job must be visible to the next pass"
        );
    }

    #[test]
    fn reap_continued_on_running_job_is_noop() {
        let mut t = JobTable::new();
        let _ = t.add(4242, vec![4242], "sleep 100".to_string());
        t.reap(4242, fake_continued_raw());
        assert!(matches!(t.jobs_mut()[0].state, JobState::Running));
        assert!(!t.jobs_mut()[0].reaped[0]);
    }

    #[test]
    fn reap_with_exit_after_stop_finally_transitions_to_done() {
        let mut t = JobTable::new();
        let _ = t.add(4242, vec![4242], "sleep 100".to_string());
        let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        let exited: libc::c_int = 0;
        t.reap(4242, stopped);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(_)));
        t.reap(4242, exited);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Done(0)));
    }

    #[test]
    fn pipeline_reap_stop_then_exit_in_order_finalizes_with_last_stage_status() {
        // Pipeline `a | b`: SIGTSTP both, then a exits 0 then b exits 7.
        // Final state must be Done(7) — last stage wins.
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100, 200], "a | b".to_string());
        let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        t.reap(100, stopped);
        t.reap(200, stopped);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(_)));
        assert_eq!(t.jobs_mut()[0].reaped, vec![false, false]);

        let exit_a: libc::c_int = 0; // WEXITSTATUS = 0
        let exit_b: libc::c_int = 7 << 8; // WEXITSTATUS = 7
        t.reap(100, exit_a);
        assert!(
            matches!(t.jobs_mut()[0].state, JobState::Stopped(_)),
            "still stopped while b is alive"
        );
        t.reap(200, exit_b);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Done(7)));
    }

    #[test]
    fn pipeline_reap_stop_then_exit_reverse_order_still_uses_last_stage_status() {
        // Same as above but b exits BEFORE a.
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100, 200], "a | b".to_string());
        let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        t.reap(100, stopped);
        t.reap(200, stopped);

        let exit_b: libc::c_int = 7 << 8;
        let exit_a: libc::c_int = 0;
        t.reap(200, exit_b);
        assert!(
            matches!(t.jobs_mut()[0].state, JobState::Stopped(_)),
            "still stopped while a is alive"
        );
        t.reap(100, exit_a);
        assert!(
            matches!(t.jobs_mut()[0].state, JobState::Done(7)),
            "last stage status (b=7) must win, not a=0"
        );
    }

    #[test]
    fn resolve_id_returns_matching_id() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        let _ = t.add(200, vec![200], "b".to_string());
        let spec = crate::job_spec::JobSpec::Id(2);
        assert_eq!(t.resolve(&spec), Ok(2));
    }

    #[test]
    fn resolve_id_missing_returns_not_found() {
        let t = JobTable::new();
        let spec = crate::job_spec::JobSpec::Id(99);
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::NotFound));
    }

    #[test]
    fn resolve_current_uses_current_id() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        let _ = t.add(200, vec![200], "b".to_string());
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Current), Ok(2));
    }

    #[test]
    fn resolve_previous_returns_second_most_recent() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        let _ = t.add(200, vec![200], "b".to_string());
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), Ok(1));
    }

    #[test]
    fn resolve_previous_returns_not_found_when_only_one_job() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        assert_eq!(
            t.resolve(&crate::job_spec::JobSpec::Previous),
            Err(JobSpecResolveError::NotFound)
        );
    }

    #[test]
    fn resolve_prefix_unique_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "sleep 30".to_string());
        let spec = crate::job_spec::JobSpec::Prefix("sleep".to_string());
        assert_eq!(t.resolve(&spec), Ok(1));
    }

    #[test]
    fn resolve_prefix_no_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "sleep 30".to_string());
        let spec = crate::job_spec::JobSpec::Prefix("xyz".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::NotFound));
    }

    #[test]
    fn resolve_prefix_ambiguous() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "sleep 30".to_string());
        t.add(1235, vec![1235], "sleep 60".to_string());
        let spec = crate::job_spec::JobSpec::Prefix("sleep".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::Ambiguous));
    }

    #[test]
    fn resolve_substring_unique_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "find . -name foo".to_string());
        let spec = crate::job_spec::JobSpec::Substring("name".to_string());
        assert_eq!(t.resolve(&spec), Ok(1));
    }

    #[test]
    fn resolve_substring_no_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "find . -name foo".to_string());
        let spec = crate::job_spec::JobSpec::Substring("xyz".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::NotFound));
    }

    #[test]
    fn resolve_substring_ambiguous() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "find . -name foo".to_string());
        t.add(1235, vec![1235], "grep foo bar".to_string());
        let spec = crate::job_spec::JobSpec::Substring("foo".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::Ambiguous));
    }

    #[test]
    fn reap_repeated_stopped_status_same_signal_is_idempotent_for_notification() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100, 200], "a | b".to_string());
        let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        // First stop: synchronous waiter would have set notified=true after this.
        t.reap(100, stopped);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(s) if s == libc::SIGTSTP));
        t.jobs_mut()[0].notified = true; // simulate the synchronous waiter's bookkeeping
        // Second stop for the same job (other pid, same signal).
        t.reap(200, stopped);
        assert!(
            t.jobs_mut()[0].notified,
            "must NOT reset notified for the second stop"
        );
    }
}
