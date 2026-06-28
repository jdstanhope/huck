use crate::command::Sequence;
use crate::err_thread_local::with_err;
use crate::executor;
use crate::lexer::{TildeSpec, Word, WordPart};
use crate::shell_state::Shell;
use glob::{glob_with, MatchOptions};

/// Pathname-expansion behavior toggles derived from `shopt` state.
/// All-false ⇒ huck's default (pre-v86) globbing behavior.
#[derive(Clone, Copy, Default, Debug)]
pub struct GlobOpts {
    pub nullglob: bool,
    pub dotglob: bool,
    pub nocaseglob: bool,
    pub failglob: bool,
    pub extglob: bool,
    pub noglob: bool,
    pub globstar: bool,
}

fn resolve_tilde(spec: &TildeSpec, shell: &Shell) -> Option<String> {
    match spec {
        TildeSpec::Home   => shell.get("HOME").map(str::to_string),
        TildeSpec::Pwd    => shell.get("PWD").map(str::to_string),
        TildeSpec::OldPwd => shell.get("OLDPWD").map(str::to_string),
        TildeSpec::User(name) => lookup_home_for_user(name),
    }
}

fn render_tilde_literal(spec: &TildeSpec) -> String {
    match spec {
        TildeSpec::Home       => "~".to_string(),
        TildeSpec::Pwd        => "~+".to_string(),
        TildeSpec::OldPwd     => "~-".to_string(),
        TildeSpec::User(name) => format!("~{name}"),
    }
}

fn lookup_home_for_user(name: &str) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::mem::MaybeUninit;
    use std::ptr;

    let cname = CString::new(name).ok()?;
    let mut buf: Vec<u8> = vec![0; 1024];
    loop {
        let mut pwd: MaybeUninit<libc::passwd> = MaybeUninit::uninit();
        let mut result: *mut libc::passwd = ptr::null_mut();
        let rc = unsafe {
            libc::getpwnam_r(
                cname.as_ptr(),
                pwd.as_mut_ptr(),
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 && !result.is_null() {
            let pwd = unsafe { pwd.assume_init() };
            if pwd.pw_dir.is_null() {
                return None;
            }
            let home = unsafe { CStr::from_ptr(pwd.pw_dir) };
            return home.to_str().ok().map(str::to_string);
        }
        if rc == libc::ERANGE && buf.len() < 16384 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        return None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub chars: String,
    pub quoted: Vec<bool>,
}

impl Field {
    pub fn new() -> Self {
        Self { chars: String::new(), quoted: Vec::new() }
    }

    pub fn push_str(&mut self, s: &str, quoted: bool) {
        let count = s.chars().count();
        self.chars.push_str(s);
        self.quoted.extend(std::iter::repeat_n(quoted, count));
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

impl Default for Field {
    fn default() -> Self {
        Self::new()
    }
}

/// Expands a subscript `Word` to a string key. Variable expansion and
/// command substitution apply, but no arith. Used for associative
/// array subscripts. The caller decides string vs arith based on the
/// variable's current `VarValue` variant.
pub(crate) fn eval_subscript_key(
    subscript: &Word,
    shell: &mut Shell,
) -> String {
    crate::param_expansion::expand_word_to_string(subscript, shell)
}

/// Returns the expanded arith source string and the eval result, so callers
/// can render bash-compatible errors (which echo the source + error token).
pub(crate) fn eval_arith_word_src(
    body: &Word,
    shell: &mut Shell,
) -> (String, Result<i64, crate::arith::ArithError>) {
    let s = crate::param_expansion::expand_word_to_string(body, shell);
    if s.trim().is_empty() {
        return (s, Ok(0));
    }
    let res = crate::arith::parse(&s).and_then(|e| crate::arith::eval(&e, shell));
    (s, res)
}

/// Back-compat thin wrapper for callers that only need the value
/// (arith-`for`, `${a[i]}` coercion, etc.).
pub(crate) fn eval_arith_word(
    body: &Word,
    shell: &mut Shell,
) -> Result<i64, crate::arith::ArithError> {
    eval_arith_word_src(body, shell).1
}

/// Arith-evaluates an array subscript `Word` to a `usize`, honouring
/// bash's bash-4.3+ rule that a negative result counts from the end:
/// `${a[-1]}` is the highest-subscript element. Returns `Err(msg)` if
/// the subscript fails to parse/eval, or if the wrap-around still
/// yields a negative index. The caller decides whether to print the
/// diagnostic and set `pending_fatal_status`.
pub(crate) fn eval_subscript(
    subscript: &Word,
    shell: &mut Shell,
    name: &str,
) -> Result<usize, String> {
    let s = crate::param_expansion::expand_word_to_string(subscript, shell);
    let expr = crate::arith::parse(&s)
        .map_err(|_| format!("{name}: bad array subscript"))?;
    let n = crate::arith::eval(&expr, shell)
        .map_err(|_| format!("{name}: bad array subscript"))?;
    if n >= 0 {
        Ok(n as usize)
    } else {
        let max = shell
            .array_max_index(name)
            .ok_or_else(|| format!("{name}: bad array subscript"))?;
        let wrapped = max as i64 + 1 + n;
        if wrapped < 0 {
            Err(format!("{name}: bad array subscript"))
        } else {
            Ok(wrapped as usize)
        }
    }
}

/// Slices a word list per `${a[@]:off:len}` semantics. Negative offset
/// counts from the end of the value list; negative length means "index
/// from end". Used by the array `[@]`/`[*]` slicing path. Note: the
/// positional-param path (`${@:o:l}` / `${*:o:l}`) is `expand_positional_substring`,
/// which duplicates this arithmetic to handle bash's `$0`-prepend
/// semantics for offset 0 — they intentionally do not share an
/// implementation.
pub(crate) fn slice_word_list(
    values: &[String],
    offset: &Word,
    length: Option<&Word>,
    shell: &mut Shell,
) -> Result<Vec<String>, String> {
    let off_s = crate::param_expansion::expand_word_to_string(offset, shell);
    let off_n = crate::arith::parse(&off_s)
        .and_then(|e| crate::arith::eval(&e, shell))
        .map_err(|_| "bad slice offset".to_string())?;
    let total = values.len() as i64;
    let start = if off_n >= 0 {
        (off_n as usize).min(values.len())
    } else {
        ((total + off_n).max(0) as usize).min(values.len())
    };
    let end = match length {
        Some(lw) => {
            let len_s = crate::param_expansion::expand_word_to_string(lw, shell);
            let len_n = crate::arith::parse(&len_s)
                .and_then(|e| crate::arith::eval(&e, shell))
                .map_err(|_| "bad slice length".to_string())?;
            if len_n < 0 {
                (((total + len_n).max(start as i64)) as usize).min(values.len())
            } else {
                ((start as i64 + len_n) as usize).min(values.len())
            }
        }
        None => values.len(),
    };
    Ok(values[start..end].to_vec())
}

/// Handles `${@:o:l}` / `${*:o:l}` — the positional-param slicing
/// case (v33 deferral). Bash's `${@:o:l}` uses 1-based indexing when
/// off >= 1 (since `$0` is the script name, not in `$@`); we follow
/// that convention: positive offsets are relative to `${1}`, negative
/// offsets count from the end.
fn expand_positional_substring(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    quoted: bool,
    shell: &mut Shell,
) -> crate::param_expansion::ExpansionResult {
    use crate::lexer::ParamModifier as PM;
    use crate::param_expansion::ExpansionResult;
    let (offset, length) = match modifier {
        PM::Substring { offset, length } => (offset, length.as_ref()),
        _ => unreachable!("caller checks ParamModifier::Substring"),
    };
    // Bash: `${@:0}` includes `$0`; `${@:1}` is the regular positional list.
    // We model this as: prepend `$0` then take slice with the user's offset.
    let mut values: Vec<String> = Vec::with_capacity(shell.positional_args.len() + 1);
    // `$0` (the invocation name) is not rebound inside functions.
    values.push(shell.shell_argv0.clone());
    values.extend(shell.positional_args.iter().cloned());
    // Evaluate user offset; if it's >= 0, do NOT auto-shift (matches bash:
    // `${@:0}` is the whole list including $0; `${@:1}` starts at $1).
    let off_s = crate::param_expansion::expand_word_to_string(offset, shell);
    let off_n = match crate::arith::parse(&off_s).and_then(|e| crate::arith::eval(&e, shell)) {
        Ok(n) => n,
        Err(_) => {
            with_err(|err| e!(err, "huck: {name}: bad slice offset"));
            return ExpansionResult::Fatal { status: 1 };
        }
    };
    // For the negative case, bash counts from the end of the present
    // positional list (i.e. excluding `$0`). We have prepended `$0`, so
    // adjust: `${@: -k}` means "last k positionals" not "last k of $0+positionals".
    let posargs_len = shell.positional_args.len() as i64;
    let start = if off_n >= 0 {
        (off_n as usize).min(values.len())
    } else {
        // negative: count from end of positionals; $0 is at index 0 in
        // `values` so the positional region is `1..=posargs_len`.
        // start = 1 + (posargs_len + off_n), clamped to [1, len].
        let raw = 1 + (posargs_len + off_n);
        if raw < 1 {
            1usize
        } else {
            (raw as usize).min(values.len())
        }
    };
    let end = match length {
        Some(lw) => {
            let len_s = crate::param_expansion::expand_word_to_string(lw, shell);
            let len_n = match crate::arith::parse(&len_s).and_then(|e| crate::arith::eval(&e, shell)) {
                Ok(n) => n,
                Err(_) => {
                    with_err(|err| e!(err, "huck: {name}: bad slice length"));
                    return ExpansionResult::Fatal { status: 1 };
                }
            };
            if len_n < 0 {
                let total = values.len() as i64;
                (((total + len_n).max(start as i64)) as usize).min(values.len())
            } else {
                ((start as i64 + len_n) as usize).min(values.len())
            }
        }
        None => values.len(),
    };
    let sliced = values[start..end].to_vec();
    // Result shape: quoted `@` produces separate fields (WordList); all
    // other forms produce a single IFS-joined value.
    if name == "@" && quoted {
        ExpansionResult::WordList(sliced)
    } else {
        let ifs = shell.ifs();
        let sep = ifs_join_sep(&ifs);
        ExpansionResult::Value(sliced.join(&sep))
    }
}

/// Whether this modifier dispatches to the per-element arm.
/// Case / RemovePrefix / RemoveSuffix / Substitute always do;
/// Transform dispatches per-element ONLY for the 6 scalar-style ops
/// (P/Q/U/L/u/E). The 4 whole-array ops (A/K/k/a) route through the
/// sibling whole-array arm; see `is_whole_array_transform_op`.
fn is_per_element_modifier(m: &crate::lexer::ParamModifier) -> bool {
    use crate::lexer::ParamModifier as PM;
    match m {
        PM::Case { .. }
        | PM::RemovePrefix { .. }
        | PM::RemoveSuffix { .. }
        | PM::Substitute { .. } => true,
        PM::Transform { op } => is_per_element_transform_op(*op),
        _ => false,
    }
}

/// `${var@OP}` ops that operate on a single value (per-element when
/// applied across an array): P (prompt-expand), Q (shell-quote),
/// U (upper), L (lower), u (upper-first), E (escape-expand).
fn is_per_element_transform_op(op: crate::lexer::TransformOp) -> bool {
    use crate::lexer::TransformOp::*;
    matches!(op, PromptExpand | Quote | Upper | Lower | UpperFirst | EscapeExpand)
}

/// `${var@OP}` ops that operate on the whole array (KEYS+VALUES or
/// type info): A (declare-style), K (k/v string), k (k/v word list),
/// a (attribute flags).
fn is_whole_array_transform_op(op: crate::lexer::TransformOp) -> bool {
    use crate::lexer::TransformOp::*;
    matches!(op, AssignDecl | KvString | KvWords | AttrFlags)
}

/// Apply a scalar modifier to one element's value via the existing
/// `expand_modifier_with_value` scalar path. Wraps the element in
/// `ParamLookup::Element(Some(_))` so default/error modifiers see a present
/// element (every element here has a concrete value — even an empty
/// string).
///
/// Used by the per-element arm in `expand_array_param` / `expand_assoc_param`.
/// Falls through to empty-string output for non-Value results; per-element
/// scalar modifiers should never produce WordList/Fields/Fatal in practice.
fn scalar_apply_per_element(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    element: &str,
    quoted: bool,
    shell: &mut crate::shell_state::Shell,
) -> String {
    use crate::param_expansion::{expand_modifier_with_value, ExpansionResult, ParamLookup};
    match expand_modifier_with_value(
        name,
        modifier,
        ParamLookup::Element(Some(element)),
        quoted,
        shell,
    ) {
        ExpansionResult::Value(s) => s,
        ExpansionResult::Empty => String::new(),
        _ => String::new(),
    }
}

/// Dispatches `${m[...]}` forms when `m` is an associative array.
/// String-key subscripts (no arith), insertion-order iteration for
/// `@`/`*`, and string keys for `${!m[@]}`. Routed from
/// `expand_array_param` based on the variable's current `VarValue`
/// variant.
fn expand_assoc_param(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    subscript: &crate::lexer::SubscriptKind,
    quoted: bool,
    shell: &mut Shell,
) -> crate::param_expansion::ExpansionResult {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK};
    use crate::param_expansion::ExpansionResult;

    // Snapshot the pairs once up-front so the rest of the function can
    // borrow `shell` mutably for sub-expansions (e.g., modifier word
    // evaluation, subscript-as-Word expansion).
    let pairs: Vec<(String, String)> = shell
        .get_associative(name)
        .cloned()
        .unwrap_or_default();
    let values: Vec<String> = pairs.iter().map(|(_, v)| v.clone()).collect();
    let keys: Vec<String> = pairs.iter().map(|(k, _)| k.clone()).collect();

    match (modifier, subscript) {
        // ${m[@]} / ${m[*]} — pure expansion, no scalar modifier.
        (PM::None, SK::All) => ExpansionResult::WordList(values),
        (PM::None, SK::Star) => {
            let ifs = shell.ifs();
            let sep = ifs_join_sep(&ifs);
            ExpansionResult::Value(values.join(&sep))
        }
        // ${m[k]} — string-key lookup (no arith on `k`).
        (PM::None, SK::Index(w)) => {
            let key = eval_subscript_key(w, shell);
            let val = shell.lookup_associative_element(name, &key);
            if val.is_none() && shell.shell_options.nounset {
                with_err(|err| e!(err, "huck: {name}[{key}]: unbound variable"));
                shell.pending_fatal_status = Some(1);
                return ExpansionResult::Fatal { status: 1 };
            }
            ExpansionResult::Value(val.unwrap_or_default())
        }
        // ${#m[@]} / ${#m[*]} — pair count.
        (PM::Length, SK::All) | (PM::Length, SK::Star) => {
            ExpansionResult::Value(pairs.len().to_string())
        }
        // ${#m[k]} — char count of the value at string key `k`.
        (PM::Length, SK::Index(w)) => {
            let key = eval_subscript_key(w, shell);
            let val = shell.lookup_associative_element(name, &key).unwrap_or_default();
            ExpansionResult::Value(val.chars().count().to_string())
        }
        // ${!m[@]} / ${!m[*]} — list of string keys in insertion order.
        (PM::IndirectKeys, SK::All) => {
            if quoted {
                ExpansionResult::WordList(keys)
            } else {
                let ifs = shell.ifs();
                let sep = ifs_join_sep(&ifs);
                ExpansionResult::Value(keys.join(&sep))
            }
        }
        (PM::IndirectKeys, SK::Star) => {
            let ifs = shell.ifs();
            let sep = ifs_join_sep(&ifs);
            ExpansionResult::Value(keys.join(&sep))
        }
        // `${!m[k]}` — indirect ref through a specific element; not
        // supported in v72 (would require resolving the value as a
        // variable name). Produce empty for parity with the indexed path.
        (PM::IndirectKeys, SK::Index(_)) => ExpansionResult::Value(String::new()),
        // ${m[@]:o:l} / ${m[*]:o:l} — slicing in insertion order.
        (PM::Substring { offset, length }, SK::All)
        | (PM::Substring { offset, length }, SK::Star) => {
            let sliced = match slice_word_list(&values, offset, length.as_ref(), shell) {
                Ok(v) => v,
                Err(e) => {
                    with_err(|err| e!(err, "huck: {name}: {e}"));
                    shell.pending_fatal_status = Some(1);
                    return ExpansionResult::Fatal { status: 1 };
                }
            };
            if matches!(subscript, SK::All) && quoted {
                ExpansionResult::WordList(sliced)
            } else {
                let ifs = shell.ifs();
                let sep = ifs_join_sep(&ifs);
                ExpansionResult::Value(sliced.join(&sep))
            }
        }
        // ${m[k]:...} — scalar-style modifier on a specific element.
        // Pass the element as ParamLookup::Element so missing keys
        // correctly trigger default/error modifiers instead of falling
        // through to the array's scalar view.
        (modif, SK::Index(w)) => {
            let key = eval_subscript_key(w, shell);
            let val = shell.lookup_associative_element(name, &key);
            crate::param_expansion::expand_modifier_with_value(
                name,
                modif,
                crate::param_expansion::ParamLookup::Element(val.as_deref()),
                quoted,
                shell,
            )
        }
        // `${m[@]+word}` / `${m[@]-word}` (and :+/:-) on a whole assoc array.
        // Set iff it has >=1 pair; empty `declare -A n=()` counts as UNSET.
        // Mirrors the indexed-array path. The word is field-preserving.
        (PM::UseAlternate { word, colon: _ }, SK::All | SK::Star) => {
            if values.is_empty() {
                ExpansionResult::Empty
            } else if quoted {
                // Quoted outer: keep the existing field-preserving WordList /
                // [*]-join path (already correct).
                let words: Vec<String> =
                    expand(word, shell).into_iter().map(|f| f.chars).collect();
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(words.join(&sep))
                } else {
                    ExpansionResult::WordList(words)
                }
            } else {
                // Unquoted outer: emit the alternate's own fields verbatim
                // (preserves empties / quoted-spaced fields).
                ExpansionResult::Fields(expand(word, shell))
            }
        }
        (PM::UseDefault { word, colon: _ }, SK::All | SK::Star) => {
            if !values.is_empty() {
                // Set: behave exactly like ${m[@]} / ${m[*]} (unchanged).
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(values.join(&sep))
                } else {
                    ExpansionResult::WordList(values)
                }
            } else if quoted {
                // Unset, quoted outer: existing field-preserving path.
                let words: Vec<String> =
                    expand(word, shell).into_iter().map(|f| f.chars).collect();
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(words.join(&sep))
                } else {
                    ExpansionResult::WordList(words)
                }
            } else {
                // Unset, unquoted outer: emit the default word's own fields.
                ExpansionResult::Fields(expand(word, shell))
            }
        }
        (modif, SK::All | SK::Star) if is_per_element_modifier(modif) => {
            let transformed: Vec<String> = values
                .iter()
                .map(|v| scalar_apply_per_element(name, modif, v, quoted, shell))
                .collect();
            if matches!(subscript, SK::All) && quoted {
                ExpansionResult::WordList(transformed)
            } else {
                let ifs = shell.ifs();
                let sep = ifs_join_sep(&ifs);
                ExpansionResult::Value(transformed.join(&sep))
            }
        }
        (crate::lexer::ParamModifier::Transform { op }, sub)
            if is_whole_array_transform_op(*op) =>
        {
            use crate::array_transforms::{self as at, ScopeMode};
            use crate::lexer::TransformOp::*;
            let scope = if matches!(sub, SK::All | SK::Star) {
                ScopeMode::Whole
            } else {
                let val = match sub {
                    SK::Index(_) => values.first().cloned().unwrap_or_default(),
                    _ => String::new(),
                };
                ScopeMode::ScalarOrElement(val)
            };
            match op {
                AssignDecl => ExpansionResult::Value(at::assign_decl(name, scope, shell)),
                KvString => ExpansionResult::Value(at::kv_string(name, scope, shell)),
                KvWords => {
                    let words = at::kv_words(name, scope, shell);
                    if matches!(sub, SK::All) && quoted {
                        ExpansionResult::WordList(words)
                    } else {
                        let ifs = shell.ifs();
                        let sep = ifs_join_sep(&ifs);
                        ExpansionResult::Value(words.join(&sep))
                    }
                }
                AttrFlags => ExpansionResult::Value(at::attr_flags(name, shell)),
                _ => unreachable!("guarded by is_whole_array_transform_op"),
            }
        }
        // Other scalar modifiers on @/* — explicit error for v72 scope
        // (per-element modifiers across the whole array are deferred).
        (other, SK::All | SK::Star) => {
            with_err(|err| e!(err,
                "huck: ${{{name}[…]}}: modifier {:?} not supported on associative array in v72",
                other
            ));
            ExpansionResult::Value(String::new())
        }
    }
}

