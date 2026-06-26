use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use crate::completion_spec::{CompletionSpec, CompletionSpecs};
use crate::err_thread_local::with_err;
use crate::jobs::JobTable;

/// What kind of call-stack frame this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameKind {
    Function,
    Source,
    Main,
}

/// One entry in the unified call stack (`Shell.call_stack`).
#[derive(Debug, Clone)]
pub struct Frame {
    /// Function name, "source", or "main".
    pub funcname: String,
    /// File where this frame's code is defined (def-source).
    pub source: String,
    /// Line in the caller where this frame was invoked (0 for base).
    pub call_line: u32,
    pub kind: FrameKind,
}

/// Storage for a shell variable. Scalar covers ordinary strings;
/// Indexed is a sparse map of usize subscripts to element values
/// (sorted by key — BTreeMap so `${a[@]}` and `${!a[@]}` walk in
/// ascending subscript order).
#[derive(Debug, Clone)]
pub enum VarValue {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),
    Associative(Vec<(String, String)>),
}

impl VarValue {
    /// Returns the "scalar view" of this value: the string itself
    /// for `Scalar`, or the element at subscript 0 (or "" if no such
    /// element) for `Indexed`. This is the bash rule that `$a` and
    /// `${a}` on an indexed array mean `${a[0]}`. For `Associative`
    /// returns "" (bash: `$m` on an associative array is empty,
    /// not the first value).
    pub fn scalar_view(&self) -> &str {
        match self {
            VarValue::Scalar(s) => s.as_str(),
            VarValue::Indexed(m) => m.get(&0).map(String::as_str).unwrap_or(""),
            VarValue::Associative(_) => "",
        }
    }
}

/// The case-fold attribute set by `declare -l` / `declare -u`. Mutually
/// exclusive by construction — a variable is Lower, Upper, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseFold {
    Lower,
    Upper,
}

#[derive(Debug, Clone)]
pub struct Variable {
    pub value: VarValue,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,
    pub case_fold: Option<CaseFold>,
    pub nameref: bool,
}

impl Variable {
    /// Convenience constructor for the common case: an unexported,
    /// non-readonly, non-integer scalar.
    pub fn scalar(value: String) -> Self {
        Variable {
            value: VarValue::Scalar(value),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        }
    }
}

/// Error kind returned by the readonly-aware array mutator helpers
/// (`replace_indexed`, `set_indexed_element`, `extend_indexed`,
/// `append_indexed_element`, `unset_indexed_element`). The mutator prints
/// the user-facing diagnostic itself; callers translate this into the
/// appropriate exit status.
#[derive(Debug)]
pub enum AssignErr {
    Readonly,
    #[allow(dead_code)] // reserved for future bad-subscript paths
    BadSubscript,
    /// Variable exists but its current shape disagrees with the
    /// requested operation (e.g., set_associative_element called on
    /// an indexed variable). Caller should have routed to the
    /// correct shape-specific mutator. Defensive only — the executor
    /// must check variants before calling and surface a user-facing
    /// diagnostic.
    TypeMismatch,
}

/// Where an assigned value lands. Subscripts are ALREADY resolved by the
/// caller (which holds expansion context); the funnel takes only primitives.
#[derive(Debug, Clone)]
pub enum AssignDest {
    /// Whole variable: `name=…`, `name=(…)`, `read -a name`, `mapfile name`.
    Whole(String),
    /// A single element with an already-resolved subscript.
    Element { name: String, sub: Subscript },
}

/// Result of following a nameref chain to its effective destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedName {
    /// A plain variable target (the original name if `name` is not a nameref).
    Name(String),
    /// An element target `base[subscript-text]` — the subscript is evaluated
    /// at the use site in the base's current shape.
    Element { name: String, subscript: String },
    /// A nameref whose target value is empty (attribute set, not yet bound).
    /// Carries the nameref's own name (assignment BINDS it; reads are unset).
    Unbound(String),
    /// A cycle was detected; the `circular name reference` warning was emitted.
    Cycle,
}

/// A subscript resolved by the caller. Index → indexed array (arith-evaluated);
/// Key → associative array (string-evaluated). The caller picks the variant
/// from the target's current shape, as `apply_one_assignment` does today.
#[derive(Debug, Clone)]
pub enum Subscript {
    Index(usize),
    Key(String),
}

/// `=` vs `+=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignKind {
    Set,
    Append,
}

/// The value(s) to store, already fully expanded by the caller.
#[derive(Debug, Clone)]
pub enum AssignSource {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),
    Associative(Vec<(String, String)>),
}

impl AssignDest {
    fn name(&self) -> &str {
        match self {
            AssignDest::Whole(n) => n,
            AssignDest::Element { name, .. } => name,
        }
    }
}

/// Errors specific to declaration-builtin paths (declare -A on existing
/// indexed/scalar, etc.) that distinguish themselves from assignment errors.
/// Callers translate these into a "huck: {cmd}: ..." diagnostic via
/// [`declare_err_message`] so that `local -A` and `readonly -A` print the
/// correct command name (not a misleading "declare" prefix).
#[derive(Debug)]
pub enum DeclareErr {
    /// `declare -A NAME` where NAME is already an indexed array.
    IndexedExists,
    /// `declare -A NAME` where NAME is already a scalar.
    ScalarExists,
}

/// Formats a user-facing diagnostic for a [`DeclareErr`] using the given
/// command name (e.g., "declare", "local", "readonly") and variable name.
pub fn declare_err_message(cmd: &str, name: &str, err: &DeclareErr) -> String {
    match err {
        DeclareErr::IndexedExists => {
            format!("huck: {cmd}: {name}: cannot convert indexed to associative array")
        }
        DeclareErr::ScalarExists => {
            format!("huck: {cmd}: {name}: cannot convert scalar to associative array")
        }
    }
}

/// Persistent shell-option state controlled by `set -X` / `set -o NAME`.
/// Extend the struct AND the option-name table in `src/builtins.rs`
/// together when adding a new option.
#[derive(Debug, Clone, Default)]
pub struct ShellOptions {
    pub errexit: bool,
    pub nounset: bool,
    pub pipefail: bool,
    pub verbose: bool,
    pub xtrace: bool,
    pub noglob: bool,
    pub noclobber: bool,
    /// `set -n` / `-o noexec` / the `-n` CLI flag: read and parse commands but
    /// do not execute them (syntax check). Honored only in a non-interactive
    /// shell (bash ignores `-n` interactively). Once on, it cannot be turned
    /// back off mid-script (`set +n` is itself skipped) — matching bash.
    pub noexec: bool,
    /// `set -o physical`: when enabled, `cd` and `pwd` resolve symlinks
    /// (use the physical directory structure). When off (default), the logical
    /// PWD is maintained (symlinks preserved in `$PWD`). Mirrors bash `-P`.
    pub physical: bool,
    /// `set -o posix` / `--posix` / invoked-as-`sh` / `POSIXLY_CORRECT`: enable
    /// strict POSIX semantics. Currently gates special-builtin prefix-assignment
    /// persistence (executor.rs); more posix-mode behaviors hang off this later.
    pub posix: bool,
}

/// One row of the bash `shopt` option table.
pub struct ShoptInfo {
    pub name: &'static str,
    pub default: bool,
}

/// bash 5.2's complete `shopt` option table, in bash's display order, with
/// non-interactive default values. Bare `shopt` and `shopt -p` emit in this
/// order. Only `nullglob`/`dotglob`/`nocaseglob`/`failglob`/`nocasematch`
/// change huck's behavior; the rest are faithful inert toggles.
pub const SHOPT_TABLE: &[ShoptInfo] = &[
    ShoptInfo { name: "autocd", default: false },
    ShoptInfo { name: "assoc_expand_once", default: false },
    ShoptInfo { name: "cdable_vars", default: false },
    ShoptInfo { name: "cdspell", default: false },
    ShoptInfo { name: "checkhash", default: false },
    ShoptInfo { name: "checkjobs", default: false },
    ShoptInfo { name: "checkwinsize", default: true },
    ShoptInfo { name: "cmdhist", default: true },
    ShoptInfo { name: "compat31", default: false },
    ShoptInfo { name: "compat32", default: false },
    ShoptInfo { name: "compat40", default: false },
    ShoptInfo { name: "compat41", default: false },
    ShoptInfo { name: "compat42", default: false },
    ShoptInfo { name: "compat43", default: false },
    ShoptInfo { name: "compat44", default: false },
    ShoptInfo { name: "complete_fullquote", default: true },
    ShoptInfo { name: "direxpand", default: false },
    ShoptInfo { name: "dirspell", default: false },
    ShoptInfo { name: "dotglob", default: false },
    ShoptInfo { name: "execfail", default: false },
    ShoptInfo { name: "expand_aliases", default: false },
    ShoptInfo { name: "extdebug", default: false },
    ShoptInfo { name: "extglob", default: false },
    ShoptInfo { name: "extquote", default: true },
    ShoptInfo { name: "failglob", default: false },
    ShoptInfo { name: "force_fignore", default: true },
    ShoptInfo { name: "globasciiranges", default: true },
    ShoptInfo { name: "globskipdots", default: true },
    ShoptInfo { name: "globstar", default: false },
    ShoptInfo { name: "gnu_errfmt", default: false },
    ShoptInfo { name: "histappend", default: false },
    ShoptInfo { name: "histreedit", default: false },
    ShoptInfo { name: "histverify", default: false },
    ShoptInfo { name: "hostcomplete", default: true },
    ShoptInfo { name: "huponexit", default: false },
    ShoptInfo { name: "inherit_errexit", default: false },
    ShoptInfo { name: "interactive_comments", default: true },
    ShoptInfo { name: "lastpipe", default: false },
    ShoptInfo { name: "lithist", default: false },
    ShoptInfo { name: "localvar_inherit", default: false },
    ShoptInfo { name: "localvar_unset", default: false },
    ShoptInfo { name: "login_shell", default: false },
    ShoptInfo { name: "mailwarn", default: false },
    ShoptInfo { name: "no_empty_cmd_completion", default: false },
    ShoptInfo { name: "nocaseglob", default: false },
    ShoptInfo { name: "nocasematch", default: false },
    ShoptInfo { name: "noexpand_translation", default: false },
    ShoptInfo { name: "nullglob", default: false },
    ShoptInfo { name: "patsub_replacement", default: true },
    ShoptInfo { name: "progcomp", default: true },
    ShoptInfo { name: "progcomp_alias", default: false },
    ShoptInfo { name: "promptvars", default: true },
    ShoptInfo { name: "restricted_shell", default: false },
    ShoptInfo { name: "shift_verbose", default: false },
    ShoptInfo { name: "sourcepath", default: true },
    ShoptInfo { name: "varredir_close", default: false },
    ShoptInfo { name: "xpg_echo", default: false },
];

/// Number of `shopt` options (length of `SHOPT_TABLE`).
pub const SHOPT_COUNT: usize = SHOPT_TABLE.len();

/// Persistent `shopt` option state: one bool per `SHOPT_TABLE` entry,
/// indexed by table position. Seeded from each option's bash default.
#[derive(Debug, Clone)]
pub struct ShoptOptions {
    state: [bool; SHOPT_COUNT],
}

impl Default for ShoptOptions {
    fn default() -> Self {
        let mut state = [false; SHOPT_COUNT];
        let mut i = 0;
        while i < SHOPT_COUNT {
            state[i] = SHOPT_TABLE[i].default;
            i += 1;
        }
        Self { state }
    }
}

impl ShoptOptions {
    fn idx(name: &str) -> Option<usize> {
        SHOPT_TABLE.iter().position(|o| o.name == name)
    }

    /// `Some(value)` for a known option, `None` for an unknown name.
    pub fn get(&self, name: &str) -> Option<bool> {
        Self::idx(name).map(|i| self.state[i])
    }

    /// Sets a known option; returns `false` (no-op) for an unknown name.
    pub fn set(&mut self, name: &str, value: bool) -> bool {
        match Self::idx(name) {
            Some(i) => { self.state[i] = value; true }
            None => false,
        }
    }
}

/// readline-style settings driven by the `bind` builtin and applied to the
/// rustyline editor by the run loop. Pure data — no rustyline types here.
#[derive(Debug, Clone)]
pub struct ReadlineSettings {
    /// Every `set VAR value` (seeded with the 5 editor-mapped vars at their
    /// rustyline defaults so `bind -v` lists bash-matching defaults).
    pub vars: std::collections::BTreeMap<String, String>,
    /// Pending key bindings (keyseq, function) for the loop to apply.
    pub pending_binds: Vec<(String, String)>,
    /// Pending unbinds (keyseq) from `bind -r`.
    pub pending_unbinds: Vec<String>,
    /// Bindings the loop has applied — for `bind -p`/`-P` (keyseq -> function).
    pub active_binds: std::collections::BTreeMap<String, String>,
    /// Keyseqs the user removed via `bind -r` — subtracted from the effective
    /// keymap so unbinding a DEFAULT keyseq is reflected in `bind -p`/`-P`.
    pub unbound: std::collections::BTreeSet<String>,
    /// Set when the loop must re-sync vars/binds to the editor.
    pub dirty: bool,
}

impl Default for ReadlineSettings {
    fn default() -> Self {
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("editing-mode".to_string(), "emacs".to_string());
        vars.insert("bell-style".to_string(), "audible".to_string());
        vars.insert("show-all-if-ambiguous".to_string(), "off".to_string());
        vars.insert("completion-query-items".to_string(), "100".to_string());
        vars.insert("keyseq-timeout".to_string(), "500".to_string());
        ReadlineSettings {
            vars,
            pending_binds: Vec::new(),
            pending_unbinds: Vec::new(),
            active_binds: std::collections::BTreeMap::new(),
            unbound: std::collections::BTreeSet::new(),
            dirty: false,
        }
    }
}

/// A live coprocess started by `coproc`. The shell holds the two pipe ends
/// (relocated to high fds, close-on-exec): `read_fd` = NAME[0] (read the
/// coproc's stdout), `write_fd` = NAME[1] (write the coproc's stdin).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Coproc {
    pub name: String,
    pub pid: libc::pid_t,
    pub read_fd: std::os::unix::io::RawFd,
    pub write_fd: std::os::unix::io::RawFd,
}

