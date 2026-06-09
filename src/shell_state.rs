use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::completion_spec::{CompletionSpec, CompletionSpecs};
use crate::jobs::JobTable;

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

#[derive(Debug, Clone)]
pub struct Variable {
    pub value: VarValue,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,
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
        }
    }
}

/// Error kind returned by the readonly-aware array mutator helpers
/// (`replace_array`, `set_array_element`, `extend_indexed`,
/// `append_array_element`, `unset_array_element`). The mutator prints
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

/// Per-session shell state: variables (each either exported or not) and the
/// last command's exit status. The initial set of variables is seeded from
/// the process environment huck inherited at startup, every one marked
/// exported.
#[derive(Debug, Clone)]
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
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
    pub functions: HashMap<String, Box<crate::command::Command>>,
    /// User-defined aliases. `name` → expansion text. Populated by
    /// the `alias` builtin; consumed by `expand_aliases_in_tokens`
    /// during interactive REPL input.
    pub aliases: std::collections::HashMap<String, String>,
    #[allow(dead_code)]
    pub jobs: JobTable,
    pub sigchld_flag: Arc<AtomicBool>,
    pub sigint_flag: Arc<AtomicBool>,
    pub shell_pgid: i32,
    pub history: crate::history::History,
    /// Shell PID, cached at startup via `getpid()`. Used for `$$`.
    pub shell_pid: i32,
    /// PID of the most-recently-backgrounded pipeline's last stage. Used for `$!`.
    pub last_bg_pid: Option<i32>,
    /// The shell's argv[0], cached at startup. Used for `$0` at the top level.
    pub shell_argv0: String,
    /// Stack of function names pushed/popped around each `call_function`.
    /// `$0` returns the top of this stack when inside a function.
    pub function_arg0: Vec<String>,
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
    pub command_hash: std::collections::HashMap<String, (std::path::PathBuf, u32)>,

    /// Directory stack maintained by the `pushd`/`popd`/`dirs`
    /// builtins. Top is index 0 — always synced with `$PWD` at
    /// the top of each pushd/popd/dirs call.
    pub dir_stack: Vec<std::path::PathBuf>,

    /// Programmable-completion registry (filled by the `complete` builtin).
    pub completion_specs: CompletionSpecs,
    /// Ephemeral slot used by `compopt` inside a `-F` function to mutate
    /// the live spec. Set by `dispatch::resolve` before invoking `-F`;
    /// taken back out afterward.
    pub current_completion_spec: Option<CompletionSpec>,
}