/// `${!name<modifier>}` indirect expansion: resolve `name`(+`subscript`)'s
/// scalar value to an effective name N, then expand `${N<modifier>}`.
///
/// Through-value resolution: a plain name uses `shell.lookup_var` (which
/// resolves named vars, positionals, and specials); a subscripted source
/// (`${!a[i]}`) reads the array element scalar. An empty/all-whitespace
/// through-value (source unset OR set-but-empty) is a FATAL "invalid
/// indirect expansion" in bash and fires regardless of `set -u`. The
/// effective name N is interpreted as a parameter reference: a plain
/// name, a positional digit / special param, or `name[sub]` (re-expanded
/// as an array element).
///
/// `set -u`: when N is a valid-but-unset name (a bare reference, no
/// substitution modifier), it raises the same unbound-variable fatal as a
/// normal `${N}` reference would.
fn expand_indirect(
    name: &str,
    subscript: Option<&crate::lexer::SubscriptKind>,
    modifier: &crate::lexer::ParamModifier,
    quoted: bool,
    shell: &mut Shell,
) -> crate::param_expansion::ExpansionResult {
    use crate::param_expansion::ExpansionResult;
    // Nameref special case: ${!r} where r is a nameref yields the TARGET NAME
    // (the raw stored value), not value-as-name indirection (bash behavior).
    if subscript.is_none() && shell.is_nameref(name) {
        return ExpansionResult::Value(
            shell.nameref_raw_target(name).unwrap_or_default(),
        );
    }
    // `${!*}` / `${!@}` (v233 M2): indirect through `$*` / `$@`. bash uses
    // the IFS-joined positional params as the effective NAME: no positionals
    // -> empty, rc 0; a single valid name (or positional digit) -> resolve it;
    // a multi-word / IFS-joined value (e.g. "foo bar", "a,b") is an invalid
    // variable name -> fatal rc 1. (The positionals are NOT reachable via
    // `lookup_var("*")`, so handle them before the generic through-value path.)
    if subscript.is_none() && (name == "*" || name == "@") {
        let through = shell
            .positional_args
            .join(&ifs_join_sep(&shell.ifs()));
        if through.is_empty() {
            return ExpansionResult::Value(String::new());
        }
        let valid = crate::builtins::is_valid_name(&through)
            || through.bytes().all(|b| b.is_ascii_digit());
        if !valid {
            with_err(|err| e!(err, "huck: {through}: invalid variable name"));
            return ExpansionResult::Fatal { status: 1 };
        }
        return crate::param_expansion::expand_modifier_quoted(&through, modifier, quoted, shell);
    }
    // Step 1: through-value = scalar value of (name, subscript).
    let through = match subscript {
        None => shell.lookup_var(name).unwrap_or_default(),
        Some(sub) => {
            // Indirect through a subscripted source. For `[@]`/`[*]` bash uses
            // the IFS-JOINED array values as the effective name (a single
            // element -> that value; multiple -> a space-joined string that is
            // an invalid name -> the `invalid variable name` fatal below). For
            // a single-index `[i]` read that element's scalar value.
            match sub {
                crate::lexer::SubscriptKind::All | crate::lexer::SubscriptKind::Star => {
                    match expand_array_param(name, &crate::lexer::ParamModifier::None, sub, /* quoted */ true, shell) {
                        ExpansionResult::WordList(ws) => ws.join(&ifs_join_sep(&shell.ifs())),
                        ExpansionResult::Value(v) => v,
                        _ => String::new(),
                    }
                }
                _ => match expand_array_param(name, &crate::lexer::ParamModifier::None, sub, quoted, shell) {
                    ExpansionResult::Value(v) => v,
                    _ => String::new(),
                },
            }
        }
    };
    // Use the through-value VERBATIM as the effective name N — bash does
    // not trim or word-split it. An all-whitespace or space-containing
    // value is a non-empty (invalid) name that falls through to the normal
    // lookup path and yields empty, matching bash's observable result.
    let n: &str = &through;
    // A non-empty through-value that is not a valid name (e.g. the space-joined
    // values of a real `${!arr[@]<op>}`) is rejected by bash as an invalid
    // variable name, before any modifier is applied.
    if !through.is_empty()
        && !crate::builtins::is_valid_name(n)
        && !n.bytes().all(|b| b.is_ascii_digit())
    {
        with_err(|err| e!(err, "{}{}: invalid variable name", shell.error_prefix(None), n));
        return ExpansionResult::Fatal { status: 1 };
    }
    if through.is_empty() {
        // Empty through-value: bash distinguishes three cases (verified
        // against bash 5.x). All route through the fatal-PE mechanism so a
        // non-interactive shell exits and an interactive one aborts the
        // command, EXCEPT the unset-positional case which expands to empty.
        if subscript.is_none() {
            // Source SET but empty: the (empty) effective name is invalid —
            // bash prints "<name>: invalid variable name" (here the effective
            // name is the empty string).
            if shell.is_set(name) {
                with_err(|err| e!(err, "huck: : invalid variable name"));
                return ExpansionResult::Fatal { status: 1 };
            }
            // Source UNSET and a POSITIONAL parameter ($1.. beyond $#): bash
            // treats the indirection as unset and expands to empty (so a
            // `:-`/`:+` modifier sees "unset"). Under `set -u` a bare
            // reference is fatal "!<name>: unbound variable" (huck's nounset
            // convention is exit 1, matching `${unset}` here — bash uses 127).
            if name.bytes().all(|b| b.is_ascii_digit()) {
                use crate::lexer::ParamModifier as PM;
                // bash reports the indirect spec under the `!N` name for the
                // positional path's diagnostics.
                if shell.shell_options.nounset && matches!(modifier, PM::None) {
                    with_err(|err| e!(err, "huck: !{name}: unbound variable"));
                    return ExpansionResult::Fatal { status: 1 };
                }
                match modifier {
                    // `:=`/`=`: bash rejects assignment to an indirect-unset
                    // positional ("!N: invalid indirect expansion"). Must NOT
                    // forward with an empty effective name (would write
                    // `vars[""]`).
                    PM::AssignDefault { .. } => {
                        with_err(|err| e!(err, "huck: !{name}: invalid indirect expansion"));
                        return ExpansionResult::Fatal { status: 1 };
                    }
                    // `:?`/`?`: the parameter is reported unset under `!N` —
                    // forwarding that effective name reuses the standard
                    // ErrorIfUnset rendering ("!N: <msg>" / "!N: parameter …").
                    PM::ErrorIfUnset { .. } => {
                        let effname = format!("!{name}");
                        return crate::param_expansion::expand_modifier_quoted(
                            &effname, modifier, quoted, shell,
                        );
                    }
                    // Every value-substituting/transforming modifier
                    // (`:-`/`:+`/`#`/`%`/`/`/`:off:len`/`^`/`,`/None) operates
                    // on the empty value and yields empty/default.
                    _ => {
                        return crate::param_expansion::expand_modifier_quoted(
                            "", modifier, quoted, shell,
                        );
                    }
                }
            }
        }
        // Source UNSET and a named variable (or a subscripted source): fatal
        // "invalid indirect expansion".
        with_err(|err| e!(err, "huck: {name}: invalid indirect expansion"));
        return ExpansionResult::Fatal { status: 1 };
    }
    // Step 2: parse N into (effective_name, effective_subscript) and
    // re-expand. The only structured form we honor is `name[sub]`.
    if let Some((base, sub_text)) = split_name_subscript(n) {
        let sub = crate::lexer::SubscriptKind::Index(Word(vec![
            WordPart::Literal { text: sub_text, quoted: false },
        ]));
        return expand_array_param(&base, modifier, &sub, quoted, shell);
    }
    // The effective name N is a valid parameter. When N is itself unset
    // and `set -u` is active, a bare reference must raise the same
    // unbound-variable fatal as a normal `${N}` would — `expand_modifier`
    // does not enforce nounset, so apply it here for the bare-reference
    // case (a substitution modifier like `:-`/`-`/`+` handles unset on
    // its own and must not be pre-empted).
    if matches!(modifier, crate::lexer::ParamModifier::None)
        && shell.shell_options.nounset
        && shell.lookup_var(n).is_none()
    {
        with_err(|err| e!(err, "huck: {n}: unbound variable"));
        return ExpansionResult::Fatal { status: 1 };
    }
    crate::param_expansion::expand_modifier_quoted(n, modifier, quoted, shell)
}

/// Splits a `name[sub]` effective-name string into `(name, sub)`. Returns
/// `None` for a plain name / positional / special param (the common path).
/// Only the simple `ends-with-']' and contains-'['` shape is recognized;
/// the inner `sub` text is re-parsed as an arithmetic subscript Word.
pub(crate) fn split_name_subscript(n: &str) -> Option<(String, String)> {
    if !n.ends_with(']') {
        return None;
    }
    let open = n.find('[')?;
    if open == 0 {
        return None;
    }
    let base = n[..open].to_string();
    let sub = n[open + 1..n.len() - 1].to_string();
    Some((base, sub))
}