/// Per-session shell state: variables (each either exported or not) and the
/// last command's exit status. The initial set of variables is seeded from
/// the process environment huck inherited at startup, every one marked
/// exported.
#[derive(Debug, Clone)]
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
    /// Exit status of the most recent command substitution; gives a bare
    /// assignment command (`VAR=$(cmd)`) bash's exit status. Set by
    /// `run_substitution`; read+reset by the `SimpleCommand::Assign` arm.
    last_cmd_sub_status: Option<i32>,
    /// Current frame of positional parameters. Populated only by
    /// function calls (Task 5); empty at the top level.
    pub positional_args: Vec<String>,
    /// getopts: 1-based char offset of the next option char within the current
    /// word (1 = "fresh word, re-check the leading dash"). Paired with
    /// `getopts_optind_cache` to detect an external OPTIND reset. (M-106)
    pub getopts_sp: usize,
    /// getopts: the OPTIND value getopts itself last wrote. If the live OPTIND
    /// differs at entry, the caller reset it → start a fresh scan.
    pub getopts_optind_cache: usize,
    /// User-defined functions. Populated by `Command::FunctionDef`
    /// execution; looked up by `run_exec_single` when dispatching a
    /// simple command.
    ///
    /// Wrapped in `Rc` for copy-on-write: `clone()` (used for every
    /// `$(…)` subshell isolation) is O(1) — just a refcount bump. A
    /// write (`define_function`/`remove_function`) calls `Rc::make_mut`,
    /// which copies the map only when the `Rc` is shared. huck is
    /// single-threaded so `Rc` (not `Arc`) is correct here.
    pub functions: Rc<HashMap<String, Box<crate::command::Command>>>,
    /// Names of functions marked `export -f`. Parallel to `functions` (no attribute
    /// slot). Re-defining a function keeps its export mark.
    pub exported_functions: std::collections::HashSet<String>,
    /// User-defined aliases. `name` → expansion text. Populated by
    /// the `alias` builtin; consumed by `expand_aliases_in_tokens`
    /// during interactive REPL input.
    pub aliases: std::collections::HashMap<String, String>,
    #[allow(dead_code)]
    pub jobs: JobTable,
    pub sigchld_flag: Arc<AtomicBool>,
    pub sigint_flag: Arc<AtomicBool>,
    /// Set by a timer thread when an `ExecBuilder::timeout` deadline elapses.
    /// Polled by `executor::check_interrupt`; when seen, the executor aborts the
    /// current run with `ExecOutcome::Interrupted(InterruptReason::Timeout)`.
    pub timeout_flag: Arc<AtomicBool>,
    /// PIDs of external children currently being waited on. Pushed at fork
    /// sites, popped after `waitpid` success. The timeout timer thread iterates
    /// this list to send SIGTERM when the deadline fires.
    pub live_external_children: Arc<Mutex<Vec<libc::pid_t>>>,
    /// True while the current `ExecBuilder::run`/`capture` call is running
    /// under `.restricted(true)`. Snapshot-and-restored by the builder.
    pub restricted: bool,
    pub shell_pgid: i32,
    /// Command history. `Rc` so cloning the Shell (per command substitution) is
    /// O(1); the rare mutation (append/load/clear) uses `Rc::make_mut` (COW).
    pub history: Rc<crate::history::History>,
    /// Shell PID, cached at startup via `getpid()`. Used for `$$`.
    pub shell_pid: i32,
    /// PID of the most-recently-backgrounded pipeline's last stage. Used for `$!`.
    pub last_bg_pid: Option<i32>,
    /// Set when the just-finished foreground command/pipeline was STOPPED
    /// (Ctrl-Z / SIGTSTP) rather than exiting. Its process substitutions are
    /// still alive (tied to the stopped job), so the post-command procsub drain
    /// must be NON-blocking — a blocking `waitpid` on a live procsub child whose
    /// consumer is also stopped deadlocks the shell. Consumed (reset) by the
    /// single-command drain epilogue.
    pub fg_stopped: bool,
    /// The shell's argv[0], cached at startup. Used for `$0` at the top level.
    pub shell_argv0: String,
    /// `$_`: the last argument (post-expansion) of the previously run simple
    /// command, or that command's program name when it had no arguments. At
    /// startup it holds the shell's invocation path (`shell_argv0`), matching
    /// bash. Updated for every simple command (builtins and externals) just
    /// after the program+args are resolved in `run_exec_single`.
    pub last_arg: String,
    /// Unified call stack. Each `call_function` pushes a `Frame` with
    /// `kind == FrameKind::Function`. Replaces the old `function_arg0: Vec<String>`.
    /// (`$0` is NOT taken from here — bash keeps `shell_argv0` inside functions.)
    pub call_stack: Vec<Frame>,
    /// Stack of name-sets for the currently-active inline-assignment scopes in
    /// `run_exec_single` (innermost last). A posix special-builtin persist
    /// deletes its names from all enclosing scopes so the live value survives
    /// their restores. Empty between top-level commands.
    pub inline_scopes: Vec<std::collections::HashSet<String>>,
    /// Map of function-name → defining source file path. Populated when a
    /// function is defined. Used to fill `Frame.source` and ultimately
    /// `BASH_SOURCE`.
    pub function_source: std::collections::HashMap<String, String>,
    /// `Some(status)` after a fatal parameter-expansion error fires
    /// inside an `expand_*` call. The executor peeks this to bail the
    /// current simple command; the REPL loop drains it via
    /// `take_pending_fatal_pe_error` to decide whether to exit (in
    /// non-interactive mode) or return to prompt (interactive).
    pub pending_fatal_pe_error: Option<i32>,
    /// True if stdin was a TTY at startup. Determines whether fatal PE
    /// errors exit the shell or just return to the prompt.
    pub is_interactive: bool,

    /// True when this process is a forked subshell child. A subshell must NOT
    /// perform interactive job-control process-grouping for its inner pipelines
    /// (that deadlocks on a controlling terminal — M-104).
    pub in_subshell: bool,

    /// Set for the dynamic extent of a completion-function invocation
    /// (`call_completion_function`). Suppresses interactive job control for the
    /// completer's subprocesses/pipelines so they don't `setpgid` / hand the
    /// controlling terminal to a new process group mid-line-edit (that wedges
    /// the shell — M-116). bash runs completion functions without job control.
    pub in_completion: bool,

    /// xtrace (`set -x`) nesting depth: the PS4 first character is repeated
    /// `xtrace_depth + 1` times. Incremented inside a command substitution
    /// (the `run_substitution` clone) and around `eval`. Functions and plain
    /// subshells do NOT change it (matching bash).
    pub xtrace_depth: usize,

    /// Persistent shell-option flags toggled by `set -e`/`-u`/`-o NAME`.
    /// See `ShellOptions` for the field list.
    pub shell_options: ShellOptions,

    /// Persistent `shopt` option flags. See `ShoptOptions` / `SHOPT_TABLE`.
    pub shopt_options: ShoptOptions,

    /// Registered trap handlers. `None` value = ignore that signal
    /// (corresponds to `trap "" SIGNAL`); `Some(text)` = action to
    /// re-parse and execute when the signal fires. Absent key =
    /// default disposition.
    pub traps: std::collections::HashMap<crate::traps::TrapSignal, Option<String>>,

    /// Per-signal bitmask of "trap pending" flags. Signal handlers set
    /// bits via `fetch_or`; the main loop drains via `swap` at the
    /// polling checkpoints. Bit N corresponds to libc signal number N.
    /// EXIT is NOT here — it fires at the exit-path boundary, not via
    /// a real signal.
    pub trap_pending: std::sync::Arc<std::sync::atomic::AtomicU32>,

    /// Map of signal number → signal-hook SigId for each currently-
    /// installed trap handler. Used by `traps::reset` to unregister.
    pub trap_sigids: std::collections::HashMap<i32, signal_hook::SigId>,

    /// Currently-firing pseudo-trap, if any. Set on entry to
    /// fire_err/fire_debug/fire_return; cleared on exit. Used to
    /// suppress re-firing of the SAME trap from within its own action.
    /// Different signals do NOT cross-suppress (a DEBUG action that
    /// triggers ERR still fires ERR).
    pub firing_trap: Option<crate::traps::TrapSignal>,

    /// Depth counter for ERR-suppression contexts (if/elif/while/until
    /// conditions). ERR trap only fires when this is 0.
    pub err_suppressed_depth: u32,

    /// Recursive `source`/`.` call depth. Capped at 64 in
    /// `builtin_source` to prevent runaway loops. Increment on
    /// enter, decrement on exit.
    pub source_depth: u32,

    /// Stack of `local`-snapshot frames. Pushed in `call_function`
    /// before the body runs; popped + restored after. Each frame
    /// maps `var_name` → the pre-`local` snapshot (None if the var
    /// was unset). Outside any function, this vec is empty —
    /// `builtin_local` checks for that.
    pub local_scopes: Vec<std::collections::HashMap<String, Option<Variable>>>,

    /// Tracks current loop-nesting depth. Incremented by run_for /
    /// run_while / run_arith_for via single-return-path wrappers,
    /// decremented on exit. Saved+restored across call_function so
    /// `break` inside a function called from a loop correctly errors.
    /// Used by `break` / `continue` builtins to validate they're in
    /// a loop and to cap the level argument to actual depth.
    pub loop_depth: u32,

    /// Command-name hash table populated by the `hash` builtin.
    /// Maps a bare name to (resolved path, hit count). Hit count
    /// is currently always 0 — no executor integration yet (see
    /// M-34 in docs/bash-divergences.md).
    pub command_hash: Rc<std::collections::HashMap<String, (std::path::PathBuf, u32)>>,

    /// Directory stack maintained by the `pushd`/`popd`/`dirs`
    /// builtins. Top is index 0 — always synced with `$PWD` at
    /// the top of each pushd/popd/dirs call.
    pub dir_stack: Vec<std::path::PathBuf>,

    /// Programmable-completion registry (filled by the `complete` builtin).
    pub completion_specs: Rc<CompletionSpecs>,
    /// Ephemeral slot used by `compopt` inside a `-F` function to mutate
    /// the live spec. Set by `dispatch::resolve` before invoking `-F`;
    /// taken back out afterward.
    pub current_completion_spec: Option<CompletionSpec>,

    /// Live process substitutions whose inner process + fd must be cleaned up
    /// after the current command (see src/procsub.rs). Snapshot/drained by the
    /// executor around each command.
    pub procsub_pending: Vec<crate::procsub::ProcSub>,

    /// Line number of the currently-executing command (POSIX `$LINENO`).
    /// Stamped by the executor at the top of `run_exec_single` from
    /// `ExecCommand.line`. Zero means "unknown / not yet set".
    pub current_lineno: u32,

    /// LCG state for `$RANDOM`. Uses interior mutability so `lookup_var`
    /// (&self) can advance the state on each read. Seeded at startup from
    /// PID + nanoseconds; reseedable via `RANDOM=n`.
    pub random_state: std::cell::Cell<u64>,

    /// Monotonic baseline for `$SECONDS`. Elapsed seconds since this
    /// instant gives the current `$SECONDS` value. Resettable via
    /// `SECONDS=n` (sets base to `now - n`).
    pub seconds_base: std::time::Instant,

    /// Live coprocesses started by `coproc NAME { ... }`. At most one active
    /// in bash 5.x (a second `coproc` kills the first), but stored as a Vec
    /// to make the structure multi-coproc-ready.
    ///
    /// COW-clone note: a `$(...)`/subshell Shell clone copies these Coproc
    /// records but does NOT own the fds; only the owning shell's reap path
    /// calls `reap_coproc`, and a forked subshell exits without running it,
    /// so there is no double-close.
    #[allow(dead_code)]
    pub coprocs: Vec<Coproc>,

    /// readline-style settings populated by the `bind` builtin; applied to
    /// the rustyline editor by the run loop. See `ReadlineSettings`.
    pub readline_settings: ReadlineSettings,
}

// ---- Static builtin variable helpers (platform strings, libc wrappers) ----

#[cfg(target_os = "linux")]
const BUILTIN_OSTYPE: &str = "linux-gnu";
#[cfg(target_os = "macos")]
const BUILTIN_OSTYPE: &str = "darwin";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const BUILTIN_OSTYPE: &str = "unknown";

fn builtin_machtype() -> String {
    let arch = std::env::consts::ARCH;
    #[cfg(target_os = "linux")]
    { format!("{arch}-pc-linux-gnu") }
    #[cfg(target_os = "macos")]
    { format!("{arch}-apple-darwin") }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    { format!("{arch}-unknown") }
}

fn builtin_current_groups() -> Vec<u32> {
    unsafe {
        let n = libc::getgroups(0, std::ptr::null_mut());
        if n <= 0 { return Vec::new(); }
        let mut buf = vec![0 as libc::gid_t; n as usize];
        let m = libc::getgroups(n, buf.as_mut_ptr());
        if m < 0 { return Vec::new(); }
        buf.truncate(m as usize);
        buf.into_iter().collect()
    }
}

