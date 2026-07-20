//! `Engine` — the embedding entry point for `huck-engine`.
//!
//! Owns a persistent shell session; run/capture script strings, run files, and
//! get/set variables and positional parameters. Shells signal failure via exit
//! codes, so these methods return exit codes (no `Result`): a parse error is
//! exit 2 (+ a message on stderr), a missing file is 127.
//!
//! ```
//! use huck_engine::Engine;
//! let mut e = Engine::new();
//! e.set_var("NAME", "world");
//! assert_eq!(e.run("echo \"hi $NAME\""), 0);          // prints: hi world
//! let out = e.capture("echo $((6 * 7)); echo done >&2");
//! assert_eq!(out.stdout, "42\n");
//! assert_eq!(out.stderr, "done\n");
//! assert_eq!(out.exit_code, 0);
//! assert_eq!(e.var("NAME").as_deref(), Some("world"));
//!
//! // For stdin + stderr capture:
//! let out = e.prepare("read x; printf 'got=%s\\n' \"$x\"")
//!     .stdin(b"hello\n".to_vec())
//!     .capture();
//! assert_eq!(out.stdout, "got=hello\n");
//!
//! // Sandboxed run: tmpdir cwd, restricted mode, 5-second budget.
//! # let sandbox_dir = std::env::temp_dir();
//! # let generated_script = "echo hi";
//! let out = e.prepare(generated_script)
//!     .cwd(sandbox_dir)
//!     .restricted()
//!     .timeout(std::time::Duration::from_secs(5))
//!     .capture();
//!
//! // Stream output as the script runs.
//! let mut lines: Vec<String> = Vec::new();
//! let exit = e.prepare("for i in 1 2 3; do echo $i; done")
//!     .on_stdout_line(|line| lines.push(line.to_string()))
//!     .run();
//! assert_eq!(exit, 0);
//! assert_eq!(lines, vec!["1", "2", "3"]);
//!
//! // Tab-completion query: what would complete at the cursor?
//! let line = "echo $HO";
//! let comp = e.complete(line, line.len());
//! for c in &comp.candidates {
//!     println!("[{:?}] {}", c.kind, c.display);
//! }
//! // Prints lines like:  [Variable] HOME, [Variable] HOSTNAME (if set), etc.
//! ```
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::completion::Candidate;
use crate::executor::{StderrSink, StdoutSink};
use crate::shell_state::Shell;

/// The captured result of [`Engine::capture`] (or [`ExecBuilder::capture`]).
///
/// [`ExecBuilder::capture`]: crate::exec_builder::ExecBuilder::capture
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Output {
    /// Everything the script wrote to stdout. Under `merge_stderr` this also
    /// contains the script's stderr bytes, interleaved in execution order.
    pub stdout: String,
    /// Everything the script wrote to stderr. Empty when none was written, or
    /// when `merge_stderr` routed it into `stdout`.
    pub stderr: String,
    /// The script's exit status.
    pub exit_code: i32,
}

/// The result of a completion query — see [`Engine::complete`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Completion {
    /// Byte offset in the input line where the replacement starts.
    /// Embedders substitute `line[start..cursor]` with each candidate's
    /// `replacement` when the user picks it. `start <= cursor`.
    pub start: usize,
    /// Candidates in the order huck would offer them at the prompt.
    /// Alphabetical within each kind; `complete -F` results respect `-o nosort`.
    pub candidates: Vec<Candidate>,
}

/// A persistent, embeddable huck shell session.
pub struct Engine {
    cell: Rc<RefCell<Shell>>,
}

impl Engine {
    /// A fresh session (`$0` = "huck"). Installs no signal handlers, reads no rc file.
    pub fn new() -> Self {
        Engine {
            cell: Rc::new(RefCell::new(Shell::new())),
        }
    }

    /// Start building a configured engine.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Wrap a caller-owned (possibly pre-configured) shell cell. The caller keeps
    /// ownership of any process-global concerns (e.g. signal handlers).
    #[doc(hidden)]
    pub fn from_shell_cell(cell: Rc<RefCell<Shell>>) -> Self {
        Engine { cell }
    }

    /// Run a script string with `bash -c` semantics (no "main" call frame).
    /// stdout + stderr inherit the process. Returns the exit status.
    pub fn run(&mut self, src: &str) -> i32 {
        let mut sink = StdoutSink::Terminal;
        self.run_with(src, false, &mut sink)
    }

    /// Run a script string, capturing stdout and stderr into separate buffers.
    /// `bash -c` semantics; returns `{ stdout, stderr, exit_code }`.
    pub fn capture(&mut self, src: &str) -> Output {
        self.prepare(src).capture()
    }

