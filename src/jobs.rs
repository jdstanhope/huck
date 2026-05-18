//! Job table for tracking background pipelines.
//!
//! A `Job` represents one background pipeline. Its `pids` are the PIDs of
//! the pipeline stages in order; its `pgid` is the process group ID
//! (always equal to the first stage's PID). `reap` updates per-pid state
//! when a child is reaped; when all pids are reaped, the job's overall
//! state transitions to `Done` or `Signaled` based on the LAST stage's
//! status (matching bash's pipeline exit-status rule without `pipefail`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped(i32),
    Done(i32),
    Signaled(i32),
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
}

#[derive(Debug, Clone, Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    next_created_at: u64,
}

impl JobTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a new Running job. Allocates the lowest unused job id
    /// (bash-style reuse). Returns the allocated id.
    pub fn add(&mut self, pgid: i32, pids: Vec<i32>, command: String) -> u32 {
        let id = self.next_id();
        let n = pids.len();
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
        };
        self.next_created_at += 1;
        self.jobs.push(job);
        self.jobs.sort_by_key(|j| j.id);
        id
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
        };
        self.next_created_at += 1;
        self.jobs.push(job);
        self.jobs.sort_by_key(|j| j.id);
        id
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
                    let already_in_this_state = matches!(job.state, JobState::Stopped(s) if s == new_sig);
                    if !already_in_this_state {
                        job.state = JobState::Stopped(new_sig);
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

    /// Drops all jobs that are non-Running AND notified.
    pub fn remove_notified(&mut self) {
        self.jobs.retain(|j| {
            matches!(j.state, JobState::Running | JobState::Stopped(_)) || !j.notified
        });
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
    pub fn resolve(&self, spec: &crate::job_spec::JobSpec) -> Option<u32> {
        match spec {
            crate::job_spec::JobSpec::Id(id) => {
                self.jobs.iter().find(|j| j.id == *id).map(|j| j.id)
            }
            crate::job_spec::JobSpec::Current => self.current_id(),
            crate::job_spec::JobSpec::Previous => {
                let (_, prev) = self.current_and_previous();
                prev
            }
        }
    }

    pub fn jobs_mut(&mut self) -> &mut Vec<Job> {
        &mut self.jobs
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
}

/// Drains all reapable children via non-blocking `waitpid(WNOHANG)`, feeding
/// each into the shell's job table. Also resets the SIGCHLD flag.
pub fn reap_completed(shell: &mut crate::shell_state::Shell) {
    shell
        .sigchld_flag
        .store(false, std::sync::atomic::Ordering::Relaxed);
    loop {
        let mut raw_status: libc::c_int = 0;
        let pid = unsafe {
            libc::waitpid(-1, &mut raw_status, libc::WNOHANG | libc::WUNTRACED)
        };
        if pid <= 0 {
            // 0 = no children changed state; -1 = no children at all (ECHILD)
            break;
        }
        shell.jobs.reap(pid as i32, raw_status);
    }
}

/// Reaps and then prints `[N]<flag> <state> <cmd> &` for any newly-completed
/// jobs. Drops the printed jobs from the table.
pub fn reap_and_notify(shell: &mut crate::shell_state::Shell) {
    reap_completed(shell);
    let (current, previous) = shell.jobs.current_and_previous();
    let notifs = shell.jobs.drain_notifications();
    for job in notifs {
        let flag = if Some(job.id) == current {
            '+'
        } else if Some(job.id) == previous {
            '-'
        } else {
            ' '
        };
        eprintln!("{}", notification_line(&job, flag));
    }
    shell.jobs.remove_notified();
}

pub fn render_state(state: &JobState) -> String {
    match state {
        JobState::Running => "Running".to_string(),
        JobState::Stopped(s) => match *s {
            libc::SIGTSTP => "Stopped".to_string(),
            libc::SIGTTIN => "Stopped (tty input)".to_string(),
            libc::SIGTTOU => "Stopped (tty output)".to_string(),
            n => format!("Stopped (signal {n})"),
        },
        JobState::Done(0) => "Done".to_string(),
        JobState::Done(n) => format!("Exit {n}"),
        JobState::Signaled(s) => format!("Killed (signal {s})"),
    }
}

/// Renders one notification/listing line for a job. The trailing `&` is
/// included for Running and Done/Signaled jobs — Stopped jobs are not
/// "running in the background" so the suffix would be misleading. Column
/// width is 24 to fit `Stopped (tty output)`.
pub fn notification_line(job: &Job, flag: char) -> String {
    let state = render_state(&job.state);
    let suffix = match job.state {
        JobState::Stopped(_) => "",
        _ => " &",
    };
    format!("[{}]{} {:<24} {}{}", job.id, flag, state, job.command, suffix)
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
        let _ = t.add(100, vec![100], "a".to_string());      // id 1
        let _ = t.add(200, vec![200], "b".to_string());      // id 2 — more recent
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
        let _ = t.add(100, vec![100], "a".to_string());      // Running, id 1
        let _ = t.add(200, vec![200], "b".to_string());      // Running, id 2 (more recent)
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

    #[test]
    fn render_state_stopped_sigttin_includes_tty_input() {
        assert_eq!(
            render_state(&JobState::Stopped(libc::SIGTTIN)),
            "Stopped (tty input)"
        );
    }

    #[test]
    fn render_state_stopped_sigttou_includes_tty_output() {
        assert_eq!(
            render_state(&JobState::Stopped(libc::SIGTTOU)),
            "Stopped (tty output)"
        );
    }

    #[test]
    fn render_state_stopped_unknown_signal_falls_back_to_numeric() {
        assert_eq!(
            render_state(&JobState::Stopped(99)),
            "Stopped (signal 99)"
        );
    }

    #[test]
    fn notification_line_for_stopped_omits_ampersand() {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "sleep 100".to_string());
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
        let line = notification_line(&t.jobs_mut()[0], '+');
        assert_eq!(line, "[1]+ Stopped                  sleep 100");
    }

    #[test]
    fn notification_line_for_done_includes_ampersand() {
        let mut t = JobTable::new();
        t.add_synthetic_done("echo hi".to_string(), 0);
        let line = notification_line(&t.jobs_mut()[0], ' ');
        assert_eq!(line, "[1]  Done                     echo hi &");
    }

    #[test]
    fn notification_line_for_stopped_tty_input_shows_reason() {
        let mut t = JobTable::new();
        t.add(4242, vec![4242], "cat".to_string());
        t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTTIN);
        let line = notification_line(&t.jobs_mut()[0], '+');
        assert_eq!(line, "[1]+ Stopped (tty input)      cat");
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
            "got state {:?}", j.state
        );
        assert!(!j.reaped[0], "stopped is not reaped — child still exists");
        assert!(!j.notified, "stopped jobs must be visible to the next notification pass");
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
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(_)),
            "still stopped while b is alive");
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
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(_)),
            "still stopped while a is alive");
        t.reap(100, exit_a);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Done(7)),
            "last stage status (b=7) must win, not a=0");
    }

    #[test]
    fn resolve_id_returns_matching_id() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        let _ = t.add(200, vec![200], "b".to_string());
        let spec = crate::job_spec::JobSpec::Id(2);
        assert_eq!(t.resolve(&spec), Some(2));
    }

    #[test]
    fn resolve_id_missing_returns_none() {
        let t = JobTable::new();
        let spec = crate::job_spec::JobSpec::Id(99);
        assert_eq!(t.resolve(&spec), None);
    }

    #[test]
    fn resolve_current_uses_current_id() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        let _ = t.add(200, vec![200], "b".to_string());
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Current), Some(2));
    }

    #[test]
    fn resolve_previous_returns_second_most_recent() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        let _ = t.add(200, vec![200], "b".to_string());
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), Some(1));
    }

    #[test]
    fn resolve_previous_returns_none_when_only_one_job() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string());
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), None);
    }

    #[test]
    fn reap_repeated_stopped_status_same_signal_is_idempotent_for_notification() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100, 200], "a | b".to_string());
        let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        // First stop: synchronous waiter would have set notified=true after this.
        t.reap(100, stopped);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(s) if s == libc::SIGTSTP));
        t.jobs_mut()[0].notified = true;  // simulate the synchronous waiter's bookkeeping
        // Second stop for the same job (other pid, same signal).
        t.reap(200, stopped);
        assert!(t.jobs_mut()[0].notified, "must NOT reset notified for the second stop");
    }
}
