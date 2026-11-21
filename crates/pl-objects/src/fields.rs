//! DRF field validation for the writable fields of the object serializers.
//!
//! upstream: rest_framework 3.18.3 fields.py (Field.validate_empty_values, CharField, BooleanField,
//!           ChoiceField, IntegerField, ListField), relations.py (RelatedField.run_validation,
//!           PrimaryKeyRelatedField), serializers.py (Serializer.to_internal_value),

use serde_json::{Map, Value, json};

use crate::text::{is_py_space, py_str, py_type_name};

/// Raw input of one field.
#[derive(Clone, Copy, Debug)]
pub enum Input<'a> {
    Missing,
    Null,
    Val(&'a Value),
}

pub fn input<'a>(data: &'a Map<String, Value>, key: &str) -> Input<'a> {
    match data.get(key) {
        None => Input::Missing,
        Some(Value::Null) => Input::Null,
        Some(v) => Input::Val(v),
    }
}

/// Outcome of a field: `Skip` = not in validated data.
#[derive(Clone, Debug, PartialEq)]
pub enum Out<T> {
    Skip,
    Set(T),
}

pub type FieldResult<T> = Result<Out<T>, Value>;

fn err(msg: &str) -> Value {
    json!([msg])
}

pub struct CharOpts {
    pub required: bool,
    pub allow_blank: bool,
    pub max_length: usize,
}

/// `validate_empty_values` for fields without default.
fn empty_values<T>(inp: Input, partial: bool, required: bool, allow_null: bool) -> Option<FieldResult<Option<T>>> {
    match inp {
        Input::Missing => {
            if partial {
                return Some(Ok(Out::Skip));
            }
            if required {
                return Some(Err(err("This field is required.")));
            }
            Some(Ok(Out::Skip))
        }
        Input::Null => {
            if !allow_null {
                return Some(Err(err("This field may not be null.")));
            }
            Some(Ok(Out::Set(None)))
        }
        Input::Val(_) => None,
    }
}

/// `CharField.run_validation` (trim_whitespace=True, allow_null=False).
pub fn char_field(inp: Input, partial: bool, o: &CharOpts) -> FieldResult<String> {
    if let Input::Val(v) = inp {
        let s = py_str(v);
        if s.trim_matches(is_py_space).is_empty() && !matches!(v, Value::Array(_) | Value::Object(_)) {
            if !o.allow_blank {
                return Err(err("This field may not be blank."));
            }
            return Ok(Out::Set(String::new()));
        }
    }
    if let Some(r) = empty_values::<String>(inp, partial, o.required, false) {
        return r.map(|o| match o {
            Out::Skip => Out::Skip,
            Out::Set(v) => Out::Set(v.unwrap_or_default()),
        });
    }
    let Input::Val(v) = inp else { return Ok(Out::Skip) };
    let s = match v {
        Value::String(_) | Value::Number(_) => py_str(v),
        _ => return Err(err("Not a valid string.")),
    };
    let s = s.trim_matches(is_py_space).to_string();
    let mut errors = vec![];
    if s.chars().count() > o.max_length {
        errors.push(Value::String(format!("Ensure this field has no more than {} characters.", o.max_length)));
    }
    if s.contains('\0') {
        errors.push(Value::String("Null characters are not allowed.".into()));
    }
    if !errors.is_empty() {
        return Err(Value::Array(errors));
    }
    Ok(Out::Set(s))
}

/// `BooleanField` (required=False, allow_null=False).
pub fn bool_field(inp: Input, partial: bool) -> FieldResult<bool> {
    if let Some(r) = empty_values::<bool>(inp, partial, false, false) {
        return r.map(|o| match o {
            Out::Skip => Out::Skip,
            Out::Set(v) => Out::Set(v.unwrap_or(false)),
        });
    }
    let Input::Val(v) = inp else { return Ok(Out::Skip) };
    let t = match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.to_lowercase().as_str() {
            "t" | "y" | "yes" | "true" | "on" | "1" => Some(true),
            "f" | "n" | "no" | "false" | "off" | "0" => Some(false),
            _ => None,
        },
        Value::Number(n) => match n.as_f64() {
            Some(1.0) => Some(true),
            Some(0.0) => Some(false),
            _ => None,
        },
        _ => None,
    };
    match t {
        Some(b) => Ok(Out::Set(b)),
        None => Err(err("Must be a valid boolean.")),
    }
}

/// `ChoiceField` over `MatchingModel.MATCHING_ALGORITHMS` (required=False).
pub fn matching_algorithm(inp: Input, partial: bool) -> FieldResult<i16> {
    if let Some(r) = empty_values::<i16>(inp, partial, false, false) {
        return r.map(|o| match o {
            Out::Skip => Out::Skip,
            Out::Set(v) => Out::Set(v.unwrap_or(1)),
        });
    }
    let Input::Val(v) = inp else { return Ok(Out::Skip) };
    let s = py_str(v);
    match s.as_str() {
        "0" | "1" | "2" | "3" | "4" | "5" | "6" => Ok(Out::Set(s.parse().unwrap_or(1))),
        _ => Err(err(&format!("\"{s}\" is not a valid choice."))),
    }
}

