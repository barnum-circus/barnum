//! Builtin handler implementations.
//!
//! Each [`BuiltinKind`] variant maps to a pure data transformation executed
//! inline by the scheduler (no subprocess). All builtins are infallible
//! except for type mismatches, which produce [`BuiltinError`].

use barnum_ast::BuiltinKind;
use serde_json::{Value, json};

/// Errors from builtin execution (type mismatches).
#[derive(Debug, thiserror::Error)]
#[error("{builtin}: expected {expected}, got {actual}")]
pub struct BuiltinError {
    /// Which builtin failed.
    pub builtin: &'static str,
    /// What type was expected.
    pub expected: &'static str,
    /// The actual value received.
    pub actual: Value,
}

/// Execute a builtin operation.
///
/// # Errors
///
/// Returns [`BuiltinError`] if the input doesn't match the builtin's
/// expected type (e.g., `Merge` on a non-array).
#[allow(clippy::too_many_lines)]
pub fn execute_builtin(builtin_kind: &BuiltinKind, input: &Value) -> Result<Value, BuiltinError> {
    match builtin_kind {
        BuiltinKind::Constant { value } => Ok(value.clone()),

        BuiltinKind::Identity => Ok(input.clone()),

        BuiltinKind::Drop => Ok(Value::Null),

        BuiltinKind::Tag { value: tag } => Ok(json!({ "kind": tag, "value": input })),

        BuiltinKind::Merge => {
            let Value::Array(items) = input else {
                return Err(BuiltinError {
                    builtin: "Merge",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let mut merged = serde_json::Map::new();
            for item in items {
                let Value::Object(obj) = item else {
                    return Err(BuiltinError {
                        builtin: "Merge",
                        expected: "object in array",
                        actual: item.clone(),
                    });
                };
                for (k, v) in obj {
                    merged.insert(k.clone(), v.clone());
                }
            }
            Ok(Value::Object(merged))
        }

        BuiltinKind::Flatten => {
            let Value::Array(outer) = input else {
                return Err(BuiltinError {
                    builtin: "Flatten",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let mut result = Vec::new();
            for item in outer {
                let Value::Array(inner) = item else {
                    return Err(BuiltinError {
                        builtin: "Flatten",
                        expected: "array element",
                        actual: item.clone(),
                    });
                };
                result.extend(inner.iter().cloned());
            }
            Ok(Value::Array(result))
        }

        BuiltinKind::ExtractField { value: field } => {
            let Value::String(field_name) = field else {
                return Err(BuiltinError {
                    builtin: "ExtractField",
                    expected: "string field name",
                    actual: field.clone(),
                });
            };
            let Value::Object(obj) = input else {
                return Err(BuiltinError {
                    builtin: "ExtractField",
                    expected: "object",
                    actual: input.clone(),
                });
            };
            Ok(obj.get(field_name.as_str()).cloned().unwrap_or(Value::Null))
        }

        BuiltinKind::ExtractIndex { value: index } => {
            let Some(index_number) = index.as_u64() else {
                return Err(BuiltinError {
                    builtin: "ExtractIndex",
                    expected: "non-negative integer index",
                    actual: index.clone(),
                });
            };
            let Value::Array(arr) = input else {
                return Err(BuiltinError {
                    builtin: "ExtractIndex",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let index = usize::try_from(index_number).map_err(|_| BuiltinError {
                builtin: "ExtractIndex",
                expected: "index within usize range",
                actual: index.clone(),
            })?;
            Ok(arr.get(index).cloned().unwrap_or(Value::Null))
        }

        BuiltinKind::Pick { value: keys } => {
            let Value::Array(key_values) = keys else {
                return Err(BuiltinError {
                    builtin: "Pick",
                    expected: "array of field names",
                    actual: keys.clone(),
                });
            };
            let Value::Object(obj) = input else {
                return Err(BuiltinError {
                    builtin: "Pick",
                    expected: "object",
                    actual: input.clone(),
                });
            };
            let mut picked = serde_json::Map::new();
            for key_value in key_values {
                let Value::String(key) = key_value else {
                    return Err(BuiltinError {
                        builtin: "Pick",
                        expected: "string field name in keys array",
                        actual: key_value.clone(),
                    });
                };
                if let Some(value) = obj.get(key.as_str()) {
                    picked.insert(key.clone(), value.clone());
                }
            }
            Ok(Value::Object(picked))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constant_ignores_input() {
        let result = execute_builtin(
            &BuiltinKind::Constant { value: json!(42) },
            &json!("ignored"),
        );
        assert_eq!(result.unwrap(), json!(42));
    }

    #[test]
    fn identity_returns_input() {
        let result = execute_builtin(&BuiltinKind::Identity, &json!({"x": 1}));
        assert_eq!(result.unwrap(), json!({"x": 1}));
    }

    #[test]
    fn drop_returns_null() {
        let result = execute_builtin(&BuiltinKind::Drop, &json!("anything"));
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[test]
    fn tag_wraps_input() {
        let result = execute_builtin(
            &BuiltinKind::Tag {
                value: json!("Continue"),
            },
            &json!(42),
        );
        assert_eq!(result.unwrap(), json!({"kind": "Continue", "value": 42}));
    }

    #[test]
    fn merge_combines_objects() {
        let input = json!([{"a": 1}, {"b": 2}, {"a": 3}]);
        let result = execute_builtin(&BuiltinKind::Merge, &input);
        assert_eq!(result.unwrap(), json!({"a": 3, "b": 2}));
    }

    #[test]
    fn merge_rejects_non_array() {
        let result = execute_builtin(&BuiltinKind::Merge, &json!("not array"));
        assert!(result.is_err());
    }

    #[test]
    fn merge_rejects_non_object_element() {
        let result = execute_builtin(&BuiltinKind::Merge, &json!([{"a": 1}, "bad"]));
        assert!(result.is_err());
    }

    #[test]
    fn flatten_one_level() {
        let input = json!([[1, 2], [3], [4, 5, 6]]);
        let result = execute_builtin(&BuiltinKind::Flatten, &input);
        assert_eq!(result.unwrap(), json!([1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn flatten_rejects_non_array() {
        let result = execute_builtin(&BuiltinKind::Flatten, &json!("not array"));
        assert!(result.is_err());
    }

    #[test]
    fn flatten_rejects_non_array_element() {
        let result = execute_builtin(&BuiltinKind::Flatten, &json!([[1], "bad"]));
        assert!(result.is_err());
    }

    #[test]
    fn extract_field_gets_value() {
        let input = json!({"name": "Alice", "age": 30});
        let result = execute_builtin(
            &BuiltinKind::ExtractField {
                value: json!("name"),
            },
            &input,
        );
        assert_eq!(result.unwrap(), json!("Alice"));
    }

    #[test]
    fn extract_field_missing_returns_null() {
        let input = json!({"name": "Alice"});
        let result = execute_builtin(
            &BuiltinKind::ExtractField {
                value: json!("missing"),
            },
            &input,
        );
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[test]
    fn extract_field_rejects_non_object() {
        let result = execute_builtin(
            &BuiltinKind::ExtractField {
                value: json!("field"),
            },
            &json!("not object"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn extract_index_gets_value() {
        let input = json!(["a", "b", "c"]);
        let result = execute_builtin(&BuiltinKind::ExtractIndex { value: json!(1) }, &input);
        assert_eq!(result.unwrap(), json!("b"));
    }

    #[test]
    fn extract_index_out_of_bounds_returns_null() {
        let input = json!(["a"]);
        let result = execute_builtin(&BuiltinKind::ExtractIndex { value: json!(5) }, &input);
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[test]
    fn extract_index_rejects_non_array() {
        let result = execute_builtin(
            &BuiltinKind::ExtractIndex { value: json!(0) },
            &json!("not array"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn extract_index_rejects_non_integer() {
        let result = execute_builtin(
            &BuiltinKind::ExtractIndex {
                value: json!("bad"),
            },
            &json!([1, 2]),
        );
        assert!(result.is_err());
    }

    #[test]
    fn pick_selects_fields() {
        let input = json!({"name": "Alice", "age": 30, "email": "a@b.com"});
        let result = execute_builtin(
            &BuiltinKind::Pick {
                value: json!(["name", "age"]),
            },
            &input,
        );
        assert_eq!(result.unwrap(), json!({"name": "Alice", "age": 30}));
    }

    #[test]
    fn pick_ignores_missing_fields() {
        let input = json!({"name": "Alice"});
        let result = execute_builtin(
            &BuiltinKind::Pick {
                value: json!(["name", "missing"]),
            },
            &input,
        );
        assert_eq!(result.unwrap(), json!({"name": "Alice"}));
    }

    #[test]
    fn pick_rejects_non_object() {
        let result = execute_builtin(
            &BuiltinKind::Pick {
                value: json!(["name"]),
            },
            &json!("not object"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn pick_empty_keys_returns_empty_object() {
        let input = json!({"name": "Alice", "age": 30});
        let result = execute_builtin(&BuiltinKind::Pick { value: json!([]) }, &input);
        assert_eq!(result.unwrap(), json!({}));
    }
}
