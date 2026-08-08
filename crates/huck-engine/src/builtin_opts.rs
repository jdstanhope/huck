//! One option scanner for the builtins, modelled on bash's `internal_getopt`.
//!
//! Owns the whole contract, measured against bash 5.2.21: bundled shorts
//! (order-independent), `--` as terminator, a lone `-` as an OPERAND, values
//! attached (`-n3`) or separate (`-n 3`), and scanning that STOPS at the first
//! non-option (POSIX — no permutation).
//!
//! Each builtin keeps its own `match` on the option character. What it loses is
//! the scanning, which is where 23 hand-rolled copies drifted apart (#496).
//!
//! TEMPORARY: nothing outside this module calls into it yet — Task 1 lands it
//! standalone per the plan, and Tasks 4-7 (#496) convert the builtins onto it.
//! Until the first conversion lands, `cargo clippy --all-targets` (which
//! compiles the non-test `lib` target as well as the `#[cfg(test)]` one)
//! finds every item in this file unreachable outside tests: both structs, both
//! enum variants, and all 9 methods/functions — 14 items total, i.e. the
//! entire module. A per-item `#[allow(dead_code)]` on each would be strictly
//! worse than this one module-scoped allow (more lines, same coverage, and no
//! finer-grained removability: nothing here has a partial caller yet, so
//! Task 4 makes most of the surface live in one commit regardless of how the
//! suppression is spelled today). Delete this attribute once a builtin calls
//! `Getopt`.
#![allow(dead_code)]

use crate::command::DeclArg;
use crate::shell_state::Shell;
use std::io::Write;

pub(crate) struct Opt {
    pub ch: char,
    pub value: Option<String>,
}

/// Declaration builtins receive `&[DeclArg]`; everything else `&[String]`. A
/// `DeclArg::Assign` can never be an option, so it terminates scanning exactly
/// like a non-option string does.
pub(crate) enum ArgView<'a> {
    Plain(&'a [String]),
    Decl(&'a [DeclArg]),
}

impl ArgView<'_> {
    fn len(&self) -> usize {
        match self {
            ArgView::Plain(v) => v.len(),
            ArgView::Decl(v) => v.len(),
        }
    }
    /// `None` when the slot cannot be an option (a compound assignment).
    fn at(&self, i: usize) -> Option<&str> {
        match self {
            ArgView::Plain(v) => v.get(i).map(|s| s.as_str()),
            ArgView::Decl(v) => match v.get(i) {
                Some(DeclArg::Plain(s)) => Some(s.as_str()),
                _ => None,
            },
        }
    }
}

pub(crate) struct Getopt<'a> {
    name: &'a str,
    args: ArgView<'a>,
    spec: &'a str,
    idx: usize,
    /// Byte offset within the current bundled cluster, 0 when not inside one.
    ch: usize,
    done: bool,
}

impl<'a> Getopt<'a> {
    pub fn new(name: &'a str, args: ArgView<'a>, spec: &'a str) -> Self {
        Self {
            name,
            args,
            spec,
            idx: 0,
            ch: 0,
            done: false,
        }
    }

    /// Index of the first operand. Valid once `next_opt` has returned
    /// `Ok(None)` or `Err`.
    pub fn rest_index(&self) -> usize {
        self.idx
    }

    fn takes_value(&self, c: char) -> bool {
        let mut it = self.spec.chars().peekable();
        while let Some(sc) = it.next() {
            if sc == c {
                return it.peek() == Some(&':');
            }
        }
        false
    }

    fn accepts(&self, c: char) -> bool {
        self.spec.chars().any(|sc| sc == c)
    }

    pub fn next_opt(&mut self, shell: &mut Shell, err: &mut dyn Write) -> Result<Option<Opt>, i32> {
        loop {
            if self.done {
                return Ok(None);
            }
            if self.ch == 0 {
                // Positioned at a fresh argument: decide whether it opens options.
                let Some(cur) = self.args.at(self.idx) else {
                    self.done = true;
                    return Ok(None);
                };
                if cur == "--" {
                    self.idx += 1; // consumed
                    self.done = true;
                    return Ok(None);
                }
                // A lone "-" is an operand, and so is anything not starting with '-'.
                if !cur.starts_with('-') || cur == "-" {
                    self.done = true;
                    return Ok(None);
                }
                self.ch = 1;
            }

            let cur = self
                .args
                .at(self.idx)
                .expect("cluster arg present")
                .to_string();
            let bytes = cur.as_bytes();
            if self.ch >= bytes.len() {
                self.idx += 1;
                self.ch = 0;
                continue;
            }
            let c = bytes[self.ch] as char;
            self.ch += 1;

            if !self.accepts(c) {
                self.fail_invalid(c, shell, err);
                return Err(2);
            }

            if !self.takes_value(c) {
                if self.ch >= bytes.len() {
                    self.idx += 1;
                    self.ch = 0;
                }
                return Ok(Some(Opt { ch: c, value: None }));
            }

            // Value: the rest of this cluster, else the next argument.
            let rest = &cur[self.ch..];
            if !rest.is_empty() {
                let v = rest.to_string();
                self.idx += 1;
                self.ch = 0;
                return Ok(Some(Opt {
                    ch: c,
                    value: Some(v),
                }));
            }
            self.idx += 1;
            self.ch = 0;
            match self.args.at(self.idx) {
                Some(v) => {
                    let v = v.to_string();
                    self.idx += 1;
                    return Ok(Some(Opt {
                        ch: c,
                        value: Some(v),
                    }));
                }
                None => {
                    self.fail_missing_value(c, shell, err);
                    return Err(2);
                }
            }
        }
    }