fn builtin_hostname() -> String {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

// ---- (end static builtin helpers) ------------------------------------------

/// Securely parse a `BASH_FUNC_<name>%%` env value into a function body.
/// Reconstructs `"{name} {value}"` (= `name () { body }`) and parses it, accepting
/// ONLY a single `FunctionDef` whose name matches, with NOTHING after the `}` and
/// no background. NEVER executes the value. Returns `None` to skip (parse error,
/// trailing tokens, non-function, name mismatch, invalid name) — Shellshock-safe.
fn parse_imported_function(name: &str, value: &str) -> Option<Box<crate::command::Command>> {
    // name must be a plain identifier (huck functions are POSIX identifiers).
    if name.is_empty()
        || !name
            .chars()
            .next()
            .map(|c| c == '_' || c.is_ascii_alphabetic())
            .unwrap_or(false)
        || !name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
    {
        return None;
    }
    let src = format!("{name} {value}");
    let tokens = crate::lexer::tokenize(&src).ok()?;
    let seq = crate::command::parse(tokens).ok()??;
    if !seq.rest.is_empty() || seq.background {
        return None;
    }
    match seq.first {
        crate::command::Command::FunctionDef { name: n, body } if n == name => Some(body),
        _ => None,
    }
}

/// Advance the LCG and return the next RANDOM value (0..=32767).
fn random_next(state: &std::cell::Cell<u64>) -> u32 {
    let s = state.get()
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    state.set(s);
    ((s >> 33) as u32) & 0x7fff
}

/// Variables the shell maintains itself and whose user writes bash silently
/// discards (rc 0, no error). Currently FUNCNAME only; BASH_SOURCE/BASH_LINENO
/// share the behavior but are top-level-populated and deferred.
fn is_write_protected_var(name: &str) -> bool {
    name == "FUNCNAME"
}

/// Special variables that are valid/known but not always present in the vars table
/// (computed dynamics + the sometimes-unset call-stack arrays). Surfaced in variable-name
/// completion and `compgen -v` so they complete like bash even when unset.
pub const DYNAMIC_SPECIAL_VARS: &[&str] =
    &["RANDOM", "SECONDS", "EPOCHSECONDS", "BASHPID", "LINENO", "BASH_SOURCE", "BASH_LINENO"];

impl Shell {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        let mut bash_funcs: Vec<(String, String)> = Vec::new();
        for (key, value) in std::env::vars() {
            if let Some(fname) = key
                .strip_prefix("BASH_FUNC_")
                .and_then(|s| s.strip_suffix("%%"))
            {
                bash_funcs.push((fname.to_string(), value)); // function encoding, not a variable
                continue;
            }
            // bash does NOT import the interactive prompt strings PS1/PS2 from the
            // environment — a non-interactive bash leaves them empty (cf. PS0/PS4,
            // which ARE imported). huck follows suit, and additionally declines them
            // for interactive shells too: inheriting an env-exported
            // PS1='$(some_fn)' from a polluted parent (an IDE/terminal integration
            // that exports PS1) would make a fresh huck run a function it hasn't
            // defined ("command not found"). PS1/PS2 are set by the shell or a
            // sourced rc, never inherited.
            if matches!(key.as_str(), "PS1" | "PS2") {
                continue;
            }
            vars.insert(key, Variable {
                value: VarValue::Scalar(value),
                exported: true,
                readonly: false,
                integer: false,
                case_fold: None,
                nameref: false,
            });
        }
        let shell_pid = unsafe { libc::getpid() };
        let shell_argv0 = std::env::args().next().unwrap_or_else(|| "huck".to_string());
        let mut shell = Self {
            vars,
            last_status: 0,
            last_cmd_sub_status: None,
            positional_args: Vec::new(),
            getopts_sp: 0,
            getopts_optind_cache: 0,
            functions: Rc::new(HashMap::new()),
            exported_functions: std::collections::HashSet::new(),
            aliases: std::collections::HashMap::new(),
            jobs: JobTable::new(),
            sigchld_flag: Arc::new(AtomicBool::new(false)),
            sigint_flag: Arc::new(AtomicBool::new(false)),
            timeout_flag: Arc::new(AtomicBool::new(false)),
            live_external_children: Arc::new(Mutex::new(Vec::new())),
            restricted: false,
            shell_pgid: unsafe { libc::getpgrp() },
            history: Rc::new(crate::history::History::new()),
            shell_pid,
            last_bg_pid: None,
            fg_stopped: false,
            last_arg: shell_argv0.clone(),
            shell_argv0,
            call_stack: Vec::new(),
            inline_scopes: Vec::new(),
            function_source: std::collections::HashMap::new(),
            pending_fatal_pe_error: None,
            is_interactive: std::io::stdin().is_terminal(),
            in_subshell: false,
            in_completion: false,
            xtrace_depth: 0,
            shell_options: ShellOptions::default(),
            shopt_options: ShoptOptions::default(),
            traps: std::collections::HashMap::new(),
            trap_pending: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            trap_sigids: std::collections::HashMap::new(),
            firing_trap: None,
            err_suppressed_depth: 0,
            source_depth: 0,
            local_scopes: Vec::new(),
            loop_depth: 0,
            command_hash: Rc::new(std::collections::HashMap::new()),
            dir_stack: Vec::new(),
            completion_specs: Rc::new(CompletionSpecs::default()),
            current_completion_spec: None,
            procsub_pending: Vec::new(),
            current_lineno: 0,
            random_state: std::cell::Cell::new({
                let pid = shell_pid as u64;
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                pid.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(nanos) | 1
            }),
            seconds_base: std::time::Instant::now(),
            coprocs: Vec::new(),
            readline_settings: ReadlineSettings::default(),
        };
        // Make the trap_pending Arc visible to async-signal-safe
        // signal handlers installed by the traps module.
        crate::traps::init_pending_bitmask(std::sync::Arc::clone(&shell.trap_pending));
        // Import functions encoded in BASH_FUNC_<name>%% env vars. Parse-only
        // (single matching FunctionDef, never executed) — Shellshock-safe.
        for (fname, value) in bash_funcs {
            if let Some(body) = parse_imported_function(&fname, &value) {
                shell.define_function(fname.clone(), body);
                shell.mark_function_exported(&fname);
            }
        }
        // Install static builtin variables AFTER env-load and BASH_FUNC import
        // so they overwrite any inherited env values (e.g. a parent shell's
        // exported UID, SHLVL, etc.).
        shell.install_builtin_vars();
        shell
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|v| v.value.scalar_view())
    }

    /// True when this shell should use job control (own process groups +
    /// terminal handoff) for the commands it forks: an interactive shell not
    /// inside a subshell environment or a completion function. The single source
    /// of truth — replaces the inline `matches!(sink, Terminal) && !in_subshell
    /// && !in_completion` copies that had drifted (they omitted `is_interactive`).
    /// Foreground callers additionally require a `StdoutSink::Terminal` sink.
    pub fn job_control_active(&self) -> bool {
        self.is_interactive && !self.in_subshell && !self.in_completion
    }

    /// Returns the value of `$-` — alphabetical concatenation of
    /// short-flag letters reflecting current shell-options state
    /// and the interactive flag. Order: `e` (errexit), `f` (noglob),
    /// `i` (interactive), `u` (nounset), `v` (verbose), `x` (xtrace),
    /// `C` (noclobber).
    pub fn dollar_dash_value(&self) -> String {
        let mut out = String::new();
        if self.shell_options.errexit { out.push('e'); }
        if self.shell_options.noglob { out.push('f'); }
        if self.is_interactive { out.push('i'); }
        if self.shell_options.nounset { out.push('u'); }
        if self.shell_options.verbose { out.push('v'); }
        if self.shell_options.xtrace { out.push('x'); }
        if self.shell_options.noclobber { out.push('C'); }
        out
    }

    /// Derives pathname-expansion toggles from current `shopt` state.
    pub fn glob_opts(&self) -> crate::expand::GlobOpts {
        crate::expand::GlobOpts {
            nullglob: self.shopt_options.get("nullglob").unwrap_or(false),
            dotglob: self.shopt_options.get("dotglob").unwrap_or(false),
            nocaseglob: self.shopt_options.get("nocaseglob").unwrap_or(false),
            failglob: self.shopt_options.get("failglob").unwrap_or(false),
            extglob: self.shopt_options.get("extglob").unwrap_or(false),
            noglob: self.shell_options.noglob,
            globstar: self.shopt_options.get("globstar").unwrap_or(false),
        }
    }

    /// True when `shopt -s nocasematch` is in effect.
    pub fn nocasematch(&self) -> bool {
        self.shopt_options.get("nocasematch").unwrap_or(false)
    }

    /// Bash-compatible error prologue: `<name>: [line N: ][cmd: ]`.
    /// Mirrors bash `get_name_for_error` + `error_prolog`/`builtin_error_prolog`.
    /// `cmd` is the command context (`let`, `((`) or `None` for `$(( ))`.
    pub fn error_prefix(&self, cmd: Option<&str>) -> String {
        let name = if !self.is_interactive {
            self.get_indexed("BASH_SOURCE")
                .and_then(|m| m.get(&0))
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| self.shell_argv0.clone())
        } else {
            "huck".to_string()
        };
        let mut out = format!("{name}: ");
        if !self.is_interactive && self.current_lineno > 0 {
            out.push_str(&format!("line {}: ", self.current_lineno));
        }
        if let Some(c) = cmd {
            out.push_str(&format!("{c}: "));
        }
        out
    }

    /// Variable lookup for expansion. Recognises positional names
    /// (`"1"`-`"9"`/`"10"`/..., and `"#"`) before falling back to the
    /// regular variable HashMap. Returns an owned `String` because
    /// positional/computed values are not stored as references.
    pub fn lookup_var(&self, name: &str) -> Option<String> {
        // Special parameters (v26).
        match name {
            // `$0` is the shell/script invocation name and is NOT rebound on
            // function entry (bash keeps the script name inside functions —
            // unlike ksh/zsh). Sourced and Main frames already use this too.
            "0" => return Some(self.shell_argv0.clone()),
            "_" => return Some(self.last_arg.clone()),
            "$" => return Some(self.shell_pid.to_string()),
            "!" => return Some(
                // Returns "" not None when unset: bash expands $! to empty before
                // any background has happened (v26 spec §lookup_var changes).
                self.last_bg_pid.map(|p| p.to_string()).unwrap_or_default()
            ),
            "-" => return Some(self.dollar_dash_value()),
            "?" => return Some(self.last_status().to_string()),
            "LINENO" => return Some(self.current_lineno.to_string()),
            "RANDOM" => return Some(random_next(&self.random_state).to_string()),
            "SECONDS" => return Some(self.seconds_base.elapsed().as_secs().to_string()),
            "EPOCHSECONDS" => return Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    .to_string()
            ),
            "BASHPID" => return Some((unsafe { libc::getpid() }).to_string()),
            _ => {}
        }
        if name == "#" {
            return Some(self.positional_args.len().to_string());
        }
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
            let n: usize = name.parse().ok()?;
            if n == 0 {
                // unreachable: "0" is matched by the special-params block above
                return None;
            }
            return self.positional_args.get(n - 1).cloned();
        }
        // Nameref resolution: a nameref reads its target. Gate behind a cheap
        // attribute check so non-namerefs skip allocation entirely.
        if self.is_nameref(name) {
            match self.resolve_nameref(name) {
                ResolvedName::Name(n) if n != name => return self.lookup_var(&n),
                ResolvedName::Element { name: arr, subscript } => {
                    return self.lookup_nameref_element(&arr, &subscript);
                }
                ResolvedName::Unbound(_) | ResolvedName::Cycle => return None,
                ResolvedName::Name(_) => {} // not a nameref → fall through to normal read
            }
        }
        self.vars.get(name).map(|v| v.value.scalar_view().to_string())
    }

    /// Parse $FUNCNEST. Some(n) for a positive integer limit; None (unlimited)
    /// for unset / 0 / negative / non-numeric — matching bash.
    pub fn funcnest_limit(&self) -> Option<usize> {
        self.lookup_var("FUNCNEST")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|&n| n > 0)
            .map(|n| n as usize)
    }

    /// Return the raw `Variable` (value + attribute flags) for `name`,
    /// following nameref chains. Used by `array_transforms` to read the
    /// var kind (`Scalar`/`Indexed`/`Associative`) and the per-var
    /// attribute flags (`exported`/`readonly`/`integer`/etc.) when
    /// rendering `${var@A}` / `${var@K}` / `${var@k}` / `${var@a}`.
    /// Returns `None` for an unset variable.
    pub(crate) fn get_var(&self, name: &str) -> Option<&Variable> {
        let resolved = if self.is_nameref(name) {
            match self.resolve_nameref(name) {
                ResolvedName::Name(n) => n,
                ResolvedName::Element { name: arr, .. } => arr,
                ResolvedName::Unbound(_) | ResolvedName::Cycle => return None,
            }
        } else {
            name.to_string()
        };
        self.vars.get(&resolved)
    }

    /// Returns the current value of `$IFS`.
    ///
    /// - Unset → POSIX default `" \t\n"`.
    /// - Empty string → empty (caller's word-splitter must short-circuit
    ///   "no splitting" semantics; the `${*}` join treats empty IFS as
    ///   "concatenate without separator").
    /// - Otherwise → the literal IFS value.
    ///
    /// Centralized so the unset-vs-empty boundary is explicit at every
    /// expansion-site call.
    pub fn ifs(&self) -> String {
        self.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string())
    }

    /// If `name` is a reseed/reset-on-assignment dynamic special (`RANDOM`/`SECONDS`),
    /// apply the side effect and return `true` (caller must NOT store it as a var —
    /// these are computed in `lookup_var`). Returns `false` for ordinary names.
    fn reseed_special_on_assign(&mut self, name: &str, value: &str) -> bool {
        match name {
            "RANDOM" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.random_state.set(
                        n.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(0x1234) | 1,
                    );
                }
                true
            }
            "SECONDS" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.seconds_base = std::time::Instant::now()
                        .checked_sub(std::time::Duration::from_secs(n))
                        .unwrap_or_else(std::time::Instant::now);
                }
                true
            }
            _ => false,
        }
    }

    /// Raw scalar store: NO readonly check and NO attribute application
    /// (integer-coerce / case-fold) — for shell-internal writes only (env
    /// import, special/numeric vars). `reseed_special_on_assign` still fires.
    /// When the existing value is an `Indexed` array, only element 0 is
    /// overwritten — the rest of the map is preserved (bash's `a=v` rule).
    /// User-facing assignments must use `assign()` / `try_set` instead.
    ///
    /// In restricted mode, assignment to SHELL/PATH/ENV/BASH_ENV is refused
    /// with a diagnostic emitted via `err_thread_local::with_err`; if no
    /// executor sink is installed (e.g. direct unit tests), the diagnostic
    /// falls through to `io::stderr()`.
    pub fn set(&mut self, name: &str, value: String) {
        if is_write_protected_var(name) {
            return; // bash silently discards writes to FUNCNAME
        }
        if self.restricted
            && let Err(msg) = crate::restricted::check_special_assign(name)
        {
            crate::err_thread_local::with_err(|err| e!(err, "{msg}"));
            return;
        }
        self.store_scalar(name, value);
    }

    /// Resolves `$HISTSIZE` to the in-memory history cap. `None` = unlimited.
    /// unset/empty/non-numeric -> default 1000; negative -> unlimited; else n.
    /// (v139, M-59)
    pub fn resolve_histsize(&self) -> Option<usize> {
        match self.lookup_var("HISTSIZE") {
            Some(v) => match v.trim().parse::<i64>() {
                Ok(n) if n < 0 => None,
                Ok(n) => Some(n as usize),
                Err(_) => Some(crate::history::HISTORY_MAX),
            },
            None => Some(crate::history::HISTORY_MAX),
        }
    }

    /// Resolves `$HISTFILESIZE` to the history-file cap. `None` = no truncation.
    /// unset -> effective HISTSIZE; negative/non-numeric -> inhibit; else n.
    /// (v139, M-59)
    pub fn resolve_histfilesize(&self) -> Option<usize> {
        match self.lookup_var("HISTFILESIZE") {
            Some(v) => match v.trim().parse::<i64>() {
                Ok(n) if n < 0 => None,
                Ok(n) => Some(n as usize),
                Err(_) => None,
            },
            None => self.resolve_histsize(),
        }
    }

    /// Records a command in history, applying the current `$HISTSIZE` cap. (v139)
    pub fn record_history(&mut self, line: String) {
        let cap = self.resolve_histsize();
        let h = std::rc::Rc::make_mut(&mut self.history);
        h.set_max(cap);
        h.add(line);
    }

    /// Saves history to the histfile, applying the `$HISTFILESIZE` cap. (v139)
    pub fn save_history(&self) {
        self.history.save_capped(self.resolve_histfilesize());
    }

    /// True if the named variable/parameter is currently **set** (a
    /// set-but-empty variable counts as set; unset is false). Backs
    /// `[[ -v NAME ]]` and `test -v NAME`. Supports scalar names and
    /// positional parameters; array-element forms (`arr[i]`) are out of
    /// scope (M-14b) and fall through to a plain-name lookup (→ false).
    pub fn is_set(&self, name: &str) -> bool {
        // Always-defined special parameters.
        match name {
            "0" | "$" | "#" | "-" | "?" | "_" => return true,
            "!" => return self.last_bg_pid.is_some(),
            _ => {}
        }
        // Positional parameter: `1`, `2`, …
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
            return name
                .parse::<usize>()
                .map(|n| n >= 1 && n <= self.positional_args.len())
                .unwrap_or(false);
        }
        // Nameref resolution: a nameref's -v tests the TARGET. Gate behind a
        // cheap attribute check so non-namerefs skip allocation entirely.
        if self.is_nameref(name) {
            match self.resolve_nameref(name) {
                ResolvedName::Name(n) if n != name => return self.is_set(&n),
                ResolvedName::Element { name: arr, subscript } => {
                    return self.lookup_nameref_element(&arr, &subscript).is_some();
                }
                ResolvedName::Unbound(_) | ResolvedName::Cycle => return false,
                ResolvedName::Name(_) => {}
            }
        }
        self.vars.contains_key(name)
    }

    /// `-v` target for `test`/`[[ ]]`: a bare name / positional / special
    /// param, OR an array element `name[sub]` (M-14b). Element form:
    /// associative arrays use `sub` as a literal key; indexed arrays
    /// arith-evaluate `sub` to an index (matching `${arr[i]}`). Anything
    /// that isn't the `name[sub]` shape falls through to `is_set`.
    ///
    /// Subscript arithmetic is done by a READ-ONLY evaluator
    /// (`read_only_arith` below) because this method only has `&self`
    /// (`builtin_test`'s predicate is `&Shell`). It reuses
    /// `crate::arith::parse` for the AST and `array_max_index` for
    /// negative-index wrapping (so `arr[-1]` is the last element, matching
    /// `${arr[-1]}`). Side-effecting forms (`++`/`--`/assignment) and
    /// command substitutions inside a `-v` subscript are not supported and
    /// yield false — a rare edge.
    pub fn element_or_var_is_set(&self, target: &str) -> bool {
        if let Some((name, sub)) = crate::expand::split_name_subscript(target) {
            // Resolve a nameref base so `[[ -v r[i] ]]` (r→arr) tests arr[i],
            // not the nameref's own scalar value (its target name) at index i.
            let name = if self.is_nameref(&name) {
                match self.resolve_nameref(&name) {
                    ResolvedName::Name(n) => n,
                    ResolvedName::Element { name: base, .. } => base,
                    ResolvedName::Unbound(_) | ResolvedName::Cycle => return false,
                }
            } else {
                name
            };
            if self.get_associative(&name).is_some() {
                return self.lookup_associative_element(&name, &sub).is_some();
            }
            let n = match self.read_only_arith(&sub) {
                Some(n) => n,
                None => return false,
            };
            let idx = if n >= 0 {
                n as usize
            } else {
                // Negative subscript wraps from the end (bash `arr[-1]` =
                // last element), matching `eval_subscript`/`${arr[-n]}`.
                match self.array_max_index(&name) {
                    Some(max) => {
                        let wrapped = max as i64 + 1 + n;
                        if wrapped < 0 {
                            return false;
                        }
                        wrapped as usize
                    }
                    None => return false,
                }
            };
            return self.lookup_indexed_element(&name, idx).is_some();
        }
        self.is_set(target)
    }

    /// Read-only arithmetic evaluation of an array subscript string, backed
    /// by `&self` (variable lookups only — no mutation). Returns `None` for
    /// a parse error or any side-effecting / command-substitution form,
    /// which the `-v` element check treats as "not set". Pure-operator
    /// arms mirror `crate::arith::eval` (which needs `&mut Shell` for the
    /// assignment/inc-dec arms we deliberately reject here). The `match` on
    /// `ArithExpr` is EXHAUSTIVE, so a new arith operator is a compile error
    /// here — drift from `arith::eval` is caught by the compiler, not silent.
    fn read_only_arith(&self, sub: &str) -> Option<i64> {
        use crate::arith::ArithExpr as E;
        fn ev(e: &E, sh: &Shell) -> Option<i64> {
            Some(match e {
                E::Num(n) => *n,
                E::Var(name) => {
                    let raw = sh.lookup_var(name).unwrap_or_default();
                    if raw.is_empty() {
                        0
                    } else {
                        raw.parse::<i64>().ok()?
                    }
                }
                E::Index { name, subscript, subscript_raw } => {
                    // Array-element read: associative uses the raw key, indexed
                    // arith-evaluates the subscript. An unset element reads as 0.
                    let raw = if sh.get_associative(name).is_some() {
                        sh.lookup_associative_element(name, subscript_raw)
                    } else {
                        let idx = ev(subscript, sh)?;
                        if idx < 0 { return None; }
                        sh.lookup_indexed_element(name, idx as usize)
                    };
                    let raw = raw.unwrap_or_default();
                    if raw.is_empty() { 0 } else { raw.parse::<i64>().ok()? }
                }
                E::Neg(a) => ev(a, sh)?.wrapping_neg(),
                E::Not(a) => i64::from(ev(a, sh)? == 0),
                E::BitNot(a) => !ev(a, sh)?,
                E::Add(a, b) => ev(a, sh)?.wrapping_add(ev(b, sh)?),
                E::Sub(a, b) => ev(a, sh)?.wrapping_sub(ev(b, sh)?),
                E::Mul(a, b) => ev(a, sh)?.wrapping_mul(ev(b, sh)?),
                E::Div(a, b, _) => {
                    let r = ev(b, sh)?;
                    if r == 0 { return None; }
                    ev(a, sh)?.wrapping_div(r)
                }
                E::Mod(a, b, _) => {
                    let r = ev(b, sh)?;
                    if r == 0 { return None; }
                    ev(a, sh)?.wrapping_rem(r)
                }
                E::Eq(a, b) => i64::from(ev(a, sh)? == ev(b, sh)?),
                E::Ne(a, b) => i64::from(ev(a, sh)? != ev(b, sh)?),
                E::Lt(a, b) => i64::from(ev(a, sh)? < ev(b, sh)?),
                E::Le(a, b) => i64::from(ev(a, sh)? <= ev(b, sh)?),
                E::Gt(a, b) => i64::from(ev(a, sh)? > ev(b, sh)?),
                E::Ge(a, b) => i64::from(ev(a, sh)? >= ev(b, sh)?),
                E::And(a, b) => i64::from(ev(a, sh)? != 0 && ev(b, sh)? != 0),
                E::Or(a, b) => i64::from(ev(a, sh)? != 0 || ev(b, sh)? != 0),
                E::Ternary(c, t, f) => {
                    if ev(c, sh)? != 0 { ev(t, sh)? } else { ev(f, sh)? }
                }
                E::Comma(a, b) => {
                    ev(a, sh)?;
                    ev(b, sh)?
                }
                E::BitAnd(a, b) => ev(a, sh)? & ev(b, sh)?,
                E::BitOr(a, b) => ev(a, sh)? | ev(b, sh)?,
                E::BitXor(a, b) => ev(a, sh)? ^ ev(b, sh)?,
                // Shift count out of range → invalid (matches arith::eval, which
                // errors here; for a `-v` subscript that means "not set").
                E::Shl(a, b) => {
                    let (l, r) = (ev(a, sh)?, ev(b, sh)?);
                    if !(0..64).contains(&r) { return None; }
                    l.wrapping_shl(r as u32)
                }
                E::Shr(a, b) => {
                    let (l, r) = (ev(a, sh)?, ev(b, sh)?);
                    if !(0..64).contains(&r) { return None; }
                    l.wrapping_shr(r as u32)
                }
                E::Pow(a, b) => {
                    let exp = ev(b, sh)?;
                    if exp < 0 { return None; }
                    ev(a, sh)?.wrapping_pow(exp as u32)
                }
                // Side-effecting / unsupported in a read-only subscript.
                E::Assign { .. }
                | E::PreInc(_)
                | E::PreDec(_)
                | E::PostInc(_)
                | E::PostDec(_) => return None,
            })
        }
        let expr = crate::arith::parse(sub).ok()?;
        ev(&expr, self)
    }

    /// Marks an existing variable as exported. If it doesn't exist, creates
    /// it with an empty value, already exported.
    pub fn export(&mut self, name: &str) {
        self.vars
            .entry(name.to_string())
            .and_modify(|v| v.exported = true)
            .or_insert_with(|| Variable {
                value: VarValue::Scalar(String::new()),
                exported: true,
                readonly: false,
                integer: false,
                case_fold: None,
                nameref: false,
            });
    }

    /// Sets a variable's value AND marks it exported. Preserves the
    /// `readonly` flag on an existing entry — callers that need to
    /// reject writes to readonly vars must check `is_readonly` first
    /// (see `builtin_export` and `apply_inline_assignments`).
    pub fn export_set(&mut self, name: &str, value: String) {
        if self.reseed_special_on_assign(name, &value) { return; }
        match self.vars.get_mut(name) {
            Some(existing) => {
                install_scalar_value(existing, value);
                existing.exported = true;
            }
            None => {
                self.vars.insert(
                    name.to_string(),
                    Variable {
                        value: VarValue::Scalar(value),
                        exported: true,
                        readonly: false,
                        integer: false,
                        case_fold: None,
                        nameref: false,
                    },
                );
            }
        }
    }

    /// Flips the `exported` flag off on an existing variable. No-op
    /// if the variable doesn't exist. Used by `declare +x NAME`.
    pub fn unexport(&mut self, name: &str) {
        if let Some(v) = self.vars.get_mut(name) {
            v.exported = false;
        }
    }

    /// Iterator over all variable entries (name, Variable). Used by
    /// `declare` to list every variable in sorted form.
    pub fn iter_vars(&self) -> impl Iterator<Item = (&String, &Variable)> {
        self.vars.iter()
    }

    pub fn unset(&mut self, name: &str) {
        self.vars.remove(name);
    }

    /// Called from the reap path when child `pid` has exited: if it is a live
    /// coproc, close its held fds, unset NAME + NAME_PID, and drop the record.
    /// bash unsets the coproc variables once the coprocess is reaped.
    pub fn reap_coproc(&mut self, pid: libc::pid_t) {
        let Some(idx) = self.coprocs.iter().position(|c| c.pid == pid) else { return; };
        let c = self.coprocs.remove(idx);
        unsafe {
            libc::close(c.read_fd);
            libc::close(c.write_fd);
        }
        self.unset(&c.name);                       // the NAME array
        self.unset(&format!("{}_PID", c.name));    // NAME_PID
    }

    /// Scope-aware variable unset for the `unset` builtin's `-v`/default path
    /// (M-115). Implements bash's dynamic scope: `unset NAME` acts on the
    /// nearest dynamically-visible binding.
    ///
    /// - NAME local to the CURRENT function (snapshot in the TOP `local_scopes`
    ///   frame), or not local anywhere: plain `vars.remove` — the local
    ///   attribute persists via the kept snapshot, a read shows unset, and the
    ///   enclosing binding is restored on return (cases C/F/E).
    /// - NAME local to an ENCLOSING function (snapshot in a lower frame): pop
    ///   that frame's snapshot (so it will NOT restore-clobber on return) and
    ///   reveal the value it was shadowing, so a subsequent assignment promotes
    ///   upward (cases A/B/D/G/H).
    ///
    /// `Shell::unset` (plain `vars.remove`) is left for internal callers.
    pub fn unset_var(&mut self, name: &str) {
        // Nearest frame holding a snapshot for `name`, innermost-first
        // (the stack's top is the last element).
        let nearest = self
            .local_scopes
            .iter()
            .rposition(|frame| frame.contains_key(name));
        match nearest {
            // An ENCLOSING frame (not the top) localized `name`: pop it + reveal.
            Some(i) if i + 1 < self.local_scopes.len() => {
                match self.local_scopes[i].remove(name) {
                    Some(Some(var)) => {
                        self.vars.insert(name.to_string(), var);
                    }
                    Some(None) => {
                        self.vars.remove(name);
                    }
                    // rposition just found the key, so the entry is present.
                    None => {}
                }
            }
            // Top-frame local, or not local anywhere: plain unset.
            _ => {
                self.vars.remove(name);
            }
        }
    }

    /// Returns a clone of the named variable's current state, or
    /// None if unset. Used by `local` to snapshot pre-local state.
    pub fn snapshot_var(&self, name: &str) -> Option<Variable> {
        self.vars.get(name).cloned()
    }

    /// Restores `name` to `snapshot`: Some → reinstall; None →
    /// remove. Used by `call_function` on exit to undo `local`s.
    pub fn restore_var(&mut self, name: &str, snapshot: Option<Variable>) {
        match snapshot {
            Some(v) => {
                self.vars.insert(name.to_string(), v);
            }
            None => {
                self.vars.remove(name);
            }
        }
    }

    /// True if `name` is set and marked exported.
    #[allow(dead_code)] // public introspection used by integration tests
    pub fn is_exported(&self, name: &str) -> bool {
        self.vars.get(name).is_some_and(|v| v.exported)
    }

    /// Names of variables whose value is an array (indexed or associative).
    /// Used by `compgen -A arrayvar`.
    pub fn array_var_names(&self) -> Vec<String> {
        self.vars
            .iter()
            .filter(|(_, v)| matches!(v.value, VarValue::Indexed(_) | VarValue::Associative(_)))
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// True if `name` is set and marked readonly. Unset names are
    /// trivially not readonly.
    pub fn is_readonly(&self, name: &str) -> bool {
        self.vars.get(name).map(|v| v.readonly).unwrap_or(false)
    }

    /// Resolves an assignment target through any nameref indirection.
    /// Returns `None` on a cycle (warning already emitted; caller drops the
    /// write and returns `Ok(())`). For an unbound nameref, returns
    /// `Some(Whole(refname))` so that `assign()` stores the RHS into the
    /// nameref's own scalar, binding the target name.
    fn resolve_assign_target(&mut self, dest: AssignDest) -> Option<AssignDest> {
        let name = dest.name().to_string();
        // Fast path: only namerefs are resolved.
        if !self.is_nameref(&name) {
            return Some(dest);
        }
        match self.resolve_nameref(&name) {
            ResolvedName::Name(n) => Some(match dest {
                AssignDest::Whole(_) => AssignDest::Whole(n),
                AssignDest::Element { sub, .. } => AssignDest::Element { name: n, sub },
            }),
            ResolvedName::Element { name: arr, subscript } => {
                let sub = self.eval_nameref_subscript(&arr, &subscript);
                Some(AssignDest::Element { name: arr, sub })
            }
            ResolvedName::Unbound(refname) => {
                // Assigning to an UNBOUND nameref BINDS it: store the value as
                // the nameref's own scalar (which becomes the target name).
                Some(AssignDest::Whole(refname))
            }
            ResolvedName::Cycle => None, // warning already emitted; drop the write
        }
    }

    /// Evaluates a nameref element-target subscript into a `Subscript` value,
    /// choosing between associative (string key) and indexed (arith) based on
    /// the target array's current shape — mirroring `apply_one_assignment`.
    fn eval_nameref_subscript(&mut self, arr: &str, subscript: &str) -> Subscript {
        let sub_word = crate::lexer::Word(vec![crate::lexer::WordPart::Literal {
            text: subscript.to_string(),
            quoted: false,
        }]);
        if self.get_associative(arr).is_some() {
            Subscript::Key(crate::expand::eval_subscript_key(&sub_word, self))
        } else {
            match crate::expand::eval_subscript(&sub_word, self, arr) {
                Ok(i) => Subscript::Index(i),
                Err(_) => Subscript::Index(0),
            }
        }
    }

    /// Raw scalar store: RANDOM/SECONDS interception, else overwrite an existing
    /// Indexed array's element 0 (bash's `a=v` rule) or set/create a Scalar.
    /// No readonly check, no attributes — those belong to `assign()`.
    fn store_scalar(&mut self, name: &str, value: String) {
        if self.reseed_special_on_assign(name, &value) {
            return;
        }
        match self.vars.get_mut(name) {
            Some(existing) => install_scalar_value(existing, value),
            None => {
                self.vars.insert(name.to_string(), Variable::scalar(value));
            }
        }
    }

    /// Storage only: insert `value` at `idx`, promoting a scalar to indexed
    /// (element-0 rule). Caller has done readonly + fold.
    fn store_indexed_element(&mut self, name: &str, idx: usize, value: String) -> Result<(), AssignErr> {
        match self.vars.get_mut(name) {
            Some(v) => match &mut v.value {
                VarValue::Indexed(m) => { m.insert(idx, value); }
                VarValue::Scalar(s) => {
                    let mut m = BTreeMap::new();
                    if idx == 0 {
                        m.insert(0, value);
                    } else {
                        m.insert(0, std::mem::take(s));
                        m.insert(idx, value);
                    }
                    v.value = VarValue::Indexed(m);
                }
                VarValue::Associative(_) => {
                    with_err(|err| e!(err, "huck: {name}: set_indexed_element on associative variable"));
                    return Err(AssignErr::TypeMismatch);
                }
            },
            None => {
                let mut m = BTreeMap::new();
                m.insert(idx, value);
                self.vars.insert(name.to_string(), Variable {
                    value: VarValue::Indexed(m),
                    exported: false, readonly: false, integer: false, case_fold: None, nameref: false,
                });
            }
        }
        Ok(())
    }

    /// Storage only: replace the whole variable with an indexed array of the
    /// given (already-folded) elements, preserving exported/integer/case_fold.
    fn store_indexed_replace(&mut self, name: &str, elements: BTreeMap<usize, String>) -> Result<(), AssignErr> {
        let (exported, integer, case_fold) = match self.vars.get(name) {
            Some(v) => (v.exported, v.integer, v.case_fold),
            None => (false, false, None),
        };
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Indexed(elements),
            exported, readonly: false, integer, case_fold, nameref: false,
        });
        Ok(())
    }

    /// Storage only: merge (already-folded) entries into the indexed array,
    /// promoting a scalar to element 0 and creating if absent.
    fn store_indexed_extend(&mut self, name: &str, entries: BTreeMap<usize, String>) -> Result<(), AssignErr> {
        if matches!(self.vars.get(name).map(|v| &v.value), Some(VarValue::Scalar(_)))
            && let Some(v) = self.vars.get_mut(name)
            && let VarValue::Scalar(s) = &mut v.value
        {
            let mut m = BTreeMap::new();
            m.insert(0, std::mem::take(s));
            v.value = VarValue::Indexed(m);
        }
        if !self.vars.contains_key(name) {
            self.vars.insert(name.to_string(), Variable {
                value: VarValue::Indexed(BTreeMap::new()),
                exported: false, readonly: false, integer: false, case_fold: None, nameref: false,
            });
        }
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Indexed(m) = &mut v.value
        {
            for (idx, val) in entries { m.insert(idx, val); }
            Ok(())
        } else {
            with_err(|err| e!(err, "huck: {name}: cannot append array literal to associative array"));
            Err(AssignErr::TypeMismatch)
        }
    }

    /// Storage only: set `key`=`value` (already folded) in the associative
    /// array, preserving insertion order; type-error if non-associative/unset.
    fn store_assoc_element(&mut self, name: &str, key: String, value: String) -> Result<(), AssignErr> {
        match self.vars.get_mut(name) {
            Some(v) => match &mut v.value {
                VarValue::Associative(pairs) => {
                    if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == &key) {
                        slot.1 = value;
                    } else {
                        pairs.push((key, value));
                    }
                }
                _ => {
                    with_err(|err| e!(err, "huck: {name}: set_associative_element on non-associative variable"));
                    return Err(AssignErr::TypeMismatch);
                }
            },
            None => {
                with_err(|err| e!(err, "huck: {name}: set_associative_element on unset variable"));
                return Err(AssignErr::TypeMismatch);
            }
        }
        Ok(())
    }

    /// Storage only: replace the whole variable with an associative array of the
    /// given (already-folded) pairs, preserving exported/case_fold.
    fn store_assoc_replace(&mut self, name: &str, pairs: Vec<(String, String)>) -> Result<(), AssignErr> {
        let (exported, integer, case_fold) = match self.vars.get(name) {
            Some(v) => (v.exported, v.integer, v.case_fold),
            None => (false, false, None),
        };
        self.vars.insert(
            name.to_string(),
            Variable {
                value: VarValue::Associative(pairs),
                exported,
                readonly: false,
                // Integer associative arrays (`declare -Ai`) coerce VALUES on
                // assignment (the funnel does this before storing); preserve the
                // flag so later `m[k]=expr` writes keep coercing (L-49).
                integer,
                case_fold,
                nameref: false,
            },
        );
        Ok(())
    }

    /// The single chokepoint for variable assignment. Applies cross-cutting
    /// concerns in a fixed order — resolve target (nameref seam) → readonly →
    /// integer-coerce → case-fold → store — then dispatches to a storage
    /// primitive. All value-producing paths route through here (directly or via
    /// the thin leaf-mutator wrappers).
    pub fn assign(
        &mut self,
        dest: AssignDest,
        op: AssignKind,
        source: AssignSource,
    ) -> Result<(), AssignErr> {
        let dest = match self.resolve_assign_target(dest) {
            Some(d) => d,
            None => return Ok(()), // cycle: warning emitted, write dropped (rc 0)
        };
        let name = dest.name().to_string();
        if is_write_protected_var(&name) {
            return Ok(()); // bash silently discards writes to FUNCNAME
        }

        // Restricted-mode gate: refuse assignment to SHELL/PATH/ENV/BASH_ENV.
        // Mirrors the gate in `Shell::set`, but covers the user-facing path
        // (script `PATH=...` or `declare PATH=...`) which routes through here
        // rather than the leaf `set`.
        if self.restricted
            && let Err(msg) = crate::restricted::check_special_assign(&name)
        {
            with_err(|err| e!(err, "{msg}"));
            return Err(AssignErr::Readonly);
        }

        // The single readonly check, before any store (no partial array
        // writes); the storage primitives do not re-check.
        if self.is_readonly(&name) {
            with_err(|err| e!(err, "huck: {name}: readonly variable"));
            return Err(AssignErr::Readonly);
        }

        match (&dest, op, source) {
            // ── Scalar value into a whole variable: `x=v` / `x+=v` ──
            // The Append sub-case has no caller today (the executor pre-
            // concatenates scalar `+=` and calls Set); it is intentionally
            // live, not `unreachable!()`, because v160 nameref `r+=v` resolves
            // to `assign(Whole(r), Append, Scalar(v))`.
            (AssignDest::Whole(_), _, AssignSource::Scalar(v)) => {
                let v = if op == AssignKind::Append {
                    let existing = self.get(&name).map(str::to_string).unwrap_or_default();
                    existing + &v
                } else {
                    v
                };
                let stored = self.value_with_scalar_attrs(&name, v);
                self.store_scalar(&name, stored);
                Ok(())
            }
            // ── Element + Scalar (indexed): apply fold then call primitive ──
            (AssignDest::Element { name: n, sub: Subscript::Index(idx) }, _, AssignSource::Scalar(v)) => {
                let n = n.clone();
                let idx = *idx;
                let v = if op == AssignKind::Append {
                    self.lookup_indexed_element(&n, idx).unwrap_or_default() + &v
                } else { v };
                // Integer arrays (v-L49) coerce each element value via arith
                // before case-fold, mirroring the scalar attribute order in
                // `value_with_scalar_attrs`. Non-integer arrays are untouched.
                let v = if self.is_integer(&n) { eval_integer_coerce(self, &v) } else { v };
                let v = apply_case_fold(self.case_fold_of(&n), v);
                self.store_indexed_element(&n, idx, v)
            }
            (AssignDest::Element { name: n, sub: Subscript::Key(key) }, _, AssignSource::Scalar(v)) => {
                let n = n.clone();
                let key = key.clone();
                let v = if op == AssignKind::Append {
                    self.lookup_associative_element(&n, &key).unwrap_or_default() + &v
                } else { v };
                // Integer associative arrays coerce the VALUE (never the key).
                let v = if self.is_integer(&n) { eval_integer_coerce(self, &v) } else { v };
                let v = apply_case_fold(self.case_fold_of(&n), v);
                self.store_assoc_element(&n, key, v)
            }
            // ── Whole + Indexed source: fold then call primitive ──
            (AssignDest::Whole(n), _, AssignSource::Indexed(m)) => {
                let n = n.clone();
                let fold = self.case_fold_of(&n);
                let is_int = self.is_integer(&n);
                let m: BTreeMap<usize, String> = if is_int {
                    // eval_integer_coerce needs &mut self: build sequentially.
                    let mut out = BTreeMap::new();
                    for (k, v) in m {
                        out.insert(k, apply_case_fold(fold, eval_integer_coerce(self, &v)));
                    }
                    out
                } else {
                    m.into_iter().map(|(k, v)| (k, apply_case_fold(fold, v))).collect()
                };
                match op {
                    AssignKind::Set => self.store_indexed_replace(&n, m),
                    AssignKind::Append => self.store_indexed_extend(&n, m),
                }
            }
            (AssignDest::Whole(n), AssignKind::Set, AssignSource::Associative(p)) => {
                let n = n.clone();
                let fold = self.case_fold_of(&n);
                let is_int = self.is_integer(&n);
                let p: Vec<(String, String)> = if is_int {
                    let mut out = Vec::with_capacity(p.len());
                    for (k, v) in p {
                        out.push((k, apply_case_fold(fold, eval_integer_coerce(self, &v))));
                    }
                    out
                } else {
                    p.into_iter().map(|(k, v)| (k, apply_case_fold(fold, v))).collect()
                };
                self.store_assoc_replace(&n, p)
            }
            (AssignDest::Whole(_), AssignKind::Append, AssignSource::Associative(_)) => {
                unreachable!("associative whole-append is not produced by any caller")
            }
            (AssignDest::Element { .. }, _, AssignSource::Indexed(_))
            | (AssignDest::Element { .. }, _, AssignSource::Associative(_)) => {
                unreachable!("Element dest with array/assoc source is not produced by any caller")
            }
        }
    }

    /// Applies the SCALAR attribute chain (integer-coerce only on an existing
    /// integer-flagged Scalar, then case-fold) to a whole-variable value.
    fn value_with_scalar_attrs(&mut self, name: &str, value: String) -> String {
        // Coerce whenever the target is integer-flagged, regardless of its
        // current shape. For an integer SCALAR this is unchanged. For an
        // integer INDEXED array, `a=v` funnels here (Whole + Scalar arm →
        // store_scalar overwrites element 0) and bash coerces that element 0
        // value (e.g. `declare -ai a=(1 2); a=2+3` → element 0 becomes 5). An
        // integer associative `m=v` is a separate type-error path, so coercing
        // the value first is harmless.
        let do_integer_coerce = self.is_integer(name);
        let coerced = if do_integer_coerce {
            eval_integer_coerce(self, &value)
        } else {
            value
        };
        apply_case_fold(self.case_fold_of(name), coerced)
    }

    /// Checked write: refuses to overwrite a readonly variable. Returns
    /// `Err(())` if `name` is readonly (caller prints the diagnostic);
    /// otherwise sets the value and returns `Ok(())`. Consumed by
    /// executor/expansion write paths in v54 task 2.
    ///
    /// When `name` is integer-flagged (v65), the RHS is routed through
    /// `arith::parse` + `arith::eval` and stored as the decimal string.
    /// Parse/eval failures silently coerce to `"0"` (matches bash for
    /// non-`declare` integer write paths).
    // `Err(())` is an intentional single-failure-mode sentinel (readonly
    // refused; the caller prints its own diagnostic) — not a discarded error.
    #[allow(clippy::result_unit_err)]
    pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Set, AssignSource::Scalar(value))
            .map_err(|_| ())
    }

    /// Checked unset: refuses to remove a readonly variable. Returns
    /// `Err(())` if `name` is readonly; otherwise removes and returns
    /// `Ok(())`. Reserved for future read-only-aware unset call sites.
    // `Err(())` is an intentional single-failure-mode sentinel (see `try_set`).
    #[allow(dead_code, clippy::result_unit_err)]
    pub fn try_unset(&mut self, name: &str) -> Result<(), ()> {
        if self.is_readonly(name) {
            return Err(());
        }
        self.unset(name);
        Ok(())
    }

    /// Marks `name` readonly. If `name` is unset, creates it with an
    /// empty value (matching bash's behavior for `readonly NAME`
    /// against an unset name).
    pub fn mark_readonly(&mut self, name: &str) {
        if let Some(v) = self.vars.get_mut(name) {
            v.readonly = true;
        } else {
            self.vars.insert(
                name.to_string(),
                Variable {
                    value: VarValue::Scalar(String::new()),
                    exported: false,
                    readonly: true,
                    integer: false,
                    case_fold: None,
                    nameref: false,
                },
            );
        }
    }

    /// True if `name` is set and marked integer (v65). Unset names are
    /// trivially not integer.
    pub fn is_integer(&self, name: &str) -> bool {
        self.vars.get(name).map(|v| v.integer).unwrap_or(false)
    }

    /// Marks `name` integer. If `name` is unset, creates it with an
    /// empty value (mirrors `mark_readonly`). Used by `declare -i`.
    pub fn mark_integer(&mut self, name: &str) {
        if let Some(v) = self.vars.get_mut(name) {
            v.integer = true;
        } else {
            self.vars.insert(
                name.to_string(),
                Variable {
                    value: VarValue::Scalar(String::new()),
                    exported: false,
                    readonly: false,
                    integer: true,
                    case_fold: None,
                    nameref: false,
                },
            );
        }
    }

    /// Flips the `integer` flag off on an existing variable. No-op
    /// if the variable doesn't exist. Used by `declare +i NAME`.
    pub fn unmark_integer(&mut self, name: &str) {
        if let Some(v) = self.vars.get_mut(name) {
            v.integer = false;
        }
    }

    /// The case-fold attribute on `name`, or `None` if unset / no attribute.
    pub fn case_fold_of(&self, name: &str) -> Option<CaseFold> {
        self.vars.get(name).and_then(|v| v.case_fold)
    }

    /// Sets (or clears, with `None`) the case-fold attribute on `name`.
    /// Creates an empty scalar if the variable is unset, mirroring
    /// `mark_integer` (so `declare -l NAME` with no value declares it).
    pub fn set_case_fold(&mut self, name: &str, fold: Option<CaseFold>) {
        if let Some(v) = self.vars.get_mut(name) {
            v.case_fold = fold;
        } else {
            self.vars.insert(
                name.to_string(),
                Variable {
                    value: VarValue::Scalar(String::new()),
                    exported: false,
                    readonly: false,
                    integer: false,
                    case_fold: fold,
                    nameref: false,
                },
            );
        }
    }

    /// Reader for the nameref attribute.
    pub fn is_nameref(&self, name: &str) -> bool {
        self.vars.get(name).map(|v| v.nameref).unwrap_or(false)
    }

    /// Sets/clears the nameref attribute, creating the variable if absent
    /// (mirrors `set_case_fold`). The target name (if any) is stored as the
    /// scalar value separately, via the normal store path.
    pub fn set_nameref(&mut self, name: &str, on: bool) {
        if let Some(v) = self.vars.get_mut(name) {
            v.nameref = on;
        } else {
            self.vars.insert(name.to_string(), Variable {
                value: VarValue::Scalar(String::new()),
                exported: false, readonly: false, integer: false,
                case_fold: None, nameref: on,
            });
        }
    }

    /// Follows the nameref chain starting at `name` to its effective storage
    /// destination. A no-op for ordinary variables (returns `Name(name)`), so
    /// callers may resolve unconditionally. Detects cycles via a visited set
    /// and emits bash's `circular name reference` warning.
    pub fn resolve_nameref(&self, name: &str) -> ResolvedName {
        let mut current = name.to_string();
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(current.clone()) {
                with_err(|err| e!(err, "huck: warning: {current}: circular name reference"));
                return ResolvedName::Cycle;
            }
            match self.vars.get(&current) {
                Some(v) if v.nameref => {
                    let target = v.value.scalar_view().to_string();
                    if target.is_empty() {
                        return ResolvedName::Unbound(current);
                    }
                    // Element target `base[sub]`?
                    match crate::builtins::parse_subscripted_arg(&target) {
                        Ok(Some((base, sub))) => {
                            return ResolvedName::Element {
                                name: base.to_string(),
                                subscript: sub.to_string(),
                            };
                        }
                        _ => { current = target; } // plain name → follow the chain
                    }
                }
                _ => return ResolvedName::Name(current),
            }
        }
    }

    /// The raw target-name value of a nameref (its stored scalar), WITHOUT
    /// dereferencing. `None` if `name` is not a nameref.
    pub fn nameref_raw_target(&self, name: &str) -> Option<String> {
        self.vars.get(name).filter(|v| v.nameref).map(|v| v.value.scalar_view().to_string())
    }

    /// Reads `arr[subscript-text]` as a scalar, evaluating the subscript in
    /// arr's current shape (associative → literal string key; else → read-only
    /// arith index). Read-only (&self): used by nameref element-target resolution.
    fn lookup_nameref_element(&self, arr: &str, subscript: &str) -> Option<String> {
        if self.get_associative(arr).is_some() {
            // Associative: treat the raw subscript text as the string key
            // (no expansion needed for the common literal-key case).
            self.lookup_associative_element(arr, subscript)
        } else {
            // Resolve the subscript to a usize index.
            let idx: Option<usize> = if let Ok(i) = subscript.parse::<usize>() {
                // Fast path: literal non-negative integer (the common arr[0] case).
                Some(i)
            } else if let Some(n) = self.read_only_arith(subscript) {
                // Read-only arith evaluation for computed subscripts.
                if n >= 0 {
                    Some(n as usize)
                } else {
                    // Negative index: wrap from end.
                    self.array_max_index(arr).and_then(|max| {
                        let wrapped = max as i64 + 1 + n;
                        if wrapped >= 0 { Some(wrapped as usize) } else { None }
                    })
                }
            } else {
                None
            };
            idx.and_then(|i| self.lookup_indexed_element(arr, i))
        }
    }

    /// Sorted list of all variable names currently marked readonly.
    pub fn readonly_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .vars
            .iter()
            .filter(|(_, v)| v.readonly)
            .map(|(k, _)| k.clone())
            .collect();
        names.sort();
        names
    }

    // ---- Static builtin variable installation (Shell::new) -----------------

    /// Install a scalar builtin variable, overwriting any inherited env value.
    fn install_var(&mut self, name: &str, value: String, readonly: bool) {
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Scalar(value),
            exported: false,
            readonly,
            integer: false,
            case_fold: None,
            nameref: false,
        });
    }

    /// Install an indexed array builtin variable.
    fn install_indexed(&mut self, name: &str, elems: Vec<String>, readonly: bool) {
        let map: BTreeMap<usize, String> = elems.into_iter().enumerate().collect();
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Indexed(map),
            exported: false,
            readonly,
            integer: false,
            case_fold: None,
            nameref: false,
        });
    }

    /// Called once from `Shell::new` (after env-load + BASH_FUNC import) to
    /// populate the static builtin variables that are always present.
    fn install_builtin_vars(&mut self) {
        // IDs (readonly)
        unsafe {
            self.install_var("UID",  libc::getuid().to_string(),  true);
            self.install_var("EUID", libc::geteuid().to_string(), true);
            self.install_var("PPID", libc::getppid().to_string(), true);
        }
        // Groups (readonly indexed array)
        self.install_indexed(
            "GROUPS",
            builtin_current_groups().iter().map(|g| g.to_string()).collect(),
            true,
        );
        // Host / platform strings
        self.install_var("HOSTNAME", builtin_hostname(), false);
        self.install_var("HOSTTYPE", std::env::consts::ARCH.to_string(), false);
        self.install_var("OSTYPE",   BUILTIN_OSTYPE.to_string(), false);
        self.install_var("MACHTYPE", builtin_machtype(), false);
        // huck / bash identity
        self.install_var(
            "BASH",
            std::env::current_exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "huck".into()),
            false,
        );
        self.install_var("BASH_VERSION", "5.2.0(1)-release".to_string(), false);
        // readline's default word-break set. bash initializes this (interactive
        // AND -c); programmable-completion scripts APPEND to it (e.g. git does
        // `COMP_WORDBREAKS="$COMP_WORDBREAKS:"`), so it must start non-empty and
        // whitespace-inclusive or COMP_WORDS tokenization breaks after they load.
        self.install_var("COMP_WORDBREAKS", " \t\n\"'@><=;|&(:".to_string(), false);
        self.install_indexed(
            "BASH_VERSINFO",
            vec![
                "5".into(), "2".into(), "0".into(), "1".into(),
                "release".into(), builtin_machtype(),
            ],
            true,
        );
        self.install_var("HUCK_VERSION", env!("CARGO_PKG_VERSION").to_string(), false);
        // SHLVL: read inherited value (already in vars from env-load), add 1.
        let lvl = self.vars
            .get("SHLVL")
            .and_then(|v| v.value.scalar_view().parse::<i64>().ok())
            .unwrap_or(0);
        self.vars.insert("SHLVL".to_string(), Variable {
            value: VarValue::Scalar((lvl + 1).max(1).to_string()),
            exported: true,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        });
    }

    // ---- (end static builtin vars) -----------------------------------------

    /// Returns a reference to the indexed array stored under `name`,
    /// or `None` if the variable is unset or a scalar.
    pub fn get_indexed(&self, name: &str) -> Option<&BTreeMap<usize, String>> {
        // Resolve through namerefs so that array operations on a nameref
        // target the actual array (e.g. `local -n a=arr; a+=(z)` sees arr).
        let resolved = if self.is_nameref(name) {
            match self.resolve_nameref(name) {
                ResolvedName::Name(n) => n,
                _ => return None,
            }
        } else {
            name.to_string()
        };
        match self.vars.get(&resolved) {
            Some(v) => match &v.value {
                VarValue::Indexed(m) => Some(m),
                VarValue::Scalar(_) | VarValue::Associative(_) => None,
            },
            None => None,
        }
    }

    /// Returns the value at subscript `idx` for the indexed array named
    /// `name`. For scalar variables, idx=0 returns the scalar string —
    /// matches bash's `$a ≡ ${a[0]}` rule.
    pub fn lookup_indexed_element(&self, name: &str, idx: usize) -> Option<String> {
        match self.vars.get(name) {
            Some(v) => match &v.value {
                VarValue::Indexed(m) => m.get(&idx).cloned(),
                VarValue::Scalar(s) if idx == 0 => Some(s.clone()),
                VarValue::Scalar(_) => None,
                VarValue::Associative(_) => None,
            },
            None => None,
        }
    }

    /// Returns the maximum subscript present in the named array, or
    /// `None` if no elements / not an array. Used for negative-subscript
    /// wrapping in `${a[-n]}`.
    pub fn array_max_index(&self, name: &str) -> Option<usize> {
        self.get_indexed(name)
            .and_then(|m| m.keys().next_back().copied())
    }

    /// Overwrites the `PIPESTATUS` indexed-array variable with the given
    /// per-stage exit statuses. Always overwrites (even if a user marked
    /// PIPESTATUS readonly) — bash maintains it unconditionally.
    pub fn set_pipestatus(&mut self, statuses: &[i32]) {
        let elements: BTreeMap<usize, String> = statuses
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.to_string()))
            .collect();
        self.vars.insert(
            "PIPESTATUS".to_string(),
            Variable {
                value: VarValue::Indexed(elements),
                exported: false,
                readonly: false,
                integer: false,
                case_fold: None,
                nameref: false,
            },
        );
    }

    /// Insert an indexed array variable into `vars`.
    fn set_indexed_var(&mut self, name: &str, elements: BTreeMap<usize, String>) {
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Indexed(elements),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        });
    }

    /// Rebuild FUNCNAME/BASH_SOURCE/BASH_LINENO from `call_stack`.
    /// `FUNCNAME[0]` is the currently-executing function, `[1]` its caller, etc.
    /// When the stack is empty (top level) all three arrays are unset, matching bash.
    /// Called by `call_function` after every push/pop of `call_stack`.
    pub(crate) fn sync_call_arrays(&mut self) {
        if self.call_stack.is_empty() {
            self.vars.remove("FUNCNAME");
            self.vars.remove("BASH_SOURCE");
            self.vars.remove("BASH_LINENO");
            return;
        }
        let n = self.call_stack.len();
        let mut funcnames = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut linenos = BTreeMap::new();
        for i in 0..n {
            let f = &self.call_stack[n - 1 - i]; // i=0 -> top/current frame
            funcnames.insert(i, f.funcname.clone());
            sources.insert(i, f.source.clone());
            linenos.insert(i, f.call_line.to_string());
        }
        self.set_indexed_var("BASH_SOURCE", sources);
        self.set_indexed_var("BASH_LINENO", linenos);
        // FUNCNAME is unset when there are no Function frames (only Main/Source frames present).
        // bash omits FUNCNAME entirely at top-level script and during a top-level `source`,
        // but shows [source, func, ..., main] when source is called inside a function.
        let has_function_frame = self
            .call_stack
            .iter()
            .any(|f| f.kind == FrameKind::Function);
        if has_function_frame {
            self.set_indexed_var("FUNCNAME", funcnames);
        } else {
            self.vars.remove("FUNCNAME");
        }
    }

    /// Defines (or replaces) a shell function. Copy-on-write: if the function
    /// table is shared (e.g. with a command-substitution clone), this copies it
    /// first so the mutation does not leak across the isolation boundary.
    pub(crate) fn define_function(&mut self, name: String, body: Box<crate::command::Command>) {
        let def_src = self.call_stack.last().map(|f| f.source.clone())
            .unwrap_or_else(|| "environment".to_string());
        self.function_source.insert(name.clone(), def_src);
        Rc::make_mut(&mut self.functions).insert(name, body);
    }

    /// Marks `name` as an exported function (`export -f NAME`).
    pub(crate) fn mark_function_exported(&mut self, name: &str) {
        self.exported_functions.insert(name.to_string());
    }

    /// Removes the export mark from `name` (e.g. `export -nf`).
    pub(crate) fn unmark_function_exported(&mut self, name: &str) {
        self.exported_functions.remove(name);
    }

    /// True if `name` is marked exported via `export -f`.
    /// (Used by later v147 tasks; kept as part of the export-function API.)
    #[allow(dead_code)]
    pub fn is_function_exported(&self, name: &str) -> bool {
        self.exported_functions.contains(name)
    }

    /// Sorted names of all functions currently marked exported.
    pub fn exported_function_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.exported_functions.iter().cloned().collect();
        v.sort();
        v
    }

    /// `(BASH_FUNC_<name>%%, "() { body }")` pairs for each exported function still
    /// defined — injected into child process environments.
    pub fn exported_function_env(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for name in self.exported_function_names() {
            if let Some(body) = self.functions.get(&name) {
                out.push((
                    format!("BASH_FUNC_{name}%%"),
                    crate::generate::exported_function_value(body),
                ));
            }
        }
        out
    }

    /// Removes a shell function. Returns true if it existed. Copy-on-write.
    /// Also clears any `export -f` mark so `unset -f` un-exports.
    pub(crate) fn remove_function(&mut self, name: &str) -> bool {
        self.exported_functions.remove(name);
        self.function_source.remove(name);
        Rc::make_mut(&mut self.functions).remove(name).is_some()
    }

    /// Replaces (or creates) `name` as an indexed array with the given
    /// elements. Honors readonly. Preserves the existing `exported` and
    /// `integer` flags if the variable already exists.
    pub fn replace_indexed(
        &mut self,
        name: &str,
        elements: BTreeMap<usize, String>,
    ) -> Result<(), AssignErr> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Set, AssignSource::Indexed(elements))
    }

    /// Sets a single element. Promotes a scalar variable to indexed
    /// (the existing scalar value becomes element 0, unless `idx == 0`
    /// in which case it is overwritten). Honors readonly.
    pub fn set_indexed_element(
        &mut self,
        name: &str,
        idx: usize,
        value: String,
    ) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Index(idx) }, AssignKind::Set, AssignSource::Scalar(value))
    }

    /// Merges explicit `(index → value)` entries into the named indexed
    /// array, creating it if missing and promoting a scalar to element 0
    /// first. Thin wrapper over `assign()`, which performs the readonly check
    /// (callers may still pre-check to avoid a partial write). Used by
    /// `a+=(elements)` after the elements are field-expanded with continuation
    /// indices already computed. Appending to an associative array is a type error.
    pub fn extend_indexed(
        &mut self,
        name: &str,
        entries: BTreeMap<usize, String>,
    ) -> Result<(), AssignErr> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Append, AssignSource::Indexed(entries))
    }

    /// Appends `value` to the existing element at `idx` (concatenation).
    /// Used by `a[i]+=v`. If the element doesn't exist, treats prior
    /// value as empty.
    pub fn append_indexed_element(
        &mut self,
        name: &str,
        idx: usize,
        value: &str,
    ) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Index(idx) }, AssignKind::Append, AssignSource::Scalar(value.to_string()))
    }

    /// Removes a single element from an indexed array. No-op if the
    /// variable is missing, scalar, or doesn't contain that subscript.
    /// Honors readonly.
    pub fn unset_indexed_element(&mut self, name: &str, idx: usize) -> Result<(), AssignErr> {
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            with_err(|err| e!(err, "huck: {name}: readonly variable"));
            return Err(AssignErr::Readonly);
        }
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Indexed(m) = &mut v.value
        {
            m.remove(&idx);
        }
        Ok(())
    }

    /// Returns a reference to the associative array stored under `name`,
    /// or `None` if the variable is unset, scalar, or indexed.
    /// Resolves through namerefs so that `local -n m=mymap; m[k]=v` dispatches
    /// correctly to the associative path.
    pub fn get_associative(&self, name: &str) -> Option<&Vec<(String, String)>> {
        // Resolve through namerefs.
        let resolved = if self.is_nameref(name) {
            match self.resolve_nameref(name) {
                ResolvedName::Name(n) => n,
                _ => return None,
            }
        } else {
            name.to_string()
        };
        match self.vars.get(&resolved) {
            Some(v) => match &v.value {
                VarValue::Associative(pairs) => Some(pairs),
                _ => None,
            },
            None => None,
        }
    }

    /// Returns the value at string key `key` for the associative array `name`.
    /// `None` if the variable is unset, not associative, or has no such key.
    pub fn lookup_associative_element(&self, name: &str, key: &str) -> Option<String> {
        self.get_associative(name).and_then(|pairs| {
            pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        })
    }

    /// Sets `key` to `value` in the associative array `name`. Preserves
    /// insertion order on update (existing key keeps its position; new
    /// keys are appended). Honors readonly. Errors with AssignErr::Readonly
    /// (used as a generic "could not set" signal) if the variable exists
    /// and is NOT associative — callers (the executor) must check the
    /// variant before calling this.
    pub fn set_associative_element(
        &mut self,
        name: &str,
        key: String,
        value: String,
    ) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Key(key) }, AssignKind::Set, AssignSource::Scalar(value))
    }

    /// `m[k]+=v` — concatenate `value` to the existing element at `key`,
    /// or set to `value` if no such key. Honors readonly.
    pub fn append_associative_element(
        &mut self,
        name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Key(key.to_string()) }, AssignKind::Append, AssignSource::Scalar(value.to_string()))
    }

    /// Removes the entry at `key` from the associative array `name`.
    /// No-op if missing/not-associative/no-such-key. Honors readonly.
    /// Reached from `builtin_unset` when the target is an associative
    /// array (see `src/builtins.rs`, the `get_associative(name).is_some()`
    /// branch).
    pub fn unset_associative_element(
        &mut self,
        name: &str,
        key: &str,
    ) -> Result<(), AssignErr> {
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            with_err(|err| e!(err, "huck: {name}: readonly variable"));
            return Err(AssignErr::Readonly);
        }
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Associative(pairs) = &mut v.value
        {
            pairs.retain(|(k, _)| k != key);
        }
        Ok(())
    }

    /// Replaces (or creates) `name` as an associative array with the given
    /// pairs in insertion order. Honors readonly. Preserves exported flag
    /// if the variable exists.
    pub fn replace_associative(
        &mut self,
        name: &str,
        pairs: Vec<(String, String)>,
    ) -> Result<(), AssignErr> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Set, AssignSource::Associative(pairs))
    }

    /// Creates an empty associative array under `name`. Enforces bash rules:
    /// - Unset → create empty associative.
    /// - Already associative → no-op.
    /// - Indexed → error: `DeclareErr::IndexedExists`.
    /// - Scalar → error: `DeclareErr::ScalarExists`.
    ///
    /// Does NOT print any diagnostic; callers should format via
    /// [`declare_err_message`] so the correct command name (declare,
    /// local, readonly) appears in the message.
    pub fn declare_associative(&mut self, name: &str) -> Result<(), DeclareErr> {
        match self.vars.get(name).map(|v| &v.value) {
            None => {
                self.vars.insert(
                    name.to_string(),
                    Variable {
                        value: VarValue::Associative(Vec::new()),
                        exported: false,
                        readonly: false,
                        integer: false,
                        case_fold: None,
                        nameref: false,
                    },
                );
                Ok(())
            }
            Some(VarValue::Associative(_)) => Ok(()),
            Some(VarValue::Indexed(_)) => Err(DeclareErr::IndexedExists),
            Some(VarValue::Scalar(_)) => Err(DeclareErr::ScalarExists),
        }
    }

    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    pub fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }

    pub fn set_last_cmd_sub_status(&mut self, s: Option<i32>) {
        self.last_cmd_sub_status = s;
    }
    pub fn last_cmd_sub_status(&self) -> Option<i32> {
        self.last_cmd_sub_status
    }

    /// Returns and clears the pending fatal-PE-error flag.
    pub fn take_pending_fatal_pe_error(&mut self) -> Option<i32> {
        self.pending_fatal_pe_error.take()
    }

    /// Iterates only the exported variables, suitable for passing to a child
    /// process's `Command::envs`.
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            .map(|(k, v)| (k.as_str(), v.value.scalar_view()))
    }

    /// Iterates the names of all variables (exported or not).
    pub fn var_names(&self) -> impl Iterator<Item = &str> {
        self.vars.keys().map(|s| s.as_str())
    }

    /// Variable names for completion / `compgen -v`: the vars table plus the known
    /// dynamic/special names not always stored. Deduped (sorted).
    pub fn completion_var_names(&self) -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> = self.vars.keys().cloned().collect();
        for &n in DYNAMIC_SPECIAL_VARS {
            set.insert(n.to_string());
        }
        set.into_iter().collect()
    }

    /// Sends SIGHUP to every live job not marked for nohup. Called
    /// on each clean shell-exit path. Stopped jobs get SIGCONT first
    /// so they wake to die. Errors from `killpg` (e.g. ESRCH for an
    /// already-reaped pgid) are intentionally ignored; this is a
    /// best-effort cleanup.
    pub fn hangup_jobs(&mut self) {
        for job in self.jobs.iter() {
            if !should_hangup(job) {
                continue;
            }
            if job.own_pgroup {
                unsafe {
                    libc::killpg(job.pgid, libc::SIGCONT);
                    libc::killpg(job.pgid, libc::SIGHUP);
                }
            } else {
                for &pid in &job.pids {
                    unsafe {
                        libc::kill(pid, libc::SIGCONT);
                        libc::kill(pid, libc::SIGHUP);
                    }
                }
            }
        }
    }
}

