use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON Schema document embedded in the AST.
///
/// Newtype over `Value` — the TS side produces it via `zodToCheckedJsonSchema`,
/// and `HANDLER_VALIDATION.md` will compile it with the `jsonschema` crate
/// at workflow init time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonSchema(pub Value);
