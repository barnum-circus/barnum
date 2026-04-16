//! Builtin handler implementations.
//!
//! Each [`BuiltinKind`] variant is executed inline by the scheduler (no
//! subprocess). Most are pure data transformations. [`BuiltinKind::Sleep`]
//! is the exception — it awaits a tokio timer before returning.
//! All builtins are infallible except for type mismatches, which produce
//! [`BuiltinError`].

use barnum_ast::BuiltinKind;
use serde_json::{Value, json};

/// Construct a tagged union value: `{ "kind": "{enum_name}.{variant}", "value": value }`.
#[must_use]
pub fn tagged_value(variant: &str, enum_name: &str, value: Value) -> Value {
    json!({ "kind": format!("{enum_name}.{variant}"), "value": value })
}

/// Check whether a JSON value is a specific tagged union variant.
#[must_use]
pub fn is_variant(value: &Value, variant: &str, enum_name: &str) -> bool {
    let expected = format!("{enum_name}.{variant}");
    value
        .get("kind")
        .and_then(Value::as_str)
        .is_some_and(|k| k == expected)
}

/// Extract the `"value"` field from a tagged union value.
#[must_use]
pub fn extract_tagged_value(value: &Value) -> Option<&Value> {
    value.get("value")
}

/// Errors from builtin execution.
#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    /// A builtin received an input of the wrong type.
    #[error("{builtin}: expected {expected}, got {actual}")]
    TypeMismatch {
        /// Which builtin failed.
        builtin: &'static str,
        /// What type was expected.
        expected: &'static str,
        /// The actual value received.
        actual: Value,
    },
    /// A `Panic` builtin was executed. Fatal, not caught by tryCatch.
    #[error("panic: {message}")]
    Panic {
        /// The panic message.
        message: String,
    },
}

