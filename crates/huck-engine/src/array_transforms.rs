//! Whole-array and scalar `${var@OP}` transform implementations for
//! `@A` (declare-style assignment), `@K` (k/v pairs string), `@k` (k/v
//! pairs word list), and `@a` (attribute flag letters). Called from
//! `expand_array_param` / `expand_assoc_param` for the whole-array
//! forms and from `param_expansion::expand_modifier_with_value`'s
//! `Transform { op }` arm for the scalar / single-element forms.

use crate::shell_state::Shell;

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
pub(crate) fn assign_decl(_name: &str, _scope: ScopeMode, _shell: &Shell) -> String {
    // Body lands in a later commit.
    String::new()
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
