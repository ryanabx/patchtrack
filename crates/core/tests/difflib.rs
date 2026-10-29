//! The difflib port against Python's own difflib, on generated cases
//! (tests/fixtures/difflib.json; some long enough for autojunk).

use patchtrack_core::difflib::{SequenceMatcher, Tag, unified_diff};
use serde_json::Value;

fn strings(v: &Value) -> Vec<&str> {
    v.as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect()
}

#[test]
fn matches_python() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../../../tests/fixtures/difflib.json")).unwrap();
    assert!(cases.len() > 40);
    for (k, c) in cases.iter().enumerate() {
        let (a, b) = (strings(&c["a"]), strings(&c["b"]));
        let n = c["n"].as_u64().unwrap() as usize;
        assert_eq!(unified_diff(&a, &b, "old", "new", n), strings(&c["diff"]), "case {k}");
        let sm = SequenceMatcher::new(&a, &b);
        let opcodes: Vec<Value> = sm
            .opcodes()
            .into_iter()
            .map(|(t, i1, i2, j1, j2)| {
                let tag = match t {
                    Tag::Equal => "equal",
                    Tag::Replace => "replace",
                    Tag::Delete => "delete",
                    Tag::Insert => "insert",
                };
                serde_json::json!([tag, i1, i2, j1, j2])
            })
            .collect();
        assert_eq!(&Value::Array(opcodes), &c["opcodes"], "case {k}");
        assert!((sm.ratio() - c["ratio"].as_f64().unwrap()).abs() < 1e-12, "case {k}");
    }
}