/// Dispatches `${a[...]}` forms. The `subscript` field of
/// `WordPart::ParamExpansion` distinguishes `[@]`, `[*]`, and
/// `[<expr>]`; the `modifier` is the scalar-style suffix (or
/// `ParamModifier::None` for bare `${a[i]}`).
fn expand_array_param(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    subscript: &crate::lexer::SubscriptKind,
    quoted: bool,
    shell: &mut Shell,
) -> crate::param_expansion::ExpansionResult {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::ResolvedName;

    if shell.pending_fatal_status.is_some() {
        return ExpansionResult::Empty;
    }

    // Nameref resolution: if `name` is a nameref, resolve to the effective
    // array name before any array expansion. Gate behind a cheap attribute
    // check so non-namerefs skip allocation entirely.
    let resolved_name: String;
    let name: &str = if shell.is_nameref(name) {
        resolved_name = match shell.resolve_nameref(name) {
            ResolvedName::Name(n) => n,
            // Element namerefs (e.g. r=arr[1]) on whole-array expansions:
            // resolve to the base array name so ${r[@]} expands the whole array.
            ResolvedName::Element { name: base, .. } => base,
            ResolvedName::Unbound(_) | ResolvedName::Cycle => return ExpansionResult::Empty,
        };
        &resolved_name
    } else {
        name
    };

    // Type-aware dispatch: associative arrays get string-key semantics.
    // Must come before the indexed/scalar/unset path below, so a
    // declared `${m[k]}` is not arith-evaluated like an indexed
    // subscript.
    if shell.get_associative(name).is_some() {
        return expand_assoc_param(name, modifier, subscript, quoted, shell);
    }

    // Snapshot the array's values / keys in subscript-ascending order.
    let collect_values = |sh: &Shell| -> Vec<String> {
        match sh.get_indexed(name) {
            Some(m) => m.values().cloned().collect(),
            None => match sh.get(name) {
                Some(s) => vec![s.to_string()],
                None => Vec::new(),
            },
        }
    };
    let collect_keys = |sh: &Shell| -> Vec<usize> {
        match sh.get_indexed(name) {
            Some(m) => m.keys().copied().collect(),
            None => match sh.get(name) {
                Some(_) => vec![0],
                None => Vec::new(),
            },
        }
    };

    match (modifier, subscript) {
        // ${a[@]} / ${a[*]} — pure expansion, no scalar modifier.
        (PM::None, SK::All) => ExpansionResult::WordList(collect_values(shell)),
        (PM::None, SK::Star) => {
            // Quoted `${a[*]}` joins with first IFS char; unquoted is
            // also joined-then-split (we hand back Value and let the
            // consumer's split path do the rest, so emitting a single
            // joined string here matches both quoted and unquoted
            // semantics modulo the consumer's split step).
            let ifs = shell.ifs();
            let sep = ifs_join_sep(&ifs);
            ExpansionResult::Value(collect_values(shell).join(&sep))
        }
        // ${a[i]} — read a specific element.
        (PM::None, SK::Index(w)) => {
            let idx = match eval_subscript(w, shell, name) {
                Ok(i) => i,
                Err(e) => {
                    with_err(|err| e!(err, "huck: {e}"));
                    shell.pending_fatal_status = Some(1);
                    return ExpansionResult::Fatal { status: 1 };
                }
            };
            let val = shell.lookup_indexed_element(name, idx);
            if val.is_none() && shell.shell_options.nounset {
                with_err(|err| e!(err, "huck: {name}[{idx}]: unbound variable"));
                shell.pending_fatal_status = Some(1);
                return ExpansionResult::Fatal { status: 1 };
            }
            ExpansionResult::Value(val.unwrap_or_default())
        }
        // ${#a[@]} / ${#a[*]} — element count (NOT max index).
        (PM::Length, SK::All) | (PM::Length, SK::Star) => {
            ExpansionResult::Value(collect_keys(shell).len().to_string())
        }
        // ${#a[i]} — char count of the element at `i`.
        (PM::Length, SK::Index(w)) => {
            let idx = match eval_subscript(w, shell, name) {
                Ok(i) => i,
                Err(e) => {
                    with_err(|err| e!(err, "huck: {e}"));
                    shell.pending_fatal_status = Some(1);
                    return ExpansionResult::Fatal { status: 1 };
                }
            };
            let val = shell.lookup_indexed_element(name, idx).unwrap_or_default();
            ExpansionResult::Value(val.chars().count().to_string())
        }
        // ${!a[@]} / ${!a[*]} — list of subscripts.
        (PM::IndirectKeys, SK::All) | (PM::IndirectKeys, SK::Star) => {
            let keys: Vec<String> = collect_keys(shell)
                .iter()
                .map(usize::to_string)
                .collect();
            if matches!(subscript, SK::All) && quoted {
                ExpansionResult::WordList(keys)
            } else {
                let ifs = shell.ifs();
                let sep = ifs_join_sep(&ifs);
                ExpansionResult::Value(keys.join(&sep))
            }
        }
        // `${!a[i]}` — not supported in v71 (would be "indirect ref
        // through array element"); produce empty.
        (PM::IndirectKeys, SK::Index(_)) => ExpansionResult::Value(String::new()),
        // ${a[@]:o:l} / ${a[*]:o:l} — slicing.
        (PM::Substring { offset, length }, SK::All) | (PM::Substring { offset, length }, SK::Star) => {
            let values = collect_values(shell);
            let sliced = match slice_word_list(&values, offset, length.as_ref(), shell) {
                Ok(v) => v,
                Err(e) => {
                    with_err(|err| e!(err, "huck: {name}: {e}"));
                    shell.pending_fatal_status = Some(1);
                    return ExpansionResult::Fatal { status: 1 };
                }
            };
            if matches!(subscript, SK::All) && quoted {
                ExpansionResult::WordList(sliced)
            } else {
                let ifs = shell.ifs();
                let sep = ifs_join_sep(&ifs);
                ExpansionResult::Value(sliced.join(&sep))
            }
        }
        // ${a[i]:...} — scalar-style modifier on a single element.
        // Pass the element as ParamLookup::Element so missing keys
        // correctly trigger default/error modifiers instead of falling
        // through to the array's scalar view.
        (modif, SK::Index(w)) => {
            let idx = match eval_subscript(w, shell, name) {
                Ok(i) => i,
                Err(_) => return ExpansionResult::Value(String::new()),
            };
            let val = shell.lookup_indexed_element(name, idx);
            crate::param_expansion::expand_modifier_with_value(
                name,
                modif,
                crate::param_expansion::ParamLookup::Element(val.as_deref()),
                quoted,
                shell,
            )
        }
        // `${arr[@]+word}` / `${arr[@]-word}` (and :+/:-) on a whole array.
        // A whole array is "set and non-null" iff it has >=1 element; the
        // colon and non-colon variants behave identically (a whole array
        // can't be "set but null"). Empty array () counts as UNSET. Matches
        // bash. The alternate/default `word` is expanded field-preserving so
        // the idiom ${arr[@]+"${arr[@]}"} keeps element boundaries.
        (PM::UseAlternate { word, colon: _ }, SK::All | SK::Star) => {
            if collect_values(shell).is_empty() {
                ExpansionResult::Empty
            } else if quoted {
                // Quoted outer: keep the existing field-preserving WordList /
                // [*]-join path (already correct).
                let words: Vec<String> =
                    expand(word, shell).into_iter().map(|f| f.chars).collect();
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(words.join(&sep))
                } else {
                    ExpansionResult::WordList(words)
                }
            } else {
                // Unquoted outer: emit the alternate's own fields verbatim
                // (preserves empties / quoted-spaced fields).
                ExpansionResult::Fields(expand(word, shell))
            }
        }
        (PM::UseDefault { word, colon: _ }, SK::All | SK::Star) => {
            let values = collect_values(shell);
            if !values.is_empty() {
                // Set: behave exactly like ${arr[@]} / ${arr[*]} (unchanged).
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(values.join(&sep))
                } else {
                    ExpansionResult::WordList(values)
                }
            } else if quoted {
                // Unset, quoted outer: existing field-preserving path.
                let words: Vec<String> =
                    expand(word, shell).into_iter().map(|f| f.chars).collect();
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(words.join(&sep))
                } else {
                    ExpansionResult::WordList(words)
                }
            } else {
                // Unset, unquoted outer: emit the default word's own fields.
                ExpansionResult::Fields(expand(word, shell))
            }
        }
        (modif, SK::All | SK::Star) if is_per_element_modifier(modif) => {
            let values = collect_values(shell);
            let transformed: Vec<String> = values
                .iter()
                .map(|v| scalar_apply_per_element(name, modif, v, quoted, shell))
                .collect();
            if matches!(subscript, SK::All) && quoted {
                ExpansionResult::WordList(transformed)
            } else {
                let ifs = shell.ifs();
                let sep = ifs_join_sep(&ifs);
                ExpansionResult::Value(transformed.join(&sep))
            }
        }
        (crate::lexer::ParamModifier::Transform { op }, sub)
            if is_whole_array_transform_op(*op) =>
        {
            use crate::array_transforms::{self as at, ScopeMode};
            use crate::lexer::TransformOp::*;
            let scope = if matches!(sub, SK::All | SK::Star) {
                ScopeMode::Whole
            } else {
                // Specific subscript or no subscript → scalar-or-
                // element form. For [i], the value is the element
                // at that subscript; for no subscript, the scalar
                // view (already resolved by collect_values to
                // values[0] or empty).
                let val = match sub {
                    SK::Index(_) => {
                        let vs = collect_values(shell);
                        vs.into_iter().next().unwrap_or_default()
                    }
                    _ => {
                        shell.lookup_var(name).unwrap_or_default()
                    }
                };
                ScopeMode::ScalarOrElement(val)
            };
            match op {
                AssignDecl => ExpansionResult::Value(at::assign_decl(name, scope, shell)),
                KvString => ExpansionResult::Value(at::kv_string(name, scope, shell)),
                KvWords => {
                    let words = at::kv_words(name, scope, shell);
                    if matches!(sub, SK::All) && quoted {
                        ExpansionResult::WordList(words)
                    } else {
                        let ifs = shell.ifs();
                        let sep = ifs_join_sep(&ifs);
                        ExpansionResult::Value(words.join(&sep))
                    }
                }
                AttrFlags => ExpansionResult::Value(at::attr_flags(name, shell)),
                _ => unreachable!("guarded by is_whole_array_transform_op"),
            }
        }
        // Other scalar modifiers on @/* — explicit error for v71 scope.
        (other, SK::All | SK::Star) => {
            with_err(|err| e!(err,
                "huck: ${{{name}[…]}}: modifier {:?} not supported on array in v71",
                other
            ));
            ExpansionResult::Value(String::new())
        }
    }
}

/// Process a single `WordPart` in the context of `expand()`, mutating the
/// in-progress `current` field, the accumulated `result` vector, and the
/// `has_emitted` sentinel. Returns `ControlFlow::Break(())` when the callers
/// should immediately return `result` (fatal parameter error / nounset).
fn expand_part(
    part: &WordPart,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
    shell: &mut Shell,
    snapshot_status: i32,
    word: &Word,
) -> std::ops::ControlFlow<()> {
    use std::ops::ControlFlow;
    match part {
        WordPart::Literal { text, quoted } => {
            current.push_str(text, *quoted);
            *has_emitted = true;
        }
        WordPart::Tilde(spec) => {
            // Tilde expansion result is always unquoted — pathname
            // expansion treats the expanded path as if the user typed it.
            let text = resolve_tilde(spec, shell)
                .unwrap_or_else(|| render_tilde_literal(spec));
            current.push_str(&text, false);
            *has_emitted = true;
        }
        WordPart::Var { name, quoted: true } => {
            match shell.lookup_var(name) {
                Some(value) => current.push_str(&value, true),
                None => {
                    if shell.shell_options.nounset {
                        with_err(|err| e!(err, "huck: {name}: unbound variable"));
                        shell.pending_fatal_status = Some(1);
                        return ControlFlow::Break(());
                    }
                }
            }
            // Unset quoted var: relies on `has_emitted` so end-of-word
            // still produces a (possibly empty) Field.
            *has_emitted = true;
        }
        WordPart::LastStatus { quoted: true } => {
            current.push_str(&snapshot_status.to_string(), true);
            *has_emitted = true;
        }
        WordPart::Var { name, quoted: false } => {
            let value = match shell.lookup_var(name) {
                Some(v) => v,
                None => {
                    if shell.shell_options.nounset {
                        with_err(|err| e!(err, "huck: {name}: unbound variable"));
                        shell.pending_fatal_status = Some(1);
                        return ControlFlow::Break(());
                    }
                    String::new()
                }
            };
            let ifs = shell.ifs();
            emit_split_fields(&value, &ifs, current, result, has_emitted);
        }
        WordPart::AllArgs { quoted: false, joined: _ } => {
            // Unquoted $@ and $* are identical: each arg becomes its
            // own field(s), IFS-split. Args are independent — the
            // last IFS-fragment of arg N must NOT merge with the
            // first of arg N+1, so we flush current between args.
            let args = shell.positional_args.clone();
            let ifs = shell.ifs();
            for (i, arg) in args.iter().enumerate() {
                if i > 0 && !current.is_empty() {
                    result.push(std::mem::take(current));
                }
                emit_split_fields(arg, &ifs, current, result, has_emitted);
            }
        }
        WordPart::AllArgs { quoted: true, joined: false } => {
            // "$@" — each arg its own quoted field, no splitting.
            // First arg merges into current; subsequent start new
            // fields; last becomes the new current.
            let args = shell.positional_args.clone();
            if !args.is_empty() {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        // Start a new field for the next arg.
                        result.push(std::mem::take(current));
                    }
                    current.push_str(arg, true);
                    *has_emitted = true;
                }
            }
            // Empty args: zero fields — do nothing.
        }
        WordPart::AllArgs { quoted: true, joined: true } => {
            // "$*" — single field, args joined by the first IFS char.
            // Empty IFS concatenates without a separator (POSIX § 2.5.2).
            let sep = ifs_join_sep(&shell.ifs());
            let joined = shell.positional_args.join(&sep);
            current.push_str(&joined, true);
            *has_emitted = true;
        }
        WordPart::LastStatus { quoted: false } => {
            let value = snapshot_status.to_string();
            let ifs = shell.ifs();
            emit_split_fields(&value, &ifs, current, result, has_emitted);
        }
        WordPart::CommandSub { sequence, quoted: true } => {
            let output = run_substitution(sequence, shell);
            current.push_str(&output, true);
            *has_emitted = true;
        }
        WordPart::CommandSub { sequence, quoted: false } => {
            let output = run_substitution(sequence, shell);
            let ifs = shell.ifs();
            emit_split_fields(&output, &ifs, current, result, has_emitted);
        }
        WordPart::Arith { body, quoted: _ } => {
            let (src, res) = eval_arith_word_src(body, shell);
            match res {
                Ok(n) => {
                    current.push_str(&n.to_string(), true);
                    *has_emitted = true;
                }
                Err(e) => {
                    // Print the error. Default mode: NON-fatal (v215) — prints
                    // and continues; the empty contribution matches bash's empty
                    // $((..)) value on error. (-c mode divergence: L-55 in
                    // bash-divergences.md.) POSIX non-interactive: the shell
                    // exits (127) via posix_fatal (a no-op in default mode).
                    let prefix = shell.error_prefix(None);
                    with_err(|err| e!(err, "{prefix}{}", crate::arith::render_error_body(&src, &e)));
                    shell.posix_fatal(127);
                    *has_emitted = true;
                }
            }
        }
        WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect } => {
            // A lexable-but-invalid `${…}` (BadSubst) errors at runtime with
            // bash's whole-word "bad substitution" message (see emit_bad_subst).
            if emit_bad_subst(modifier, word, shell) {
                return ControlFlow::Break(());
            }
            // Task 2 (v234) promotes ${$'name'} to ParamExpansion{None}
            // instead of Var. Honor set -u for this exact shape so nounset
            // semantics are not silently dropped (regression fix F2).
            if matches!(modifier, crate::lexer::ParamModifier::None)
                && subscript.is_none()
                && !*indirect
            {
                if shell.lookup_var(name).is_none() && shell.shell_options.nounset {
                    with_err(|err| e!(err, "huck: {name}: unbound variable"));
                    shell.pending_fatal_status = Some(1);
                    return ControlFlow::Break(());
                }
            }
            // Substring on `$@` / `$*` is array-shaped (closes v33's
            // `${@:o:l}` deferral) — route through the shared
            // word-list path even though there's no `subscript`.
            let result_pe = if *indirect {
                expand_indirect(name, subscript.as_ref(), modifier, *quoted, shell)
            } else if let Some(sub) = subscript {
                expand_array_param(name, modifier, sub, *quoted, shell)
            } else if matches!(
                (name.as_str(), modifier),
                ("@" | "*", crate::lexer::ParamModifier::Substring { .. })
            ) {
                expand_positional_substring(name, modifier, *quoted, shell)
            } else {
                crate::param_expansion::expand_modifier_quoted(name, modifier, *quoted, shell)
            };
            match result_pe {
                crate::param_expansion::ExpansionResult::Value(v) => {
                    if *quoted {
                        current.push_str(&v, true);
                        *has_emitted = true;
                    } else {
                        let ifs = shell.ifs();
                        emit_split_fields(&v, &ifs, current, result, has_emitted);
                    }
                }
                crate::param_expansion::ExpansionResult::Empty => {
                    // A QUOTED empty expansion (`"${u+x}"` when unset) still
                    // contributes one empty field; an UNQUOTED one vanishes
                    // (contributes no field), matching bash. Setting
                    // has_emitted unconditionally injected a spurious empty
                    // field for unquoted `${x+alt}` / `${arr[@]+…}` (M-105).
                    if *quoted {
                        *has_emitted = true;
                    }
                }
                crate::param_expansion::ExpansionResult::WordList(words) => {
                    if *quoted {
                        // Quoted `@`-style: each element is its own
                        // field, no IFS-splitting. Mirrors the
                        // `"$@"` path above.
                        if !words.is_empty() {
                            for (i, w) in words.iter().enumerate() {
                                if i > 0 {
                                    result.push(std::mem::take(current));
                                }
                                current.push_str(w, true);
                                *has_emitted = true;
                            }
                        }
                    } else {
                        // Unquoted: join with first IFS char then
                        // let word-splitting do the rest.
                        let ifs = shell.ifs();
                        let sep = ifs_join_sep(&ifs);
                        let joined = words.join(&sep);
                        emit_split_fields(&joined, &ifs, current, result, has_emitted);
                    }
                }
                crate::param_expansion::ExpansionResult::Fields(fields) => {
                    // Substituted word of an UNQUOTED outer ${p+word} /
                    // ${p-word} (M-110). Each Field came from expand(word),
                    // so it already encodes per-char quoting; bash then
                    // word-splits the result, protecting quoted regions.
                    // Field boundaries from expand() (e.g. "${a[@]}"
                    // elements) are word boundaries; within each field we
                    // IFS-split only at UNQUOTED whitespace/IFS — so
                    // quoted-empty fields survive and quoted-spaced fields
                    // are not re-split.
                    let ifs = shell.ifs();
                    for (i, f) in fields.into_iter().enumerate() {
                        if i > 0 {
                            result.push(std::mem::take(current));
                        }
                        emit_split_field_quoted(
                            &f, &ifs, current, result, has_emitted,
                        );
                    }
                }
                crate::param_expansion::ExpansionResult::Fatal { status } => {
                    shell.pending_fatal_status = Some(status);
                    return ControlFlow::Break(());
                }
            }
        }
        WordPart::AssignPrefix { target, append } => {
            let mut lhs = render_assign_target(target, shell);
            lhs.push_str(if *append { "+=" } else { "=" });
            current.push_str(&lhs, true);
        }
        WordPart::ArrayLiteral(elems) => {
            let rendered = reconstruct_array_literal(elems, shell);
            current.push_str(&rendered, true);
        }
        WordPart::ProcessSub { sequence, dir } => {
            match crate::procsub::realize(sequence, dir.clone(), shell) {
                Ok((path, ps)) => {
                    shell.procsub_pending.push(ps);
                    // The realized path (/dev/fd/N or a FIFO) is a single
                    // non-splitting, non-glob field — mirror the
                    // `CommandSub { quoted: true }` treatment.
                    current.push_str(&path, true);
                    *has_emitted = true;
                }
                Err(e) => {
                    with_err(|err| e!(err, "huck: process substitution: {}", crate::bash_io_error(&e)));
                    // Emit nothing; the field stays empty if no other parts.
                }
            }
        }
        WordPart::Quoted { parts, .. } => {
            // Delegate to each inner part. Inner parts already carry their
            // individual `quoted: true` flags so expansion semantics are
            // unchanged; the wrapper exists only for source reconstruction.
            for inner in parts {
                if expand_part(inner, current, result, has_emitted, shell, snapshot_status, word).is_break() {
                    return ControlFlow::Break(());
                }
            }
        }
        _ => {
            // Forward-compatible catchall for future WordPart variants
            // added by huck-syntax. Emit nothing — preserves the
            // has_emitted state without producing spurious fields.
        }
    }
    ControlFlow::Continue(())
}

/// Expands a `Word` against the current `Shell` state into 0 or more
/// `Field`s. Quoted variable references append their value verbatim;
/// unquoted references split on ASCII whitespace and can yield multiple
/// fields (or zero, for an empty value).
///
/// Per-WordPart quoting propagation (v10 Task 5): each char appended to a
/// `Field` carries the `quoted` flag of its source `WordPart`. Tilde
/// expansions and IFS-split fragments are always marked unquoted. This
/// preserves the information that pathname expansion (glob) needs to skip
/// quoted metacharacters.
pub fn expand(word: &Word, shell: &mut Shell) -> Vec<Field> {
    // Snapshot $? at the start so every `LastStatus` part in this word sees
    // the same value — even if a `CommandSub` part earlier in the word
    // updates the live $?. This matches bash: substitutions update $? for
    // the next command, not for `$?` references in the same expansion.
    let snapshot_status = shell.last_status();
    let mut current = Field::new();
    let mut has_emitted = false;
    let mut result: Vec<Field> = Vec::new();

    for part in &word.0 {
        if expand_part(part, &mut current, &mut result, &mut has_emitted, shell, snapshot_status, word).is_break() {
            return result;
        }
    }

    // End-of-word: push the in-progress field if it's non-empty, OR if
    // `has_emitted` is true (preserves the "this word produced something —
    // possibly an empty arg from `""` or a `"$UNSET"`" semantic).
    if !current.is_empty() || has_emitted {
        result.push(current);
    }
    result
}

