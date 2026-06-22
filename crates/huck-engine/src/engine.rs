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
//! let out = e.exec("read x; printf 'got=%s\\n' \"$x\"")
//!     .stdin(b"hello\n".to_vec())
//!     .capture();
//! assert_eq!(out.stdout, "got=hello\n");
//! ```
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::executor::{StderrSink, StdoutSink};
use crate::shell_state::Shell;

/// The captured result of [`Engine::capture`] (or [`ExecBuilder::capture`]).
///
/// [`ExecBuilder::capture`]: crate::exec_builder::ExecBuilder::capture
#[derive(Debug, Clone)]
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

/// A persistent, embeddable huck shell session.
pub struct Engine {
    cell: Rc<RefCell<Shell>>,
}

impl Engine {
    /// A fresh session (`$0` = "huck"). Installs no signal handlers, reads no rc file.
    pub fn new() -> Self {
        Engine { cell: Rc::new(RefCell::new(Shell::new())) }
    }

    /// Start building a configured engine.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Wrap a caller-owned (possibly pre-configured) shell cell. The caller keeps
    /// ownership of any process-global concerns (e.g. signal handlers).
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
        self.exec(src).capture()
    }

    /// Start an advanced execution chain. Borrows `&mut self` for the chain's
    /// lifetime. See [`ExecBuilder`].
    ///
    /// [`ExecBuilder`]: crate::exec_builder::ExecBuilder
    pub fn exec(&mut self, src: &str) -> crate::exec_builder::ExecBuilder<'_> {
        crate::exec_builder::ExecBuilder::new(self, src.to_string())
    }

    /// Run a script STRING with script semantics (a "main" frame; `$0` = `arg0`).
    pub fn run_script(&mut self, src: &str, arg0: &str) -> i32 {
        self.cell.borrow_mut().shell_argv0 = arg0.to_string();
        let mut sink = StdoutSink::Terminal;
        self.run_with_label(src, arg0, true, &mut sink)
    }

    /// Read and run a script FILE with script semantics (`$0` = the path).
    /// A read failure prints `huck: <path>: <err>` and returns 127.
    pub fn run_file(&mut self, path: &Path) -> i32 {
        match std::fs::read_to_string(path) {
            Ok(contents) => self.run_script(&contents, &path.display().to_string()),
            Err(e) => {
                eprintln!("huck: {}: {}", path.display(), e);
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

    /// `$?` after the last run.
    pub fn last_status(&self) -> i32 {
        self.cell.borrow().last_status()
    }

    /// Access the underlying shell cell (advanced/dogfood use).
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
        // stderr always inherits the process here; the public `Engine::exec`
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
        let out = e.exec("echo hi; echo err >&2").capture();
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "err\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn exec_merge_stderr_interleaves_into_stdout() {
        let mut e = Engine::new();
        let out = e
            .exec("echo hi; echo err >&2; echo bye")
            .merge_stderr()
            .capture();
        assert_eq!(out.stdout, "hi\nerr\nbye\n");
        assert_eq!(out.stderr, "");
    }

    #[test]
    fn exec_feeds_stdin() {
        // Gate on STDIN_LOCK: this test swaps the process-global fd 0 via
        // `with_stdin_fd0`, racing with stdin_pipe's own tests if not serialized.
        let _guard = crate::test_support::STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        let out = e
            .exec("read x; read y; echo \"$x-$y\"")
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
        let _guard = crate::test_support::STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let big: Vec<u8> = std::iter::repeat_n(b'a', 5000)
            .chain(std::iter::once(b'\n'))
            .collect();
        let mut e = Engine::new();
        let out = e
            .exec("read line; echo \"len=${#line}\"")
            .stdin(big)
            .capture();
        assert_eq!(out.stdout, "len=5000\n");
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
        e.exec("x=set-in-first").run();
        let out = e.exec("echo \"$x\"").capture();
        assert_eq!(out.stdout, "set-in-first\n");
    }

    // ---- v205 task 7 fixup: in-memory routing must defer to real-fd dup chain
    // when an earlier file/pipe redirect intercepts the source fd.

    #[test]
    fn capture_with_file_then_dup_to_one_lets_file_win() {
        // bash: cmd >file 2>&1 — file gets the bytes; nothing captured.
        // Earlier `is_trailing_dup_to` predicate misfired here: it saw the
        // trailing `2>&1` and routed the builtin's stderr to the in-memory
        // stdout sink, leaving the file empty.
        use std::io::Read;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let mut e = Engine::new();
        let out = e.capture(&format!("echo HI > {path} 2>&1"));
        assert_eq!(out.stdout, "");
        assert_eq!(out.stderr, "");
        let mut s = String::new();
        std::fs::File::open(&path).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "HI\n");
    }

    #[test]
    fn capture_with_file_then_dup_to_two_lets_file_win() {
        // Symmetric: cmd 2>file >&2 — file gets the bytes; nothing captured.
        use std::io::Read;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let mut e = Engine::new();
        let out = e.capture(&format!("echo HI 2> {path} >&2"));
        assert_eq!(out.stdout, "");
        assert_eq!(out.stderr, "");
        let mut s = String::new();
        std::fs::File::open(&path).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "HI\n");
    }

    #[test]
    fn capture_bare_dup_to_one_routes_to_stdout_sink() {
        // The fixup must not regress Task 7's bare-builtin in-memory routing.
        //
        // route_out_to_err: a builtin's `>&2` under stderr capture lands in
        // the separate stderr buffer (not the embedder's terminal).
        let mut e = Engine::new();
        let out = e.exec("echo out; echo err >&2").capture();
        assert_eq!(out.stdout, "out\n");
        assert_eq!(out.stderr, "err\n");

        // route_err_to_out: a builtin's `2>&1` under stdout capture folds
        // stderr writes into the stdout buffer. Use a builtin whose primary
        // output goes to stderr — `declare -p UNSET_NAME` writes the "not
        // found" diagnostic to fd 2.
        let mut e = Engine::new();
        let out = e.exec("declare -p NOPE_NOT_DEFINED 2>&1").capture();
        assert_eq!(out.stderr, "");
        assert!(out.stdout.contains("NOPE_NOT_DEFINED"), "got stdout=[{:?}]", out.stdout);
    }
}