/// Evaluate `value` as a bash arithmetic expression and return the
/// decimal-string result, or `"0"` on parse/eval failure (bash's
/// silent-coerce-to-zero semantics for integer-flagged variable
/// writes). Module-scope so `Shell::try_set` can call it while
/// holding `&mut Shell` (the function takes `&mut Shell` itself).
fn eval_integer_coerce(shell: &mut Shell, value: &str) -> String {
    match crate::arith::parse(value) {
        Ok(expr) => match crate::arith::eval(&expr, shell) {
            Ok(n) => n.to_string(),
            Err(_) => "0".to_string(),
        },
        Err(_) => "0".to_string(),
    }
}

/// Pure predicate: should this job receive SIGHUP at shell exit?
/// True iff the job is still alive (Running or Stopped) AND has
/// not been marked for nohup by `disown -h`.
fn should_hangup(job: &crate::jobs::Job) -> bool {
    let live = matches!(
        job.state,
        crate::jobs::JobState::Running | crate::jobs::JobState::Stopped(_)
    );
    live && !job.marked_for_nohup
}

/// Wraps a `bind` key sequence in double quotes for `bind -p`/`-P` output if
/// the user didn't already supply them (bash always double-quotes the keyseq).
fn quote_keyseq(k: &str) -> String {
    if k.starts_with('"') { k.to_string() } else { format!("\"{k}\"") }
}