    /// Start an advanced execution chain. Borrows `&mut self` for the chain's
    /// lifetime. See [`ExecBuilder`].
    ///
    /// [`ExecBuilder`]: crate::exec_builder::ExecBuilder
    pub fn prepare(&mut self, src: &str) -> crate::exec_builder::ExecBuilder<'_> {
        crate::exec_builder::ExecBuilder::new(self, src.to_string())
    }

    /// Return the completion candidates at `cursor` (byte offset) in `line`.
    /// The embedder substitutes `line[start..cursor]` with each candidate's
    /// `replacement` when the user picks it.
    ///
    /// `cursor` is clamped to `line.len()`. Passing a cursor inside a
    /// multi-byte UTF-8 sequence panics (same as `&str` slicing).
    ///
    /// `&mut self` is required because `complete -F func` callbacks may
    /// mutate shell state.
    pub fn complete(&mut self, line: &str, cursor: usize) -> Completion {
        let clamped = cursor.min(line.len());
        let mut shell = self.cell.borrow_mut();
        let (start, candidates) = crate::completion::dispatch::resolve(line, clamped, &mut shell);
        Completion { start, candidates }
    }

    /// Run a script STRING with script semantics (a "main" frame; `$0` = `arg0`).
    pub fn run_script(&mut self, src: &str, arg0: &str) -> i32 {
        self.cell.borrow_mut().shell_argv0 = arg0.to_string();
        let mut sink = StdoutSink::Terminal;
        self.run_with_label(src, arg0, true, &mut sink)
    }

    /// Read and run a script FILE with script semantics (`$0` = the path).
    /// A read failure emits `<path>: <err>` (via the unified error emitter)
    /// and returns 127.
    pub fn run_file(&mut self, path: &Path) -> i32 {
        match std::fs::read_to_string(path) {
            Ok(contents) => self.run_script(&contents, &path.display().to_string()),
            Err(e) => {
                // Short-lived borrow: dropped before returning, so it can
                // never overlap a later `cell.borrow_mut()` (the `Shell` is
                // never re-entered on this path).
                crate::sh_error!(&*self.cell.borrow(), None, "{}: {}", path.display(), e);
                127
            }
        }
    }

    /// Read a shell variable.
    pub fn var(&self, name: &str) -> Option<String> {
        self.cell.borrow().lookup_var(name)
    }

    /// Set a (global) shell variable.
    pub fn set_var(&mut self, name: &str, value: &str) {
        self.cell.borrow_mut().set(name, value.to_string());
    }

    /// Set the positional parameters `$1`..`$N`.
    pub fn set_args(&mut self, args: Vec<String>) {
        self.cell.borrow_mut().positional_args = args;
    }

    /// Set `$0` (the program/script name).
    pub fn set_arg0(&mut self, name: &str) {
        self.cell.borrow_mut().shell_argv0 = name.to_string();
    }

    /// Mark the shell as invoked `huck -c '<command>'`. Drives the `-c:`
    /// prologue segment on syntax/parser diagnostics
    /// (`Diag::Syntax` — see `error_emit.rs`); bash attributes a `-c`-mode
    /// syntax error to `<name>: -c: line N:` but omits the segment for
    /// script-file/stdin mode.
    pub fn set_is_command_string(&mut self, v: bool) {
        self.cell.borrow_mut().is_command_string = v;
    }

    /// `$?` after the last run.
    pub fn last_status(&self) -> i32 {
        self.cell.borrow().last_status()
    }

    /// Access the underlying shell cell (advanced/dogfood use).
    #[doc(hidden)]
    pub fn shell_cell(&self) -> &Rc<RefCell<Shell>> {
        &self.cell
    }

    fn run_with(&mut self, src: &str, push_main_frame: bool, sink: &mut StdoutSink) -> i32 {
        let label = self.cell.borrow().shell_argv0.clone();
        self.run_with_label(src, &label, push_main_frame, sink)
    }

    fn run_with_label(
        &mut self,
        src: &str,
        label: &str,
        push_main_frame: bool,
        sink: &mut StdoutSink,
    ) -> i32 {
        // Preserve the shell's current $0 + positionals (don't clobber them).
        let args = self.cell.borrow().positional_args.clone();
        // stderr always inherits the process here; the public `Engine::prepare`
        // builder will let callers opt into Capture/Merged later.
        let mut err_sink = StderrSink::Terminal;
        let code = crate::shell::run_program_in_sinks(
            src,
            None,
            args,
            label,
            push_main_frame,
            sink,
            &mut err_sink,
            &self.cell,
        );
        // Mirror the run's exit code into `$?` so `last_status()` reflects it even
        // when the script short-circuited via `exit N` (which doesn't otherwise
        // update the shell's stored status). Matches bash's `bash -c '…'; echo $?`.
        self.cell.borrow_mut().set_last_status(code);
        code
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// Builder for a configured [`Engine`].
#[derive(Default)]
pub struct EngineBuilder {
    arg0: Option<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
    version: Option<String>,
}

impl EngineBuilder {
    /// Seed a shell variable.
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }
    /// Set `$0`.
    pub fn arg0(mut self, name: &str) -> Self {
        self.arg0 = Some(name.to_string());
        self
    }
    /// Set the positional parameters.
    pub fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }
    /// Override `$HUCK_VERSION` with the embedder's release-version
    /// string. When unset, the engine's default (its own crate version)
    /// is used.
    pub fn version(mut self, version: &str) -> Self {
        self.version = Some(version.to_string());
        self
    }
    /// Build the engine.
    pub fn build(self) -> Engine {
        let mut e = Engine::new();
        if let Some(a0) = self.arg0 {
            e.set_arg0(&a0);
        }
        e.set_args(self.args);
        for (k, v) in self.env {
            e.set_var(&k, &v);
        }
        if let Some(v) = self.version {
            e.set_var("HUCK_VERSION", &v);
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_exit_codes() {
        let mut e = Engine::new();
        assert_eq!(e.run("true"), 0);
        assert_eq!(e.run("false"), 1);
        assert_eq!(e.run("exit 3"), 3);
    }

    #[test]
    fn run_multiline_script_and_function() {
        let mut e = Engine::new();
        let code = e.run("greet() { echo \"hi $1\"; }\ngreet there\n");
        assert_eq!(code, 0);
    }

    #[test]
    fn state_persists_across_runs() {
        let mut e = Engine::new();
        assert_eq!(e.run("x=5"), 0);
        let out = e.capture("echo $((x * 2))");
        assert_eq!(out.stdout, "10\n");
    }

    #[test]
    fn heredoc_in_comsub_eof_adjacency_expands() {
        // Heredoc STARTED inside a command substitution whose close delimiter is
        // adjacent to the heredoc close-delimiter text. bash prefix-matches the
        // heredoc delimiter, so the body terminates and the `$()`/`` ` `` closes.
        // `$()`/`` `…` `` strip the trailing newline, so the body `hi\n` → `hi`.
        let mut e = Engine::new();
        // comsub-eof0: `EOF )` (delimiter, space, `)`).
        assert_eq!(
            e.capture("foo=$(cat <<EOF\nhi\nEOF )\necho $foo").stdout,
            "hi\n"
        );
        // comsub-eof1: heredoc inside a BACKTICK (the former panic case).
        assert_eq!(
            e.capture("foo=`cat <<EOF\nhi\nEOF`\necho $foo").stdout,
            "hi\n"
        );
        // comsub-eof4: `EOF)` (no space before the `)`).
        assert_eq!(
            e.capture("e=$(cat <<EOF\ncontents\nEOF)\necho $e").stdout,
            "contents\n"
        );
    }

    #[test]
    fn heredoc_delim_line_continuation_in_comsub() {
        // comsub4.sub block 2: a heredoc delimiter word that spans a `\<newline>`
        // line continuation — `<<\EOT\` + newline + `4` forms delimiter `EOT4`
        // (quoted → literal body, trailing backslash kept verbatim). Inside `$()`.
        let mut e = Engine::new();
        let out = e.capture("x=$( cat <<\\EOT\\\n4\nd \\\ng\nEOT4\n)\necho \"$x\"");
        assert_eq!(out.stdout, "d \\\ng\n");
    }

    #[test]
    fn heredoc_in_comsub_body_after_close() {
        // heredoc7.sub: a heredoc opened INSIDE `$( … )` whose `)` closes on the
        // opener line — `)` terminates the (unquoted) delimiter word, and the body
        // is taken from the lines following the ENCLOSING command line (delayed
        // heredoc across the comsub boundary). bash: `echo $(cat <<EOF)…` → body.
        let mut e = Engine::new();
        let out = e.capture("echo $(cat <<EOF)\nfoo\nbar\nEOF\n");
        assert_eq!(out.stdout, "foo bar\n");
        let out2 = e.capture("x=$(cat <<EOF)\none\ntwo\nEOF\necho \"[$x]\"");
        assert_eq!(out2.stdout, "[one\ntwo]\n");
    }

    #[test]
    fn capture_collects_stdout_and_code() {
        let mut e = Engine::new();
        let out = e.capture("echo hi; echo bye; exit 4");
        assert_eq!(out.stdout, "hi\nbye\n");
        assert_eq!(out.exit_code, 4);
    }

    #[test]
    fn parse_error_is_exit_2() {
        let mut e = Engine::new();
        // unterminated `if` — bash exits 2 on a syntax error.
        assert_eq!(e.run("if ["), 2);
    }

    #[test]
    fn var_get_set_and_args() {
        let mut e = Engine::new();
        e.set_var("NAME", "world");
        assert_eq!(e.var("NAME").as_deref(), Some("world"));
        e.set_args(vec!["a".to_string(), "b".to_string()]);
        let out = e.capture("echo \"$1-$2-$#\"");
        assert_eq!(out.stdout, "a-b-2\n");
    }

    #[test]
    fn set_arg0_visible_as_dollar_zero() {
        let mut e = Engine::new();
        e.set_arg0("myprog");
        let out = e.capture("echo $0");
        assert_eq!(out.stdout, "myprog\n");
    }

    #[test]
    fn last_status_reflects_last_run() {
        let mut e = Engine::new();
        e.run("exit 7");
        assert_eq!(e.last_status(), 7);
    }

    #[test]
    fn run_file_runs_and_missing_is_127() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "echo from-file").unwrap();
        let mut e = Engine::new();
        assert_eq!(e.run_file(f.path()), 0);
        assert_eq!(e.run_file(Path::new("/no/such/huck/script.sh")), 127);
    }

    #[test]
    fn builder_configures_engine() {
        let mut e = Engine::builder()
            .arg0("prog")
            .args(vec!["x".to_string()])
            .env("GREETING", "yo")
            .build();
        let out = e.capture("echo \"$GREETING $0 $1\"");
        assert_eq!(out.stdout, "yo prog x\n");
    }

    #[test]
    fn exec_capture_stdout_and_stderr_separately() {
        let mut e = Engine::new();
        let out = e.prepare("echo hi; echo err >&2").capture();
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "err\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn exec_merge_stderr_interleaves_into_stdout() {
        let mut e = Engine::new();
        let out = e
            .prepare("echo hi; echo err >&2; echo bye")
            .merge_stderr()
            .capture();
        assert_eq!(out.stdout, "hi\nerr\nbye\n");
        assert_eq!(out.stderr, "");
    }

    #[test]
    fn exec_feeds_stdin() {
        // Gate on STDIN_LOCK: this test swaps the process-global fd 0 via
        // `with_stdin_fd0`, racing with stdin_pipe's own tests if not serialized.
        let _guard = crate::test_support::STDIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        let out = e
            .prepare("read x; read y; echo \"$x-$y\"")
            .stdin(b"hello\nworld\n".to_vec())
            .capture();
        assert_eq!(out.stdout, "hello-world\n");
    }

    #[test]
    fn exec_large_stdin_uses_writer_thread() {
        // Feeds 5 KiB - above the 4 KiB inline threshold; ensures the writer-thread
        // path completes the read.
        // Gate on STDIN_LOCK: this test swaps the process-global fd 0 via
        // `with_stdin_fd0`, racing with stdin_pipe's own tests if not serialized.
        let _guard = crate::test_support::STDIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let big: Vec<u8> = std::iter::repeat_n(b'a', 5000)
            .chain(std::iter::once(b'\n'))
            .collect();
        let mut e = Engine::new();
        let out = e
            .prepare("read line; echo \"len=${#line}\"")
            .stdin(big)
            .capture();
        assert_eq!(out.stdout, "len=5000\n");
    }

    #[test]
    fn exec_stdin_and_merge_stderr_compose() {
        let _guard = crate::test_support::STDIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        // Read stdin, echo it, then write a separate stderr message.
        // With merge_stderr, the stderr should fold into the captured stdout.
        let out = e
            .prepare("read x; echo \"got:$x\"; echo err >&2; echo done")
            .stdin(b"hello\n".to_vec())
            .merge_stderr()
            .capture();
        assert_eq!(out.stdout, "got:hello\nerr\ndone\n");
        assert_eq!(out.stderr, "");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn capture_includes_stderr_field() {
        let mut e = Engine::new();
        let out = e.capture("echo a; echo b >&2");
        assert_eq!(out.stdout, "a\n");
        assert_eq!(out.stderr, "b\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn parse_error_diagnostic_in_stderr() {
        let mut e = Engine::new();
        let out = e.capture("if [");
        assert_eq!(out.exit_code, 2);
        assert!(out.stderr.contains("syntax error"), "got: {:?}", out.stderr);
    }

    #[test]
    fn exec_run_inherits_then_exec_capture_works() {
        // Borrow discipline: back-to-back exec chains compile and work.
        let mut e = Engine::new();
        e.prepare("x=set-in-first").run();
        let out = e.prepare("echo \"$x\"").capture();
        assert_eq!(out.stdout, "set-in-first\n");
    }

    // in-memory routing must defer to the real-fd dup chain when an earlier
    // file/pipe redirect targets the source fd.

    // NOTE: `capture_with_file_then_dup_to_{one,two}_lets_file_win` moved to
    // `tests/streaming_fd_serial.rs` — they redirect a process-global fd to a
    // file and read it back, which races libtest's own output under the parallel
    // harness (reproducible on macOS). See that file and #90.

    #[test]
    fn capture_bare_dup_to_one_routes_to_stdout_sink() {
        // The fixup must not regress Task 7's bare-builtin in-memory routing.
        //
        // route_out_to_err: a builtin's `>&2` under stderr capture lands in
        // the separate stderr buffer (not the embedder's terminal).
        let mut e = Engine::new();
        let out = e.prepare("echo out; echo err >&2").capture();
        assert_eq!(out.stdout, "out\n");
        assert_eq!(out.stderr, "err\n");

        // route_err_to_out: a builtin's `2>&1` under stdout capture folds
        // stderr writes into the stdout buffer. Use a builtin whose primary
        // output goes to stderr — `declare -p UNSET_NAME` writes the "not
        // found" diagnostic to fd 2.
        let mut e = Engine::new();
        let out = e.prepare("declare -p NOPE_NOT_DEFINED 2>&1").capture();
        assert_eq!(out.stderr, "");
        assert!(
            out.stdout.contains("NOPE_NOT_DEFINED"),
            "got stdout=[{:?}]",
            out.stdout
        );
    }

    #[test]
    fn cmdsub_bare_builtin_2to1_capture_is_nonempty() {
        // v269 T3b regression: a builtin error emitted under `$(... 2>&1)`
        // must reach the CALLER's writer (the executor's in-memory
        // route_err_to_out swap for the bare-builtin redirect), not the
        // thread-local sink — sh_error_to!, not sh_error!. Verified bug
        // (pre-fix): `x=$(cd /nonexistent 2>&1); echo "$x"` printed an empty
        // string instead of capturing `cd`'s diagnostic.
        let mut e = Engine::new();
        let out = e
            .prepare(r#"x=$(cd /nonexistent_xyz_engine_test 2>&1); echo "$x""#)
            .capture();
        assert_eq!(out.stderr, "");
        assert!(
            !out.stdout.trim().is_empty(),
            "expected the cd error to be captured, got empty stdout"
        );
        assert!(
            out.stdout.contains("No such file or directory"),
            "expected the captured cd diagnostic body, got stdout=[{:?}]",
            out.stdout
        );
    }

    // ============== CWD ==============

    #[test]
    fn exec_cwd_runs_script_in_path() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(tmp.path()).unwrap();
        let mut e = Engine::new();
        let out = e.prepare("pwd").cwd(tmp.path()).capture();
        assert_eq!(
            out.stdout.trim(),
            canonical.display().to_string(),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn exec_cwd_restores_engine_pwd() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let mut e = Engine::new();
        e.set_var("PWD", "before");
        let _ = e
            .prepare("cd /; echo \"in:$PWD\"")
            .cwd(tmp.path())
            .capture();
        assert_eq!(e.var("PWD").as_deref(), Some("before"));
    }

    #[test]
    fn exec_cwd_chdir_failure_is_best_effort() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        // .capture() routes the script's stderr into out.stderr, but the
        // chdir diagnostic in `with_cwd` goes to the PROCESS stderr (real fd
        // 2) via `eprintln!` — by design, since the cwd guard runs OUTSIDE
        // the executor's sink installation. We assert exit code and stdout
        // here; the diagnostic itself lands on the test runner's stderr.
        let out = e.prepare("echo hi").cwd("/no/such/huck/v206").capture();
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.exit_code, 0);
    }

    // ============== RESTRICTED ==============

    #[test]
    fn restricted_off_by_default() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        let out = e.prepare("cd /tmp; echo $PWD").capture();
        assert_eq!(out.exit_code, 0, "stderr={:?}", out.stderr);
        let canonical_tmp = std::fs::canonicalize("/tmp")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/tmp".to_string());
        let got = out.stdout.trim();
        assert!(
            got == "/tmp" || got == canonical_tmp,
            "expected /tmp or canonical, got {got:?}"
        );
    }

    #[test]
    fn restricted_refuses_cd() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        let out = e.prepare("cd /tmp; echo \"$PWD\"").restricted().capture();
        assert!(
            out.stderr.contains("cd: restricted"),
            "stderr={:?}",
            out.stderr
        );
        // The script keeps running after the refused cd; echo still fires.
        assert!(out.stdout.ends_with("\n"));
    }

    #[test]
    fn restricted_refuses_exec() {
        let mut e = Engine::new();
        let out = e.prepare("exec /bin/true").restricted().capture();
        assert!(
            out.stderr.contains("exec: restricted"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn restricted_refuses_command_name_with_slash() {
        let mut e = Engine::new();
        let out = e.prepare("/bin/echo hi").restricted().capture();
        assert!(
            out.stderr
                .contains("/bin/echo: restricted: cannot specify `/' in command names"),
            "stderr={:?}",
            out.stderr
        );
        assert_eq!(out.stdout, "");
    }

    #[test]
    fn restricted_accepts_command_name_without_slash() {
        let mut e = Engine::new();
        let out = e.prepare("true").restricted().capture();
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn restricted_refuses_source_with_slash() {
        let mut e = Engine::new();
        let out = e.prepare(". /etc/profile").restricted().capture();
        assert!(
            out.stderr.contains(".: /etc/profile: restricted"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn restricted_refuses_absolute_redirect() {
        let mut e = Engine::new();
        let out = e
            .prepare("echo hi > /tmp/v206-restricted-test")
            .restricted()
            .capture();
        assert!(
            out.stderr
                .contains("/tmp/v206-restricted-test: restricted: cannot redirect output"),
            "stderr={:?}",
            out.stderr
        );
        // The file MUST NOT have been written.
        let target = std::path::Path::new("/tmp/v206-restricted-test");
        assert!(
            !target.exists() || std::fs::read(target).map(|b| b.is_empty()).unwrap_or(true),
            "the refused redirect wrote a file"
        );
        let _ = std::fs::remove_file(target);
    }

    #[test]
    fn restricted_refuses_parent_dir_redirect() {
        let mut e = Engine::new();
        let out = e.prepare("echo hi > ../escape").restricted().capture();
        assert!(
            out.stderr
                .contains("../escape: restricted: cannot redirect output"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn restricted_refuses_path_assignment() {
        let mut e = Engine::new();
        let out = e.prepare("PATH=/tmp; echo done").restricted().capture();
        assert!(
            out.stderr.contains("PATH: readonly variable"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn restricted_refuses_shell_assignment() {
        let mut e = Engine::new();
        let out = e
            .prepare("SHELL=/bin/bash; echo done")
            .restricted()
            .capture();
        assert!(
            out.stderr.contains("SHELL: readonly variable"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn restricted_marks_all_five_vars_readonly() {
        let mut e = Engine::new();
        // Every write path must report through readonly machinery, not a
        // restriction-specific message. HISTFILE is included (bash does).
        for name in ["SHELL", "PATH", "HISTFILE", "ENV", "BASH_ENV"] {
            let out = e
                .prepare(&format!("{name}=/tmp; echo done"))
                .restricted()
                .capture();
            assert!(
                out.stderr.contains(&format!("{name}: readonly variable")),
                "{name}: expected readonly diagnostic, got {:?}",
                out.stderr
            );
        }
    }

    #[test]
    fn restricted_covers_non_assignment_write_paths() {
        let mut e = Engine::new();
        // These four paths escape the old check_special_assign sites entirely.
        //
        // NOTE on assertion strength: these are SUBSTRING checks, so what each
        // case pins differs. The two whose expected string carries a builtin
        // prefix (`declare:`, `unset:`) pin the exact WORDING. The two whose
        // expected string is the bare `PATH: readonly variable` pin only that
        // the write was REFUSED — a prefixed message such as
        // `export: PATH: readonly variable` would also contain that substring.
        // Exact-wording coverage for those two lives in the bash-diff harness.
        let cases = [
            ("export PATH=/tmp", "PATH: readonly variable"),
            ("PATH+=/tmp", "PATH: readonly variable"),
            ("declare PATH=/tmp", "declare: PATH: readonly variable"),
            ("unset PATH", "unset: PATH: cannot unset: readonly variable"),
        ];
        for (src, want) in cases {
            let out = e.prepare(src).restricted().capture();
            assert!(
                out.stderr.contains(want),
                "{src}: expected {want:?}, got {:?}",
                out.stderr
            );
        }
    }

    #[test]
    fn declare_nameref_bind_refused_on_readonly_var() {
        // A nameref bind writes through the `Shell::set` leaf, which does not
        // enforce readonly. Unguarded, `declare -n PATH=EVIL` gives arbitrary
        // control of PATH inside a restricted shell (command hijacking), so
        // BOTH assertions matter: that it is refused AND that nothing landed.
        let mut e = Engine::new();
        let out = e
            .prepare("declare -n PATH=EVIL; EVIL=/attacker; echo \"PATH=$PATH\"")
            .restricted()
            .capture();
        assert!(
            out.stderr.contains("declare: PATH: readonly variable"),
            "stderr={:?}",
            out.stderr
        );
        assert!(
            !out.stdout.contains("PATH=/attacker"),
            "nameref bind LANDED despite the refusal: stdout={:?}",
            out.stdout
        );

        // Same guard, plain readonly (no restricted policy) — the check is on
        // `is_readonly`, not on the policy, so it covers both for free.
        let mut e = Engine::new();
        let out = e
            .prepare("readonly FOO=1; declare -n FOO=BAR; BAR=hijacked; echo \"FOO=$FOO\"")
            .capture();
        assert!(
            out.stderr.contains("declare: FOO: readonly variable"),
            "stderr={:?}",
            out.stderr
        );
        assert!(
            out.stdout.contains("FOO=1"),
            "readonly value was clobbered: stdout={:?}",
            out.stdout
        );
    }

    #[test]
    fn declare_nameref_valueless_refused_on_readonly_var() {
        // The VALUE-LESS form `declare -n NAME` is the same escape as the
        // valued bind: bash treats NAME's existing value as the target name,
        // so once `-n` is on, `NAME=x` resolves through the nameref and the
        // readonly gate (which checks the RESOLVED name) never fires.
        // Every case asserts BOTH the diagnostic AND that no write landed —
        // a stderr-only assertion is exactly how the first round of this bug
        // survived review.
        let mut e = Engine::new();
        let out = e
            .prepare("declare -n PATH; PATH=/attacker/bin")
            .restricted()
            .capture();
        // PATH's value is a real path, so bash's invalid-name check fires
        // first (verified on bash 5.2.21); either refusal is a refusal, but
        // the write must not land.
        assert!(
            out.stderr
                .contains("invalid variable name for name reference")
                || out.stderr.contains("declare: PATH: readonly variable"),
            "value-less `declare -n PATH` was not refused: stderr={:?}",
            out.stderr
        );
        // Read back through the Engine, not a trailing `echo` — the refused
        // assignment is fatal to the rest of the non-interactive script, so a
        // later in-script command would never run and a stdout assertion
        // would pass vacuously.
        assert_ne!(
            e.var("PATH").as_deref(),
            Some("/attacker/bin"),
            "value-less nameref bind LANDED: PATH was hijacked"
        );

        // Plain readonly, no restricted policy, with a value that IS a valid
        // identifier — here the readonly check is the ONLY thing standing
        // between the attacker and the write.
        let mut e = Engine::new();
        let out = e
            .prepare("readonly RO=safe; declare -n RO; RO=/attacker")
            .capture();
        assert!(
            out.stderr.contains("declare: RO: readonly variable"),
            "stderr={:?}",
            out.stderr
        );
        // Read the value back through the Engine rather than a trailing
        // `echo`: the refused `RO=/attacker` assignment is itself a readonly
        // error, which is fatal to the rest of the non-interactive script, so
        // no in-script command after it would run to observe the value.
        assert_eq!(
            e.var("RO").as_deref(),
            Some("safe"),
            "readonly value was clobbered through the nameref"
        );
    }

    #[test]
    fn declare_nameref_valueless_invalid_name_message() {
        // bash validates the EXISTING value as a reference target, and this
        // check fires regardless of readonly:
        //   $ bash -c 'X=/some/path; declare -n X'
        //   bash: declare: `/some/path': invalid variable name for name reference
        let mut e = Engine::new();
        let out = e.prepare("X=/some/path; declare -n X").capture();
        assert!(
            out.stderr
                .contains("declare: `/some/path': invalid variable name for name reference"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn declare_nameref_valueless_on_unset_name_succeeds() {
        // Do NOT over-refuse: bash applies `-n` to an unset name at rc 0.
        //   $ bash -c 'declare -n NEWV; declare -p NEWV'  =>  declare -n NEWV
        let mut e = Engine::new();
        let out = e.prepare("declare -n NEWV; echo \"rc=$?\"").capture();
        assert!(
            out.stderr.is_empty(),
            "unexpected diagnostic: stderr={:?}",
            out.stderr
        );
        assert!(out.stdout.contains("rc=0"), "stdout={:?}", out.stdout);

        // And the attribute really is applied — the nameref still works.
        let mut e = Engine::new();
        let out = e
            .prepare("declare -n REF; REF=tgt; REF=hello; echo \"tgt=$tgt\"")
            .capture();
        assert!(
            out.stdout.contains("tgt=hello"),
            "nameref attribute was not applied: stdout={:?} stderr={:?}",
            out.stdout,
            out.stderr
        );
    }

    #[test]
    fn restricted_refuses_set_plus_r() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        let out = e.prepare("set +r; cd /tmp").restricted().capture();
        assert!(
            out.stderr.contains("restricted: cannot turn off")
                || out.stderr.contains("restricted:"),
            "stderr={:?}",
            out.stderr
        );
        // cd should STILL be refused after the refused `set +r`.
        assert!(
            out.stderr.contains("cd: restricted"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn restricted_propagates_to_function() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        let out = e.prepare("f() { cd /tmp; }; f").restricted().capture();
        assert!(
            out.stderr.contains("cd: restricted"),
            "stderr={:?}",
            out.stderr
        );
    }

    #[test]
    fn restricted_lifts_after_call() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        let _ = e.prepare("cd /tmp; pwd").restricted().capture();
        // Next call, no restricted: cd works.
        let out = e.prepare("cd /; pwd").capture();
        assert_eq!(out.stdout, "/\n", "stderr={:?}", out.stderr);
    }

    #[test]
    fn sandbox_permits_relative_redirect() {
        // Sandbox blocks ESCAPE, not local work — this is the one place it
        // deliberately diverges from bash's rbash, which refuses every file
        // target. See docs/superpowers/specs/2026-07-20-restricted-policy-design.md.
        //
        // `.cwd()` sets the PROCESS-global cwd, so this takes CWD_LOCK like
        // every other cwd test here — this box's single core serializes tests
        // and would hide the race that CI's 4 cores surface.
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join("huck-v319-sandbox-rel");
        let _ = std::fs::create_dir_all(&dir);
        let mut e = Engine::new();
        let out = e
            .prepare("echo hi > local_log; cat local_log")
            .cwd(&dir)
            .restricted()
            .capture();
        assert_eq!(out.stdout, "hi\n", "stderr: {:?}", out.stderr);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ============== TIMEOUT ==============

    #[test]
    fn exec_timeout_kills_infinite_loop() {
        use std::time::{Duration, Instant};
        let mut e = Engine::new();
        let start = Instant::now();
        let code = e
            .prepare("while true; do :; done")
            .timeout(Duration::from_millis(100))
            .run();
        let elapsed = start.elapsed();
        assert_eq!(code, 124, "expected 124, got {code}");
        assert!(
            elapsed < Duration::from_millis(2000),
            "took too long: {elapsed:?}"
        );
    }

    #[test]
    fn exec_timeout_short_script_completes_normally() {
        use std::time::Duration;
        let mut e = Engine::new();
        let out = e
            .prepare("echo hi")
            .timeout(Duration::from_secs(5))
            .capture();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "hi\n");
    }

    #[test]
    fn exec_timeout_kills_sleeping_external() {
        use std::time::{Duration, Instant};
        let mut e = Engine::new();
        let start = Instant::now();
        let code = e
            .prepare("/bin/sleep 5")
            .timeout(Duration::from_millis(100))
            .run();
        let elapsed = start.elapsed();
        assert_eq!(code, 124);
        assert!(
            elapsed < Duration::from_millis(2000),
            "took too long: {elapsed:?}"
        );
    }

    #[test]
    fn exec_timeout_exit_code_overrides_natural() {
        use std::time::Duration;
        let mut e = Engine::new();
        // Long sleep then `exit 0` — the timeout fires first.
        let code = e
            .prepare("/bin/sleep 5; exit 0")
            .timeout(Duration::from_millis(100))
            .run();
        assert_eq!(code, 124);
    }

    #[test]
    fn exec_timeout_zero_returns_124() {
        use std::time::Duration;
        let mut e = Engine::new();
        let out = e.prepare("echo hi").timeout(Duration::ZERO).capture();
        assert_eq!(out.exit_code, 124);
    }

    // ============== COMPOSITION ==============

    #[test]
    fn exec_all_knobs_compose() {
        use std::time::Duration;
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _g2 = crate::test_support::STDIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let mut e = Engine::new();
        let out = e
            .prepare("read x; echo \"got:$x\"")
            .cwd(tmp.path())
            .restricted()
            .timeout(Duration::from_secs(2))
            .stdin(b"hello\n".to_vec())
            .capture();
        assert_eq!(out.exit_code, 0, "stderr={:?}", out.stderr);
        assert_eq!(out.stdout, "got:hello\n");
    }

    #[test]
    fn exec_cwd_and_restricted() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let mut e = Engine::new();
        // `pwd` works inside restricted; `cd` doesn't.
        let out = e
            .prepare("pwd; cd /")
            .cwd(tmp.path())
            .restricted()
            .capture();
        assert!(
            out.stderr.contains("cd: restricted"),
            "stderr={:?}",
            out.stderr
        );
        let canonical_tmp = std::fs::canonicalize(tmp.path())
            .unwrap()
            .display()
            .to_string();
        let raw_tmp = tmp.path().display().to_string();
        assert!(
            out.stdout.contains(&canonical_tmp) || out.stdout.contains(&raw_tmp),
            "stdout={:?} expected to contain {:?} or {:?}",
            out.stdout,
            raw_tmp,
            canonical_tmp
        );
    }

    #[test]
    fn exec_stdin_with_timeout_blocking_read_times_out() {
        use std::time::Duration;
        let _g = crate::test_support::STDIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        // Stdin is fed but the script also sleeps — verifying stdin+timeout
        // compose (with_stdin_fd0 fires correctly before the timer aborts).
        // (Truly blocking the script's read on stdin requires a stdin source
        // that never produces EOF; our with_stdin_fd0 helper writes finite
        // bytes and closes. Use sleep as a stand-in for "stuck after read".)
        let code = e
            .prepare("read x; /bin/sleep 5; echo \"$x\"")
            .stdin(b"data\n".to_vec())
            .timeout(Duration::from_millis(100))
            .run();
        assert_eq!(code, 124);
    }

    #[test]
    fn on_stdout_line_fires_per_line() {
        let mut lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        let out = e
            .prepare("echo a; echo b; echo c")
            .on_stdout_line(|line| lines.push(line.to_string()))
            .capture();
        assert_eq!(out.exit_code, 0);
        assert_eq!(lines, vec!["a", "b", "c"]);
    }

    #[test]
    fn on_stdout_line_empty_line() {
        let mut lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        e.prepare("echo \"\"")
            .on_stdout_line(|line| lines.push(line.to_string()))
            .capture();
        assert_eq!(lines, vec![""]);
    }

    #[test]
    fn on_stdout_line_partial_at_eof() {
        let mut lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        e.prepare("printf 'no-newline'")
            .on_stdout_line(|line| lines.push(line.to_string()))
            .capture();
        assert_eq!(lines, vec!["no-newline"]);
    }

    // NOTE: `on_stderr_line_fires_per_line` moved to
    // `tests/streaming_fd_serial.rs` (process-global fd swap; races the parallel
    // harness on macOS). See that file and #90.

    #[test]
    fn on_stdout_line_captures_too() {
        let mut lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        let out = e
            .prepare("echo a; echo b")
            .on_stdout_line(|line| lines.push(line.to_string()))
            .capture();
        // Tee: BOTH the buffer AND the callback have the lines.
        assert_eq!(out.stdout, "a\nb\n");
        assert_eq!(lines, vec!["a", "b"]);
    }

    #[test]
    fn on_stdout_line_no_callback_capture_unchanged() {
        let mut e = Engine::new();
        let out = e.capture("echo unchanged");
        // Sanity: no-callback capture is exactly v205/v206 behavior.
        assert_eq!(out.stdout, "unchanged\n");
        assert_eq!(out.stderr, "");
        assert_eq!(out.exit_code, 0);
    }

    // ----- external-process poll loop --------------------------------------
    //
    // These exercise the streaming path through external children:
    // run_subprocess (single external command), the Subshell arm
    // (`( … )`), and multi-stage pipelines.

    #[test]
    fn on_stdout_line_external_real_time() {
        use std::time::{Duration, Instant};
        let mut timestamps: Vec<Instant> = Vec::new();
        let mut e = Engine::new();
        let _ = e
            .prepare("/bin/sh -c 'echo first; sleep 0.1; echo second'")
            .on_stdout_line(|_line| timestamps.push(Instant::now()))
            .capture();
        assert_eq!(timestamps.len(), 2);
        let gap = timestamps[1].duration_since(timestamps[0]);
        assert!(
            gap >= Duration::from_millis(50),
            "expected ~100ms gap, got {gap:?}"
        );
        assert!(gap <= Duration::from_secs(2), "gap too large: {gap:?}");
    }

    #[test]
    fn on_stdout_line_external_fires_during_wait() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let flag = Arc::new(AtomicBool::new(false));
        let mut e = Engine::new();
        let f = flag.clone();
        let _ = e
            .prepare("/bin/sh -c 'echo early; sleep 0.5'")
            .on_stdout_line(move |_| f.store(true, Ordering::Relaxed))
            .capture();
        assert!(flag.load(Ordering::Relaxed));
    }

    // NOTE: `on_stdout_line_pipeline_last_stage` moved to
    // `tests/forking_execution_serial.rs` as
    // `pipeline_last_stage_dispatches_on_stdout_line` (in-process fork; unsafe
    // to run concurrently with other tests — issue #184).

    // NOTE: `on_stdout_line_merge_stderr_routes_through_stdout` moved to
    // `tests/streaming_fd_serial.rs` (process-global fd swap; races the parallel
    // harness on macOS). See that file and #90.

    // NOTE: the fd-1/fd-2 tee-inheritance checks that used to live here
    // (`on_stdout_line_run_inherits_via_tee` / `on_stderr_line_run_inherits_via_tee`)
    // now live in `tests/tee_inherit.rs`. They swap a process-global std fd
    // around a fork+exec, and `dup2` clears `O_CLOEXEC`, so ANY concurrently
    // forking test in the same process can inherit the temporarily-installed
    // pipe and clobber the result. No in-process lock fixes that (the racers
    // are the other ~1800 forking tests, not each other); running them in a
    // separate integration-test binary is the only reliable isolation. See #90.

    #[test]
    fn on_stdout_line_run_no_callback_no_pipe() {
        // Sanity: no callback under run() takes the fast path.
        let mut e = Engine::new();
        let code = e.prepare("true").run();
        assert_eq!(code, 0);
    }

    #[test]
    fn on_stdout_line_external_long_line() {
        let mut got_len: usize = 0;
        let mut e = Engine::new();
        e.prepare("/bin/sh -c 'head -c 100000 < /dev/zero | tr \\\\0 a; echo'")
            .on_stdout_line(|line| got_len = line.len())
            .capture();
        assert!(got_len >= 50_000, "expected long line, got {got_len}");
    }

    // ----- composition: streaming callbacks + v205/v206 knobs --------------

    #[test]
    fn on_stdout_line_with_stdin() {
        let _g = crate::test_support::STDIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        let _ = e
            .prepare("read x; echo \"got:$x\"")
            .stdin(b"hi\n".to_vec())
            .on_stdout_line(|line| lines.push(line.to_string()))
            .capture();
        assert_eq!(lines, vec!["got:hi"]);
    }

    #[test]
    fn on_stdout_line_with_cwd() {
        let _g = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let mut lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        let _ = e
            .prepare("pwd")
            .cwd(tmp.path())
            .on_stdout_line(|line| lines.push(line.to_string()))
            .capture();
        let canonical = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(lines, vec![canonical.display().to_string()]);
    }

    #[test]
    fn on_stdout_line_with_restricted() {
        let mut err_lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        let _ = e
            .prepare("cd /tmp")
            .restricted()
            .on_stderr_line(|line| err_lines.push(line.to_string()))
            .capture();
        assert!(err_lines.iter().any(|l| l.contains("cd: restricted")));
    }

    #[test]
    fn on_stdout_line_with_timeout_fires_during_run() {
        use std::time::Duration;
        let mut lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        let code = e
            .prepare("/bin/sh -c 'echo before; sleep 5'")
            .timeout(Duration::from_millis(200))
            .on_stdout_line(|line| lines.push(line.to_string()))
            .capture()
            .exit_code;
        assert_eq!(code, 124);
        assert_eq!(lines, vec!["before"]);
    }

    #[test]
    fn all_knobs_compose() {
        use std::time::Duration;
        let _g1 = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _g2 = crate::test_support::STDIN_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let mut out_lines: Vec<String> = Vec::new();
        let mut e = Engine::new();
        let out = e
            .prepare("read x; echo \"got:$x\"")
            .cwd(tmp.path())
            .restricted()
            .timeout(Duration::from_secs(2))
            .stdin(b"hello\n".to_vec())
            .on_stdout_line(|line| out_lines.push(line.to_string()))
            .capture();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out_lines, vec!["got:hello"]);
    }

    // ----- robustness: panic + backpressure --------------------------------

    #[test]
    fn callback_panic_propagates_and_engine_recovers() {
        let mut e = Engine::new();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            e.prepare("echo a; echo b; echo c")
                .on_stdout_line(|line| {
                    if line == "b" {
                        panic!("test panic");
                    }
                })
                .capture()
        }));
        assert!(r.is_err(), "expected panic to propagate out of .capture()");
        // Engine is still usable for the next call (no state corruption).
        let out = e.capture("echo recovered");
        assert_eq!(out.stdout, "recovered\n");
    }

    #[test]
    fn callback_can_be_slow_backpressure_works() {
        use std::time::{Duration, Instant};
        let mut e = Engine::new();
        let start = Instant::now();
        let _ = e
            .prepare("for i in $(seq 1 20); do echo $i; done")
            .on_stdout_line(|_| std::thread::sleep(Duration::from_millis(20)))
            .capture();
        let elapsed = start.elapsed();
        // 20 lines × 20ms = 400ms minimum.
        assert!(
            elapsed >= Duration::from_millis(300),
            "expected backpressure to slow run, elapsed: {elapsed:?}"
        );
    }

    // NOTE: `on_stderr_line_builtin_redirect_to_err` and
    // `on_stdout_line_builtin_redirect_2to1` moved to
    // `tests/streaming_fd_serial.rs` (process-global fd swap; races the parallel
    // harness on macOS). See that file and #90.

    // ===== Completion API (v208) =====

    #[test]
    fn complete_returns_struct() {
        let mut e = Engine::new();
        let comp = e.complete("", 0);
        assert_eq!(comp.start, 0);
        // Empty-prefix command position: at minimum some builtins are present.
        assert!(
            !comp.candidates.is_empty(),
            "expected some builtins, got {:?}",
            comp.candidates
        );
    }

    #[test]
    fn complete_at_end_of_line() {
        let mut e = Engine::new();
        let line = "echo $HO";
        let comp = e.complete(line, line.len());
        assert!(
            comp.candidates.iter().any(|c| c.display == "HOME"),
            "expected HOME in {:?}",
            comp.candidates
        );
    }

    #[test]
    fn complete_with_cursor_beyond_line_len() {
        let mut e = Engine::new();
        let line = "ec";
        let at_end = e.complete(line, line.len());
        let beyond = e.complete(line, 9999);
        assert_eq!(beyond.start, at_end.start);
        assert_eq!(
            beyond
                .candidates
                .iter()
                .map(|c| c.display.as_str())
                .collect::<Vec<_>>(),
            at_end
                .candidates
                .iter()
                .map(|c| c.display.as_str())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn complete_command_position_stamps_command() {
        let mut e = Engine::new();
        let comp = e.complete("ec", 2);
        let echo = comp
            .candidates
            .iter()
            .find(|c| c.display == "echo")
            .expect("echo should complete");
        assert_eq!(echo.kind, crate::CandidateKind::Command);
    }

    #[test]
    fn complete_variable_stamps_variable() {
        let mut e = Engine::new();
        e.set_var("MY_V208_TEST_VAR", "x");
        let line = "echo $MY_V208_T";
        let comp = e.complete(line, line.len());
        let v = comp
            .candidates
            .iter()
            .find(|c| c.display == "MY_V208_TEST_VAR")
            .expect("var should complete");
        assert_eq!(v.kind, crate::CandidateKind::Variable);
    }

    #[test]
    fn complete_file_stamps_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("v208_test_file.txt");
        std::fs::write(&f, "hi").unwrap();
        let mut e = Engine::new();
        let line = format!("ls {}/v208_test", tmp.path().display());
        let comp = e.complete(&line, line.len());
        let cand = comp
            .candidates
            .iter()
            .find(|c| c.display == "v208_test_file.txt")
            .expect("file should complete");
        assert_eq!(cand.kind, crate::CandidateKind::File);
    }

    #[test]
    fn complete_directory_stamps_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("v208_test_dir");
        std::fs::create_dir(&d).unwrap();
        let mut e = Engine::new();
        let line = format!("ls {}/v208_test", tmp.path().display());
        let comp = e.complete(&line, line.len());
        let cand = comp
            .candidates
            .iter()
            .find(|c| c.display == "v208_test_dir/")
            .expect("dir should complete with trailing /");
        assert_eq!(cand.kind, crate::CandidateKind::Directory);
    }

    #[test]
    fn complete_custom_stamps_custom() {
        let mut e = Engine::new();
        // Register a -F func that produces a single candidate.
        let _ = e.run("_my_v208_completer() { COMPREPLY=( custom_v208_result ); }; complete -F _my_v208_completer mycmd");
        let comp = e.complete("mycmd ", 6);
        let cand = comp
            .candidates
            .iter()
            .find(|c| c.display == "custom_v208_result")
            .expect("custom result should appear");
        assert_eq!(cand.kind, crate::CandidateKind::Custom);
    }

    #[test]
    fn complete_does_not_modify_last_status() {
        let mut e = Engine::new();
        let _ = e.run("false");
        assert_eq!(e.last_status(), 1);
        let _ = e.complete("ec", 2);
        assert_eq!(e.last_status(), 1, "complete() must not alter $?");
    }

    #[test]
    fn complete_sees_engine_vars() {
        let mut e = Engine::new();
        e.set_var("MY_V208_VAR", "x");
        let line = "echo $MY_V208_V";
        let comp = e.complete(line, line.len());
        assert!(
            comp.candidates.iter().any(|c| c.display == "MY_V208_VAR"),
            "live engine var should be visible to complete(), got {:?}",
            comp.candidates,
        );
    }

    #[test]
    fn builder_version_sets_huck_version() {
        let mut e = Engine::builder().version("9.9.9").build();
        let out = e.capture("echo $HUCK_VERSION");
        assert_eq!(out.stdout.trim(), "9.9.9");
        assert_eq!(out.exit_code, 0);
    }
}