/// Render an `AssignTarget` LHS back to text: `name` or `name[<subscript>]`.
fn render_assign_target(target: &crate::command::AssignTarget, shell: &mut Shell) -> String {
    use crate::command::AssignTarget;
    match target {
        AssignTarget::Bare(name) => name.clone(),
        AssignTarget::Indexed { name, subscript } => {
            format!("{name}[{}]", expand_assignment(subscript, shell))
        }
    }
}

/// Render ONE array-literal element value for re-parse via `eval`/`declare`.
/// bash expands the literal word ONCE here (quote removal + variable/glob/etc.
/// expansion) and reconstructs WITHOUT re-quoting, so the re-parser word-splits
/// on the resulting text (`eval x=("a b" c)` -> elements `a` `b` `c`). We mirror
/// that: a purely-literal value verbatim (the common `a`/`b` fast path), else
/// the expanded text verbatim — NO re-quoting.
fn render_elem_value(v: &crate::lexer::Word, shell: &mut Shell) -> String {
    match crate::command::word_literal_text(v) {
        Some(t) => t.to_string(),
        None => expand_assignment(v, shell),
    }
}

/// Reconstruct an array literal to re-parseable `(e1 e2 [k]=v …)` text.
pub(crate) fn reconstruct_array_literal(
    elems: &[crate::lexer::ArrayLiteralElement],
    shell: &mut Shell,
) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(elems.len());
    for e in elems {
        match &e.subscript {
            Some(sub) => parts.push(format!(
                "[{}]={}",
                expand_assignment(sub, shell),
                render_elem_value(&e.value, shell)
            )),
            None => parts.push(render_elem_value(&e.value, shell)),
        }
    }
    format!("({})", parts.join(" "))
}

/// Re-render a parsed `Word` back to its (approximate) SOURCE text, UNEXPANDED.
/// Used for `set -x` traces of compound-command headers (case/for/select/arith),
/// which bash shows as the raw source word, not the expanded value. Pure — no
/// `Shell`, no expansion. Quote *style* is not recoverable (`'x'`/`"x"`/`x` all
/// render as `x`); deeply-nested command substitutions render their inner
/// command best-effort (single pipeline of simple commands; see
/// `reconstruct_sequence_source`).
pub(crate) fn reconstruct_word_source(word: &Word) -> String {
    let mut out = String::new();
    let parts = &word.0;
    let mut i = 0;
    while i < parts.len() {
        if part_is_quoted(&parts[i]) {
            // Maximal run of quoted parts -> one "..." group (matches bash's
            // xtrace; the single-vs-double quote char is not recoverable, so
            // double is used).
            out.push('"');
            while i < parts.len() && part_is_quoted(&parts[i]) {
                reconstruct_part(&parts[i], &mut out);
                i += 1;
            }
            out.push('"');
        } else {
            reconstruct_part(&parts[i], &mut out);
            i += 1;
        }
    }
    out
}

/// Like `reconstruct_word_source` but WITHOUT wrapping quoted runs in `"..."`.
/// Used for nested sub-words and for `(( ))` arith bodies, whose literals are
/// spuriously marked `quoted` by `arith_string_to_word` (bash shows arith bodies
/// raw, never quoted).
pub(crate) fn reconstruct_word_source_inner(word: &Word) -> String {
    let mut out = String::new();
    for part in &word.0 {
        reconstruct_part(part, &mut out);
    }
    out
}

/// If `modifier` is a lexable-but-invalid `${…}` (`BadSubst`), emit bash's
/// runtime "bad substitution" error and stash fatal status 1, then return
/// `true` so the caller bails. bash reports the ENTIRE enclosing word's source
/// (e.g. `a${-3}b`, `[${-3}]`), not just the offending `${…}` token — so every
/// word-expansion path (`expand`, `expand_assignment`, `expand_pattern`,
/// `expand_regex_operand`) routes through here with the whole `word` in scope.
/// (The token-only fallback in `param_expansion.rs` remains for any caller that
/// expands a modifier without a surrounding word, e.g. arithmetic operands.)
fn emit_bad_subst(modifier: &crate::lexer::ParamModifier, word: &Word, shell: &mut Shell) -> bool {
    if let crate::lexer::ParamModifier::BadSubst { .. } = modifier {
        let prefix = shell.error_prefix(None);
        let src = reconstruct_word_source_inner(word);
        with_err(|err| e!(err, "{prefix}{src}: bad substitution"));
        shell.pending_fatal_status = Some(1);
        true
    } else {
        false
    }
}

fn part_is_quoted(part: &WordPart) -> bool {
    use crate::lexer::WordPart as P;
    matches!(
        part,
        P::Literal { quoted: true, .. }
            | P::Var { quoted: true, .. }
            | P::LastStatus { quoted: true }
            | P::CommandSub { quoted: true, .. }
            | P::Arith { quoted: true, .. }
            | P::ParamExpansion { quoted: true, .. }
            | P::AllArgs { quoted: true, .. }
            | P::Quoted { .. }
    )
}

fn reconstruct_part(part: &WordPart, out: &mut String) {
    use crate::lexer::{ProcDir, WordPart as P};
    match part {
        P::Literal { text, .. } => out.push_str(text),
        P::Var { name, .. } => {
            out.push('$');
            out.push_str(name);
        }
        P::LastStatus { .. } => out.push_str("$?"),
        P::AllArgs { joined, .. } => out.push_str(if *joined { "$*" } else { "$@" }),
        P::Arith { body, .. } => {
            out.push_str("$((");
            out.push_str(&reconstruct_word_source_inner(body));
            out.push_str("))");
        }
        P::Tilde(spec) => out.push_str(&render_tilde_literal(spec)),
        P::CommandSub { sequence, .. } => {
            out.push_str("$(");
            out.push_str(&reconstruct_sequence_source(sequence));
            out.push(')');
        }
        P::ProcessSub { sequence, dir } => {
            out.push_str(match dir { ProcDir::In => "<(", ProcDir::Out => ">(" });
            out.push_str(&reconstruct_sequence_source(sequence));
            out.push(')');
        }
        P::ParamExpansion { name, modifier, subscript, indirect, .. } => {
            reconstruct_param_expansion(name, modifier, subscript.as_ref(), *indirect, out);
        }
        P::AssignPrefix { .. } | P::ArrayLiteral(_) => {}
        P::Quoted { parts, .. } => {
            // Recurse so quoted content still appears in xtrace output.
            for inner in parts {
                reconstruct_part(inner, out);
            }
        }
        _ => {
            // Forward-compatible: unknown future WordPart renders as nothing.
        }
    }
}

fn reconstruct_param_expansion(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    subscript: Option<&crate::lexer::SubscriptKind>,
    indirect: bool,
    out: &mut String,
) {
    use crate::lexer::{ParamModifier as M, SubstAnchor, CaseDirection, SubscriptKind as S, TransformOp};
    // A bad substitution carries its full `${…}` source verbatim; emit it as-is
    // so `set -x` traces reproduce the original (matches generate.rs).
    if let M::BadSubst { raw } = modifier {
        out.push_str(raw);
        return;
    }
    // `${!prefix*}` / `${!prefix@}` — the `!` is a prefix and `*`/`@` a
    // suffix, so it doesn't fit the generic `${[!][#]name[sub]MOD}` shape.
    if let M::PrefixNames { at } = modifier {
        out.push_str("${!");
        out.push_str(name);
        out.push(if *at { '@' } else { '*' });
        out.push('}');
        return;
    }
    out.push_str("${");
    if indirect || matches!(modifier, M::IndirectKeys) {
        out.push('!');
    }
    if matches!(modifier, M::Length) {
        out.push('#');
    }
    out.push_str(name);
    match subscript {
        None => {}
        Some(S::All) => out.push_str("[@]"),
        Some(S::Star) => out.push_str("[*]"),
        Some(S::Index(w)) => {
            out.push('[');
            out.push_str(&reconstruct_word_source_inner(w));
            out.push(']');
        }
    }
    match modifier {
        M::None | M::Length | M::IndirectKeys => {}
        M::UseDefault { word, colon } => {
            out.push_str(if *colon { ":-" } else { "-" });
            out.push_str(&reconstruct_word_source_inner(word));
        }
        M::AssignDefault { word, colon } => {
            out.push_str(if *colon { ":=" } else { "=" });
            out.push_str(&reconstruct_word_source_inner(word));
        }
        M::ErrorIfUnset { word, colon } => {
            out.push_str(if *colon { ":?" } else { "?" });
            out.push_str(&reconstruct_word_source_inner(word));
        }
        M::UseAlternate { word, colon } => {
            out.push_str(if *colon { ":+" } else { "+" });
            out.push_str(&reconstruct_word_source_inner(word));
        }
        M::RemovePrefix { pattern, longest } => {
            out.push_str(if *longest { "##" } else { "#" });
            out.push_str(&reconstruct_word_source_inner(pattern));
        }
        M::RemoveSuffix { pattern, longest } => {
            out.push_str(if *longest { "%%" } else { "%" });
            out.push_str(&reconstruct_word_source_inner(pattern));
        }
        M::Substitute { pattern, replacement, anchor, all } => {
            out.push('/');
            if *all { out.push('/'); }
            match anchor {
                SubstAnchor::None => {}
                SubstAnchor::Prefix => out.push('#'),
                SubstAnchor::Suffix => out.push('%'),
            }
            out.push_str(&reconstruct_word_source_inner(pattern));
            out.push('/');
            out.push_str(&reconstruct_word_source_inner(replacement));
        }
        M::Substring { offset, length } => {
            out.push(':');
            out.push_str(&reconstruct_word_source_inner(offset));
            if let Some(len) = length {
                out.push(':');
                out.push_str(&reconstruct_word_source_inner(len));
            }
        }
        M::Case { direction, all, pattern } => {
            let c = match direction { CaseDirection::Upper => '^', CaseDirection::Lower => ',' };
            out.push(c);
            if *all { out.push(c); }
            if let Some(p) = pattern {
                out.push_str(&reconstruct_word_source_inner(p));
            }
        }
        M::Transform { op } => {
            out.push('@');
            out.push(match op {
                TransformOp::PromptExpand => 'P',
                TransformOp::Quote => 'Q',
                TransformOp::Upper => 'U',
                TransformOp::Lower => 'L',
                TransformOp::UpperFirst => 'u',
                TransformOp::EscapeExpand => 'E',
                TransformOp::AssignDecl => 'A',
                TransformOp::KvString => 'K',
                TransformOp::KvWords => 'k',
                TransformOp::AttrFlags => 'a',
                _ => '?',
            });
        }
        _ => {
            // Forward-compatible: unknown future ParamModifier renders as bare.
        }
    }
    out.push('}');
}

/// Best-effort source for a `$(…)` / `<(…)` body: renders the command list with
/// its real connectors (`a && b`, `a; b`, `a & b`). A compound command inside the
/// list falls back to empty per `reconstruct_command_source` (documented
/// approximation — rare in a trace header).
fn reconstruct_sequence_source(seq: &crate::command::Sequence) -> String {
    use crate::command::Connector;
    let mut s = reconstruct_command_source(&seq.first);
    for (conn, cmd) in &seq.rest {
        s.push_str(match conn {
            Connector::Semi => "; ",
            Connector::And => " && ",
            Connector::Or => " || ",
            Connector::Amp => " & ",
        });
        s.push_str(&reconstruct_command_source(cmd));
    }
    s
}

fn reconstruct_command_source(cmd: &crate::command::Command) -> String {
    use crate::command::{Command, SimpleCommand};
    match cmd {
        Command::Simple(SimpleCommand::Exec(e)) => {
            let mut parts = vec![reconstruct_word_source(&e.program)];
            parts.extend(e.args.iter().map(reconstruct_word_source));
            parts.join(" ")
        }
        Command::Pipeline(p) => p
            .commands
            .iter()
            .map(reconstruct_command_source)
            .collect::<Vec<_>>()
            .join(" | "),
        _ => String::new(),
    }
}

/// Expands a `Word` for assignment context: word-splitting is suppressed and
/// the result is one string. Each `Var`/`LastStatus`/`CommandSub` part
/// contributes its value verbatim regardless of the `quoted` flag — matching
/// bash, which disables splitting on the right-hand side of `NAME=...`.
pub fn expand_assignment(word: &Word, shell: &mut Shell) -> String {
    // Snapshot $? so `LastStatus` parts read the value at the start of
    // expansion, not whatever a preceding `$(cmd)` mutated it to. Same
    // contract as `expand()` and `expand_pattern()`.
    let snapshot_status = shell.last_status();
    let mut result = String::new();
    for part in &word.0 {
        match part {
            WordPart::Literal { text, .. } => result.push_str(text),
            WordPart::Tilde(spec) => {
                let text = resolve_tilde(spec, shell)
                    .unwrap_or_else(|| render_tilde_literal(spec));
                result.push_str(&text);
            }
            WordPart::Var { name, .. } => {
                match shell.lookup_var(name) {
                    Some(value) => result.push_str(&value),
                    None => {
                        if shell.shell_options.nounset {
                            with_err(|err| e!(err, "huck: {name}: unbound variable"));
                            shell.pending_fatal_status = Some(1);
                            return result;
                        }
                    }
                }
            }
            WordPart::LastStatus { .. } => {
                result.push_str(&snapshot_status.to_string());
            }
            WordPart::CommandSub { sequence, .. } => {
                result.push_str(&run_substitution(sequence, shell));
            }
            WordPart::Arith { body, quoted: _ } => {
                let (src, res) = eval_arith_word_src(body, shell);
                match res {
                    Ok(n) => result.push_str(&n.to_string()),
                    Err(e) => {
                        // Print the error but DO NOT halt — bash script-file
                        // mode prints and continues. Empty contribution to
                        // the assignment value matches bash. (-c mode
                        // divergence: L-55.)
                        let prefix = shell.error_prefix(None);
                        with_err(|err| e!(err, "{prefix}{}", crate::arith::render_error_body(&src, &e)));
                    }
                }
            }
            WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect } => {
                if emit_bad_subst(modifier, word, shell) {
                    return result;
                }
                let result_pe = if *indirect {
                    expand_indirect(name, subscript.as_ref(), modifier, *quoted, shell)
                } else if let Some(sub) = subscript {
                    expand_array_param(name, modifier, sub, *quoted, shell)
                } else if matches!(
                    (name.as_str(), modifier),
                    ("@" | "*", crate::lexer::ParamModifier::Substring { .. })
                ) {
                    expand_positional_substring(name, modifier, *quoted, shell)
                } else {
                    crate::param_expansion::expand_modifier_quoted(name, modifier, *quoted, shell)
                };
                match result_pe {
                    crate::param_expansion::ExpansionResult::Value(v) => result.push_str(&v),
                    crate::param_expansion::ExpansionResult::Empty => {}
                    crate::param_expansion::ExpansionResult::WordList(words) => {
                        // Assignment context: no field splitting. Join
                        // with first IFS char (matches `${a[*]}` and the
                        // existing `WordPart::AllArgs` assignment path).
                        let ifs = shell.ifs();
                        let sep = ifs_join_sep(&ifs);
                        result.push_str(&words.join(&sep));
                    }
                    crate::param_expansion::ExpansionResult::Fields(fields) => {
                        let ifs = shell.ifs();
                        let sep = ifs_join_sep(&ifs);
                        let joined = fields
                            .iter()
                            .map(|f| f.chars.as_str())
                            .collect::<Vec<_>>()
                            .join(&sep);
                        result.push_str(&joined);
                    }
                    crate::param_expansion::ExpansionResult::Fatal { status } => {
                        shell.pending_fatal_status = Some(status);
                        return result;
                    }
                }
            }
            WordPart::AllArgs { .. } => {
                // No field splitting in assignment context; join with space.
                let joined = shell.positional_args.join(" ");
                result.push_str(&joined);
            }
            WordPart::AssignPrefix { target, append } => {
                result.push_str(&render_assign_target(target, shell));
                result.push_str(if *append { "+=" } else { "=" });
            }
            WordPart::ArrayLiteral(elems) => {
                result.push_str(&reconstruct_array_literal(elems, shell));
            }
            WordPart::ProcessSub { .. } => {
                // Process substitution is meaningful only in command-argument /
                // redirect-target expansion (the main expand() path). Realizing
                // it here (assignment context, no command to consume the fd)
                // would leak an fd and a child process with no benefit. No-op.
            }
            WordPart::Quoted { parts, .. } => {
                // Delegate to each inner part. Inner parts carry their own
                // `quoted` flags so expansion semantics are unchanged; the
                // wrapper exists only for source reconstruction.
                for inner in parts {
                    result.push_str(&expand_assignment(&Word(vec![inner.clone()]), shell));
                    if shell.pending_fatal_status.is_some() {
                        return result;
                    }
                }
            }
            _ => {
                // Forward-compatible: unknown future WordPart contributes nothing
                // to the assignment value.
            }
        }
    }
    result
}