/// Installs `value` as the scalar value of `existing`, preserving the
/// rest of an `Indexed` map (writing only element 0). Shared by
/// `Shell::store_scalar` (the single scalar-store primitive behind both
/// `assign()` and the raw `set()`) and `Shell::export_set`, so every
/// scalar-store path applies the "scalar assignment to an array
/// overwrites a[0]" rule identically (matches bash).
fn install_scalar_value(existing: &mut Variable, value: String) {
    match &mut existing.value {
        VarValue::Indexed(m) => {
            m.insert(0, value);
        }
        VarValue::Scalar(_) => {
            existing.value = VarValue::Scalar(value);
        }
        VarValue::Associative(_) => {
            with_err(|err| e!(err, "huck: internal: install_scalar_value on associative array"));
        }
    }
}

impl Shell {
    /// Records a `set VAR value` (sets `dirty`). The run loop applies the
    /// editor-mapped ones; others are recorded for `bind -v` round-trip.
    pub fn set_readline_var(&mut self, name: &str, value: &str) {
        self.readline_settings.vars.insert(name.to_string(), value.to_string());
        self.readline_settings.dirty = true;
    }

    /// Queues a key binding (keyseq -> function) for the loop to apply.
    pub fn add_bind(&mut self, keyseq: &str, function: &str) {
        self.readline_settings.pending_binds.push((keyseq.to_string(), function.to_string()));
        self.readline_settings.dirty = true;
    }