/// Execute a builtin operation.
///
/// # Errors
///
/// Returns [`BuiltinError`] if the input doesn't match the builtin's
/// expected type (e.g., `Merge` on a non-array).
#[allow(clippy::too_many_lines)]
pub async fn execute_builtin(
    builtin_kind: &BuiltinKind,
    input: &Value,
) -> Result<Value, BuiltinError> {
    match builtin_kind {
        BuiltinKind::Constant { value } => Ok(value.clone()),

        BuiltinKind::Identity => Ok(input.clone()),

        BuiltinKind::Drop => Ok(Value::Null),

        BuiltinKind::Merge => {
            let Value::Array(items) = input else {
                return Err(BuiltinError::TypeMismatch {
                    builtin: "Merge",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let mut merged = serde_json::Map::new();
            for item in items {
                let Value::Object(obj) = item else {
                    return Err(BuiltinError::TypeMismatch {
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
                return Err(BuiltinError::TypeMismatch {
                    builtin: "Flatten",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let mut result = Vec::new();
            for item in outer {
                let Value::Array(inner) = item else {
                    return Err(BuiltinError::TypeMismatch {
                        builtin: "Flatten",
                        expected: "array element",
                        actual: item.clone(),
                    });
                };
                result.extend(inner.iter().cloned());
            }
            Ok(Value::Array(result))
        }

        BuiltinKind::GetField { field } => {
            let Value::Object(obj) = input else {
                return Err(BuiltinError::TypeMismatch {
                    builtin: "GetField",
                    expected: "object",
                    actual: input.clone(),
                });
            };
            Ok(obj.get(field.as_str()).cloned().unwrap_or(Value::Null))
        }

        BuiltinKind::GetIndex { index } => {
            let Value::Array(arr) = input else {
                return Err(BuiltinError::TypeMismatch {
                    builtin: "GetIndex",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            arr.get(*index).map_or_else(
                || Ok(tagged_value("None", "Option", Value::Null)),
                |value| Ok(tagged_value("Some", "Option", value.clone())),
            )
        }

        BuiltinKind::SplitFirst => {
            let Value::Array(items) = input else {
                return Err(BuiltinError::TypeMismatch {
                    builtin: "SplitFirst",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            if items.is_empty() {
                Ok(tagged_value("None", "Option", Value::Null))
            } else {
                let first = items[0].clone();
                let rest = Value::Array(items[1..].to_vec());
                Ok(tagged_value("Some", "Option", json!([first, rest])))
            }
        }

        BuiltinKind::SplitLast => {
            let Value::Array(items) = input else {
                return Err(BuiltinError::TypeMismatch {
                    builtin: "SplitLast",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            if items.is_empty() {
                Ok(tagged_value("None", "Option", Value::Null))
            } else {
                let last = items[items.len() - 1].clone();
                let init = Value::Array(items[..items.len() - 1].to_vec());
                Ok(tagged_value("Some", "Option", json!([init, last])))
            }
        }

        BuiltinKind::CollectSome => {
            let Value::Array(items) = input else {
                return Err(BuiltinError::TypeMismatch {
                    builtin: "CollectSome",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let mut collected = Vec::new();
            for item in items {
                let Value::Object(obj) = item else {
                    // Skip non-object entries (e.g. null from drop)
                    continue;
                };
                if is_variant(item, "Some", "Option") {
                    collected.push(obj.get("value").cloned().unwrap_or(Value::Null));
                }
                // Skip None and anything else
            }
            Ok(Value::Array(collected))
        }

        BuiltinKind::WrapInField { field } => Ok(json!({ field.as_str(): input })),

        BuiltinKind::Sleep { ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
            Ok(Value::Null)
        }

        BuiltinKind::Panic { message } => Err(BuiltinError::Panic {
            message: message.clone(),
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn constant_ignores_input() {
        let result = execute_builtin(
            &BuiltinKind::Constant { value: json!(42) },
            &json!("ignored"),
        )
        .await;
        assert_eq!(result.unwrap(), json!(42));
    }

    #[tokio::test]
    async fn identity_returns_input() {
        let result = execute_builtin(&BuiltinKind::Identity, &json!({"x": 1})).await;
        assert_eq!(result.unwrap(), json!({"x": 1}));
    }

    #[tokio::test]
    async fn drop_returns_null() {
        let result = execute_builtin(&BuiltinKind::Drop, &json!("anything")).await;
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[tokio::test]
    async fn merge_combines_objects() {
        let input = json!([{"a": 1}, {"b": 2}, {"a": 3}]);
        let result = execute_builtin(&BuiltinKind::Merge, &input).await;
        assert_eq!(result.unwrap(), json!({"a": 3, "b": 2}));
    }

    #[tokio::test]
    async fn merge_rejects_non_array() {
        let result = execute_builtin(&BuiltinKind::Merge, &json!("not array")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn merge_rejects_non_object_element() {
        let result = execute_builtin(&BuiltinKind::Merge, &json!([{"a": 1}, "bad"])).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn flatten_one_level() {
        let input = json!([[1, 2], [3], [4, 5, 6]]);
        let result = execute_builtin(&BuiltinKind::Flatten, &input).await;
        assert_eq!(result.unwrap(), json!([1, 2, 3, 4, 5, 6]));
    }

    #[tokio::test]
    async fn flatten_rejects_non_array() {
        let result = execute_builtin(&BuiltinKind::Flatten, &json!("not array")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn flatten_rejects_non_array_element() {
        let result = execute_builtin(&BuiltinKind::Flatten, &json!([[1], "bad"])).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_field_gets_value() {
        let input = json!({"name": "Alice", "age": 30});
        let result = execute_builtin(
            &BuiltinKind::GetField {
                field: "name".to_string(),
            },
            &input,
        )
        .await;
        assert_eq!(result.unwrap(), json!("Alice"));
    }

    #[tokio::test]
    async fn get_field_missing_returns_null() {
        let input = json!({"name": "Alice"});
        let result = execute_builtin(
            &BuiltinKind::GetField {
                field: "missing".to_string(),
            },
            &input,
        )
        .await;
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[tokio::test]
    async fn get_field_rejects_non_object() {
        let result = execute_builtin(
            &BuiltinKind::GetField {
                field: "field".to_string(),
            },
            &json!("not object"),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_index_returns_some() {
        let input = json!(["a", "b", "c"]);
        let result = execute_builtin(&BuiltinKind::GetIndex { index: 1 }, &input).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.Some", "value": "b"})
        );
    }

    #[tokio::test]
    async fn get_index_out_of_bounds_returns_none() {
        let input = json!(["a"]);
        let result = execute_builtin(&BuiltinKind::GetIndex { index: 5 }, &input).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.None", "value": null})
        );
    }

    #[tokio::test]
    async fn get_index_rejects_non_array() {
        let result =
            execute_builtin(&BuiltinKind::GetIndex { index: 0 }, &json!("not array")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn collect_some_extracts_some_values() {
        let input = json!([
            {"kind": "Option.Some", "value": 1},
            {"kind": "Option.None", "value": null},
            {"kind": "Option.Some", "value": 2},
        ]);
        let result = execute_builtin(&BuiltinKind::CollectSome, &input).await;
        assert_eq!(result.unwrap(), json!([1, 2]));
    }

    #[tokio::test]
    async fn collect_some_handles_all_none() {
        let input = json!([
            {"kind": "Option.None", "value": null},
            {"kind": "Option.None", "value": null},
        ]);
        let result = execute_builtin(&BuiltinKind::CollectSome, &input).await;
        assert_eq!(result.unwrap(), json!([]));
    }

    #[tokio::test]
    async fn collect_some_skips_null_entries() {
        let input = json!([
            {"kind": "Option.Some", "value": "a"},
            null,
            {"kind": "Option.None", "value": null},
        ]);
        let result = execute_builtin(&BuiltinKind::CollectSome, &input).await;
        assert_eq!(result.unwrap(), json!(["a"]));
    }

    #[tokio::test]
    async fn collect_some_empty_array() {
        let result = execute_builtin(&BuiltinKind::CollectSome, &json!([])).await;
        assert_eq!(result.unwrap(), json!([]));
    }

    #[tokio::test]
    async fn collect_some_rejects_non_array() {
        let result = execute_builtin(&BuiltinKind::CollectSome, &json!("not array")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn split_first_non_empty() {
        let input = json!([1, 2, 3]);
        let result = execute_builtin(&BuiltinKind::SplitFirst, &input).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.Some", "value": [1, [2, 3]]})
        );
    }

    #[tokio::test]
    async fn split_first_single_element() {
        let input = json!(["only"]);
        let result = execute_builtin(&BuiltinKind::SplitFirst, &input).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.Some", "value": ["only", []]})
        );
    }

    #[tokio::test]
    async fn split_first_empty() {
        let result = execute_builtin(&BuiltinKind::SplitFirst, &json!([])).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.None", "value": null})
        );
    }

    #[tokio::test]
    async fn split_first_rejects_non_array() {
        let result = execute_builtin(&BuiltinKind::SplitFirst, &json!("not array")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn split_last_non_empty() {
        let input = json!([1, 2, 3]);
        let result = execute_builtin(&BuiltinKind::SplitLast, &input).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.Some", "value": [[1, 2], 3]})
        );
    }

    #[tokio::test]
    async fn split_last_single_element() {
        let input = json!(["only"]);
        let result = execute_builtin(&BuiltinKind::SplitLast, &input).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.Some", "value": [[], "only"]})
        );
    }

    #[tokio::test]
    async fn split_last_empty() {
        let result = execute_builtin(&BuiltinKind::SplitLast, &json!([])).await;
        assert_eq!(
            result.unwrap(),
            json!({"kind": "Option.None", "value": null})
        );
    }

    #[tokio::test]
    async fn split_last_rejects_non_array() {
        let result = execute_builtin(&BuiltinKind::SplitLast, &json!("not array")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn sleep_returns_null() {
        let result = execute_builtin(&BuiltinKind::Sleep { ms: 0 }, &json!({"x": 1})).await;
        assert_eq!(result.unwrap(), Value::Null);
    }

}