/// True when `part` carried a `quoted` flag set to true. Tilde parts
/// have no quoted flag and count as unquoted.
fn word_part_is_quoted(part: &WordPart) -> bool {
    match part {
        WordPart::Literal { quoted, .. } => *quoted,
        WordPart::Var { quoted, .. } => *quoted,
        WordPart::LastStatus { quoted } => *quoted,
        WordPart::CommandSub { quoted, .. } => *quoted,
        WordPart::Arith { quoted, .. } => *quoted,
        WordPart::ParamExpansion { quoted, .. } => *quoted,
        WordPart::AllArgs { quoted, .. } => *quoted,
        WordPart::Tilde(_) => false,
        WordPart::AssignPrefix { .. } | WordPart::ArrayLiteral(_) => false,
        // ProcessSub expands to a single /dev/fd/N path — treated as quoted
        // (no IFS-splitting, no glob expansion of the realized path).
        WordPart::ProcessSub { .. } => true,
        // A Quoted wrapper always means the content was quoted at source.
        WordPart::Quoted { .. } => true,
        // Forward-compatible: future WordPart variants default to unquoted.
        _ => false,
    }
}

/// Escapes a quoted span so its metacharacters match literally — both the
/// `glob`-crate wildcards (`* ? [ ]`, via `glob::Pattern::escape`) AND the
/// extglob structural chars `| ( )` (wrapped as single-char classes `[|]`/
/// `[(]`/`[)]`, which are literal-equivalent in both the `glob` crate and the
/// extglob engine). Without the extra step, a quoted `|`/`(`/`)` inside an
/// extglob group (e.g. `@("a|b")`) would be parsed as alternation/group syntax.
fn escape_pattern_literal(text: &str) -> String {
    // `glob::Pattern::escape` only emits `[?]`/`[*]`/`[[]`/`[]]`, so it never
    // introduces a bare `|`/`(`/`)` — the replaces below can't double-escape.
    glob::Pattern::escape(text)
        .replace('|', "[|]")
        .replace('(', "[(]")
        .replace(')', "[)]")
}

/// Expands `word` into a glob-pattern string for `case` matching.
/// Like `expand_assignment` (no field splitting), but text contributed
/// by a quoted part is escaped via `escape_pattern_literal`, so a quoted
/// `*`/`?`/`[`/`|`/`(`/`)` matches literally while an unquoted one is special.
pub fn expand_pattern(word: &Word, shell: &mut Shell) -> String {
    // Snapshot `$?` so `LastStatus` parts read the value at the start of
    // the expansion, not whatever a preceding `$(cmd)` mutated it to.
    // Matches the contract in `expand()` (used for command arguments).
    let snapshot_status = shell.last_status();
    let mut result = String::new();
    for part in &word.0 {
        // A BadSubst part errors with bash's whole-word message; intercept here
        // (with the outer `word`) before delegating per-part to expand_assignment,
        // which would otherwise only see the single-part sub-word.
        if let WordPart::ParamExpansion { modifier, .. } = part {
            if emit_bad_subst(modifier, word, shell) {
                return result;
            }
        }
        let text = if matches!(part, WordPart::LastStatus { .. }) {
            snapshot_status.to_string()
        } else {
            expand_assignment(&Word(vec![part.clone()]), shell)
        };
        if shell.pending_fatal_status.is_some() {
            return result;
        }
        if word_part_is_quoted(part) {
            result.push_str(&escape_pattern_literal(&text));
        } else {
            result.push_str(&text);
        }
    }
    result
}

/// Expands `word` into a regex string for `[[ … =~ … ]]` matching. Like
/// `expand_pattern` (no field splitting), but text contributed by a QUOTED part
/// is `regex::escape`d, so a quoted `.`/`+`/`*`/`(`/`|`/etc. matches LITERALLY
/// while an unquoted one stays an active regex metacharacter (bash 3.2+). An
/// unquoted `$var` expands to an active regex; a quoted `"$var"` is literal.
pub fn expand_regex_operand(word: &Word, shell: &mut Shell) -> String {
    // Snapshot `$?` so `LastStatus` parts read the value at the start of the
    // expansion (same contract as `expand_pattern`).
    let snapshot_status = shell.last_status();
    let mut result = String::new();
    for part in &word.0 {
        // A BadSubst part errors with bash's whole-word message; intercept here
        // (with the outer `word`) before delegating per-part to expand_assignment,
        // which would otherwise only see the single-part sub-word.
        if let WordPart::ParamExpansion { modifier, .. } = part {
            if emit_bad_subst(modifier, word, shell) {
                return result;
            }
        }
        let text = if matches!(part, WordPart::LastStatus { .. }) {
            snapshot_status.to_string()
        } else {
            expand_assignment(&Word(vec![part.clone()]), shell)
        };
        if shell.pending_fatal_status.is_some() {
            return result;
        }
        if word_part_is_quoted(part) {
            result.push_str(&regex::escape(&text));
        } else {
            result.push_str(&text);
        }
    }
    result
}

/// Runs a sub-sequence as a substituted command: clones the parent `Shell`
/// (so state mutations don't leak), captures stdout via the executor's
/// `execute_capturing`, strips trailing newlines, and propagates the
/// substituted command's exit status into the parent shell's `$?`.
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();
    cloned.procsub_pending = Vec::new(); // a clone must not inherit/duplicate the parent's pending process substitutions
    cloned.xtrace_depth += 1; // PS4 depth-repeat: $() / backticks add a level (bash)
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    shell.set_last_status(status);
    shell.set_last_cmd_sub_status(Some(status));   // for bare-assignment exit status (v126)
    strip_trailing_newlines(&output)
}

