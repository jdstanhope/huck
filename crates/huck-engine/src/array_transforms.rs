//! Whole-array and scalar `${var@OP}` transform implementations for
//! `@A` (declare-style assignment), `@K` (k/v pairs string), `@k` (k/v
//! pairs word list), and `@a` (attribute flag letters). Called from
//! `expand_array_param` / `expand_assoc_param` for the whole-array
//! forms and from `param_expansion::expand_modifier_with_value`'s
//! `Transform { op }` arm for the scalar / single-element forms.

use crate::shell_state::{Shell, VarValue, Variable};

/// Where the modifier was applied: a whole array (`[@]` / `[*]`
/// subscript) or a single value (scalar variable, no subscript, or
/// specific `[i]` subscript).
#[allow(dead_code)]
pub(crate) enum ScopeMode {
    /// `[@]` or `[*]` — operate on the whole array's key/value pairs.
    Whole,
    /// Scalar variable, no subscript, or specific `[i]` — operate on
    /// a single resolved value. Carries the value so callers don't
    /// re-look-it-up. For `${arr[0]@A}` this carries the value of
    /// `arr[0]`; for `${arr@A}` it carries the scalar view (which is
    /// `arr[0]` for indexed, empty for associative).
    ScalarOrElement(String),
}

/// `${var@A}` — declare-style assignment string.
#[allow(dead_code)]
pub(crate) fn assign_decl(name: &str, scope: ScopeMode, shell: &Shell) -> String {
    let Some(var) = shell.get_var(name) else {
        return String::new();
    };
    match scope {
        ScopeMode::Whole => assign_decl_whole(name, var),
        ScopeMode::ScalarOrElement(val) => assign_decl_scalar_or_element(name, var, &val),
    }
}

/// Whole-array form: full `declare -[aA] name=(…)` reusing the
/// shared `format_declare_line` renderer (bareword keys + trailing
/// space on assoc bodies).
#[allow(dead_code)]
fn assign_decl_whole(name: &str, var: &Variable) -> String {
    crate::builtins::format_declare_line(name, var)
}

/// Scalar / single-element form:
///   - plain scalar no attrs           → `name='value'`
///   - attributed scalar               → `declare -X name='value'`
///   - indexed array (no sub or [i])   → `declare -a name='value'`
///   - associative (no sub or [k])     → `declare -A name` (no body)
#[allow(dead_code)]
fn assign_decl_scalar_or_element(name: &str, var: &Variable, val: &str) -> String {
    let quoted_val = always_quote(val);
    let has_attrs = var.exported
        || var.readonly
        || var.integer
        || var.case_fold.is_some()
        || var.nameref;
    match &var.value {
        VarValue::Scalar(_) => {
            if has_attrs {
                let attrs = render_attr_prefix(var, false);
                format!("declare {attrs} {name}={quoted_val}")
            } else {
                format!("{name}={quoted_val}")
            }
        }
        VarValue::Indexed(_) => {
            let attrs = render_attr_prefix(var, true);
            format!("declare {attrs} {name}={quoted_val}")
        }
        VarValue::Associative(_) => {
            let attrs = render_attr_prefix(var, true);
            format!("declare {attrs} {name}")
        }
    }
}

/// Builds the `-[nAaisrxlu]+` prefix (without the leading `declare `
/// keyword). `include_kind=true` adds `a`/`A` for array/assoc; for
/// scalars use `include_kind=false`. Matches the order in
/// `format_declare_line`: `n`, `a`/`A`, `i`, `r`, `x`, `l`/`u`.
#[allow(dead_code)]
fn render_attr_prefix(var: &Variable, include_kind: bool) -> String {
    let mut flags = String::new();
    if var.nameref {
        flags.push('n');
    }
    if include_kind {
        match &var.value {
            VarValue::Indexed(_) => flags.push('a'),
            VarValue::Associative(_) => flags.push('A'),
            _ => {}
        }
    }
    if var.integer {
        flags.push('i');
    }
    if var.readonly {
        flags.push('r');
    }
    if var.exported {
        flags.push('x');
    }
    match var.case_fold {
        Some(crate::shell_state::CaseFold::Lower) => flags.push('l'),
        Some(crate::shell_state::CaseFold::Upper) => flags.push('u'),
        None => {}
    }
    if flags.is_empty() {
        "--".to_string()
    } else {
        format!("-{flags}")
    }
}

