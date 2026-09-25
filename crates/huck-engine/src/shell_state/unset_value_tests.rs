use super::*;

/// Every value mutator must MATERIALISE an unset variable — the structural
/// risk of this iteration is a write path that stores a value while leaving
/// the variable unset, which would silently report a live variable as unset.
#[test]
fn every_mutator_materialises_an_unset_variable() {
    // (label, shape, apply) — `apply` performs one write through the public
    // mutator surface. After it, the variable must be set.
    let cases: Vec<(&str, Shape, Box<dyn Fn(&mut Shell)>)> = vec![
        (
            "scalar set",
            Shape::Scalar,
            Box::new(|sh: &mut Shell| sh.set("v", "x".to_string())),
        ),
        (
            "indexed element",
            Shape::Indexed,
            Box::new(|sh: &mut Shell| {
                sh.set_indexed_element("v", 2, "x".to_string()).unwrap();
            }),
        ),
        (
            "indexed extend",
            Shape::Indexed,
            Box::new(|sh: &mut Shell| {
                let mut m = std::collections::BTreeMap::new();
                m.insert(0, "a".to_string());
                sh.extend_indexed("v", m).unwrap();
            }),
        ),
        (
            "indexed replace",
            Shape::Indexed,
            Box::new(|sh: &mut Shell| {
                let mut m = std::collections::BTreeMap::new();
                m.insert(0, "a".to_string());
                sh.replace_indexed("v", m).unwrap();
            }),
        ),
        (
            "assoc element",
            Shape::Associative,
            Box::new(|sh: &mut Shell| {
                sh.set_associative_element("v", "k".to_string(), "x".to_string())
                    .unwrap();
            }),
        ),
    ];
    for (label, shape, apply) in cases {
        let mut sh = Shell::new();
        sh.vars.insert("v".to_string(), Variable::unset(shape));
        assert!(
            sh.vars["v"].value.is_unset(),
            "{label}: fixture should start unset"
        );
        apply(&mut sh);
        assert!(
            !sh.vars["v"].value.is_unset(),
            "{label}: the mutator left the variable UNSET — a silent write path"
        );
    }
}

/// A scalar read of an unset value is the empty string (the 12 `scalar_view`
/// callers keep working); the shape survives for the conversion table.
#[test]
fn unset_value_reads_empty_and_keeps_its_shape() {
    assert_eq!(VarValue::Unset(Shape::Indexed).scalar_view(), "");
    assert_eq!(VarValue::Unset(Shape::Indexed).shape(), Shape::Indexed);
    assert_eq!(VarValue::Scalar("x".to_string()).shape(), Shape::Scalar);
    assert_eq!(
        VarValue::Associative(crate::assoc_map::AssocMap::new()).shape(),
        Shape::Associative
    );
    assert!(!VarValue::Scalar(String::new()).is_unset());
}

/// An unset scalar that is written through `install_scalar_value` becomes a
/// Scalar; an unset INDEXED written the same way becomes element 0 (bash's
/// `y=v` on an array is `y[0]=v`).
#[test]
fn install_scalar_value_materialises_by_shape() {
    let mut v = Variable::unset(Shape::Scalar);
    assert!(!install_scalar_value(&mut v, "x".to_string()));
    assert!(matches!(&v.value, VarValue::Scalar(s) if s == "x"));

    let mut v = Variable::unset(Shape::Indexed);
    assert!(!install_scalar_value(&mut v, "x".to_string()));
    match &v.value {
        VarValue::Indexed(m) => assert_eq!(m.get(&0).map(String::as_str), Some("x")),
        other => panic!("expected Indexed, got {other:?}"),
    }

    // An unset ASSOCIATIVE rejects a scalar install, exactly as a set one does.
    let mut v = Variable::unset(Shape::Associative);
    assert!(install_scalar_value(&mut v, "x".to_string()));
}

/// #600 fix round 1: store_assoc_element respects shape and rejects
/// Unset(Indexed), not materializing incompatible shapes.
#[test]
fn store_assoc_element_shape_crossing() {
    // Unset(Indexed) cannot become associative.
    let mut sh = Shell::new();
    sh.vars
        .insert("v".to_string(), Variable::unset(Shape::Indexed));
    let result = sh.set_associative_element("v", "k".to_string(), "x".to_string());
    assert!(matches!(
        result,
        Err(crate::shell_state::AssignErr::TypeMismatch)
    ));
    assert!(sh.vars["v"].value.is_unset()); // Variable unchanged
    assert_eq!(sh.vars["v"].value.shape(), Shape::Indexed);

    // Unset(Scalar) CAN become associative.
    let mut sh = Shell::new();
    sh.vars
        .insert("v".to_string(), Variable::unset(Shape::Scalar));
    let result = sh.set_associative_element("v", "k".to_string(), "x".to_string());
    assert!(result.is_ok());
    assert!(!sh.vars["v"].value.is_unset());

    // Unset(Associative) is OK.
    let mut sh = Shell::new();
    sh.vars
        .insert("v".to_string(), Variable::unset(Shape::Associative));
    let result = sh.set_associative_element("v", "k".to_string(), "x".to_string());
    assert!(result.is_ok());
    assert!(!sh.vars["v"].value.is_unset());
}