impl Shell {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        for (key, value) in std::env::vars() {
            vars.insert(key, Variable {
                value: VarValue::Scalar(value),
                exported: true,
                readonly: false,
                integer: false,
            });
        }
        let shell_pid = unsafe { libc::getpid() };
        let shell_argv0 = std::env::args().next().unwrap_or_else(|| "huck".to_string());
        let shell = Self {
            vars,
            last_status: 0,
            positional_args: Vec::new(),
            getopts_sp: 0,
            getopts_optind_cache: 0,
            functions: HashMap::new(),
            aliases: std::collections::HashMap::new(),
            jobs: JobTable::new(),
            sigchld_flag: Arc::new(AtomicBool::new(false)),
            sigint_flag: Arc::new(AtomicBool::new(false)),
            shell_pgid: unsafe { libc::getpgrp() },
            history: crate::history::History::new(),
            shell_pid,
            last_bg_pid: None,
            shell_argv0,
            function_arg0: Vec::new(),
            pending_fatal_pe_error: None,
            is_interactive: std::io::stdin().is_terminal(),
            in_subshell: false,
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
            command_hash: std::collections::HashMap::new(),
            dir_stack: Vec::new(),
            completion_specs: CompletionSpecs::default(),
            current_completion_spec: None,
        };
        // Make the trap_pending Arc visible to async-signal-safe
        // signal handlers installed by the traps module.
        crate::traps::init_pending_bitmask(std::sync::Arc::clone(&shell.trap_pending));
        shell
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|v| v.value.scalar_view())
    }

    /// Returns the value of `$-` — alphabetical concatenation of
    /// short-flag letters reflecting current shell-options state
    /// and the interactive flag. Order: `e` (errexit), `i`
    /// (interactive), `u` (nounset), `v` (verbose).
    pub fn dollar_dash_value(&self) -> String {
        let mut out = String::new();
        if self.shell_options.errexit { out.push('e'); }
        if self.shell_options.noglob { out.push('f'); }
        if self.is_interactive { out.push('i'); }
        if self.shell_options.nounset { out.push('u'); }
        if self.shell_options.verbose { out.push('v'); }
        if self.shell_options.xtrace { out.push('x'); }
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
        }
    }

    /// True when `shopt -s nocasematch` is in effect.
    pub fn nocasematch(&self) -> bool {
        self.shopt_options.get("nocasematch").unwrap_or(false)
    }

    /// Variable lookup for expansion. Recognises positional names
    /// (`"1"`-`"9"`/`"10"`/..., and `"#"`) before falling back to the
    /// regular variable HashMap. Returns an owned `String` because
    /// positional/computed values are not stored as references.
    pub fn lookup_var(&self, name: &str) -> Option<String> {
        // Special parameters (v26).
        match name {
            "0" => return Some(
                self.function_arg0.last().cloned().unwrap_or_else(|| self.shell_argv0.clone())
            ),
            "$" => return Some(self.shell_pid.to_string()),
            "!" => return Some(
                // Returns "" not None when unset: bash expands $! to empty before
                // any background has happened (v26 spec §lookup_var changes).
                self.last_bg_pid.map(|p| p.to_string()).unwrap_or_default()
            ),
            "-" => return Some(self.dollar_dash_value()),
            "?" => return Some(self.last_status().to_string()),
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
        self.vars.get(name).map(|v| v.value.scalar_view().to_string())
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

    /// Sets a variable's value, preserving its existing `exported` flag (or
    /// creating it as unexported if it didn't exist). When the existing
    /// value is an `Indexed` array, only element 0 is overwritten — the
    /// rest of the map is preserved (matches bash: `a=v` on an indexed
    /// array overwrites a[0]).
    pub fn set(&mut self, name: &str, value: String) {
        match self.vars.get_mut(name) {
            Some(existing) => install_scalar_value(existing, value),
            None => {
                self.vars.insert(name.to_string(), Variable::scalar(value));
            }
        }
    }

    /// True if the named variable/parameter is currently **set** (a
    /// set-but-empty variable counts as set; unset is false). Backs
    /// `[[ -v NAME ]]` and `test -v NAME`. Supports scalar names and
    /// positional parameters; array-element forms (`arr[i]`) are out of
    /// scope (M-14b) and fall through to a plain-name lookup (→ false).
    pub fn is_set(&self, name: &str) -> bool {
        // Always-defined special parameters.
        match name {
            "0" | "$" | "#" | "-" | "?" => return true,
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
        self.vars.contains_key(name)
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
            });
    }

    /// Sets a variable's value AND marks it exported. Preserves the
    /// `readonly` flag on an existing entry — callers that need to
    /// reject writes to readonly vars must check `is_readonly` first
    /// (see `builtin_export` and `apply_inline_assignments`).
    pub fn export_set(&mut self, name: &str, value: String) {
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

    /// Checked write: refuses to overwrite a readonly variable. Returns
    /// `Err(())` if `name` is readonly (caller prints the diagnostic);
    /// otherwise sets the value and returns `Ok(())`. Consumed by
    /// executor/expansion write paths in v54 task 2.
    ///
    /// When `name` is integer-flagged (v65), the RHS is routed through
    /// `arith::parse` + `arith::eval` and stored as the decimal string.
    /// Parse/eval failures silently coerce to `"0"` (matches bash for
    /// non-`declare` integer write paths).
    pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
        if self.is_readonly(name) {
            return Err(());
        }
        // Determine whether the integer-coerce path applies: only on
        // an existing integer-flagged Scalar. Indexed variables (even
        // if integer-flagged) take the array-element-0 overwrite path
        // instead.
        let do_integer_coerce = self.is_integer(name)
            && self
                .vars
                .get(name)
                .is_some_and(|v| matches!(v.value, VarValue::Scalar(_)));
        if do_integer_coerce {
            // Compute the coerced value BEFORE taking the &mut borrow.
            let coerced = eval_integer_coerce(self, &value);
            if let Some(existing) = self.vars.get_mut(name) {
                existing.value = VarValue::Scalar(coerced);
            }
            return Ok(());
        }
        // Non-integer (or no existing entry) path: route through `set`,
        // except that an existing Indexed variable has element 0
        // overwritten rather than the whole array being replaced
        // (matches bash: `a=v` on an indexed array overwrites a[0]).
        if let Some(existing) = self.vars.get_mut(name) {
            install_scalar_value(existing, value);
            Ok(())
        } else {
            self.vars.insert(name.to_string(), Variable::scalar(value));
            Ok(())
        }
    }

    /// Checked unset: refuses to remove a readonly variable. Returns
    /// `Err(())` if `name` is readonly; otherwise removes and returns
    /// `Ok(())`. Reserved for future read-only-aware unset call sites.
    #[allow(dead_code)]
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

    /// Returns a reference to the indexed array stored under `name`,
    /// or `None` if the variable is unset or a scalar.
    pub fn get_array(&self, name: &str) -> Option<&BTreeMap<usize, String>> {
        match self.vars.get(name) {
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
    pub fn lookup_array_element(&self, name: &str, idx: usize) -> Option<String> {
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
        self.get_array(name)
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
            },
        );
    }

    /// Replaces (or creates) `name` as an indexed array with the given
    /// elements. Honors readonly. Preserves the existing `exported` and
    /// `integer` flags if the variable already exists.
    pub fn replace_array(
        &mut self,
        name: &str,
        elements: BTreeMap<usize, String>,
    ) -> Result<(), AssignErr> {
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            eprintln!("huck: {name}: readonly variable");
            return Err(AssignErr::Readonly);
        }
        let (exported, integer) = match self.vars.get(name) {
            Some(v) => (v.exported, v.integer),
            None => (false, false),
        };
        self.vars.insert(
            name.to_string(),
            Variable {
                value: VarValue::Indexed(elements),
                exported,
                readonly: false,
                integer,
            },
        );
        Ok(())
    }

    /// Sets a single element. Promotes a scalar variable to indexed
    /// (the existing scalar value becomes element 0, unless `idx == 0`
    /// in which case it is overwritten). Honors readonly.
    pub fn set_array_element(
        &mut self,
        name: &str,
        idx: usize,
        value: String,
    ) -> Result<(), AssignErr> {
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            eprintln!("huck: {name}: readonly variable");
            return Err(AssignErr::Readonly);
        }
        match self.vars.get_mut(name) {
            Some(v) => match &mut v.value {
                VarValue::Indexed(m) => {
                    m.insert(idx, value);
                }
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
                    eprintln!(
                        "huck: {name}: set_array_element on associative variable"
                    );
                    return Err(AssignErr::TypeMismatch);
                }
            },
            None => {
                let mut m = BTreeMap::new();
                m.insert(idx, value);
                self.vars.insert(
                    name.to_string(),
                    Variable {
                        value: VarValue::Indexed(m),
                        exported: false,
                        readonly: false,
                        integer: false,
                    },
                );
            }
        }
        Ok(())
    }

    /// Merges explicit `(index → value)` entries into the named indexed
    /// array, creating it if missing and promoting a scalar to element 0
    /// first. Honors readonly (callers should pre-check to avoid a partial
    /// write; this re-checks defensively). Used by `a+=(elements)` after the
    /// elements are field-expanded with continuation indices already
    /// computed. Appending to an associative array is a type error.
    pub fn extend_indexed(
        &mut self,
        name: &str,
        entries: BTreeMap<usize, String>,
    ) -> Result<(), AssignErr> {
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            eprintln!("huck: {name}: readonly variable");
            return Err(AssignErr::Readonly);
        }
        // Promote scalar to indexed (scalar becomes element 0).
        if matches!(
            self.vars.get(name).map(|v| &v.value),
            Some(VarValue::Scalar(_))
        ) && let Some(v) = self.vars.get_mut(name)
            && let VarValue::Scalar(s) = &mut v.value
        {
            let mut m = BTreeMap::new();
            m.insert(0, std::mem::take(s));
            v.value = VarValue::Indexed(m);
        }
        if !self.vars.contains_key(name) {
            self.vars.insert(
                name.to_string(),
                Variable {
                    value: VarValue::Indexed(BTreeMap::new()),
                    exported: false,
                    readonly: false,
                    integer: false,
                },
            );
        }
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Indexed(m) = &mut v.value
        {
            for (idx, val) in entries {
                m.insert(idx, val);
            }
            Ok(())
        } else {
            eprintln!("huck: {name}: cannot append array literal to associative array");
            Err(AssignErr::TypeMismatch)
        }
    }

    /// Appends `value` to the existing element at `idx` (concatenation).
    /// Used by `a[i]+=v`. If the element doesn't exist, treats prior
    /// value as empty.
    pub fn append_array_element(
        &mut self,
        name: &str,
        idx: usize,
        value: &str,
    ) -> Result<(), AssignErr> {
        let existing = self.lookup_array_element(name, idx).unwrap_or_default();
        self.set_array_element(name, idx, existing + value)
    }

    /// Removes a single element from an indexed array. No-op if the
    /// variable is missing, scalar, or doesn't contain that subscript.
    /// Honors readonly.
    pub fn unset_array_element(&mut self, name: &str, idx: usize) -> Result<(), AssignErr> {
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            eprintln!("huck: {name}: readonly variable");
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
    pub fn get_associative(&self, name: &str) -> Option<&Vec<(String, String)>> {
        match self.vars.get(name) {
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
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            eprintln!("huck: {name}: readonly variable");
            return Err(AssignErr::Readonly);
        }
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
                    eprintln!("huck: {name}: set_associative_element on non-associative variable");
                    return Err(AssignErr::TypeMismatch);
                }
            },
            None => {
                eprintln!("huck: {name}: set_associative_element on unset variable");
                return Err(AssignErr::TypeMismatch);
            }
        }
        Ok(())
    }

    /// `m[k]+=v` — concatenate `value` to the existing element at `key`,
    /// or set to `value` if no such key. Honors readonly.
    pub fn append_associative_element(
        &mut self,
        name: &str,
        key: &str,
        value: &str,
    ) -> Result<(), AssignErr> {
        let existing = self.lookup_associative_element(name, key).unwrap_or_default();
        self.set_associative_element(name, key.to_string(), existing + value)
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
            eprintln!("huck: {name}: readonly variable");
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
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            eprintln!("huck: {name}: readonly variable");
            return Err(AssignErr::Readonly);
        }
        let exported = self.vars.get(name).map(|v| v.exported).unwrap_or(false);
        self.vars.insert(
            name.to_string(),
            Variable {
                value: VarValue::Associative(pairs),
                exported,
                readonly: false,
                // bash does not support `declare -Ai` (integer associative arrays); drop the flag.
                integer: false,
            },
        );
        Ok(())
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
            unsafe {
                libc::killpg(job.pgid, libc::SIGCONT);
                libc::killpg(job.pgid, libc::SIGHUP);
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

/// Installs `value` as the scalar value of `existing`, preserving the
/// rest of an `Indexed` map (writing only element 0). Shared by
/// `Shell::set`, `Shell::export_set`, and `Shell::try_set`'s non-
/// integer path so the three callers stay in lockstep on the
/// "scalar assignment to an array overwrites a[0]" rule (matches
/// bash: `a=v` on an indexed array overwrites a[0]).
fn install_scalar_value(existing: &mut Variable, value: String) {
    match &mut existing.value {
        VarValue::Indexed(m) => {
            m.insert(0, value);
        }
        VarValue::Scalar(_) => {
            existing.value = VarValue::Scalar(value);
        }
        VarValue::Associative(_) => {
            eprintln!("huck: internal: install_scalar_value on associative array");
        }
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
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
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        sh.replace_array("arr", elements).unwrap(); // existing pub Shell method (indexed array)
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
        let arr = sh.get_array("PIPESTATUS").expect("PIPESTATUS array");
        assert_eq!(arr.get(&0).map(String::as_str), Some("0"));
        assert_eq!(arr.get(&1).map(String::as_str), Some("1"));
        assert_eq!(arr.get(&2).map(String::as_str), Some("0"));
        assert_eq!(arr.len(), 3);
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
        assert!(shell.function_arg0.is_empty());
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
    fn lookup_var_zero_top_level_returns_shell_argv0() {
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
    }

    #[test]
    fn lookup_var_zero_in_function_returns_function_name() {
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        shell.function_arg0.push("myfunc".to_string());
        assert_eq!(shell.lookup_var("0"), Some("myfunc".to_string()));
    }

    #[test]
    fn lookup_var_zero_nested_returns_innermost() {
        let mut shell = Shell::new();
        shell.function_arg0.push("outer".to_string());
        shell.function_arg0.push("inner".to_string());
        assert_eq!(shell.lookup_var("0"), Some("inner".to_string()));
        shell.function_arg0.pop();
        assert_eq!(shell.lookup_var("0"), Some("outer".to_string()));
        shell.function_arg0.pop();
        assert!(shell.lookup_var("0").is_some());  // falls through to shell_argv0
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
            exported: false, readonly: false, integer: false,
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
}