fn strip_trailing_newlines(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

/// Returns the separator for `"$*"` / `"${a[*]}"` joins.
/// Empty IFS → empty separator (concatenate). Otherwise → first char of
/// IFS. Matches bash § 3.5.5 ("If IFS is null, the parameters are joined
/// without intervening separators").
pub(crate) fn ifs_join_sep(ifs: &str) -> String {
    ifs.chars().next().map(|c| c.to_string()).unwrap_or_default()
}

fn emit_split_fields(
    value: &str,
    ifs: &str,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
) {
    // POSIX § 2.6.5 field splitting. Two IFS classes:
    //   - whitespace IFS: subset of IFS bytes that are ' ' / '\t' / '\n'.
    //   - non-whitespace IFS: any other IFS byte.
    // Empty IFS → no splitting; value joins the in-progress field.
    if ifs.is_empty() {
        if !value.is_empty() {
            current.push_str(value, false);
            *has_emitted = true;
        }
        return;
    }

    let ifs_bytes = ifs.as_bytes();
    let is_ws = |b: u8| ifs_bytes.contains(&b) && matches!(b, b' ' | b'\t' | b'\n');
    let is_nonws = |b: u8| ifs_bytes.contains(&b) && !matches!(b, b' ' | b'\t' | b'\n');
    let is_any_ifs = |b: u8| ifs_bytes.contains(&b);

    let bytes = value.as_bytes();
    let mut i = 0usize;

    // Skip leading IFS-whitespace.
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() {
        return;
    }

    let mut first_field = true;

    while i < bytes.len() {
        // Read one field (non-IFS bytes).
        let field_start = i;
        while i < bytes.len() && !is_any_ifs(bytes[i]) {
            i += 1;
        }
        let field_end = i;
        let field_str = std::str::from_utf8(&bytes[field_start..field_end]).unwrap_or("");

        if first_field {
            current.push_str(field_str, false);
            *has_emitted = true;
            first_field = false;
        } else {
            let finished = std::mem::take(current);
            result.push(finished);
            current.push_str(field_str, false);
        }

        if i >= bytes.len() {
            break;
        }

        // We're now sitting on an IFS byte. Classify the separator run.
        //   - If the FIRST IFS byte is non-whitespace, consume EXACTLY one
        //     non-ws byte plus any trailing whitespace-IFS. This produces
        //     one separator. Continue (empty field next if another non-ws
        //     follows immediately).
        //   - If the first IFS byte is whitespace, consume the whole
        //     whitespace run. Then OPTIONALLY consume one non-whitespace
        //     IFS byte plus its trailing whitespace-IFS run.
        if is_nonws(bytes[i]) {
            i += 1;
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
        } else {
            // Whitespace IFS run.
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() && is_nonws(bytes[i]) {
                i += 1;
                while i < bytes.len() && is_ws(bytes[i]) {
                    i += 1;
                }
            }
        }

        // If we consumed all remaining input as a separator, do NOT emit
        // a trailing empty field. POSIX: "If the input string ends with a
        // non-whitespace IFS character, that delimiter does not produce
        // an empty field." (Bash: `IFS=:; v="a:"; echo $v` → `a`.)
        if i >= bytes.len() {
            break;
        }
    }
}

/// IFS-splits a single already-expanded `Field`, honoring its per-char
/// `quoted` mask: only UNQUOTED IFS characters act as separators, so quoted
/// regions (incl. quoted whitespace and quoted empties) survive intact. The
/// surviving characters keep their original `quoted` flags. Used by the
/// `ExpansionResult::Fields` consumer for an UNQUOTED outer `${p+word}` /
/// `${p-word}` (M-110): bash word-splits the substituted word's expansion
/// but protects its quoted regions.
fn emit_split_field_quoted(
    field: &Field,
    ifs: &str,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
) {
    let chars: Vec<char> = field.chars.chars().collect();
    // A char is a separator iff it is an IFS char AND unquoted.
    let sep_at = |idx: usize| -> bool {
        !field.quoted.get(idx).copied().unwrap_or(false)
            && ifs.contains(chars[idx])
    };
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\n') && ifs.contains(c);

    let n = chars.len();

    // A zero-length field (from a quoted-empty `""` inner) is a real, empty
    // word unit — mark emitted so it survives even though splitting adds
    // nothing. (A NON-empty field that splits away to nothing — e.g. all
    // unquoted whitespace — must NOT force a word; the loop decides.)
    if n == 0 {
        *has_emitted = true;
        return;
    }

    // Empty IFS → no splitting; append the whole field verbatim.
    if ifs.is_empty() {
        for (idx, c) in chars.iter().enumerate() {
            current.push_str(&c.to_string(), field.quoted.get(idx).copied().unwrap_or(false));
        }
        *has_emitted = true;
        return;
    }
    let mut i = 0usize;
    // Skip leading IFS-whitespace separators.
    while i < n && sep_at(i) && is_ws(chars[i]) {
        i += 1;
    }

    let mut first_field = true;
    let mut produced_any = false;
    while i < n {
        // Read one field: run of chars that are not unquoted-IFS separators.
        let mut piece = Field::new();
        while i < n && !sep_at(i) {
            piece.push_str(&chars[i].to_string(), field.quoted.get(i).copied().unwrap_or(false));
            i += 1;
        }
        if first_field {
            current.chars.push_str(&piece.chars);
            current.quoted.extend(piece.quoted);
            *has_emitted = true;
            first_field = false;
        } else {
            result.push(std::mem::take(current));
            *current = piece;
        }
        produced_any = true;

        if i >= n {
            break;
        }
        // Sitting on an unquoted IFS separator. Mirror emit_split_fields.
        if !is_ws(chars[i]) {
            // Non-whitespace IFS: consume one, plus trailing ws-IFS.
            i += 1;
            while i < n && sep_at(i) && is_ws(chars[i]) {
                i += 1;
            }
        } else {
            // Whitespace IFS run.
            while i < n && sep_at(i) && is_ws(chars[i]) {
                i += 1;
            }
            if i < n && sep_at(i) && !is_ws(chars[i]) {
                i += 1;
                while i < n && sep_at(i) && is_ws(chars[i]) {
                    i += 1;
                }
            }
        }
        if i >= n {
            break;
        }
    }

    // A wholly-quoted field with no chars at all (e.g. `${x+""}` quoted-empty
    // inner) still must contribute an empty field: expand() already emitted a
    // zero-length Field, so reaching here with n==0 means append nothing —
    // the empty Field was its own element and is preserved by the caller's
    // per-field push.
    let _ = produced_any;
}

/// Result of opts-aware pathname expansion. `words` are the expanded fields;
/// `failglob_unmatched` lists patterns that matched nothing under `failglob`
/// (the caller turns a non-empty list into a command abort with status 1).
pub struct GlobExpansion {
    pub words: Vec<String>,
    pub failglob_unmatched: Vec<String>,
}

/// Pathname expansion honoring `shopt` glob toggles. See `GlobOpts`.
///
/// For fields with no unquoted glob metacharacters, the field passes through
/// as-is. For fields with unquoted metacharacters, builds a glob pattern
/// (escaping quoted metachars via bracket expressions) and invokes the `glob`
/// crate. No-match behavior depends on `opts`: `failglob` records the pattern
/// for the caller to abort, `nullglob` contributes nothing, otherwise the
/// literal field survives (bash default).
pub fn glob_expand_fields_opts(fields: Vec<Field>, opts: GlobOpts) -> GlobExpansion {
    let mut words = Vec::new();
    let mut failglob_unmatched = Vec::new();
    for field in fields {
        if opts.noglob {
            words.push(field.chars);
            continue;
        }
        let pattern = build_glob_pattern(&field);
        // Route POSIX-class patterns through the own-matcher too (the glob
        // crate lacks [:name:]); unconditional on the extglob shopt.
        let is_extglob = (opts.extglob && crate::glob_match::has_extglob(&pattern))
            || crate::glob_match::has_posix_class(&pattern);

        // No globbing needed: not a wildcard field AND not an extglob field.
        if !has_unquoted_metachar(&field) && !is_extglob {
            words.push(field.chars);
            continue;
        }

        let matched: Vec<String> = if is_extglob {
            crate::glob_match::extglob_pathname_expand(&pattern, opts.nocaseglob, opts.dotglob)
        } else {
            // Existing `glob` crate path (unchanged behavior for plain globs).
            // Bash semantics: a leading literal `.` in the pattern matches a
            // leading `.` in filenames; otherwise `*` and `?` never match one.
            // The `glob` crate's `require_literal_leading_dot=true` enforces the
            // "never" rule but also blocks an explicit dot-prefix pattern (`.*`,
            // `.foo`, or a bracket class like `[.]*`) from matching dotfiles, so
            // we toggle it off when the pattern's effective first char is a
            // literal `.`. We accept both bare `.` and the `[.]` single-element
            // bracket form (verified empirically against `glob` 0.3). `dotglob`
            // forces `*`/`?` to also match a leading dot.
            let literal_leading_dot =
                pattern.starts_with('.') || pattern.starts_with("[.]");
            let match_opts = MatchOptions {
                case_sensitive: !opts.nocaseglob,
                require_literal_separator: true,
                require_literal_leading_dot: !literal_leading_dot && !opts.dotglob,
            };
            let npat = crate::glob_match::translate_bracket_negation(&pattern);
            // `**` is recursive only with `shopt -s globstar`; otherwise it is
            // two ordinary `*` (≡ `*`). The `glob` crate always treats `**` as
            // recursive, so collapse it to `*` when globstar is off.
            let npat = if opts.globstar { npat } else { collapse_globstar(&npat).into() };
            match glob_with(&npat, match_opts) {
                Ok(paths) => {
                    let mut m = Vec::new();
                    for entry in paths {
                        let Ok(path) = entry else { continue };
                        match path.into_os_string().into_string() {
                            Ok(s) => m.push(s),
                            Err(_) => with_err(|err| e!(err, "huck: skipping non-UTF8 path")),
                        }
                    }
                    // Defensive: filter `.` and `..` if the glob crate ever emits
                    // them for patterns like `.*`. (Current versions exclude them
                    // under require_literal_leading_dot, but explicit filtering
                    // makes the contract loud — and `dotglob` keeps it relevant.)
                    m.retain(|p| {
                        let last = std::path::Path::new(p).file_name().and_then(|s| s.to_str());
                        !matches!(last, Some(".") | Some(".."))
                    });
                    m
                }
                Err(_) => {
                    // Invalid glob pattern → literal fallback (unchanged).
                    words.push(field.chars);
                    continue;
                }
            }
        };

        if matched.is_empty() {
            if opts.failglob {
                failglob_unmatched.push(field.chars);
            } else if opts.nullglob {
                // contribute nothing
            } else {
                words.push(field.chars);
            }
        } else {
            words.extend(matched);
        }
    }
    GlobExpansion { words, failglob_unmatched }
}

/// Back-compat: default (all-off) globbing. Retained as a thin wrapper over
/// `glob_expand_fields_opts` for the module's own glob tests, which assert the
/// default (pre-v86) behavior is preserved. Production call sites now go
/// through `glob_expand_fields_opts` (via `executor::glob_expand_word`).
#[cfg(test)]
pub fn glob_expand_fields(fields: Vec<Field>) -> Vec<String> {
    glob_expand_fields_opts(fields, GlobOpts::default()).words
}

/// Builds the glob pattern string for a `Field`: quoted metacharacters
/// (`*`, `?`, `[`, `]`) are escaped via one-char bracket expressions
/// (`[*]`, `[?]`, `[[]`, `[]]`), so the `glob` crate treats them as literal.
/// Unquoted chars pass through verbatim.
fn build_glob_pattern(field: &Field) -> String {
    let mut p = String::new();
    for (c, &q) in field.chars.chars().zip(field.quoted.iter()) {
        if q && matches!(c, '*' | '?' | '[' | ']' | '|' | '(' | ')') {
            p.push('[');
            p.push(c);
            p.push(']');
        } else {
            p.push(c);
        }
    }
    p
}

/// Collapses a run of consecutive `*` to a single `*` (`**`→`*`, `***`→`*`),
/// matching bash when `shopt globstar` is OFF (two `*` are just one). Skips `*`
/// inside a `[…]` bracket class and honors `\`-escapes, so `[**]` and `\*\*`
/// are untouched.
fn collapse_globstar(pat: &str) -> String {
    let mut out = String::with_capacity(pat.len());
    let mut chars = pat.chars().peekable();
    let mut in_bracket = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '[' if !in_bracket => {
                in_bracket = true;
                out.push('[');
            }
            ']' if in_bracket => {
                in_bracket = false;
                out.push(']');
            }
            '*' if !in_bracket => {
                out.push('*');
                while chars.peek() == Some(&'*') {
                    chars.next();
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Checks whether a field contains any unquoted glob metacharacters: `*`, `?`, `[`.
fn has_unquoted_metachar(field: &Field) -> bool {
    field
        .chars
        .chars()
        .zip(field.quoted.iter())
        .any(|(c, &q)| !q && matches!(c, '*' | '?' | '['))
}

#[cfg(test)]
impl Field {
    pub fn from_unquoted(s: &str) -> Self {
        let count = s.chars().count();
        Self { chars: s.to_string(), quoted: vec![false; count] }
    }

    pub fn from_quoted(s: &str) -> Self {
        let count = s.chars().count();
        Self { chars: s.to_string(), quoted: vec![true; count] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, ExecCommand, Pipeline, SimpleCommand};

    fn lit(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }

    #[test]
    fn reconstruct_source_scalars() {
        use crate::lexer::tokenize;
        fn rt(src: &str) -> String {
            let toks = tokenize(src).expect("lex");
            let w = toks.iter().find_map(|t| match t {
                crate::lexer::Token::Word(w) => Some(w.clone()),
                _ => None,
            }).expect("a Word token");
            reconstruct_word_source(&w)
        }
        assert_eq!(rt("abc"), "abc");
        assert_eq!(rt("$xs"), "$xs");
        assert_eq!(rt("a$x.b"), "a$x.b");
        assert_eq!(rt("${x}"), "$x"); // bare ${x} lexes to Var, brace lost
        assert_eq!(rt("${x:-d}"), "${x:-d}");
        assert_eq!(rt("${x##*/}"), "${x##*/}");
        assert_eq!(rt("${arr[@]}"), "${arr[@]}");
        assert_eq!(rt("${#x}"), "${#x}");
        assert_eq!(rt("$((1+2))"), "$((1+2))");
        assert_eq!(rt("$(ls -l)"), "$(ls -l)");
        assert_eq!(rt("$(a && b)"), "$(a && b)");
        assert_eq!(rt("$(a; b)"), "$(a; b)");
        assert_eq!(rt("\"$x\""), "\"$x\"");
        assert_eq!(rt("a"), "a");
        assert_eq!(rt("\"a b\""), "\"a b\"");
        assert_eq!(rt("pre\"$x\"post"), "pre\"$x\"post");
        assert_eq!(rt("\"$x$y\""), "\"$x$y\"");
    }

    #[test]
    fn collapse_globstar_reduces_double_star_to_single() {
        assert_eq!(collapse_globstar("**"), "*");
        assert_eq!(collapse_globstar("***"), "*");
        assert_eq!(collapse_globstar("**/*.txt"), "*/*.txt");
        assert_eq!(collapse_globstar("a/**/b"), "a/*/b");
        assert_eq!(collapse_globstar("a*b"), "a*b");          // single star unchanged
        assert_eq!(collapse_globstar("[**]"), "[**]");        // inside bracket class: untouched
        assert_eq!(collapse_globstar("\\*\\*"), "\\*\\*");    // escaped stars: untouched
    }

    /// Test helper: project `Vec<Field>` back to `Vec<String>` so the existing
    /// assertions don't have to construct `Field` literals. (Task 4 only
    /// changes the signature; quoting propagation lands in Task 5.)
    fn expand_strings(word: &Word, shell: &mut Shell) -> Vec<String> {
        expand(word, shell).into_iter().map(|f| f.chars).collect()
    }

    fn var_unq(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: false }])
    }
    fn var_q(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: true }])
    }

    /// Builds a synthetic Sequence for `echo <args>` — used to drive
    /// CommandSub expansion in unit tests without invoking the lexer.
    fn echo_sequence(args: &[&str]) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: lit("echo"),
                    args: args.iter().map(|a| lit(a)).collect(),
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    fn exit_sequence(code: i32) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: lit("exit"),
                    args: vec![lit(&code.to_string())],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn expand_literal_word() {
        let mut shell = Shell::new();
        assert_eq!(expand_strings(&lit("hello"), &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_empty_literal_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(expand_strings(&lit(""), &mut shell), vec!["".to_string()]);
    }

    #[test]
    fn expand_multiple_literals_concatenate() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "foo".to_string(), quoted: false },
            WordPart::Literal { text: "bar".to_string(), quoted: false },
        ]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["foobar".to_string()]);
    }

    #[test]
    fn expand_unset_unquoted_yields_no_args() {
        let mut shell = Shell::new();
        assert!(expand_strings(&var_unq("DEFINITELY_NOT_SET_XYZ"), &mut shell).is_empty());
    }

    #[test]
    fn expand_unset_quoted_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(
            expand_strings(&var_q("DEFINITELY_NOT_SET_XYZ"), &mut shell),
            vec!["".to_string()]
        );
    }

    #[test]
    fn expand_set_var_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "a b".to_string());
        assert_eq!(expand_strings(&var_q("HUCK_T"), &mut shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_set_var_unquoted_splits_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "a b".to_string());
        assert_eq!(
            expand_strings(&var_unq("HUCK_T"), &mut shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_unquoted_var_with_literal_prefix_merges_first_field() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "x y".to_string());
        let word = Word(vec![
            WordPart::Literal { text: "a".to_string(), quoted: false },
            WordPart::Var { name: "HUCK_T".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["ax".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_last_status_quoted() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let word = Word(vec![WordPart::LastStatus { quoted: true }]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["42".to_string()]);
    }

    #[test]
    fn expand_tilde_uses_home() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/tmp/huck_test".to_string());
        let word = Word(vec![
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal { text: "/foo".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["/tmp/huck_test/foo".to_string()]
        );
    }

    #[test]
    fn expand_unset_unquoted_returns_no_fields_for_redirect_check() {
        let mut shell = Shell::new();
        assert_eq!(expand_strings(&Word(vec![WordPart::Var {
            name: "DEFINITELY_NOT_SET_REDIR".to_string(),
            quoted: false,
        }]), &mut shell).len(), 0);
    }

    #[test]
    fn expand_unquoted_var_with_two_fields_returns_two_for_redirect_check() {
        let mut shell = Shell::new();
        shell.set("HUCK_T_TWOFIELD", "a b".to_string());
        assert_eq!(expand_strings(&Word(vec![WordPart::Var {
            name: "HUCK_T_TWOFIELD".to_string(),
            quoted: false,
        }]), &mut shell).len(), 2);
    }

    #[test]
    fn expand_assignment_preserves_interior_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T_PAD", "a  b".to_string());
        let word = Word(vec![WordPart::Var {
            name: "HUCK_T_PAD".to_string(),
            quoted: false,
        }]);
        assert_eq!(expand_assignment(&word, &mut shell), "a  b".to_string());
    }

    #[test]
    fn expand_assignment_concatenates_parts() {
        let mut shell = Shell::new();
        shell.set("HUCK_T_X", "x".to_string());
        let word = Word(vec![
            WordPart::Literal { text: "pre-".to_string(), quoted: false },
            WordPart::Var { name: "HUCK_T_X".to_string(), quoted: false },
            WordPart::Literal { text: "-post".to_string(), quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "pre-x-post".to_string());
    }

    #[test]
    fn expand_assignment_unset_var_yields_empty_segment() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "[".to_string(), quoted: false },
            WordPart::Var { name: "DEFINITELY_NOT_SET_ASN".to_string(), quoted: false },
            WordPart::Literal { text: "]".to_string(), quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "[]".to_string());
    }

    // ---- CommandSub tests --------------------------------------------------

    #[test]
    fn expand_command_sub_invokes_inner_echo() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["hello"]),
            quoted: false,
        }]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_command_sub_unquoted_splits() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: false,
        }]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_command_sub_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: true,
        }]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_command_sub_with_literal_prefix_merges_first_field() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "pre".to_string(), quoted: false },
            WordPart::CommandSub {
                sequence: echo_sequence(&["x", "y"]),
                quoted: false,
            },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["prex".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_command_sub_strips_trailing_newlines() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["hi"]),
            quoted: true,
        }]);
        // echo emits "hi\n"; run_substitution strips -> "hi" exactly.
        assert_eq!(expand_strings(&word, &mut shell), vec!["hi".to_string()]);
    }

    #[test]
    fn expand_command_sub_updates_parent_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(0);
        let word = Word(vec![WordPart::CommandSub {
            sequence: exit_sequence(7),
            quoted: true,
        }]);
        let _ = expand(&word, &mut shell);
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn run_substitution_records_last_cmd_sub_status() {
        let mut shell = Shell::new();
        let _ = run_substitution(&exit_sequence(7), &mut shell);
        assert_eq!(shell.last_cmd_sub_status(), Some(7));
    }

    #[test]
    fn expand_assignment_last_status_after_command_sub_reads_snapshot() {
        // Parallel to expand_last_status_after_command_sub_in_same_word_reads_snapshot
        // but for assignment context. `NAME=$(exit 7)$?` with $?=3 before should
        // store "3", not "7" — `$?` reads the pre-assignment snapshot.
        let mut shell = Shell::new();
        shell.set_last_status(3);
        let word = Word(vec![
            WordPart::CommandSub {
                sequence: exit_sequence(7),
                quoted: false,
            },
            WordPart::LastStatus { quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "3".to_string());
        // The substitution still updates $? for the next command.
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn expand_assignment_command_sub_concatenates_verbatim() {
        // expand_assignment suppresses splitting, so `FOO=$(echo a b)` stores
        // "a b" (one space) as the value — same as bash's IFS=behavior in
        // assignment context. (echo's argument joining already produces "a b"
        // with one space.)
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: false,
        }]);
        assert_eq!(expand_assignment(&word, &mut shell), "a b".to_string());
    }

    #[test]
    fn expand_last_status_after_command_sub_in_same_word_reads_snapshot() {
        // Bash semantics: within a single word, `$?` reads the value of $?
        // at the start of expansion, NOT the status set by an earlier
        // CommandSub in the same word. e.g. `"$(exit 7)$?"` with $?=3 before
        // expands to "73" (the substitution's "" output then "3"), not "77".
        let mut shell = Shell::new();
        shell.set_last_status(3);
        let word = Word(vec![
            WordPart::CommandSub {
                sequence: exit_sequence(7),
                quoted: true,
            },
            WordPart::LastStatus { quoted: true },
        ]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["3".to_string()]);
        // The substitution still updates $? for the NEXT word/command.
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn expand_tilde_home_unset_falls_back_to_literal() {
        let mut shell = Shell::new();
        shell.unset("HOME");
        let word = Word(vec![WordPart::Tilde(TildeSpec::Home)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["~"]);
    }

    #[test]
    fn expand_tilde_pwd_resolves_when_pwd_set() {
        let mut shell = Shell::new();
        shell.export_set("PWD", "/var/tmp".to_string());
        let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["/var/tmp"]);
    }

    #[test]
    fn expand_tilde_pwd_unset_falls_back_to_literal_plus() {
        let mut shell = Shell::new();
        shell.unset("PWD");
        let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["~+"]);
    }

    #[test]
    fn expand_tilde_oldpwd_unset_falls_back_to_literal_minus() {
        let mut shell = Shell::new();
        shell.unset("OLDPWD");
        let word = Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["~-"]);
    }

    #[test]
    fn expand_tilde_unknown_user_falls_back_to_literal() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Tilde(TildeSpec::User("definitely_not_a_real_user_xyz_42".to_string())),
            WordPart::Literal { text: "/x".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["~definitely_not_a_real_user_xyz_42/x"]
        );
    }

    #[test]
    fn expand_assignment_tilde_home_resolves() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/h".to_string());
        let word = Word(vec![
            WordPart::Literal { text: "PATH=".to_string(), quoted: false },
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal { text: "/bin".to_string(), quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "PATH=/h/bin");
    }

    #[test]
    fn field_from_unquoted_str_marks_all_chars_unquoted() {
        let f = Field::from_unquoted("abc");
        assert_eq!(f.chars, "abc");
        assert_eq!(f.quoted, vec![false, false, false]);
    }

    #[test]
    fn field_from_quoted_str_marks_all_chars_quoted() {
        let f = Field::from_quoted("xy");
        assert_eq!(f.chars, "xy");
        assert_eq!(f.quoted, vec![true, true]);
    }

    #[test]
    fn field_push_str_appends_chars_with_quoted_flag() {
        let mut f = Field::from_unquoted("a");
        f.push_str("bc", true);
        assert_eq!(f.chars, "abc");
        assert_eq!(f.quoted, vec![false, true, true]);
    }

    #[test]
    fn field_quoted_vec_uses_char_count_not_byte_count() {
        // Multi-byte char: should produce 1 quoted entry, not the UTF-8 byte count.
        let f = Field::from_unquoted("é");
        assert_eq!(f.chars.chars().count(), 1);
        assert_eq!(f.quoted.len(), 1);
    }

    // ---- Quoting propagation (v10 Task 5) ----------------------------------

    #[test]
    fn expand_literal_unquoted_marks_chars_unquoted() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Literal { text: "abc".to_string(), quoted: false }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![false, false, false]);
    }

    #[test]
    fn expand_literal_quoted_marks_chars_quoted() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Literal { text: "abc".to_string(), quoted: true }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![true, true, true]);
    }

    #[test]
    fn expand_mixed_quoted_unquoted_literal_parts() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "foo".to_string(), quoted: false },
            WordPart::Literal { text: "*".to_string(), quoted: true },
            WordPart::Literal { text: "bar".to_string(), quoted: false },
        ]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "foo*bar");
        assert_eq!(fields[0].quoted, vec![false, false, false, true, false, false, false]);
    }

    #[test]
    fn expand_quoted_var_marks_chars_quoted() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_Q", "val".to_string());
        let word = Word(vec![WordPart::Var { name: "HUCK_Q".to_string(), quoted: true }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![true, true, true]);
    }

    #[test]
    fn expand_unquoted_var_marks_chars_unquoted() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_Q", "val".to_string());
        let word = Word(vec![WordPart::Var { name: "HUCK_Q".to_string(), quoted: false }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![false, false, false]);
    }

    #[test]
    fn expand_tilde_marks_chars_unquoted() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/h".to_string());
        let word = Word(vec![WordPart::Tilde(TildeSpec::Home)]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields[0].chars, "/h");
        assert_eq!(fields[0].quoted, vec![false, false]);
    }

    // ---- glob_expand_fields tests (v10 Task 6) ----------------------------------

    #[test]
    fn glob_expand_no_metachar_returns_chars_as_string() {
        let f = Field::from_unquoted("plain.txt");
        let out = glob_expand_fields(vec![f]);
        assert_eq!(out, vec!["plain.txt".to_string()]);
    }

    #[test]
    fn glob_expand_quoted_metachar_treated_as_literal() {
        // All chars quoted including the `*` → no globbing.
        let f = Field::from_quoted("*.txt");
        let out = glob_expand_fields(vec![f]);
        assert_eq!(out, vec!["*.txt".to_string()]);
    }

    #[test]
    fn glob_expand_question_mark_metachar_detected() {
        // CWD is process-global; run inside an empty temp dir under the lock
        // so concurrent tests can't contaminate the glob result.
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("a");
        f.push_str("?", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        // No matches in empty temp dir → literal fallback.
        assert_eq!(out, vec!["a?".to_string()]);
    }

    #[test]
    fn glob_expand_preserves_field_order() {
        let f1 = Field::from_unquoted("first");
        let f2 = Field::from_unquoted("second");
        let out = glob_expand_fields(vec![f1, f2]);
        assert_eq!(out, vec!["first".to_string(), "second".to_string()]);
    }

    // ---- glob_expand_fields filesystem tests (v10 Task 7) ----------------------

    // CWD is process-global; serialize tests that mutate it. The lock is
    // shared crate-wide so completion / executor / builtins tests that
    // also chdir take the same one.
    use crate::test_support::CWD_LOCK;

    fn touch(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), b"").unwrap();
    }

    #[test]
    fn glob_star_matches_files_in_cwd() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("*");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["a.txt".to_string(), "b.txt".to_string()]);
    }

    #[test]
    fn glob_star_excludes_dotfiles_by_default() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "visible");
        touch(tmp.path(), ".hidden");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let f = Field::from_unquoted("*");
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["visible".to_string()]);
    }

    #[test]
    fn glob_dot_star_matches_dotfiles_but_excludes_dot_and_dotdot() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), ".hidden");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted(".");
        f.push_str("*", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert!(out.contains(&".hidden".to_string()));
        assert!(!out.contains(&".".to_string()));
        assert!(!out.contains(&"..".to_string()));
    }

    #[test]
    fn glob_bracket_dot_class_matches_dotfile() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), ".hidden");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("[.]");
        f.push_str("hidden", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec![".hidden".to_string()]);
    }

    #[test]
    fn glob_bracket_class_matches_listed_chars() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.txt");
        touch(tmp.path(), "c.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("[ab]");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["a.txt".to_string(), "b.txt".to_string()]);
    }

    #[test]
    fn glob_no_match_returns_literal_pattern() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("nonex");
        f.push_str("*", false);
        f.push_str(".xyz", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["nonex*.xyz".to_string()]);
    }

    #[test]
    fn glob_partial_quoting_keeps_literal_prefix() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "fooA");
        touch(tmp.path(), "fooB");
        touch(tmp.path(), "barA");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        // `"foo"*` — first three chars quoted, then unquoted `*`.
        let mut f = Field::from_quoted("foo");
        f.push_str("*", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["fooA".to_string(), "fooB".to_string()]);
    }

    #[test]
    fn glob_negation_bracket_excludes_listed() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.txt");
        touch(tmp.path(), "c.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("[!a]");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["b.txt".to_string(), "c.txt".to_string()]);
    }

    #[test]
    fn glob_unterminated_bracket_falls_back_to_literal() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let f = Field::from_unquoted("[abc"); // no closing ]
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["[abc".to_string()]);
    }

    #[test]
    fn expand_then_glob_end_to_end_for_literal() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Literal { text: "hello".to_string(), quoted: false }]);
        let argv = glob_expand_fields(expand(&word, &mut shell));
        assert_eq!(argv, vec!["hello".to_string()]);
    }

    /// Helper: a `WordPart::Arith` whose body is a single literal (the
    /// post-v93 deferred-parse shape; arithmetic is parsed at eval time).
    fn arith_part(text: &str) -> WordPart {
        WordPart::Arith {
            body: Word(vec![WordPart::Literal { text: text.to_string(), quoted: true }]),
            quoted: false,
        }
    }

    #[test]
    fn expand_arith_part_renders_decimal_result() {
        let mut shell = Shell::new();
        let word = Word(vec![arith_part("2 + 3")]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "5");
        assert_eq!(fields[0].quoted, vec![true]);
    }

    #[test]
    fn expand_arith_part_division_by_zero_is_nonfatal() {
        // An arith eval error (e.g. division by zero) in $((…)) is NO LONGER
        // a fatal expansion error — bash script-file mode prints the error
        // and continues. The error still surfaces via stderr;
        // pending_fatal_status stays None so the surrounding command list
        // runs to completion. The `-c` mode divergence is tracked as L-55.
        let mut shell = Shell::new();
        let word = Word(vec![arith_part("1 / 0")]);
        let _ = expand(&word, &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }

    #[test]
    fn expand_arith_error_is_posix_fatal() {
        let mut shell = Shell::new();
        shell.shell_options.posix = true;
        shell.is_interactive = false;
        let word = Word(vec![arith_part("1 + ")]);
        let _ = expand(&word, &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }

    #[test]
    fn expand_arith_part_invalid_lhs_assignment_is_nonfatal() {
        // A parse-time arith error (e.g. assignment to a non-lvalue) is also
        // non-fatal. The expansion contributes empty; pending_fatal_status
        // stays None.
        let mut shell = Shell::new();
        let word = Word(vec![arith_part("1 + 2 = 3")]);
        let _ = expand(&word, &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }

    #[test]
    fn expand_assignment_arith_part_renders_decimal() {
        let mut shell = Shell::new();
        let word = Word(vec![arith_part("6 * 7")]);
        let value = expand_assignment(&word, &mut shell);
        assert_eq!(value, "42");
    }

    #[test]
    fn expand_param_expansion_use_default_unquoted_unset() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E1".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![WordPart::Literal { text: "fallback".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: false,
            subscript: None,
            indirect: false,
        }]);
        let fields = expand(&word, &mut shell);
        let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        assert_eq!(strings, vec!["fallback".to_string()]);
    }

    #[test]
    fn expand_param_expansion_quoted_value_with_space_stays_one_field() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E2".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![WordPart::Literal { text: "a b c".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: true,
            subscript: None,
            indirect: false,
        }]);
        let fields = expand(&word, &mut shell);
        let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        assert_eq!(strings, vec!["a b c".to_string()]);
    }

    #[test]
    fn expand_param_expansion_unquoted_value_with_space_splits() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_E3", "a b c".to_string());
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E3".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![]),
                colon: true,
            },
            quoted: false,
            subscript: None,
            indirect: false,
        }]);
        let fields = expand(&word, &mut shell);
        let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        assert_eq!(strings, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn expand_assignment_param_expansion_no_split() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E4".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![WordPart::Literal { text: "a b c".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: false,
            subscript: None,
            indirect: false,
        }]);
        let value = expand_assignment(&word, &mut shell);
        assert_eq!(value, "a b c");
    }

    #[test]
    fn expand_param_expansion_error_yields_empty_field_sets_status() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E5".to_string(),
            modifier: ParamModifier::ErrorIfUnset {
                word: Word(vec![WordPart::Literal { text: "missing".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: false,
            subscript: None,
            indirect: false,
        }]);
        let fields = expand(&word, &mut shell);
        // v34 (Task 4): expand() now bails early on Fatal, stashing status on
        // pending_fatal_status and returning the partial (empty) result
        // without the end-of-word push, so fields is empty.
        assert_eq!(fields.len(), 0);
        assert_eq!(shell.pending_fatal_status, Some(1));
    }

    #[test]
    fn expand_pattern_last_status_snapshots_before_command_sub() {
        use crate::command::Sequence;

        let mut shell = Shell::new();
        shell.set_last_status(7);

        // A pattern word of two parts: a CommandSub that runs `false` (which
        // mutates $? to 1), followed by $?. With the snapshot fix, $? reads
        // the pre-expansion value (7) — not the post-`false` value (1).
        let false_cmd = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: lit("false"),
                    args: vec![],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        };
        let word = Word(vec![
            WordPart::CommandSub { sequence: false_cmd, quoted: false },
            WordPart::LastStatus { quoted: false },
        ]);

        let pattern = expand_pattern(&word, &mut shell);
        assert!(
            pattern.ends_with("7"),
            "expected pattern to end with the pre-expansion $? value 7, got: {pattern:?}"
        );
    }

    #[test]
    fn glob_star_does_not_cross_path_separator() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        touch(&tmp.path().join("sub"), "deep.txt");
        touch(tmp.path(), "top.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("*");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["top.txt".to_string()]);
    }

    // ---- Positional parameter expander tests (v22 Task 4) -------------------

    #[test]
    fn expand_dollar_digit_reads_positional() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["alpha".to_string(), "beta".to_string()];
        let w = Word(vec![WordPart::Var { name: "1".to_string(), quoted: false }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "alpha");
    }

    #[test]
    fn expand_dollar_digit_unset_is_empty() {
        let mut shell = Shell::new();
        let w = Word(vec![WordPart::Var { name: "1".to_string(), quoted: false }]);
        let fields = expand(&w, &mut shell);
        // Unset positional → no field (consistent with unset var behaviour)
        assert!(fields.is_empty());
    }

    #[test]
    fn expand_dollar_hash_is_arg_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let w = Word(vec![WordPart::Var { name: "#".to_string(), quoted: false }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "3");
    }

    #[test]
    fn expand_dollar_at_quoted_produces_field_per_arg() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a a".to_string(), "b".to_string()];
        let w = Word(vec![WordPart::AllArgs { joined: false, quoted: true }]);
        let fields = expand(&w, &mut shell);
        // Each arg its own field; the space inside "a a" is preserved (no splitting).
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].chars, "a a");
        assert_eq!(fields[1].chars, "b");
    }

    #[test]
    fn expand_dollar_star_quoted_joins_with_space() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let w = Word(vec![WordPart::AllArgs { joined: true, quoted: true }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "a b c");
    }

    #[test]
    fn expand_dollar_star_quoted_joins_with_first_ifs_char() {
        // POSIX § 2.5.2: "$*" joins positional args with the first
        // character of IFS. With IFS=":" and args a b c → "a:b:c".
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        shell.set("IFS", ":".to_string());
        let w = Word(vec![WordPart::AllArgs { joined: true, quoted: true }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "a:b:c");
    }

    #[test]
    fn expand_dollar_at_empty_produces_no_fields() {
        let mut shell = Shell::new();
        let w = Word(vec![WordPart::AllArgs { joined: false, quoted: true }]);
        let fields = expand(&w, &mut shell);
        // Either zero fields or all-empty fields are acceptable per the spec.
        assert!(fields.is_empty());
    }

    #[test]
    fn expand_dollar_at_unquoted_splits_each_arg_independently() {
        // $@ unquoted with two args, one containing whitespace.
        // POSIX: each arg becomes its own field(s) after IFS-splitting;
        // args do NOT merge across boundaries.
        let mut shell = Shell::new();
        shell.positional_args = vec!["hello world".to_string(), "x".to_string()];
        let w = Word(vec![WordPart::AllArgs { joined: false, quoted: false }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 3, "fields: {fields:?}");
        assert_eq!(fields[0].chars, "hello");
        assert_eq!(fields[1].chars, "world");
        assert_eq!(fields[2].chars, "x");
    }

    #[test]
    fn case_modifier_on_indexed_array_at() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "foo".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "bar".to_string()).unwrap();
        let result = expand_array_param(
            "a",
            &crate::lexer::ParamModifier::Case {
                direction: crate::lexer::CaseDirection::Upper,
                all: true,
                pattern: None,
            },
            &crate::lexer::SubscriptKind::All,
            true, // quoted
            &mut shell,
        );
        match result {
            ExpansionResult::WordList(words) => assert_eq!(words, vec!["FOO", "BAR"]),
            other => panic!("expected WordList, got {other:?}"),
        }
    }

    #[test]
    fn case_modifier_on_indexed_array_star() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "foo".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "bar".to_string()).unwrap();
        let result = expand_array_param(
            "a",
            &crate::lexer::ParamModifier::Case {
                direction: crate::lexer::CaseDirection::Upper,
                all: true,
                pattern: None,
            },
            &crate::lexer::SubscriptKind::Star,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::Value(v) => assert_eq!(v, "FOO BAR"),
            other => panic!("expected Value, got {other:?}"),
        }
    }

    #[test]
    fn remove_suffix_per_element_indexed() {
        use crate::shell_state::Shell;
        use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "foo.txt".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "bar.md".to_string()).unwrap();
        let pat = Word(vec![WordPart::Literal { text: ".*".into(), quoted: false }]);
        let result = expand_array_param(
            "a",
            &PM::RemoveSuffix { pattern: pat, longest: false },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::WordList(words) => assert_eq!(words, vec!["foo", "bar"]),
            other => panic!("expected WordList, got {other:?}"),
        }
    }

    #[test]
    fn empty_array_per_element_modifier() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        let result = expand_array_param(
            "a",
            &crate::lexer::ParamModifier::Case {
                direction: crate::lexer::CaseDirection::Upper,
                all: true,
                pattern: None,
            },
            &crate::lexer::SubscriptKind::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::WordList(words) => assert!(words.is_empty(), "expected empty WordList, got {words:?}"),
            other => panic!("expected WordList, got {other:?}"),
        }
    }

    #[test]
    fn case_modifier_on_associative_array() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".to_string(), "foo".to_string()).unwrap();
        shell.set_associative_element("m", "j".to_string(), "bar".to_string()).unwrap();
        let result = expand_array_param(
            "m",
            &crate::lexer::ParamModifier::Case {
                direction: crate::lexer::CaseDirection::Upper,
                all: true,
                pattern: None,
            },
            &crate::lexer::SubscriptKind::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::WordList(words) => {
                let mut sorted = words.clone();
                sorted.sort();
                assert_eq!(sorted, vec!["BAR", "FOO"]);
            }
            other => panic!("expected WordList, got {other:?}"),
        }
    }

    #[test]
    fn substitute_per_element_assoc() {
        use crate::shell_state::Shell;
        use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, SubstAnchor, Word, WordPart};
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".to_string(), "foo".to_string()).unwrap();
        shell.set_associative_element("m", "j".to_string(), "boo".to_string()).unwrap();
        let pat = Word(vec![WordPart::Literal { text: "o".into(), quoted: false }]);
        let repl = Word(vec![WordPart::Literal { text: "X".into(), quoted: false }]);
        let result = expand_array_param(
            "m",
            &PM::Substitute {
                pattern: pat,
                replacement: repl,
                anchor: SubstAnchor::None,
                all: false,
            },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::WordList(words) => {
                let mut sorted = words.clone();
                sorted.sort();
                assert_eq!(sorted, vec!["bXo", "fXo"]);
            }
            other => panic!("expected WordList, got {other:?}"),
        }
    }

    #[test]
    fn assign_default_on_array_still_errors() {
        use crate::shell_state::Shell;
        use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "foo".to_string()).unwrap();
        let word = Word(vec![WordPart::Literal { text: "default".into(), quoted: false }]);
        let result = expand_array_param(
            "a",
            &PM::AssignDefault { word, colon: true },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::Value(v) => assert_eq!(v, ""),
            other => panic!("expected empty Value (catchall rejection), got {other:?}"),
        }
    }

    #[test]
    fn error_if_unset_on_array_still_errors() {
        use crate::shell_state::Shell;
        use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
        use crate::param_expansion::ExpansionResult;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Literal { text: "msg".into(), quoted: false }]);
        let result = expand_array_param(
            "a",
            &PM::ErrorIfUnset { word, colon: true },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::Value(v) => assert_eq!(v, ""),
            other => panic!("expected empty Value (catchall rejection), got {other:?}"),
        }
    }

    #[test]
    fn transform_assign_decl_on_indexed_at() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
        let result = expand_array_param(
            "a",
            &PM::Transform { op: TransformOp::AssignDecl },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::Value(v) => assert_eq!(v, r#"declare -a a=([0]="x" [1]="y")"#),
            other => panic!("expected Value, got {other:?}"),
        }
    }

    #[test]
    fn transform_kv_words_on_indexed_yields_wordlist() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
        let result = expand_array_param(
            "a",
            &PM::Transform { op: TransformOp::KvWords },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::WordList(words) => assert_eq!(words, vec!["0", "x", "1", "y"]),
            other => panic!("expected WordList, got {other:?}"),
        }
    }

    #[test]
    fn transform_attr_flags_indexed_yields_a() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        let result = expand_array_param(
            "a",
            &PM::Transform { op: TransformOp::AttrFlags },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::Value(v) => assert_eq!(v, "a"),
            other => panic!("expected Value, got {other:?}"),
        }
    }

    #[test]
    fn transform_assign_decl_on_assoc_at() {
        use crate::shell_state::Shell;
        use crate::param_expansion::ExpansionResult;
        use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".to_string(), "v1".to_string()).unwrap();
        let result = expand_array_param(
            "m",
            &PM::Transform { op: TransformOp::AssignDecl },
            &SK::All,
            true,
            &mut shell,
        );
        match result {
            ExpansionResult::Value(v) => assert_eq!(v, r#"declare -A m=([k]="v1" )"#),
            other => panic!("expected Value, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod array_expansion_tests {
    //! Task 3 (v71) end-to-end array expansion + positional slicing.
    //!
    //! Tests drive the full lex→Word→expand pipeline so the lexer's
    //! subscript handling and the new `expand_array_param` /
    //! `slice_word_list` paths are exercised together.

    use super::*;
    use crate::command::{Command, SimpleCommand};
    use crate::shell_state::Shell;

    /// Lex the input as `echo <input>` and return the first argument
    /// Word. Avoids constructing `WordPart::ParamExpansion` literals by
    /// hand and keeps the tests aligned with what the lexer actually
    /// produces (matters for the lexer-touching `${!a[@]}` shape).
    fn first_arg_word(input: &str) -> Word {
        let src = format!("echo {input}");
        let tokens = crate::lexer::tokenize(&src).expect("lex");
        let seq = crate::command::parse(tokens).expect("parse").expect("non-empty");
        let pipeline = match seq.first {
            Command::Pipeline(p) => p,
            other => panic!("expected Pipeline, got {other:?}"),
        };
        match &pipeline.commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => e.args[0].clone(),
            other => panic!("expected SimpleCommand::Exec, got {other:?}"),
        }
    }

    /// Run `expand` on the lexed input and return a single string
    /// formed by joining the resulting fields with a space. Used for
    /// tests that expect a single conceptual "string" result (e.g.
    /// `${a[i]}` reads, `${#a[@]}` counts, `${!a[@]}` keys, `${a[*]}`).
    fn expand_for_test(shell: &mut Shell, input: &str) -> String {
        let w = first_arg_word(input);
        let fields = expand(&w, shell);
        let parts: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        parts.join(" ")
    }

    /// Run `expand` and return the field list directly (each field's
    /// `chars` value as a Vec<String>). Used for tests that expect
    /// multiple separate words from a quoted `${a[@]}` form.
    fn expand_to_word_list_for_test(shell: &mut Shell, input: &str) -> Vec<String> {
        let w = first_arg_word(input);
        let fields = expand(&w, shell);
        fields.into_iter().map(|f| f.chars).collect()
    }

    fn shell_with_a() -> Shell {
        let mut s = Shell::new();
        s.seed_array_for_tests("a", &[(0, "x"), (1, "y"), (2, "z")]);
        s
    }

    #[test]
    fn read_element_returns_value() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${a[1]}");
        assert_eq!(out, "y");
    }

    #[test]
    fn out_of_range_element_is_empty() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${a[99]}");
        assert_eq!(out, "");
    }

    #[test]
    fn quoted_at_yields_separate_words() {
        let mut s = shell_with_a();
        let words = expand_to_word_list_for_test(&mut s, r#""${a[@]}""#);
        assert_eq!(words, vec!["x", "y", "z"]);
    }

    #[test]
    fn quoted_star_joins_by_ifs() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, r#""${a[*]}""#);
        assert_eq!(out, "x y z");
    }

    #[test]
    fn count_returns_element_count_not_max_index() {
        let mut s = Shell::new();
        s.seed_array_for_tests("a", &[(2, "x"), (5, "y")]);
        let out = expand_for_test(&mut s, "${#a[@]}");
        assert_eq!(out, "2");
    }

    #[test]
    fn keys_list_returns_subscripts() {
        let mut s = Shell::new();
        s.seed_array_for_tests("a", &[(2, "x"), (5, "y")]);
        let out = expand_for_test(&mut s, "${!a[@]}");
        assert_eq!(out, "2 5");
    }

    #[test]
    fn element_length() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${#a[0]}");
        assert_eq!(out, "1");
    }

    #[test]
    fn indirect_unset_positional_is_empty() {
        // v195: `${!1}` with no positional parameters expands to empty (like
        // bash), not a fatal "invalid indirect expansion". `:-` sees it unset.
        let mut s = Shell::new();
        assert_eq!(expand_for_test(&mut s, "${!1}"), "");
        assert_eq!(expand_for_test(&mut s, "${!9}"), "");
        assert_eq!(expand_for_test(&mut s, "${!1:-DEF}"), "DEF");
        // A SET positional still indirects through its value.
        s.positional_args = vec!["HOME".into()];
        assert_eq!(
            expand_for_test(&mut s, "${!1}"),
            std::env::var("HOME").unwrap_or_default()
        );
    }

    #[test]
    fn slicing_positive_offset_and_length() {
        let mut s = shell_with_a();
        let words = expand_to_word_list_for_test(&mut s, r#""${a[@]:1:1}""#);
        assert_eq!(words, vec!["y"]);
    }

    #[test]
    fn slicing_negative_offset_counts_from_end() {
        let mut s = shell_with_a();
        let words = expand_to_word_list_for_test(&mut s, r#""${a[@]: -1}""#);
        assert_eq!(words, vec!["z"]);
    }

    #[test]
    fn bare_name_returns_element_zero() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${a}");
        assert_eq!(out, "x");
    }

    #[test]
    fn negative_subscript_wraps() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${a[-1]}");
        assert_eq!(out, "z");
    }

    #[test]
    fn nounset_on_unset_element_fires_pe_error() {
        let mut s = shell_with_a();
        s.shell_options.nounset = true;
        let _ = expand_for_test(&mut s, "${a[99]}");
        assert!(s.pending_fatal_status.is_some());
    }

    #[test]
    fn slicing_negative_length_indexes_from_end() {
        let mut s = shell_with_a();
        let words = expand_to_word_list_for_test(&mut s, r#""${a[@]:1:-1}""#);
        // Bash: ${a[@]:1:-1} starts at index 1, ends one-before-last. Returns ["y"].
        assert_eq!(words, vec!["y"]);
    }

    #[test]
    fn length_of_element_at_bad_subscript_errors() {
        // ${#nonexistent[-1]} — negative subscript on an unset array
        // cannot wrap (no max index), so eval_subscript returns Err.
        // The fix to (PM::Length, SK::Index) must propagate that error
        // rather than silently using idx 0.
        let mut s = Shell::new();
        let _ = expand_for_test(&mut s, "${#nonexistent[-1]}");
        assert!(s.pending_fatal_status.is_some());
    }

    // v73 regression: ${a[i]:-default} on a missing index must substitute
    // the default, not fall through to scalar_view (element 0). Pre-v73
    // bug: get_raw saw override_value=None and consulted shell.get(name)
    // which returned a[0] — so ${a[99]:-X} returned "x" (a[0]) instead of "X".
    #[test]
    fn modifier_on_missing_index_uses_default() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${a[99]:-fallback}");
        assert_eq!(out, "fallback");
    }

    // v73 regression: ${a[i]-default} (no colon) on a missing index also
    // substitutes the default.
    #[test]
    fn modifier_no_colon_on_missing_index_uses_default() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${a[99]-fallback}");
        assert_eq!(out, "fallback");
    }

    // v73 regression: ${a[i]:?msg} on a missing index fires the fatal error
    // rather than silently returning a[0].
    #[test]
    fn error_if_unset_on_missing_index_fires() {
        let mut s = shell_with_a();
        let _ = expand_for_test(&mut s, "${a[99]:?missing}");
        assert!(s.pending_fatal_status.is_some());
    }

    // v73 regression: ${a[i]:+alt} on a missing index returns empty (the
    // alternative branch only fires when the value is set+non-null).
    #[test]
    fn alternative_value_on_missing_index_is_empty() {
        let mut s = shell_with_a();
        let out = expand_for_test(&mut s, "${a[99]:+ALT}");
        assert_eq!(out, "");
    }

    // v73 regression: ${a[i]:-default} on an existing element returns the
    // element (not the default). Pin the happy path.
    #[test]
    fn modifier_on_existing_index_returns_element() {
        let mut s = shell_with_a();  // a=[(0,"x"),(1,"y"),(2,"z")]
        let out = expand_for_test(&mut s, "${a[1]:-fallback}");
        assert_eq!(out, "y");
    }
}

