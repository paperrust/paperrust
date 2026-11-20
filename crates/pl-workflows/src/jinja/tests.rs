//! Comparison against real Jinja2 3.1.6 (vectors from `tests/parity/pl-workflows/jinja_oracle.py`).

use super::*;

#[test]
fn oracle_vectors() {
    let data: serde_json::Value = serde_json::from_str(include_str!("oracle.json")).expect("oracle.json");
    let mut failures = Vec::new();
    let cases = data.as_array().expect("array");
    for case in cases {
        let tpl = case["template"].as_str().unwrap_or_default();
        let expected = case["result"].as_str().map(str::to_string);
        let got = match validate_template(tpl) {
            Ok(()) => None,
            Err(TemplateError::Invalid(m)) => Some(m),
            Err(TemplateError::Internal(_)) => Some("INTERNAL AssertionError".to_string()),
        };
        if got != expected {
            failures.push(format!("{tpl:?}\n   expected {expected:?}\n   got      {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