    /// Queues an unbind (keyseq) for the loop to apply.
    pub fn add_unbind(&mut self, keyseq: &str) {
        self.readline_settings.pending_unbinds.push(keyseq.to_string());
        self.readline_settings.unbound.insert(keyseq.to_string());
        self.readline_settings.dirty = true;
    }

    /// `bind -v` lines: `set NAME VALUE`, sorted by name (BTreeMap iterates sorted).
    pub fn readline_var_lines(&self) -> Vec<String> {
        self.readline_settings.vars.iter().map(|(k, v)| format!("set {k} {v}")).collect()
    }

    /// `bind -V` lines: `` NAME is set to `VALUE' ``.
    pub fn readline_var_lines_verbose(&self) -> Vec<String> {
        self.readline_settings.vars.iter().map(|(k, v)| format!("{k} is set to `{v}'")).collect()
    }

    // (quote_keyseq helper is a module-level free fn below.)

    /// The effective key bindings (keyseq → function): the default emacs keymap,
    /// overlaid with the user's bindings (already-applied `active_binds` AND
    /// not-yet-applied `pending_binds`, so `-c`-mode binds show too), minus any
    /// keyseq the user unbound. Keyseqs are normalized to bash's quoted form.
    fn effective_binds(&self) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        for (k, f) in crate::readline_bind::DEFAULT_EMACS_BINDS {
            m.insert(quote_keyseq(k), (*f).to_string());
        }
        for (k, f) in &self.readline_settings.active_binds {
            m.insert(quote_keyseq(k), f.clone());
        }
        for (k, f) in &self.readline_settings.pending_binds {
            m.insert(quote_keyseq(k), f.clone());
        }
        for k in &self.readline_settings.unbound {
            m.remove(&quote_keyseq(k));
        }
        m
    }

    /// `bind -p` lines: `"KEYSEQ": FUNCTION` for each effective binding, grouped
    /// and sorted by function name (matching bash); `# FUNCTION (not bound)` for
    /// honored functions with no binding.
    pub fn active_bind_lines(&self) -> Vec<String> {
        let eff = self.effective_binds();
        let mut by_func: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for (k, f) in &eff {
            by_func.entry(f.as_str()).or_default().push(k.as_str());
        }
        let mut out = Vec::new();
        for func in crate::readline_bind::readline_function_names() {
            match by_func.get(func) {
                Some(keys) => {
                    let mut keys = keys.clone();
                    keys.sort_unstable();
                    for k in keys {
                        out.push(format!("{k}: {func}"));
                    }
                }
                None => out.push(format!("# {func} (not bound)")),
            }
        }
        out
    }

    /// `bind -P` lines: `FUNCTION can be found on "K1", "K2".` (all keyseqs) or
    /// `FUNCTION is not bound to any keys`, per honored function sorted by name.
    pub fn active_bind_lines_verbose(&self) -> Vec<String> {
        let eff = self.effective_binds();
        let mut by_func: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for (k, f) in &eff {
            by_func.entry(f.as_str()).or_default().push(k.as_str());
        }
        let mut out = Vec::new();
        for func in crate::readline_bind::readline_function_names() {
            match by_func.get(func) {
                Some(keys) => {
                    let mut keys = keys.clone();
                    keys.sort_unstable();
                    out.push(format!("{func} can be found on {}.", keys.join(", ")));
                }
                None => out.push(format!("{func} is not bound to any keys")),
            }
        }
        out
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

/// Applies a variable's case-fold attribute to a value string. `None`
/// returns the value unchanged. Lower/Upper use Rust's whole-string
/// `to_lowercase`/`to_uppercase`, which is byte-identical to the
/// `${v,,}`/`${v^^}` `case_modify` helper for the no-pattern case (and
/// therefore inherits the same documented L-04 Unicode behavior).
fn apply_case_fold(fold: Option<CaseFold>, value: String) -> String {
    match fold {
        None => value,
        Some(CaseFold::Lower) => value.to_lowercase(),
        Some(CaseFold::Upper) => value.to_uppercase(),
    }
}

#[cfg(test)]
impl Shell {
    /// Test-only helper: install an indexed-array variable directly.
    /// Used by Task 3 expansion tests before Task 4 wires the executor
    /// path that would otherwise create arrays via syntax.
    pub fn seed_array_for_tests(&mut self, name: &str, elements: &[(usize, &str)]) {
        let mut m: BTreeMap<usize, String> = BTreeMap::new();
        for (k, v) in elements {
            m.insert(*k, (*v).to_string());
        }
        self.vars.insert(
            name.to_string(),
            Variable {
                value: VarValue::Indexed(m),
                exported: false,
                readonly: false,
                integer: false,
                case_fold: None,
                nameref: false,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funcname_assignment_is_silently_discarded() {
        let mut sh = Shell::new();
        // `set` path (used by `for`, internal writers).
        sh.set("FUNCNAME", "7".to_string());
        assert_eq!(sh.lookup_var("FUNCNAME"), None, "set must not write FUNCNAME");
        // `assign` path (used by `FOO=v`, inline, declare, read via try_set).
        let _ = sh.try_set("FUNCNAME", "9".to_string());
        assert_eq!(sh.lookup_var("FUNCNAME"), None, "assign must not write FUNCNAME");
    }

    #[test]
    fn funcnest_limit_parses_positive_else_none() {
        let mut sh = Shell::new();
        assert_eq!(sh.funcnest_limit(), None);                 // unset
        sh.set("FUNCNEST", "0".to_string());
        assert_eq!(sh.funcnest_limit(), None);                 // 0 = unlimited
        sh.set("FUNCNEST", "-3".to_string());
        assert_eq!(sh.funcnest_limit(), None);                 // negative = unlimited
        sh.set("FUNCNEST", "abc".to_string());
        assert_eq!(sh.funcnest_limit(), None);                 // non-numeric = unlimited
        sh.set("FUNCNEST", "5".to_string());
        assert_eq!(sh.funcnest_limit(), Some(5));
        sh.set("FUNCNEST", " 5 ".to_string());
        assert_eq!(sh.funcnest_limit(), Some(5));              // trimmed
    }

    #[test]
    fn non_protected_var_still_writes() {
        let mut sh = Shell::new();
        sh.set("FOO", "x".to_string());
        assert_eq!(sh.lookup_var("FOO"), Some("x".to_string()));
        let _ = sh.try_set("BAR", "y".to_string());
        assert_eq!(sh.lookup_var("BAR"), Some("y".to_string()));
    }

    #[test]
    fn glob_opts_reads_globstar_shopt() {
        let mut sh = Shell::new();
        assert!(!sh.glob_opts().globstar, "globstar off by default");
        crate::shell::process_line("shopt -s globstar", &mut sh, false);
        assert!(sh.glob_opts().globstar, "globstar on after shopt -s");
    }

    #[test]
    fn bind_p_shows_defaults_user_override_and_unbind() {
        let mut sh = Shell::new();
        // default present
        let p = sh.active_bind_lines();
        assert!(p.iter().any(|l| l == "\"\\C-a\": beginning-of-line"), "missing default C-a: {p:?}");
        assert!(p.iter().any(|l| l == "# backward-kill-line (not bound)"), "missing not-bound line: {p:?}");
        // -P format
        let pv = sh.active_bind_lines_verbose();
        assert!(pv.iter().any(|l| l == "beginning-of-line can be found on \"\\C-a\"."), "{pv:?}");
        assert!(pv.iter().any(|l| l == "backward-kill-line is not bound to any keys"), "{pv:?}");
        // user override via pending_binds (the -c-mode path)
        sh.add_bind("\"\\C-a\"", "kill-line");
        let p2 = sh.active_bind_lines();
        assert!(p2.iter().any(|l| l == "\"\\C-a\": kill-line"), "override not applied: {p2:?}");
        assert!(!p2.iter().any(|l| l == "\"\\C-a\": beginning-of-line"), "default not overridden: {p2:?}");
        // unbind a default
        let mut sh2 = Shell::new();
        sh2.add_unbind("\\C-e");
        let p3 = sh2.active_bind_lines();
        assert!(!p3.iter().any(|l| l.contains("\\C-e")), "C-e still shown after unbind: {p3:?}");
    }

    #[test]
    fn bind_p_groups_multiple_keyseqs_for_one_function() {
        let sh = Shell::new();
        // accept-line is bound to both C-j and C-m by default.
        let p = sh.active_bind_lines();
        assert!(p.iter().any(|l| l == "\"\\C-j\": accept-line"), "missing C-j: {p:?}");
        assert!(p.iter().any(|l| l == "\"\\C-m\": accept-line"), "missing C-m: {p:?}");
        // -P joins both keyseqs on one line (sorted): C-j before C-m.
        let pv = sh.active_bind_lines_verbose();
        assert!(
            pv.iter().any(|l| l == "accept-line can be found on \"\\C-j\", \"\\C-m\"."),
            "multi-keyseq -P join wrong: {pv:?}"
        );
    }

    #[cfg(test)]
    fn test_fn_body() -> Box<crate::command::Command> {
        let seq = crate::command::parse(crate::lexer::tokenize("f(){ echo hi; }").unwrap())
            .unwrap()
            .unwrap();
        match seq.first {
            crate::command::Command::FunctionDef { body, .. } => body,
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn import_accepts_clean_function() {
        let body = parse_imported_function("g", "() { echo hi; }");
        assert!(body.is_some(), "clean function body should parse");
        match *body.unwrap() {
            crate::command::Command::BraceGroup(_) => {}
            other => panic!("expected brace-group body, got {other:?}"),
        }
    }

    #[test]
    fn import_rejects_trailing_command() {
        // Shellshock: a trailing command after the `}` must NOT be accepted.
        assert!(parse_imported_function("x", "() { :; }; touch /tmp/PWN").is_none());
    }

    #[test]
    fn import_rejects_bare_command() {
        assert!(parse_imported_function("x", "echo not_a_function").is_none());
    }

    #[test]
    fn import_rejects_parse_error() {
        assert!(parse_imported_function("x", "() { if; }").is_none());
    }

    #[test]
    fn import_accepts_simple_and_rejects_invalid_name() {
        assert!(parse_imported_function("x", "() { :; }").is_some());
        assert!(parse_imported_function("bad name", "() { :; }").is_none());
        // a value smuggling its OWN name (so reconstruction isn't a lone FunctionDef) → rejected.
        assert!(parse_imported_function("x", "y () { :; }").is_none());
    }

    #[test]
    fn exported_function_env_pairs() {
        let mut sh = Shell::new();
        sh.define_function("ef".to_string(), test_fn_body());
        sh.mark_function_exported("ef");
        let env = sh.exported_function_env();
        let (_, v) = env
            .iter()
            .find(|(k, _)| k == "BASH_FUNC_ef%%")
            .expect("BASH_FUNC_ef%% present");
        assert!(v.starts_with("() "), "value should be () {{...}}: {v:?}");
        assert!(v.contains("echo hi"), "{v:?}");
    }

    #[test]
    fn mark_and_query_exported_function() {
        let mut sh = Shell::new();
        sh.define_function("f".to_string(), test_fn_body());
        assert!(!sh.is_function_exported("f"));
        sh.mark_function_exported("f");
        assert!(sh.is_function_exported("f"));
        assert_eq!(sh.exported_function_names(), vec!["f".to_string()]);
    }

    #[test]
    fn remove_function_unexports() {
        let mut sh = Shell::new();
        sh.define_function("f".to_string(), test_fn_body());
        sh.mark_function_exported("f");
        assert!(sh.remove_function("f"));
        assert!(!sh.is_function_exported("f"), "unset -f must un-export");
    }

    #[test]
    fn is_set_true_for_set_var_even_when_empty() {
        let mut sh = Shell::new();
        sh.set("X", String::new()); // Shell::set(&mut self, name: &str, value: String)
        assert!(sh.is_set("X"));
    }

    #[test]
    fn array_var_names_lists_arrays_not_scalars() {
        let mut sh = Shell::new();
        sh.set("scal", "x".to_string());
        let mut elements = std::collections::BTreeMap::new();
        elements.insert(0usize, "a".to_string());
        elements.insert(1usize, "b".to_string());
        sh.replace_indexed("arr", elements).unwrap(); // existing pub Shell method (indexed array)
        let names = sh.array_var_names();
        assert!(names.contains(&"arr".to_string()));
        assert!(!names.contains(&"scal".to_string()));
    }

    #[test]
    fn is_set_false_for_unset() {
        let sh = Shell::new();
        assert!(!sh.is_set("DEFINITELY_UNSET_VAR_XYZ"));
    }

    #[test]
    fn is_set_positional_params() {
        let mut sh = Shell::new();
        sh.positional_args = vec!["a".into(), "b".into()];
        assert!(sh.is_set("1"));
        assert!(sh.is_set("2"));
        assert!(!sh.is_set("3"));
    }

    #[test]
    fn is_set_special_zero_is_true() {
        let sh = Shell::new();
        assert!(sh.is_set("0"));
    }

    #[test]
    fn set_pipestatus_writes_indexed_array() {
        let mut sh = Shell::new();
        sh.set_pipestatus(&[0, 1, 0]);
        let arr = sh.get_indexed("PIPESTATUS").expect("PIPESTATUS array");
        assert_eq!(arr.get(&0).map(String::as_str), Some("0"));
        assert_eq!(arr.get(&1).map(String::as_str), Some("1"));
        assert_eq!(arr.get(&2).map(String::as_str), Some("0"));
        assert_eq!(arr.len(), 3);
    }

    fn make_func_frame(name: &str) -> Frame {
        Frame {
            funcname: name.to_string(),
            source: "environment".to_string(),
            call_line: 0,
            kind: FrameKind::Function,
        }
    }

    #[test]
    fn sync_call_arrays_builds_reversed_stack() {
        let mut sh = Shell::new();
        sh.call_stack.push(make_func_frame("outer"));
        sh.call_stack.push(make_func_frame("inner"));
        sh.sync_call_arrays();
        let arr = sh.get_indexed("FUNCNAME").expect("FUNCNAME array");
        assert_eq!(arr.get(&0).map(String::as_str), Some("inner")); // [0] = current
        assert_eq!(arr.get(&1).map(String::as_str), Some("outer")); // [1] = caller
        assert_eq!(arr.len(), 2);
        assert_eq!(sh.lookup_var("FUNCNAME"), Some("inner".to_string()));
    }

    #[test]
    fn sync_call_arrays_empty_stack_unsets() {
        let mut sh = Shell::new();
        sh.call_stack.push(make_func_frame("f"));
        sh.sync_call_arrays();
        assert!(sh.get_indexed("FUNCNAME").is_some());
        sh.call_stack.pop();
        sh.sync_call_arrays();
        assert!(sh.get_indexed("FUNCNAME").is_none(), "empty stack unsets FUNCNAME");
        assert_eq!(sh.lookup_var("FUNCNAME"), None);
    }

    #[test]
    fn sync_call_arrays_single_frame() {
        let mut sh = Shell::new();
        sh.call_stack.push(make_func_frame("solo"));
        sh.sync_call_arrays();
        assert_eq!(sh.lookup_var("FUNCNAME"), Some("solo".to_string()));
        assert_eq!(sh.get_indexed("FUNCNAME").expect("array").len(), 1);
    }

    #[test]
    fn shell_clone_shares_functions_and_cow_isolates_defines() {
        let mut a = Shell::new();
        // Use the same minimal body shape as the builtins tests.
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        a.define_function("f".to_string(), body.clone());
        assert_eq!(Rc::strong_count(&a.functions), 1);
        let b = a.clone();
        // After clone both shells share the same Rc — O(1) clone, NOT a deep copy.
        assert_eq!(Rc::strong_count(&a.functions), 2);
        // COW: defining a new function in `a` must NOT affect `b`.
        a.define_function("g".to_string(), body);
        assert!(a.functions.contains_key("g"));
        assert!(!b.functions.contains_key("g")); // isolation preserved
        // After make_mut the two Rcs are now independent.
        assert_eq!(Rc::strong_count(&a.functions), 1);
    }

    #[test]
    fn shell_clone_shares_command_hash_history_completion_specs() {
        let mut a = Shell::new();
        let b = a.clone();
        // All three Rc fields are shared after clone — O(1), not deep copies.
        assert_eq!(Rc::strong_count(&a.command_hash), 2);
        assert_eq!(Rc::strong_count(&a.history), 2);
        assert_eq!(Rc::strong_count(&a.completion_specs), 2);

        // COW: a mutation on `a` must not affect `b`.
        Rc::make_mut(&mut a.command_hash).insert(
            "myls".to_string(),
            (std::path::PathBuf::from("/bin/ls"), 0),
        );
        assert!(a.command_hash.contains_key("myls"), "a should have myls");
        assert!(!b.command_hash.contains_key("myls"), "b must not see a's mutation");
        // After make_mut the two command_hash Rcs are now independent.
        assert_eq!(Rc::strong_count(&a.command_hash), 1);
        assert_eq!(Rc::strong_count(&b.command_hash), 1);
    }

    #[test]
    fn new_captures_inherited_env_as_exported() {
        let shell = Shell::new();
        // PATH is reliably present in test environments.
        assert!(shell.get("PATH").is_some(), "PATH should be inherited");
        let path_exported = shell.exported_env().any(|(k, _)| k == "PATH");
        assert!(path_exported);
    }

    #[test]
    fn set_creates_unexported_var() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_SET", "value".to_string());
        assert_eq!(shell.get("HUCK_TEST_SET"), Some("value"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_SET");
        assert!(!in_exported);
    }

    #[test]
    fn set_preserves_existing_exported_flag() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_KEEP", "v1".to_string());
        shell.set("HUCK_TEST_KEEP", "v2".to_string());
        assert_eq!(shell.get("HUCK_TEST_KEEP"), Some("v2"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_KEEP");
        assert!(in_exported);
    }

    #[test]
    fn export_marks_existing_exported() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_EX", "value".to_string());
        shell.export("HUCK_TEST_EX");
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_EX");
        assert!(in_exported);
    }

    #[test]
    fn export_creates_empty_when_missing() {
        let mut shell = Shell::new();
        shell.export("HUCK_TEST_EMPTY");
        assert_eq!(shell.get("HUCK_TEST_EMPTY"), Some(""));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_EMPTY");
        assert!(in_exported);
    }

    #[test]
    fn unset_removes_variable() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_REMOVE", "v".to_string());
        shell.unset("HUCK_TEST_REMOVE");
        assert_eq!(shell.get("HUCK_TEST_REMOVE"), None);
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_REMOVE");
        assert!(!in_exported);
    }

    #[test]
    fn unset_var_enclosing_local_pops_and_reveals() {
        let mut s = Shell::new();
        s.set("x", "midval".into());
        let mut outer = std::collections::HashMap::new();
        outer.insert("x".to_string(), None); // outer `local x` shadowed an unset global
        let mut mid = std::collections::HashMap::new();
        mid.insert("x".to_string(), Some(Variable::scalar("orig".into())));
        s.local_scopes.push(outer);
        s.local_scopes.push(mid);
        s.local_scopes.push(std::collections::HashMap::new()); // top frame (inner): no local x
        s.unset_var("x");
        assert_eq!(s.get("x"), Some("orig"));         // mid's snapshot revealed
        assert!(!s.local_scopes[1].contains_key("x")); // mid's snapshot popped
    }

    #[test]
    fn unset_var_top_frame_local_plain_removes() {
        let mut s = Shell::new();
        s.set("x", "v".into());
        let mut top = std::collections::HashMap::new();
        top.insert("x".to_string(), Some(Variable::scalar("orig".into())));
        s.local_scopes.push(top);
        s.unset_var("x");
        assert_eq!(s.get("x"), None);                 // value removed
        assert!(s.local_scopes[0].contains_key("x")); // snapshot KEPT (restores on return)
    }

    #[test]
    fn unset_var_no_frames_plain_removes() {
        let mut s = Shell::new();
        s.set("x", "v".into());
        s.unset_var("x");
        assert_eq!(s.get("x"), None);
    }

    #[test]
    fn last_status_round_trip() {
        let mut shell = Shell::new();
        assert_eq!(shell.last_status(), 0);
        shell.set_last_status(42);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn exported_env_excludes_unexported() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_HIDDEN", "v".to_string());
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_HIDDEN");
        assert!(!in_exported);
    }

    #[test]
    fn new_captures_shell_pgid_from_getpgrp() {
        let s = Shell::new();
        let expected = unsafe { libc::getpgrp() };
        assert_eq!(s.shell_pgid, expected);
        assert!(s.shell_pgid > 0, "pgrp should be positive");
    }

    #[test]
    fn new_initializes_sigint_flag_to_false() {
        let s = Shell::new();
        assert!(!s.sigint_flag.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn new_initializes_timeout_flag_to_false() {
        let s = Shell::new();
        assert!(!s.timeout_flag.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn new_initializes_live_external_children_empty() {
        let s = Shell::new();
        assert!(s.live_external_children.lock().unwrap().is_empty());
    }

    #[test]
    fn new_initializes_restricted_to_false() {
        let s = Shell::new();
        assert!(!s.restricted);
    }

    #[test]
    fn var_names_lists_all_variables() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_VN", "value".to_string());
        let names: Vec<&str> = shell.var_names().collect();
        assert!(names.contains(&"HUCK_TEST_VN"));
    }

    #[test]
    fn is_exported_unset_var_is_false() {
        let shell = Shell::new();
        assert!(!shell.is_exported("DEFINITELY_NOT_SET"));
    }

    #[test]
    fn is_exported_after_set_is_false() {
        let mut shell = Shell::new();
        shell.set("FOO", "bar".to_string());
        assert!(!shell.is_exported("FOO"));
    }

    #[test]
    fn is_exported_after_export_set_is_true() {
        let mut shell = Shell::new();
        shell.export_set("FOO", "bar".to_string());
        assert!(shell.is_exported("FOO"));
    }

    #[test]
    fn shell_new_caches_pid_and_argv0() {
        let shell = Shell::new();
        assert!(shell.shell_pid > 0, "shell_pid should be positive");
        assert!(!shell.shell_argv0.is_empty(), "shell_argv0 should be non-empty");
        assert_eq!(shell.last_bg_pid, None);
        assert!(shell.call_stack.is_empty());
    }

    #[test]
    fn lookup_var_dollar_returns_cached_pid_as_string() {
        let mut shell = Shell::new();
        shell.shell_pid = 12345;
        assert_eq!(shell.lookup_var("$"), Some("12345".to_string()));
    }

    #[test]
    fn lookup_var_bang_unset_returns_empty_string() {
        let shell = Shell::new();
        assert_eq!(shell.lookup_var("!"), Some(String::new()));
    }

    #[test]
    fn lookup_var_bang_after_set_returns_pid_string() {
        let mut shell = Shell::new();
        shell.last_bg_pid = Some(54321);
        assert_eq!(shell.lookup_var("!"), Some("54321".to_string()));
    }

    #[test]
    fn lookup_var_underscore_returns_last_arg() {
        let mut shell = Shell::new();
        // At startup `$_` mirrors the invocation path (shell_argv0).
        assert_eq!(shell.lookup_var("_"), Some(shell.shell_argv0.clone()));
        // After a command updates it, `$_` reflects the last argument.
        shell.last_arg = "world".to_string();
        assert_eq!(shell.lookup_var("_"), Some("world".to_string()));
        // `_` is always considered set (backs `${_-x}` / `[[ -v _ ]]`).
        assert!(shell.is_set("_"));
    }

    #[test]
    fn lookup_var_zero_top_level_returns_shell_argv0() {
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
    }

    #[test]
    fn lookup_var_zero_in_function_keeps_shell_argv0() {
        // bash: `$0` is NOT rebound on function entry — it stays the script /
        // shell invocation name, even nested. (Other shells differ; bash does not.)
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        shell.call_stack.push(make_func_frame("myfunc"));
        assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
    }

    #[test]
    fn lookup_var_zero_nested_keeps_shell_argv0() {
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        shell.call_stack.push(make_func_frame("outer"));
        shell.call_stack.push(make_func_frame("inner"));
        assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
        shell.call_stack.pop();
        assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
        shell.call_stack.pop();
        assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
    }

    #[test]
    fn should_hangup_skips_marked_and_done_jobs() {
        use crate::jobs::{JobState, JobTable};
        let mut t = JobTable::new();
        let id = t.add(0, vec![1234], "sleep 30".to_string());

        // Running + not marked → hangup
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(super::should_hangup(job));

        // Running + marked → skip
        t.mark_for_nohup(id);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(!super::should_hangup(job));

        // Done + not marked → skip
        t.jobs_mut()[0].marked_for_nohup = false;
        t.jobs_mut()[0].state = JobState::Done(0);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(!super::should_hangup(job));

        // Stopped + not marked → hangup (Stopped is "live" for SIGHUP purposes)
        t.jobs_mut()[0].marked_for_nohup = false;
        t.jobs_mut()[0].state = JobState::Stopped(::libc::SIGTSTP);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(super::should_hangup(job));
    }
}

#[cfg(test)]
mod array_value_tests {
    use super::*;

    #[test]
    fn scalar_view_returns_string_for_scalar() {
        let v = VarValue::Scalar("hello".to_string());
        assert_eq!(v.scalar_view(), "hello");
    }

    #[test]
    fn scalar_view_returns_element_zero_for_indexed() {
        let mut m = BTreeMap::new();
        m.insert(0, "first".to_string());
        m.insert(1, "second".to_string());
        let v = VarValue::Indexed(m);
        assert_eq!(v.scalar_view(), "first");
    }

    #[test]
    fn scalar_view_empty_for_indexed_without_zero() {
        let mut m = BTreeMap::new();
        m.insert(5, "x".to_string());
        let v = VarValue::Indexed(m);
        assert_eq!(v.scalar_view(), "");
    }

    #[test]
    fn scalar_view_empty_for_empty_indexed() {
        let v = VarValue::Indexed(BTreeMap::new());
        assert_eq!(v.scalar_view(), "");
    }

    #[test]
    fn variable_scalar_constructor_sets_defaults() {
        let v = Variable::scalar("x".to_string());
        assert!(!v.exported);
        assert!(!v.readonly);
        assert!(!v.integer);
        assert_eq!(v.value.scalar_view(), "x");
    }

    #[test]
    fn try_set_on_indexed_overwrites_element_zero_only() {
        let mut shell = Shell::new();
        let mut m = BTreeMap::new();
        m.insert(0, "old".to_string());
        m.insert(1, "x".to_string());
        shell.vars.insert(
            "a".to_string(),
            Variable {
                value: VarValue::Indexed(m),
                exported: false,
                readonly: false,
                integer: false,
                case_fold: None,
                nameref: false,
            },
        );
        let result = shell.try_set("a", "new".to_string());
        assert!(result.is_ok());
        match &shell.vars.get("a").unwrap().value {
            VarValue::Indexed(m) => {
                assert_eq!(m.get(&0).map(String::as_str), Some("new"));
                assert_eq!(m.get(&1).map(String::as_str), Some("x"));
            }
            _ => panic!("expected Indexed"),
        }
    }

    #[test]
    fn set_on_indexed_overwrites_element_zero_only() {
        let mut shell = Shell::new();
        let mut m = BTreeMap::new();
        m.insert(0, "old".to_string());
        m.insert(1, "x".to_string());
        shell.vars.insert(
            "a".to_string(),
            Variable {
                value: VarValue::Indexed(m),
                exported: false,
                readonly: false,
                integer: false,
                case_fold: None,
                nameref: false,
            },
        );
        shell.set("a", "new".to_string());
        match &shell.vars.get("a").unwrap().value {
            VarValue::Indexed(m) => {
                assert_eq!(m.get(&0).map(String::as_str), Some("new"));
                assert_eq!(m.get(&1).map(String::as_str), Some("x"));
            }
            _ => panic!("expected Indexed"),
        }
    }

    #[test]
    fn export_set_on_indexed_overwrites_element_zero_only_and_marks_exported() {
        let mut shell = Shell::new();
        let mut m = BTreeMap::new();
        m.insert(0, "old".to_string());
        m.insert(1, "x".to_string());
        shell.vars.insert(
            "a".to_string(),
            Variable {
                value: VarValue::Indexed(m),
                exported: false,
                readonly: false,
                integer: false,
                case_fold: None,
                nameref: false,
            },
        );
        shell.export_set("a", "new".to_string());
        let v = shell.vars.get("a").unwrap();
        assert!(v.exported);
        match &v.value {
            VarValue::Indexed(m) => {
                assert_eq!(m.get(&0).map(String::as_str), Some("new"));
                assert_eq!(m.get(&1).map(String::as_str), Some("x"));
            }
            _ => panic!("expected Indexed"),
        }
    }
}

#[cfg(test)]
mod assoc_value_tests {
    use super::*;

    #[test]
    fn scalar_view_returns_empty_for_associative() {
        let v = VarValue::Associative(vec![
            ("k1".to_string(), "v1".to_string()),
            ("k2".to_string(), "v2".to_string()),
        ]);
        assert_eq!(v.scalar_view(), "");
    }

    #[test]
    fn declare_associative_on_unset_creates_empty() {
        let mut shell = Shell::new();
        assert!(shell.declare_associative("m").is_ok());
        assert_eq!(shell.get_associative("m").map(Vec::len), Some(0));
    }

    #[test]
    fn declare_associative_on_existing_associative_is_noop() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".into(), "v".into()).unwrap();
        assert!(shell.declare_associative("m").is_ok());
        assert_eq!(shell.lookup_associative_element("m", "k"), Some("v".into()));
    }

    #[test]
    fn declare_associative_on_indexed_errors() {
        let mut shell = Shell::new();
        let mut m = BTreeMap::new();
        m.insert(0, "x".into());
        shell.vars.insert("a".into(), Variable {
            value: VarValue::Indexed(m),
            exported: false, readonly: false, integer: false, case_fold: None, nameref: false,
        });
        assert!(matches!(shell.declare_associative("a"), Err(DeclareErr::IndexedExists)));
    }

    #[test]
    fn declare_associative_on_scalar_errors() {
        let mut shell = Shell::new();
        shell.set("s", "hello".into());
        assert!(matches!(shell.declare_associative("s"), Err(DeclareErr::ScalarExists)));
    }

    #[test]
    fn declare_err_message_uses_command_name() {
        use super::declare_err_message;
        assert_eq!(
            declare_err_message("declare", "a", &DeclareErr::IndexedExists),
            "huck: declare: a: cannot convert indexed to associative array",
        );
        assert_eq!(
            declare_err_message("local", "s", &DeclareErr::ScalarExists),
            "huck: local: s: cannot convert scalar to associative array",
        );
        assert_eq!(
            declare_err_message("readonly", "s", &DeclareErr::ScalarExists),
            "huck: readonly: s: cannot convert scalar to associative array",
        );
    }

    #[test]
    fn set_associative_element_preserves_insertion_order_on_update() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "a".into(), "1".into()).unwrap();
        shell.set_associative_element("m", "b".into(), "2".into()).unwrap();
        shell.set_associative_element("m", "c".into(), "3".into()).unwrap();
        shell.set_associative_element("m", "a".into(), "999".into()).unwrap();
        let pairs = shell.get_associative("m").unwrap();
        assert_eq!(pairs[0], ("a".into(), "999".into()));
        assert_eq!(pairs[1], ("b".into(), "2".into()));
        assert_eq!(pairs[2], ("c".into(), "3".into()));
    }

    #[test]
    fn append_associative_element_concatenates() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".into(), "hello".into()).unwrap();
        shell.append_associative_element("m", "k", "_world").unwrap();
        assert_eq!(shell.lookup_associative_element("m", "k"), Some("hello_world".into()));
    }

    #[test]
    fn append_associative_element_creates_when_missing() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.append_associative_element("m", "new", "value").unwrap();
        assert_eq!(shell.lookup_associative_element("m", "new"), Some("value".into()));
    }

    #[test]
    fn unset_associative_element_removes_one_key() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "a".into(), "1".into()).unwrap();
        shell.set_associative_element("m", "b".into(), "2".into()).unwrap();
        shell.set_associative_element("m", "c".into(), "3".into()).unwrap();
        shell.unset_associative_element("m", "b").unwrap();
        let pairs = shell.get_associative("m").unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "a");
        assert_eq!(pairs[1].0, "c");
    }

    #[test]
    fn unset_associative_element_on_missing_key_is_noop() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "a".into(), "1".into()).unwrap();
        assert!(shell.unset_associative_element("m", "nope").is_ok());
        assert_eq!(shell.lookup_associative_element("m", "a"), Some("1".into()));
    }

    #[test]
    fn unset_associative_element_on_non_associative_is_noop() {
        let mut shell = Shell::new();
        shell.set("s", "hello".into());
        // Non-associative variable — should NOT modify it and NOT error.
        assert!(shell.unset_associative_element("s", "anything").is_ok());
        assert_eq!(shell.get("s"), Some("hello"));
    }

    #[test]
    fn replace_associative_overwrites() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "old".into(), "1".into()).unwrap();
        let new_pairs = vec![
            ("x".to_string(), "10".to_string()),
            ("y".to_string(), "20".to_string()),
        ];
        shell.replace_associative("m", new_pairs).unwrap();
        assert!(shell.lookup_associative_element("m", "old").is_none());
        assert_eq!(shell.lookup_associative_element("m", "x"), Some("10".into()));
        assert_eq!(shell.lookup_associative_element("m", "y"), Some("20".into()));
    }

    #[test]
    fn readonly_blocks_set_associative_element() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".into(), "v".into()).unwrap();
        shell.mark_readonly("m");
        assert!(matches!(
            shell.set_associative_element("m", "k2".into(), "v2".into()),
            Err(AssignErr::Readonly)
        ));
        assert!(shell.lookup_associative_element("m", "k2").is_none());
    }
}