#[cfg(test)]
mod positional_slicing_tests {
    //! Task 3 closes v33's `${@:o:l}` / `${*:o:l}` deferral. These
    //! tests drive the slice helper through the lex→expand pipeline.

    use super::*;
    use crate::command::{Command, SimpleCommand};
    use crate::shell_state::Shell;

    fn first_arg_word(input: &str) -> Word {
        let src = format!("echo {input}");
        let tokens = crate::lexer::tokenize(&src).expect("lex");
        let seq = crate::command::parse(tokens).expect("parse").expect("non-empty");
        let pipeline = match seq.first {
            Command::Pipeline(p) => p,
            other => panic!("expected Pipeline, got {other:?}"),
        };
        match &pipeline.commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => e.args[0].clone(),
            other => panic!("expected SimpleCommand::Exec, got {other:?}"),
        }
    }

    fn expand_for_test(shell: &mut Shell, input: &str) -> String {
        let w = first_arg_word(input);
        let fields = expand(&w, shell);
        let parts: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        parts.join(" ")
    }

    fn expand_to_word_list_for_test(shell: &mut Shell, input: &str) -> Vec<String> {
        let w = first_arg_word(input);
        let fields = expand(&w, shell);
        fields.into_iter().map(|f| f.chars).collect()
    }

    fn shell_with_posargs() -> Shell {
        let mut s = Shell::new();
        s.positional_args = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        s
    }

    #[test]
    fn at_slice_positive() {
        let mut s = shell_with_posargs();
        let words = expand_to_word_list_for_test(&mut s, r#""${@:2:2}""#);
        assert_eq!(words, vec!["b", "c"]);
    }

    #[test]
    fn at_slice_negative_offset() {
        let mut s = shell_with_posargs();
        let words = expand_to_word_list_for_test(&mut s, r#""${@: -2}""#);
        assert_eq!(words, vec!["c", "d"]);
    }

    #[test]
    fn star_slice_joins_by_ifs() {
        let mut s = shell_with_posargs();
        let out = expand_for_test(&mut s, r#""${*:1:3}""#);
        assert_eq!(out, "a b c");
    }

    #[test]
    fn at_slice_offset_zero_includes_dollar_zero() {
        let mut s = shell_with_posargs();
        s.shell_argv0 = "huck".to_string();
        let words = expand_to_word_list_for_test(&mut s, r#""${@:0:2}""#);
        // Bash returns "huck a" for ${@:0:2} when $0 is "huck" and positionals are [a,b,c,d].
        assert_eq!(words, vec!["huck", "a"]);
    }

    #[test]
    fn at_slice_negative_length_indexes_from_end() {
        let mut s = shell_with_posargs();
        let words = expand_to_word_list_for_test(&mut s, r#""${@:1:-1}""#);
        // Bash: ${@:1:-1} starts at $1, ends one-before-last. Returns ["a", "b", "c"].
        assert_eq!(words, vec!["a", "b", "c"]);
    }
}

#[cfg(test)]
mod assoc_expansion_tests {
    //! v72 task 2: read paths for associative arrays. Mirrors the
    //! indexed-array test module but exercises string-key semantics
    //! and insertion-order iteration.

    use super::*;
    use crate::command::{Command, SimpleCommand};
    use crate::shell_state::Shell;

    fn first_arg_word(input: &str) -> Word {
        let src = format!("echo {input}");
        let tokens = crate::lexer::tokenize(&src).expect("lex");
        let seq = crate::command::parse(tokens).expect("parse").expect("non-empty");
        let pipeline = match seq.first {
            Command::Pipeline(p) => p,
            other => panic!("expected Pipeline, got {other:?}"),
        };
        match &pipeline.commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => e.args[0].clone(),
            other => panic!("expected SimpleCommand::Exec, got {other:?}"),
        }
    }

    fn expand_for_test(shell: &mut Shell, input: &str) -> String {
        let w = first_arg_word(input);
        let fields = expand(&w, shell);
        let parts: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        parts.join(" ")
    }

    fn expand_to_word_list_for_test(shell: &mut Shell, input: &str) -> Vec<String> {
        let w = first_arg_word(input);
        let fields = expand(&w, shell);
        fields.into_iter().map(|f| f.chars).collect()
    }

    fn shell_with_m() -> Shell {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "first".into(), "x".into()).unwrap();
        s.set_associative_element("m", "second".into(), "y".into()).unwrap();
        s.set_associative_element("m", "third".into(), "z".into()).unwrap();
        s
    }

    #[test]
    fn read_element_by_string_key() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[second]}");
        assert_eq!(out, "y");
    }

    #[test]
    fn missing_key_is_empty() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[nope]}");
        assert_eq!(out, "");
    }

    #[test]
    fn quoted_at_yields_values_in_insertion_order() {
        let mut s = shell_with_m();
        let words = expand_to_word_list_for_test(&mut s, r#""${m[@]}""#);
        assert_eq!(words, vec!["x", "y", "z"]);
    }

    #[test]
    fn quoted_star_joins_values_in_insertion_order() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, r#""${m[*]}""#);
        assert_eq!(out, "x y z");
    }

    #[test]
    fn count_returns_pair_count() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${#m[@]}");
        assert_eq!(out, "3");
    }

    #[test]
    fn keys_list_returns_string_keys_in_insertion_order() {
        let mut s = shell_with_m();
        let words = expand_to_word_list_for_test(&mut s, r#""${!m[@]}""#);
        assert_eq!(words, vec!["first", "second", "third"]);
    }

    #[test]
    fn quoted_star_keys_joins_by_ifs() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, r#""${!m[*]}""#);
        assert_eq!(out, "first second third");
    }

    #[test]
    fn element_length_for_associative() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "k".into(), "hello".into()).unwrap();
        let out = expand_for_test(&mut s, "${#m[k]}");
        assert_eq!(out, "5");
    }

    #[test]
    fn slicing_returns_values_in_insertion_order() {
        let mut s = shell_with_m();
        let words = expand_to_word_list_for_test(&mut s, r#""${m[@]:1:1}""#);
        assert_eq!(words, vec!["y"]);
    }

    #[test]
    fn bare_name_returns_empty_for_associative() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m}");
        assert_eq!(out, "");
    }

    #[test]
    fn variable_subscript_expands_as_string() {
        let mut s = shell_with_m();
        s.set("k", "second".into());
        let out = expand_for_test(&mut s, "${m[$k]}");
        assert_eq!(out, "y");
    }

    #[test]
    fn nounset_on_missing_key_fires_pe_error() {
        let mut s = shell_with_m();
        s.shell_options.nounset = true;
        let _ = expand_for_test(&mut s, "${m[nope]}");
        assert!(s.pending_fatal_status.is_some());
    }

    #[test]
    fn modifier_on_missing_key_uses_default() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[nope]:-fallback}");
        assert_eq!(out, "fallback");
    }

    // v73 regression: ${m[nope]-fallback} (no colon) MUST also substitute
    // the default when the key is missing — previously fell through to
    // scalar_view (which for associative is "" → tested non-null only
    // for colon variant → returned "" instead of fallback).
    #[test]
    fn modifier_no_colon_on_missing_key_uses_default() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[nope]-fallback}");
        assert_eq!(out, "fallback");
    }

    // v73 regression: ${m[k]:?msg} on a missing key should fire the error,
    // not fall through to scalar_view.
    #[test]
    fn error_if_unset_on_missing_associative_key_fires() {
        let mut s = shell_with_m();
        let _ = expand_for_test(&mut s, "${m[nope]:?missing}");
        assert!(s.pending_fatal_status.is_some());
    }

    // v73 regression: ${m[k]:+alt} on a missing key returns empty (the
    // alternative branch only fires when the value is set+non-null).
    #[test]
    fn alternative_value_on_missing_key_is_empty() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[nope]:+ALT}");
        assert_eq!(out, "");
    }
}