/// Single-quote a scalar value for `@A` output. Unlike
/// `declare_scalar_quote` (which leaves bare ASCII bare), bash's
/// `@A` transform ALWAYS single-quotes scalar values — even plain
/// ASCII. Empty string renders as `''`; control chars use ANSI-C
/// `$'…'`; everything else gets single-quoted with `'\''` escaping.
#[allow(dead_code)]
fn always_quote(v: &str) -> String {
    if v.is_empty() {
        return "''".to_string();
    }
    if v.chars().any(|c| c.is_control()) {
        return crate::param_expansion::ansi_c_quote(v);
    }
    format!("'{}'", crate::builtins::escape_alias_value(v))
}

/// `${var@K}` — k/v pairs as a single quoted-internally string.
#[allow(dead_code)]
pub(crate) fn kv_string(_name: &str, _scope: ScopeMode, _shell: &Shell) -> String {
    // Body lands in a later commit.
    String::new()
}

/// `${var@k}` — k/v pairs as a word list (each k and v a separate
/// field when used under quoted `[@]`).
#[allow(dead_code)]
pub(crate) fn kv_words(_name: &str, _scope: ScopeMode, _shell: &Shell) -> Vec<String> {
    // Body lands in a later commit.
    Vec::new()
}

/// `${var@a}` — attribute flag letters in canonical order, or empty.
#[allow(dead_code)]
pub(crate) fn attr_flags(_name: &str, _shell: &Shell) -> String {
    // Body lands in a later commit.
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn assign_decl_scalar_no_attrs() {
        let mut shell = Shell::new();
        shell.set("s", "hello".to_string());
        let out = assign_decl("s", ScopeMode::ScalarOrElement("hello".into()), &shell);
        assert_eq!(out, "s='hello'");
    }

    #[test]
    fn assign_decl_scalar_with_metachar_quotes() {
        let mut shell = Shell::new();
        shell.set("s", "a b".to_string());
        let out = assign_decl("s", ScopeMode::ScalarOrElement("a b".into()), &shell);
        assert_eq!(out, "s='a b'");
    }

    #[test]
    fn assign_decl_exported_scalar() {
        let mut shell = Shell::new();
        shell.set("ev", "42".to_string());
        shell.export("ev");
        let out = assign_decl("ev", ScopeMode::ScalarOrElement("42".into()), &shell);
        assert_eq!(out, "declare -x ev='42'");
    }

    #[test]
    fn assign_decl_unset_is_empty() {
        let shell = Shell::new();
        let out = assign_decl("nope", ScopeMode::ScalarOrElement(String::new()), &shell);
        assert_eq!(out, "");
    }

    #[test]
    fn assign_decl_indexed_whole_uses_renderer() {
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
        let out = assign_decl("a", ScopeMode::Whole, &shell);
        assert_eq!(out, r#"declare -a a=([0]="x" [1]="y")"#);
    }

    #[test]
    fn assign_decl_assoc_whole_has_trailing_space() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell
            .set_associative_element("m", "k".to_string(), "v1".to_string())
            .unwrap();
        let out = assign_decl("m", ScopeMode::Whole, &shell);
        assert_eq!(out, r#"declare -A m=([k]="v1" )"#);
    }

    #[test]
    fn assign_decl_assoc_no_subscript_no_body() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell
            .set_associative_element("m", "k".to_string(), "v".to_string())
            .unwrap();
        // ${m@A} (no subscript) → scalar_view is empty → no body.
        let out = assign_decl("m", ScopeMode::ScalarOrElement(String::new()), &shell);
        assert_eq!(out, "declare -A m");
    }
}
