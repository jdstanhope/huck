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

#[derive(Debug, Clone, Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    next_created_at: u64,
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
    /// interactive job control). Allocates the lowest unused job id. Returns it.
    pub fn add(&mut self, pgid: i32, pids: Vec<i32>, command: String) -> u32 {
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

    /// Returns all non-Running, not-yet-notified jobs (in id order),
    /// marking them notified as a side effect.
    pub fn drain_notifications(&mut self) -> Vec<Job> {
        let mut out = Vec::new();
        for job in self.jobs.iter_mut() {
            let pending = !matches!(job.state, JobState::Running);
            if pending && !job.notified {
                job.notified = true;
                out.push(job.clone());
            }
        }
        out.sort_by_key(|j| j.id);
        out
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
        self.jobs
            .retain(|j| matches!(j.state, JobState::Running | JobState::Stopped(_)) || !j.notified);
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

    /// Removes the job with id `id`, first recording its terminal status in the
    /// saved-status ring (so a later `wait $pid` still resolves). Used by the
    /// `wait %n` path, which prunes the waited job immediately (matching bash).
    pub fn remove_job_recording_status(&mut self, id: u32) {
        if let Some(job) = self.jobs.iter().find(|j| j.id == id) {
            let job = job.clone();
            self.record_pruned_job(&job);
        }
        self.jobs.retain(|j| j.id != id);
    }

    /// Returns the most-recent and previous job ids (for `+`/`-` markers).
    /// Unlike [`current_id`], this includes Done/Signaled jobs that are
    /// still in the table awaiting notification, so the `+`/`-` flags on
    /// `jobs` output match what the user just saw.
    /// Most-recent is the highest `created_at`; previous is the next.
    pub fn current_and_previous(&self) -> (Option<u32>, Option<u32>) {
        let mut by_age: Vec<&Job> = self.jobs.iter().collect();
        by_age.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        let current = by_age.first().map(|j| j.id);
        let previous = by_age.get(1).map(|j| j.id);
        (current, previous)
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
            JobSpec::Current => self.current_id().ok_or(JobSpecResolveError::NotFound),
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

    fn next_id(&self) -> u32 {
        let mut id = 1u32;
        loop {
            if !self.jobs.iter().any(|j| j.id == id) {
                return id;
            }
            id += 1;
        }
    }

    fn insert_job(&mut self, job: Job) -> u32 {
        let id = job.id;
        self.next_created_at += 1;
        self.jobs.push(job);
        self.jobs.sort_by_key(|j| j.id);
        id
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
    targets.extend(shell.coprocs.iter().map(|c| c.pid as i32));
    targets.sort_unstable();
    targets.dedup();

    let mut reaped_any = false;
    for pid in targets {
        let mut raw_status: libc::c_int = 0;
        let r = unsafe {
            libc::waitpid(
                pid,
                &mut raw_status,
                libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED,
            )
        };
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
        // If the reaped child is a live coproc that actually exited, close its
        // fds + unset NAME/NAME_PID. A WIFSTOPPED (WUNTRACED) report means the
        // coproc is merely stopped, and a WIFCONTINUED (WCONTINUED) report means
        // it just resumed — in BOTH cases it is still alive, so do NOT reap it
        // (reap_coproc tears the coproc down unconditionally by pid).
        if !libc::WIFSTOPPED(raw_status) && !libc::WIFCONTINUED(raw_status) {
            // Terminal: no longer a live child.
            shell.jobs.release_owned_pid(r);
            shell.reap_coproc(r);
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

/// The three shell facts the decision depends on.
#[derive(Debug, Clone, Copy)]
pub struct NoticeCtx {
    pub interactive: bool,
    /// `is_interactive || set -m`: bash announces nothing without it.
    pub job_control: bool,
    /// Whether the signal that killed this job has a trap installed.
    pub trapped: bool,
}

/// Decides what to announce for `job`, or `None` for silence. Pure: the whole
/// per-signal matrix is unit-testable without a Shell or a child process.
///
/// bash uses the pid-prefixed form when ALL of: the shell is non-interactive,
/// the job died from a signal, that signal is not SIGINT/SIGTERM/SIGPIPE, and
/// the signal is untrapped. Everything else takes the `[N]+` form.
pub fn job_notice(job: &Job, flag: char, ctx: NoticeCtx) -> Option<Notice> {
    if !ctx.job_control {
        return None;
    }
    if matches!(job.state, JobState::Running) {
        return None;
    }
    if let JobState::Signaled(sig) = job.state {
        let quiet_signal = sig == libc::SIGINT || sig == libc::SIGTERM || sig == libc::SIGPIPE;
        if !ctx.interactive && !quiet_signal && !ctx.trapped {
            let (state, _) = job_state_and_suffix(job);
            let pid = job.pids.first().copied().unwrap_or(job.pgid);
            return Some(Notice::SignalLine(format!(
                "{pid} {state:<24}{}",
                job.command
            )));
        }
    }
    // #418: bash precedes a STOP notice with a bare newline, and only that one.
    let lead = if matches!(job.state, JobState::Stopped(_)) {
        "\n"
    } else {
        ""
    };
    Some(Notice::JobLine(format!(
        "{lead}{}",
        notification_line(job, flag)
    )))
}

/// Reaps, announces any newly-changed job, and drops the terminal ones.
///
/// #418/#420: the announcement is gated on JOB CONTROL, not on interactivity —
/// bash reports a background job's state change in a non-interactive `set -m`
/// shell too. Notices stay suppressed inside a subshell and inside completion
/// functions, matching bash's suppression in a subshell environment.
pub fn reap_and_notify(shell: &mut crate::shell_state::Shell) {
    reap_and_notify_ex(shell, true)
}

/// `announce = false` reaps and prunes SILENTLY. The between-command pass
/// passes `false` when the group that just ran never blocked on a child (#418):
/// bash only reaps while blocked, so a run of builtins leaves a background
/// death unnoticed — `sleep 3 & kill -TERM %1; echo A; echo B` says nothing in
/// bash, and huck must not invent a notice there.
pub fn reap_and_notify_ex(shell: &mut crate::shell_state::Shell, announce: bool) {
    reap_completed(shell);
    let job_control = (shell.is_interactive || shell.shell_options.monitor)
        && !shell.in_subshell
        && !shell.in_completion;
    // With job control on but no blocking point since the last pass, bash has
    // not noticed this death yet — so leave the job PENDING rather than
    // draining it. Draining here would mark it notified and silence it
    // forever, which is what made the notice depend on a race. Without job
    // control there is no notice to lose, and the silent drain+prune of #175
    // still applies.
    if job_control && !announce {
        return;
    }
    let (current, previous) = shell.jobs.current_and_previous();
    let notifs = shell.jobs.drain_notifications();
    let ctx_base = NoticeCtx {
        interactive: shell.is_interactive,
        job_control,
        trapped: false,
    };
    for job in notifs {
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
        match job_notice(
            &job,
            flag,
            NoticeCtx {
                trapped,
                ..ctx_base
            },
        ) {
            Some(Notice::JobLine(line)) => with_err(|err| e!(err, "{line}")),
            Some(Notice::SignalLine(body)) => {
                crate::sh_error!(shell, None, "{}", body);
            }
            None => {}
        }
    }
    shell.jobs.remove_notified();
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

    #[test]
    fn add_allocates_id_one_first() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        assert_eq!(id, 1);
    }

    #[test]
    fn add_after_remove_reuses_lowest_id() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string()); // id 1
        let _ = t.add(101, vec![101], "b".to_string()); // id 2
        let _ = t.add(102, vec![102], "c".to_string()); // id 3
        // Reap b fully so it can be removed.
        t.reap(101, fake_done_raw(0));
        let _ = t.drain_notifications();
        t.remove_notified();
        // Next add should reuse id 2.
        let new_id = t.add(200, vec![200], "d".to_string());
        assert_eq!(new_id, 2);
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
    fn drain_notifications_returns_completed_unnotified() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        t.reap(100, fake_done_raw(0));
        let notifs = t.drain_notifications();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].id, id);
        // Second call should be empty (notified flag set).
        let notifs2 = t.drain_notifications();
        assert!(notifs2.is_empty());
    }

    #[test]
    fn drain_notifications_skips_running() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "running".to_string());
        let notifs = t.drain_notifications();
        assert!(notifs.is_empty());
    }

    #[test]
    fn remove_notified_drops_only_notified_completed() {
        let mut t = JobTable::new();
        let id_a = t.add(100, vec![100], "a".to_string()); // 1, running
        let id_b = t.add(101, vec![101], "b".to_string()); // 2, running
        t.reap(100, fake_done_raw(0));
        let _ = t.drain_notifications(); // marks id_a notified
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
        }
    }

    /// #418/#420: nothing is announced without job control, whatever happened.
    #[test]
    fn job_notice_is_silent_without_job_control() {
        let job = signaled_job(libc::SIGKILL);
        assert_eq!(job_notice(&job, '+', ctx(false, false, false)), None);
        let mut t = JobTable::new();
        t.add(1, vec![1], "x".to_string());
        t.jobs_mut()[0].state = JobState::Done(0);
        assert_eq!(
            job_notice(&t.jobs_mut()[0], '+', ctx(true, false, false)),
            None
        );
    }

    /// A still-running job is not news.
    #[test]
    fn job_notice_is_silent_for_a_running_job() {
        let mut t = JobTable::new();
        t.add(1, vec![1], "x".to_string());
        assert_eq!(
            job_notice(&t.jobs_mut()[0], '+', ctx(false, true, false)),
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
                    job_notice(&job, '+', ctx(false, true, false)),
                    Some(Notice::SignalLine(_))
                ),
                "signal {sig} non-interactive untrapped should take the pid form"
            );
            // Interactive, or trapped, flips it back to the job line.
            assert!(matches!(
                job_notice(&job, '+', ctx(true, true, false)),
                Some(Notice::JobLine(_))
            ));
            assert!(matches!(
                job_notice(&job, '+', ctx(false, true, true)),
                Some(Notice::JobLine(_))
            ));
        }
        // bash's quiet set keeps the job-line form even non-interactively.
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGPIPE] {
            let job = signaled_job(sig);
            assert!(
                matches!(
                    job_notice(&job, '+', ctx(false, true, false)),
                    Some(Notice::JobLine(_))
                ),
                "signal {sig} should stay in the job-line form"
            );
        }
    }

    /// The pid form is `<pid> <state padded to 24><command>`, with the
    /// `prog: line N:` prologue left to the caller.
    #[test]
    fn job_notice_signal_line_layout() {
        let job = signaled_job(libc::SIGKILL);
        match job_notice(&job, '+', ctx(false, true, false)) {
            Some(Notice::SignalLine(body)) => {
                assert_eq!(body, "4242 Killed                  sleep 5");
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
        match job_notice(&t.jobs_mut()[0], '+', ctx(false, true, false)) {
            Some(Notice::JobLine(line)) => {
                assert_eq!(line, "\n[1]+  Stopped                 sleep 5")
            }
            other => panic!("expected a JobLine, got {other:?}"),
        }
        t.jobs_mut()[0].state = JobState::Done(0);
        t.jobs_mut()[0].notified = false;
        match job_notice(&t.jobs_mut()[0], '+', ctx(false, true, false)) {
            Some(Notice::JobLine(line)) => {
                assert_eq!(line, "[1]+  Done                    sleep 5")
            }
            other => panic!("expected a JobLine, got {other:?}"),
        }
    }

    /// #420: a core-dumping death carries bash's suffix. Probed out of the
    /// harness because apport makes those signals nondeterministic here.
    #[test]
    fn core_dumped_job_carries_the_suffix() {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "crash".to_string());
        t.jobs_mut()[0].state = JobState::Signaled(libc::SIGQUIT);
        t.jobs_mut()[0].core_dumped = true;
        assert_eq!(
            notification_line(&t.jobs_mut()[0], '+'),
            "[1]+  Quit (core dumped)      crash"
        );
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
    #[test]
    fn render_state_signaled_uses_the_system_description() {
        assert_eq!(
            render_state(&JobState::Signaled(libc::SIGTERM)),
            "Terminated"
        );
        assert_eq!(render_state(&JobState::Signaled(libc::SIGKILL)), "Killed");
        assert_eq!(render_state(&JobState::Signaled(libc::SIGHUP)), "Hangup");
        assert_eq!(render_state(&JobState::Signaled(libc::SIGINT)), "Interrupt");
        assert_eq!(
            render_state(&JobState::Signaled(libc::SIGPIPE)),
            "Broken pipe"
        );
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
