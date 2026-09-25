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