#[cfg(test)]
mod ifs_helper_tests {
    use super::*;

    #[test]
    fn ifs_default_when_unset() {
        let s = Shell::new();
        assert_eq!(s.ifs(), " \t\n");
    }

    #[test]
    fn ifs_returns_set_value() {
        let mut s = Shell::new();
        s.set("IFS", ":".to_string());
        assert_eq!(s.ifs(), ":");
    }

    #[test]
    fn ifs_returns_empty_when_set_to_empty() {
        let mut s = Shell::new();
        s.set("IFS", "".to_string());
        assert_eq!(s.ifs(), "");
    }
}

#[cfg(test)]
mod shopt_tests {
    use super::*;

    #[test]
    fn shopt_table_has_57_entries() {
        assert_eq!(SHOPT_TABLE.len(), 57);
    }

    #[test]
    fn shopt_defaults_match_bash() {
        let o = ShoptOptions::default();
        // default-off
        assert_eq!(o.get("nullglob"), Some(false));
        assert_eq!(o.get("dotglob"), Some(false));
        assert_eq!(o.get("extglob"), Some(false));
        // default-on
        assert_eq!(o.get("checkwinsize"), Some(true));
        assert_eq!(o.get("interactive_comments"), Some(true));
        assert_eq!(o.get("sourcepath"), Some(true));
        // exactly 13 default-on
        assert_eq!(SHOPT_TABLE.iter().filter(|e| e.default).count(), 13);
        // unknown
        assert_eq!(o.get("bogus"), None);
    }