    fn fail_invalid(&self, c: char, shell: &mut Shell, err: &mut dyn Write) {
        crate::sh_error_to!(shell, err, None, "{}: -{c}: invalid option", self.name);
        let _ = writeln!(err, "{}: usage: {}", self.name, usage_for(self.name));
        shell.builtin_usage_error = Some(2);
        shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage { status: 2 });
    }

    fn fail_missing_value(&self, c: char, shell: &mut Shell, err: &mut dyn Write) {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "{}: option requires an argument -- {c}",
            self.name
        );
        let _ = writeln!(err, "{}: usage: {}", self.name, usage_for(self.name));
        shell.builtin_usage_error = Some(2);
        shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage { status: 2 });
    }
}

/// Usage text, keyed on the INVOKED name. Transcribed verbatim from bash
/// 5.2.21; the differential harness pins every one byte-for-byte, so a typo
/// here is a red test rather than a silent divergence.
pub(crate) fn usage_for(name: &str) -> &'static str {
    match name {
        "alias" => "alias [-p] [name[=value] ... ]",
        "builtin" => "builtin [shell-builtin [arg ...]]",
        "cd" => "cd [-L|[-P [-e]] [-@]] [dir]",
        "command" => "command [-pVv] command [arg ...]",
        "compgen" => {
            "compgen [-abcdefgjksuv] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [word]"
        }
        "complete" => {
            "complete [-abcdefgjksuv] [-pr] [-DEI] [-o option] [-A action] [-G globpat] [-W wordlist] [-F function] [-C command] [-X filterpat] [-P prefix] [-S suffix] [name ...]"
        }
        "compopt" => "compopt [-o|+o option] [-DEI] [name ...]",
        "declare" => {
            "declare [-aAfFgiIlnrtux] [name[=value] ...] or declare -p [-aAfFilnrtux] [name ...]"
        }
        "disown" => "disown [-h] [-ar] [jobspec ... | pid ...]",
        "enable" => "enable [-a] [-dnps] [-f filename] [name ...]",
        "export" => "export [-fn] [name[=value] ...] or export -p",
        "getopts" => "getopts optstring name [arg ...]",
        "hash" => "hash [-lr] [-p pathname] [-dt] [name ...]",
        "help" => "help [-dms] [pattern ...]",
        "history" => {
            "history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]"
        }
        "jobs" => "jobs [-lnprs] [jobspec ...] or jobs -x command [args]",
        "local" => "local [option] name[=value] ...",
        "mapfile" => {
            "mapfile [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]"
        }
        "printf" => "printf [-v var] format [arguments]",
        "pwd" => "pwd [-LP]",
        "read" => {
            "read [-ers] [-a array] [-d delim] [-i text] [-n nchars] [-N nchars] [-p prompt] [-t timeout] [-u fd] [name ...]"
        }
        "readarray" => {
            "readarray [-d delim] [-n count] [-O origin] [-s count] [-t] [-u fd] [-C callback] [-c quantum] [array]"
        }
        "readonly" => "readonly [-aAf] [name[=value] ...] or readonly -p",
        "shopt" => "shopt [-pqsu] [-o] [optname ...]",
        "trap" => "trap [-lp] [[arg] signal_spec ...]",
        "type" => "type [-afptP] name [name ...]",
        "typeset" => {
            "typeset [-aAfFgiIlnrtux] name[=value] ... or typeset -p [-aAfFilnrtux] [name ...]"
        }
        "ulimit" => "ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]",
        "umask" => "umask [-p] [-S] [mode]",
        "unalias" => "unalias [-a] name [name ...]",
        "unset" => "unset [-f] [-v] [-n] [name ...]",
        "wait" => "wait [-fn] [-p var] [id ...]",
        other => {
            debug_assert!(false, "no usage string for builtin {other}");
            ""
        }
    }
}

#[cfg(test)]
#[path = "builtin_opts/tests.rs"]
mod tests;