/// Candidate pk of a `PrimaryKeyRelatedField` before the DB lookup.
#[derive(Clone, Debug, PartialEq)]
pub enum PkCandidate {
    /// Python `int(data)` succeeded (`None` = outside the column range → does not exist).
    Id(Option<i64>),
}

/// `RelatedField.run_validation` + type part of `PrimaryKeyRelatedField.to_internal_value`
/// (allow_null=True). `default_none`: field has `default=None` (owner via DRF uniqueness kwargs).
/// Returns `Set(None)` for null, `Set(Some((candidate, repr)))` for a value to look up.
pub fn pk_field(inp: Input, partial: bool, default_none: bool) -> FieldResult<Option<(PkCandidate, String)>> {
    let inp = match inp {
        Input::Val(Value::String(s)) if s.is_empty() => Input::Null,
        other => other,
    };
    match inp {
        Input::Missing => {
            if partial || !default_none {
                Ok(Out::Skip)
            } else {
                Ok(Out::Set(None))
            }
        }
        Input::Null => Ok(Out::Set(None)),
        Input::Val(v) => {
            let incorrect = || err(&format!("Incorrect type. Expected pk value, received {}.", py_type_name(v)));
            let id: Option<i64> = match v {
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Some(i)
                    } else if let Some(f) = n.as_f64() {
                        // Django IntegerField.get_prep_value: int(value) truncates
                        if f.is_finite() { Some(f.trunc() as i64) } else { return Err(incorrect()) }
                    } else {
                        None
                    }
                }
                Value::String(s) => match crate::text::py_int(s) {
                    Some(i) => Some(i),
                    None => return Err(incorrect()),
                },
                _ => return Err(incorrect()),
            };
            let cand = PkCandidate::Id(id.filter(|i| i32::try_from(*i).is_ok()));
            Ok(Out::Set(Some((cand, py_str(v)))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(v: &Value) -> Input<'_> {
        Input::Val(v)
    }

    #[test]
    fn char_rules() {
        let o = CharOpts { required: true, allow_blank: false, max_length: 3 };
        assert_eq!(char_field(Input::Missing, false, &o), Err(json!(["This field is required."])));
        assert_eq!(char_field(Input::Missing, true, &o), Ok(Out::Skip));
        assert_eq!(char_field(Input::Null, false, &o), Err(json!(["This field may not be null."])));
        assert_eq!(char_field(val(&json!("  ")), false, &o), Err(json!(["This field may not be blank."])));
        assert_eq!(char_field(val(&json!(" ab ")), false, &o), Ok(Out::Set("ab".into())));
        assert_eq!(char_field(val(&json!(12)), false, &o), Ok(Out::Set("12".into())));
        assert_eq!(char_field(val(&json!(true)), false, &o), Err(json!(["Not a valid string."])));
        assert_eq!(char_field(val(&json!("abcd")), false, &o), Err(json!(["Ensure this field has no more than 3 characters."])));
        let blank = CharOpts { required: false, allow_blank: true, max_length: 3 };
        assert_eq!(char_field(val(&json!(" ")), false, &blank), Ok(Out::Set(String::new())));
    }

    #[test]
    fn bool_and_choice() {
        assert_eq!(bool_field(val(&json!("Yes")), false), Ok(Out::Set(true)));
        assert_eq!(bool_field(val(&json!(1.0)), false), Ok(Out::Set(true)));
        assert_eq!(bool_field(val(&json!(2)), false), Err(json!(["Must be a valid boolean."])));
        assert_eq!(matching_algorithm(val(&json!("4")), false), Ok(Out::Set(4)));
        assert_eq!(matching_algorithm(val(&json!(7)), false), Err(json!(["\"7\" is not a valid choice."])));
        assert_eq!(matching_algorithm(val(&json!(true)), false), Err(json!(["\"True\" is not a valid choice."])));
    }

    #[test]
    fn pk_rules() {
        assert_eq!(pk_field(Input::Missing, false, true), Ok(Out::Set(None)));
        assert_eq!(pk_field(Input::Missing, true, true), Ok(Out::Skip));
        assert_eq!(pk_field(val(&json!("")), false, false), Ok(Out::Set(None)));
        assert_eq!(pk_field(val(&json!(true)), false, false), Err(json!(["Incorrect type. Expected pk value, received bool."])));
        assert_eq!(pk_field(val(&json!("x")), false, false), Err(json!(["Incorrect type. Expected pk value, received str."])));
        assert_eq!(pk_field(val(&json!("5")), false, false), Ok(Out::Set(Some((PkCandidate::Id(Some(5)), "5".into())))));
    }
}