#[cfg(test)]
mod ifs_splitter_tests {
    //! POSIX § 2.6.5 field-splitting unit tests for `emit_split_fields`.
    //! These tests drive the splitter directly, not the lex→expand
    //! pipeline, so they isolate the IFS classification logic.

    use super::*;

    fn run(value: &str, ifs: &str) -> Vec<String> {
        let mut current = Field::default();
        let mut result: Vec<Field> = Vec::new();
        let mut has_emitted = false;
        emit_split_fields(value, ifs, &mut current, &mut result, &mut has_emitted);
        if has_emitted {
            result.push(current);
        }
        result.into_iter().map(|f| f.chars).collect()
    }

    #[test]
    fn default_ifs_collapses_whitespace_runs() {
        assert_eq!(run("a  b\tc", " \t\n"), vec!["a", "b", "c"]);
    }

    #[test]
    fn colon_ifs_preserves_empty_between() {
        assert_eq!(run("a::b", ":"), vec!["a", "", "b"]);
    }

    #[test]
    fn colon_ifs_leading_produces_empty() {
        assert_eq!(run(":a", ":"), vec!["", "a"]);
    }

    #[test]
    fn colon_ifs_trailing_no_empty() {
        // POSIX: trailing non-ws IFS does NOT add a trailing empty field.
        assert_eq!(run("a:", ":"), vec!["a"]);
    }

    #[test]
    fn mixed_ifs_ws_collapses_around_nonws() {
        // IFS=" :", value "a : b" → 2 fields (colon plus adjacent
        // spaces collapse to one separator).
        assert_eq!(run("a : b", " :"), vec!["a", "b"]);
    }

    #[test]
    fn empty_ifs_no_split() {
        assert_eq!(run("a b c", ""), vec!["a b c"]);
    }

    #[test]
    fn whitespace_only_value_yields_no_fields() {
        let empty: Vec<String> = Vec::new();
        assert_eq!(run("   ", " \t\n"), empty);
    }

    #[test]
    fn mixed_consecutive_nonws_yields_empty_field() {
        // IFS=":,", value "a:,b" → a/""/"b"
        assert_eq!(run("a:,b", ":,"), vec!["a", "", "b"]);
    }

    #[test]
    fn single_nonws_only_yields_empty_field() {
        // IFS=":", value ":" → 1 empty field
        assert_eq!(run(":", ":"), vec![""]);
    }

    #[test]
    fn leading_nonws_then_value() {
        assert_eq!(run(":x", ":"), vec!["", "x"]);
    }

    #[test]
    fn ws_only_ifs_pure_whitespace_collapses() {
        assert_eq!(run(" a b ", " "), vec!["a", "b"]);
    }

    #[test]
    fn nonws_ifs_with_ws_value_no_split() {
        // IFS=":" (no whitespace), value "a b" → 1 field "a b".
        assert_eq!(run("a b", ":"), vec!["a b"]);
    }

    #[test]
    fn empty_value_emits_nothing() {
        let empty: Vec<String> = Vec::new();
        assert_eq!(run("", ":"), empty);
        assert_eq!(run("", " \t\n"), empty);
    }

    #[test]
    fn current_field_continuation() {
        // If `current` already has text, the first split fragment
        // continues it rather than starting a new field.
        let mut current = Field::default();
        current.push_str("prefix-", false);
        let mut result: Vec<Field> = Vec::new();
        let mut has_emitted = true;
        emit_split_fields("a b c", " \t\n", &mut current, &mut result,
                          &mut has_emitted);
        result.push(current);
        let words: Vec<String> = result.into_iter().map(|f| f.chars).collect();
        assert_eq!(words, vec!["prefix-a", "b", "c"]);
    }
}