    #[test]
    fn shopt_set_and_read_back() {
        let mut o = ShoptOptions::default();
        assert!(o.set("nullglob", true));
        assert_eq!(o.get("nullglob"), Some(true));
        assert!(!o.set("bogus", true)); // unknown → false (not applied)
    }

    #[test]
    fn shell_glob_opts_reflects_shopt() {
        let mut shell = Shell::new();
        shell.shopt_options.set("nullglob", true);
        shell.shopt_options.set("dotglob", true);
        let g = shell.glob_opts();
        assert!(g.nullglob && g.dotglob && !g.nocaseglob && !g.failglob);
        assert!(!shell.nocasematch());
        shell.shopt_options.set("nocasematch", true);
        assert!(shell.nocasematch());
    }

    #[test]
    fn dollar_dash_includes_v_when_verbose() {
        let mut sh = Shell::new();
        assert!(!sh.dollar_dash_value().contains('v'));
        sh.shell_options.verbose = true;
        assert!(sh.dollar_dash_value().contains('v'));
    }

    #[test]
    fn dollar_dash_includes_x_when_xtrace() {
        let mut sh = Shell::new();
        assert!(!sh.dollar_dash_value().contains('x'));
        sh.shell_options.xtrace = true;
        assert!(sh.dollar_dash_value().contains('x'));
    }

    #[test]
    fn dollar_dash_x_after_v() {
        let mut sh = Shell::new();
        sh.shell_options.verbose = true;
        sh.shell_options.xtrace = true;
        let d = sh.dollar_dash_value();
        let v = d.find('v').unwrap();
        let x = d.find('x').unwrap();
        assert!(v < x, "expected v before x in {d:?}");
    }

    #[test]
    fn dollar_dash_v_after_u() {
        let mut sh = Shell::new();
        sh.shell_options.nounset = true;
        sh.shell_options.verbose = true;
        let d = sh.dollar_dash_value();
        assert!(d.find('u').unwrap() < d.find('v').unwrap(), "got {d:?}");
    }

    #[test]
    fn dollar_dash_c_after_x() {
        let mut sh = Shell::new();
        sh.shell_options.xtrace = true;
        sh.shell_options.noclobber = true;
        let d = sh.dollar_dash_value();
        let xi = d.find('x').expect("x present");
        let ci = d.find('C').expect("C present");
        assert!(ci > xi, "C must come after x in $-: got {d:?}");
    }

    #[test]
    fn noclobber_off_by_default() {
        let sh = Shell::new();
        assert!(!sh.shell_options.noclobber);
        assert!(!sh.dollar_dash_value().contains('C'));
    }

    #[test]
    fn noclobber_shows_in_dollar_dash() {
        let mut sh = Shell::new();
        sh.shell_options.noclobber = true;
        assert!(sh.dollar_dash_value().contains('C'));
    }

    #[test]
    fn resolve_histsize_bash_semantics() {
        let mut s = Shell::new();
        assert_eq!(s.resolve_histsize(), Some(1000)); // unset -> default
        s.set("HISTSIZE", "".to_string());
        assert_eq!(s.resolve_histsize(), Some(1000)); // empty -> default
        s.set("HISTSIZE", "abc".to_string());
        assert_eq!(s.resolve_histsize(), Some(1000)); // non-numeric -> default
        s.set("HISTSIZE", "0".to_string());
        assert_eq!(s.resolve_histsize(), Some(0)); // zero -> empty
        s.set("HISTSIZE", "200".to_string());
        assert_eq!(s.resolve_histsize(), Some(200)); // positive -> cap
        s.set("HISTSIZE", "-1".to_string());
        assert_eq!(s.resolve_histsize(), None); // negative -> unlimited
    }

    #[test]
    fn resolve_histfilesize_bash_semantics() {
        let mut s = Shell::new();
        s.set("HISTSIZE", "200".to_string());
        assert_eq!(s.resolve_histfilesize(), Some(200)); // unset -> effective HISTSIZE
        s.set("HISTFILESIZE", "50".to_string());
        assert_eq!(s.resolve_histfilesize(), Some(50)); // positive -> cap
        s.set("HISTFILESIZE", "0".to_string());
        assert_eq!(s.resolve_histfilesize(), Some(0)); // zero -> empty file
        s.set("HISTFILESIZE", "-1".to_string());
        assert_eq!(s.resolve_histfilesize(), None); // negative -> inhibit
        s.set("HISTFILESIZE", "abc".to_string());
        assert_eq!(s.resolve_histfilesize(), None); // non-numeric -> inhibit
    }

    #[test]
    fn apply_case_fold_lower_upper_and_none() {
        assert_eq!(apply_case_fold(None, "AbC".to_string()), "AbC");
        assert_eq!(apply_case_fold(Some(CaseFold::Lower), "AbC".to_string()), "abc");
        assert_eq!(apply_case_fold(Some(CaseFold::Upper), "AbC".to_string()), "ABC");
        // idempotent
        assert_eq!(apply_case_fold(Some(CaseFold::Lower), "abc".to_string()), "abc");
    }

    #[test]
    fn storage_mutators_apply_case_fold() {
        let mut shell = Shell::new();

        // scalar via try_set
        shell.set_case_fold("s", Some(CaseFold::Lower));
        shell.try_set("s", "ABCdef".to_string()).unwrap();
        assert_eq!(shell.get("s"), Some("abcdef"));

        // scalar += (try_set with concatenated value) folds the whole result
        shell.try_set("s", "abcdef".to_string() + "GHI").unwrap();
        assert_eq!(shell.get("s"), Some("abcdefghi"));

        // indexed element
        shell.set_case_fold("arr", Some(CaseFold::Lower));
        shell.set_indexed_element("arr", 1, "XYZ".to_string()).unwrap();
        assert_eq!(shell.lookup_indexed_element("arr", 1).as_deref(), Some("xyz"));

        // associative value folded, key NOT folded
        // must declare as associative first (set_case_fold creates a Scalar)
        shell.declare_associative("m").unwrap();
        shell.set_case_fold("m", Some(CaseFold::Lower));
        shell.set_associative_element("m", "Key".to_string(), "VALUE".to_string()).unwrap();
        assert_eq!(shell.get_associative("m").unwrap().iter()
            .find(|(k, _)| k == "Key").map(|(_, v)| v.as_str()), Some("value"));

        // whole-array literal via replace_indexed, attribute preserved
        shell.set_case_fold("lit", Some(CaseFold::Lower));
        let mut map = std::collections::BTreeMap::new();
        map.insert(0usize, "ABC".to_string());
        map.insert(1usize, "DeF".to_string());
        shell.replace_indexed("lit", map).unwrap();
        assert_eq!(shell.lookup_indexed_element("lit", 0).as_deref(), Some("abc"));
        assert_eq!(shell.lookup_indexed_element("lit", 1).as_deref(), Some("def"));
        assert_eq!(shell.case_fold_of("lit"), Some(CaseFold::Lower)); // preserved

        // upper attribute through array append (extend_indexed)
        shell.set_case_fold("app", Some(CaseFold::Upper));
        let mut em = std::collections::BTreeMap::new();
        em.insert(0usize, "abc".to_string());
        shell.extend_indexed("app", em).unwrap();
        assert_eq!(shell.lookup_indexed_element("app", 0).as_deref(), Some("ABC"));

        // whole associative-array literal via replace_associative, attribute preserved
        shell.declare_associative("am").unwrap();
        shell.set_case_fold("am", Some(CaseFold::Upper));
        shell.replace_associative("am", vec![("k".to_string(), "abc".to_string())]).unwrap();
        assert_eq!(
            shell.get_associative("am").unwrap().iter()
                .find(|(k, _)| k == "k").map(|(_, v)| v.as_str()),
            Some("ABC")
        );
        assert_eq!(shell.case_fold_of("am"), Some(CaseFold::Upper)); // preserved
    }

    #[test]
    fn set_case_fold_creates_and_clears() {
        let mut shell = Shell::new();
        // create-if-absent, like mark_integer
        shell.set_case_fold("x", Some(CaseFold::Lower));
        assert_eq!(shell.case_fold_of("x"), Some(CaseFold::Lower));
        // overwrite (later-wins mutual exclusivity is handled by the caller)
        shell.set_case_fold("x", Some(CaseFold::Upper));
        assert_eq!(shell.case_fold_of("x"), Some(CaseFold::Upper));
        // clear
        shell.set_case_fold("x", None);
        assert_eq!(shell.case_fold_of("x"), None);
        // unknown var reads None
        assert_eq!(shell.case_fold_of("nope"), None);
    }

    // ── v159 Task 4: funnel-uniformity unit tests ────────────────────────────

    /// Proves that assign() applies case-fold on every storage path:
    /// scalar whole-variable, indexed element, and whole indexed-array literal.
    #[test]
    fn assign_funnel_applies_case_fold_on_every_path() {
        let mut shell = Shell::new();

        // scalar whole-variable path
        shell.set_case_fold("s", Some(CaseFold::Upper));
        shell.assign(
            AssignDest::Whole("s".into()),
            AssignKind::Set,
            AssignSource::Scalar("abc".into()),
        ).unwrap();
        assert_eq!(shell.get("s"), Some("ABC"));

        // indexed element path
        shell.set_case_fold("a", Some(CaseFold::Upper));
        shell.assign(
            AssignDest::Element { name: "a".into(), sub: Subscript::Index(2) },
            AssignKind::Set,
            AssignSource::Scalar("xy".into()),
        ).unwrap();
        assert_eq!(shell.lookup_indexed_element("a", 2).as_deref(), Some("XY"));

        // whole indexed-array literal path
        let mut m = std::collections::BTreeMap::new();
        m.insert(0usize, "lo".to_string());
        shell.set_case_fold("b", Some(CaseFold::Upper));
        shell.assign(
            AssignDest::Whole("b".into()),
            AssignKind::Set,
            AssignSource::Indexed(m),
        ).unwrap();
        assert_eq!(shell.lookup_indexed_element("b", 0).as_deref(), Some("LO"));
    }

    /// L-49: an integer-flagged array arith-coerces element VALUES on every
    /// storage path (whole indexed literal, indexed element, whole associative
    /// literal, associative element); a non-integer array stays literal.
    #[test]
    fn assign_funnel_integer_coerces_array_values() {
        let mut shell = Shell::new();

        // whole indexed-array literal coerces each value
        shell.mark_integer("a");
        let mut m = std::collections::BTreeMap::new();
        m.insert(0usize, "2+3".to_string());
        m.insert(1usize, "4*5".to_string());
        shell.assign(
            AssignDest::Whole("a".into()),
            AssignKind::Set,
            AssignSource::Indexed(m),
        ).unwrap();
        assert_eq!(shell.lookup_indexed_element("a", 0).as_deref(), Some("5"));
        assert_eq!(shell.lookup_indexed_element("a", 1).as_deref(), Some("20"));
        assert!(shell.is_integer("a")); // flag survives the replace

        // indexed element coerces
        shell.assign(
            AssignDest::Element { name: "a".into(), sub: Subscript::Index(2) },
            AssignKind::Set,
            AssignSource::Scalar("6/2".into()),
        ).unwrap();
        assert_eq!(shell.lookup_indexed_element("a", 2).as_deref(), Some("3"));

        // whole associative literal coerces VALUES (not keys)
        shell.declare_associative("m").unwrap();
        shell.mark_integer("m");
        shell.assign(
            AssignDest::Whole("m".into()),
            AssignKind::Set,
            AssignSource::Associative(vec![("x".into(), "2+3".into())]),
        ).unwrap();
        assert_eq!(
            shell.get_associative("m").unwrap().iter()
                .find(|(k, _)| k == "x").map(|(_, v)| v.as_str()),
            Some("5")
        );
        assert!(shell.is_integer("m")); // flag survives the assoc replace

        // associative element coerces
        shell.assign(
            AssignDest::Element { name: "m".into(), sub: Subscript::Key("k".into()) },
            AssignKind::Set,
            AssignSource::Scalar("10-1".into()),
        ).unwrap();
        assert_eq!(
            shell.get_associative("m").unwrap().iter()
                .find(|(k, _)| k == "k").map(|(_, v)| v.as_str()),
            Some("9")
        );

        // non-integer array stays literal
        let mut m2 = std::collections::BTreeMap::new();
        m2.insert(0usize, "2+3".to_string());
        shell.assign(
            AssignDest::Whole("plain".into()),
            AssignKind::Set,
            AssignSource::Indexed(m2),
        ).unwrap();
        assert_eq!(shell.lookup_indexed_element("plain", 0).as_deref(), Some("2+3"));
    }

    /// Proves that assign() enforces readonly on every write path (scalar).
    #[test]
    fn assign_funnel_readonly_blocks_all_paths() {
        let mut shell = Shell::new();
        shell.try_set("r", "init".into()).unwrap();
        shell.mark_readonly("r");
        assert!(
            shell.assign(
                AssignDest::Whole("r".into()),
                AssignKind::Set,
                AssignSource::Scalar("x".into()),
            ).is_err()
        );
        assert_eq!(shell.get("r"), Some("init")); // value unchanged
    }

    #[test]
    fn resolve_nameref_covers_plain_chain_cycle_element_unbound() {
        let mut shell = Shell::new();
        // plain (not a nameref) → itself
        assert_eq!(shell.resolve_nameref("x"), ResolvedName::Name("x".into()));
        // single hop r -> x
        shell.set_nameref("r", true);
        shell.set("r", "x".into()); // store target name
        assert_eq!(shell.resolve_nameref("r"), ResolvedName::Name("x".into()));
        // chain a -> b -> c
        shell.set_nameref("a", true); shell.set("a", "b".into());
        shell.set_nameref("b", true); shell.set("b", "c".into());
        assert_eq!(shell.resolve_nameref("a"), ResolvedName::Name("c".into()));
        // element target e -> arr[2]
        shell.set_nameref("e", true); shell.set("e", "arr[2]".into());
        assert_eq!(shell.resolve_nameref("e"), ResolvedName::Element { name: "arr".into(), subscript: "2".into() });
        // unbound u (attribute set, empty value)
        shell.set_nameref("u", true);
        assert_eq!(shell.resolve_nameref("u"), ResolvedName::Unbound("u".into()));
        // cycle p -> q -> p
        shell.set_nameref("p", true); shell.set("p", "q".into());
        shell.set_nameref("q", true); shell.set("q", "p".into());
        assert_eq!(shell.resolve_nameref("p"), ResolvedName::Cycle);
    }

    #[test]
    fn readline_settings_set_and_list() {
        let mut shell = Shell::new();
        // default seeded vars present
        assert_eq!(shell.readline_settings.vars.get("editing-mode").map(String::as_str), Some("emacs"));
        // set a mapped var
        shell.set_readline_var("editing-mode", "vi");
        assert_eq!(shell.readline_settings.vars.get("editing-mode").map(String::as_str), Some("vi"));
        assert!(shell.readline_settings.dirty);
        // -v listing form
        let lines = shell.readline_var_lines();
        assert!(lines.iter().any(|l| l == "set editing-mode vi"));
        assert!(lines.iter().any(|l| l == "set bell-style audible"));
        // record a binding + list it
        shell.add_bind("\"\\C-x\"", "kill-line");
        assert_eq!(shell.readline_settings.pending_binds, vec![("\"\\C-x\"".to_string(), "kill-line".to_string())]);
    }

    #[test]
    fn error_prefix_noninteractive_script_with_line_and_cmd() {
        let mut sh = Shell::new();
        sh.is_interactive = false;
        sh.shell_argv0 = "./arith.tests".to_string();
        sh.current_lineno = 168;
        assert_eq!(sh.error_prefix(None), "./arith.tests: line 168: ");
        assert_eq!(sh.error_prefix(Some("let")), "./arith.tests: line 168: let: ");
        assert_eq!(sh.error_prefix(Some("((")), "./arith.tests: line 168: ((: ");
    }

    #[test]
    fn error_prefix_interactive_keeps_huck_no_line() {
        let mut sh = Shell::new();
        sh.is_interactive = true;
        sh.shell_argv0 = "huck".to_string();
        sh.current_lineno = 5;
        assert_eq!(sh.error_prefix(None), "huck: ");
        assert_eq!(sh.error_prefix(Some("((")), "huck: ((: ");
    }

    #[test]
    fn error_prefix_prefers_bash_source_zero() {
        let mut sh = Shell::new();
        sh.is_interactive = false;
        sh.shell_argv0 = "huck".to_string();
        sh.current_lineno = 3;
        sh.seed_array_for_tests("BASH_SOURCE", &[(0, "./sourced.sh")]);
        assert_eq!(sh.error_prefix(None), "./sourced.sh: line 3: ");
    }
}