/// #600 fix round 1: declare_associative respects shape. Unset(Scalar)
/// accepts the declaration; Unset(Associative) is already correct;
/// Unset(Indexed) refuses like a materialised indexed array.
#[test]
fn declare_associative_shape_crossing() {
    use crate::shell_state::DeclareErr;

    // Unset(Scalar) becomes associative.
    let mut sh = Shell::new();
    sh.vars
        .insert("v".to_string(), Variable::unset(Shape::Scalar));
    let result = sh.declare_associative("v");
    assert!(matches!(result, Ok(())));
    assert!(matches!(sh.vars["v"].value, VarValue::Associative(_)));

    // Unset(Associative) is already associative.
    let mut sh = Shell::new();
    sh.vars
        .insert("v".to_string(), Variable::unset(Shape::Associative));
    let result = sh.declare_associative("v");
    assert!(matches!(result, Ok(())));
    assert!(matches!(sh.vars["v"].value, VarValue::Associative(_)));

    // Unset(Indexed) cannot become associative (like a materialised indexed array).
    let mut sh = Shell::new();
    sh.vars
        .insert("v".to_string(), Variable::unset(Shape::Indexed));
    let result = sh.declare_associative("v");
    assert!(matches!(result, Err(DeclareErr::IndexedExists)));
}

/// #600 fix round 1: attr_flags drives array markers off shape, not
/// the materialised value. Unset(Indexed) → 'a', Unset(Associative) → 'A',
/// Unset(Scalar) → no marker.
#[test]
fn attr_flags_respects_unset_shape() {
    use crate::array_transforms::attr_flags;

    // Unset(Indexed) should render the 'a' flag.
    let mut sh = Shell::new();
    sh.vars
        .insert("y".to_string(), Variable::unset(Shape::Indexed));
    let flags = attr_flags("y", &sh);
    assert!(
        flags.contains('a'),
        "Unset(Indexed) should have 'a' in flags"
    );

    // Unset(Associative) should render the 'A' flag.
    let mut sh = Shell::new();
    sh.vars
        .insert("x".to_string(), Variable::unset(Shape::Associative));
    let flags = attr_flags("x", &sh);
    assert!(
        flags.contains('A'),
        "Unset(Associative) should have 'A' in flags"
    );

    // Unset(Scalar) should not render an array marker.
    let mut sh = Shell::new();
    sh.vars
        .insert("s".to_string(), Variable::unset(Shape::Scalar));
    let flags = attr_flags("s", &sh);
    assert!(
        !flags.contains('a') && !flags.contains('A'),
        "Unset(Scalar) should not have 'a' or 'A' in flags"
    );

    // With integer flag on an Unset(Indexed).
    let mut sh = Shell::new();
    let mut v = Variable::unset(Shape::Indexed);
    v.integer = true;
    sh.vars.insert("n".to_string(), v);
    let flags = attr_flags("n", &sh);
    assert!(flags.contains('a'), "should have 'a'");
    assert!(flags.contains('i'), "should have 'i'");
}

/// An unset variable reads exactly like a name that does not exist — which
/// is what keeps all 63 `lookup_var` callers correct without edits.
#[test]
fn unset_reads_as_absent() {
    let mut sh = Shell::new();
    sh.vars
        .insert("v".to_string(), Variable::unset(Shape::Scalar));
    assert_eq!(sh.lookup_var("v"), None);
    assert_eq!(sh.get("v"), None);
    assert!(!sh.is_set("v"), "[[ -v v ]] must be false while unset");

    // …and like a normal variable once it has one.
    sh.set("v", String::new());
    assert_eq!(sh.lookup_var("v").as_deref(), Some(""));
    assert!(sh.is_set("v"), "an empty assignment SETS the variable");
}

/// `declare -p` prints the declaration with no `=…` while unset, and with it
/// once set. Attributes are real either way.
#[test]
fn declare_p_omits_the_value_while_unset() {
    let mut y = Variable::unset(Shape::Indexed);
    assert_eq!(
        crate::builtins::format_declare_line("y", &y),
        "declare -a y"
    );
    y.readonly = true;
    assert_eq!(
        crate::builtins::format_declare_line("y", &y),
        "declare -ar y"
    );

    let mut n = Variable::unset(Shape::Scalar);
    n.integer = true;
    assert_eq!(
        crate::builtins::format_declare_line("n", &n),
        "declare -i n"
    );

    let mut v = Variable::unset(Shape::Scalar);
    v.exported = true;
    assert_eq!(
        crate::builtins::format_declare_line("v", &v),
        "declare -x v"
    );

    // Plain, no attributes: bash prints `declare -- v`.
    let v = Variable::unset(Shape::Scalar);
    assert_eq!(
        crate::builtins::format_declare_line("v", &v),
        "declare -- v"
    );
}
