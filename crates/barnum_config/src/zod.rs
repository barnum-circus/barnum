//! Zod schema renderer for schemars `RootSchema`.
//!
//! Walks a schemars `RootSchema` tree and emits a TypeScript file containing
//! Zod schemas. This is a second renderer for the same schemars introspection
//! tree that the JSON Schema renderer uses — both start from Rust types, not
//! from each other.

use schemars::schema::{
    InstanceType, Metadata, ObjectValidation, RootSchema, Schema, SchemaObject, SingleOrVec,
};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

type Defs = BTreeMap<String, Schema>;

/// Render a schemars `RootSchema` as a TypeScript file containing Zod schemas.
#[must_use]
pub fn emit_zod(root: &RootSchema) -> String {
    let mut e = Emitter::new();

    let _ = writeln!(e.out, "import {{ z }} from \"zod\";");

    let ordered = topological_sort(&root.definitions);
    for name in &ordered {
        let _ = writeln!(e.out);
        let schema = &root.definitions[name.as_str()];
        let _ = write!(e.out, "const {name} = ");
        e.emit_schema(schema);
        let _ = writeln!(e.out, ";");
    }

    let _ = writeln!(e.out);
    let _ = write!(e.out, "export const configFileSchema = ");
    e.emit_schema_object(&root.schema);
    let _ = writeln!(e.out, ";");

    let _ = writeln!(e.out);
    let _ = writeln!(
        e.out,
        "export type ConfigFile = z.infer<typeof configFileSchema>;"
    );
    for name in &ordered {
        let _ = writeln!(e.out, "export type {name} = z.infer<typeof {name}>;");
    }

    let _ = writeln!(e.out);
    let _ = writeln!(
        e.out,
        "export function defineConfig(config: z.input<typeof configFileSchema>): ConfigFile {{"
    );
    let _ = writeln!(e.out, "  return configFileSchema.parse(config);");
    let _ = writeln!(e.out, "}}");

    e.out
}

struct Emitter {
    out: String,
    indent: usize,
}

impl Emitter {
    const fn new() -> Self {
        Self {
            out: String::new(),
            indent: 0,
        }
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent {
            self.out.push(' ');
        }
    }

    fn emit_schema(&mut self, schema: &Schema) {
        match schema {
            Schema::Bool(true) => {
                let _ = write!(self.out, "z.any()");
            }
            Schema::Bool(false) => {
                let _ = write!(self.out, "z.never()");
            }
            Schema::Object(obj) => self.emit_schema_object(obj),
        }
    }

    fn emit_schema_object(&mut self, obj: &SchemaObject) {
        self.emit_base_type(obj);
        self.emit_metadata_modifiers(obj.metadata.as_deref());
    }

    fn emit_property_schema(&mut self, schema: &Schema, required: bool) {
        match schema {
            Schema::Bool(true) => {
                let _ = write!(self.out, "z.any()");
                if !required {
                    let _ = write!(self.out, ".optional()");
                }
            }
            Schema::Bool(false) => {
                let _ = write!(self.out, "z.never()");
            }
            Schema::Object(obj) => {
                self.emit_base_type(obj);
                if !required {
                    let _ = write!(self.out, ".optional()");
                }
                self.emit_metadata_modifiers(obj.metadata.as_deref());
            }
        }
    }

    fn emit_base_type(&mut self, obj: &SchemaObject) {
        if let Some(ref r) = obj.reference
            && let Some(name) = r.strip_prefix("#/definitions/")
        {
            let _ = write!(self.out, "{name}");
            return;
        }

        if let Some(ref subs) = obj.subschemas {
            if let Some(ref one_of) = subs.one_of {
                self.emit_one_of(one_of);
                return;
            }
            if let Some(ref any_of) = subs.any_of {
                self.emit_any_of(any_of);
                return;
            }
            if let Some(ref all_of) = subs.all_of
                && all_of.len() == 1
            {
                self.emit_schema(&all_of[0]);
                return;
            }
        }

        if let Some(ref enum_vals) = obj.enum_values {
            self.emit_enum_values(enum_vals);
            return;
        }

        if let Some(ref instance_type) = obj.instance_type {
            self.emit_instance_type(instance_type, obj);
            return;
        }

        let _ = write!(self.out, "z.any()");
    }

    fn emit_schema_list(&mut self, kind: &str, schemas: &[Schema]) {
        let _ = writeln!(self.out, "{kind}[");
        self.indent += 2;
        for schema in schemas {
            self.write_indent();
            self.emit_schema(schema);
            let _ = writeln!(self.out, ",");
        }
        self.indent -= 2;
        self.write_indent();
        let _ = write!(self.out, "])");
    }

    fn emit_one_of(&mut self, schemas: &[Schema]) {
        if let Some(discriminator) = find_discriminator(schemas) {
            self.emit_schema_list(
                &format!("z.discriminatedUnion(\"{discriminator}\", "),
                schemas,
            );
        } else {
            self.emit_schema_list("z.union(", schemas);
        }
    }

    fn emit_any_of(&mut self, schemas: &[Schema]) {
        if schemas.len() == 2 {
            let (null_idx, other_idx) = if is_null_schema(&schemas[1]) {
                (Some(1), 0)
            } else if is_null_schema(&schemas[0]) {
                (Some(0), 1)
            } else {
                (None, 0)
            };
            if null_idx.is_some() {
                self.emit_schema(&schemas[other_idx]);
                let _ = write!(self.out, ".nullable()");
                return;
            }
        }

        self.emit_schema_list("z.union(", schemas);
    }

    fn emit_enum_values(&mut self, values: &[serde_json::Value]) {
        if values.len() == 1 {
            let _ = write!(self.out, "z.literal(");
            emit_json_value(&mut self.out, &values[0]);
            let _ = write!(self.out, ")");
        } else {
            let _ = write!(self.out, "z.enum([");
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    let _ = write!(self.out, ", ");
                }
                emit_json_value(&mut self.out, v);
            }
            let _ = write!(self.out, "])");
        }
    }

    fn emit_instance_type(
        &mut self,
        instance_type: &SingleOrVec<InstanceType>,
        obj: &SchemaObject,
    ) {
        match instance_type {
            SingleOrVec::Single(t) => self.emit_single_type(**t, obj),
            SingleOrVec::Vec(types) => self.emit_type_vec(types, obj),
        }
    }

    fn emit_single_type(&mut self, t: InstanceType, obj: &SchemaObject) {
        match t {
            InstanceType::String => {
                let _ = write!(self.out, "z.string()");
            }
            InstanceType::Integer => {
                let _ = write!(self.out, "z.number().int()");
                self.emit_number_constraints(obj);
            }
            InstanceType::Number => {
                let _ = write!(self.out, "z.number()");
                self.emit_number_constraints(obj);
            }
            InstanceType::Boolean => {
                let _ = write!(self.out, "z.boolean()");
            }
            InstanceType::Null => {
                let _ = write!(self.out, "z.null()");
            }
            InstanceType::Object => self.emit_object(obj.object.as_deref()),
            InstanceType::Array => self.emit_array(obj),
        }
    }

    fn emit_type_vec(&mut self, types: &[InstanceType], obj: &SchemaObject) {
        let non_null: Vec<&InstanceType> =
            types.iter().filter(|t| **t != InstanceType::Null).collect();
        let has_null = types.contains(&InstanceType::Null);

        if non_null.len() == 1 && has_null {
            self.emit_single_type(*non_null[0], obj);
            let _ = write!(self.out, ".nullable()");
        } else {
            let _ = write!(self.out, "z.union([");
            for (i, t) in types.iter().enumerate() {
                if i > 0 {
                    let _ = write!(self.out, ", ");
                }
                match t {
                    InstanceType::String => {
                        let _ = write!(self.out, "z.string()");
                    }
                    InstanceType::Integer => {
                        let _ = write!(self.out, "z.number().int()");
                    }
                    InstanceType::Number => {
                        let _ = write!(self.out, "z.number()");
                    }
                    InstanceType::Boolean => {
                        let _ = write!(self.out, "z.boolean()");
                    }
                    InstanceType::Null => {
                        let _ = write!(self.out, "z.null()");
                    }
                    _ => {
                        let _ = write!(self.out, "z.any()");
                    }
                }
            }
            let _ = write!(self.out, "])");
        }
    }

    fn emit_number_constraints(&mut self, obj: &SchemaObject) {
        if let Some(ref num) = obj.number
            && let Some(min) = num.minimum
        {
            if min == 0.0 {
                let _ = write!(self.out, ".nonnegative()");
            } else {
                let _ = write!(self.out, ".min({min})");
            }
        }
    }

    fn emit_object(&mut self, object: Option<&ObjectValidation>) {
        let Some(obj_val) = object else {
            let _ = write!(self.out, "z.object({{}})");
            return;
        };

        if obj_val.properties.is_empty() {
            let _ = write!(self.out, "z.object({{}})");
        } else {
            let _ = writeln!(self.out, "z.object({{");
            self.indent += 2;
            for (name, prop_schema) in &obj_val.properties {
                let required = obj_val.required.contains(name);
                self.write_indent();
                emit_property_key(&mut self.out, name);
                let _ = write!(self.out, ": ");
                self.emit_property_schema(prop_schema, required);
                let _ = writeln!(self.out, ",");
            }
            self.indent -= 2;
            self.write_indent();
            let _ = write!(self.out, "}})");
        }

        if matches!(
            obj_val.additional_properties.as_deref(),
            Some(Schema::Bool(false))
        ) {
            let _ = write!(self.out, ".strict()");
        }
    }

    fn emit_array(&mut self, obj: &SchemaObject) {
        let Some(ref arr) = obj.array else {
            let _ = write!(self.out, "z.array(z.any())");
            return;
        };

        match &arr.items {
            Some(SingleOrVec::Single(schema)) => {
                let _ = write!(self.out, "z.array(");
                self.emit_schema(schema);
                let _ = write!(self.out, ")");
            }
            Some(SingleOrVec::Vec(schemas)) => {
                let _ = write!(self.out, "z.tuple([");
                for (i, schema) in schemas.iter().enumerate() {
                    if i > 0 {
                        let _ = write!(self.out, ", ");
                    }
                    self.emit_schema(schema);
                }
                let _ = write!(self.out, "])");
            }
            None => {
                let _ = write!(self.out, "z.array(z.any())");
            }
        }
    }

    fn emit_metadata_modifiers(&mut self, metadata: Option<&Metadata>) {
        if let Some(meta) = metadata {
            if let Some(ref default) = meta.default {
                let _ = write!(self.out, ".default(");
                emit_json_value(&mut self.out, default);
                let _ = write!(self.out, ")");
            }
            if let Some(ref desc) = meta.description {
                let _ = write!(self.out, ".describe(");
                emit_js_string(&mut self.out, desc);
                let _ = write!(self.out, ")");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Discriminated union detection
// ---------------------------------------------------------------------------

fn find_discriminator(schemas: &[Schema]) -> Option<String> {
    let mut common_key: Option<String> = None;

    for schema in schemas {
        let Schema::Object(obj) = schema else {
            return None;
        };
        let obj_val = obj.object.as_ref()?;

        let mut found_key = None;
        for (key, prop_schema) in &obj_val.properties {
            if let Schema::Object(prop_obj) = prop_schema
                && let Some(ref enum_vals) = prop_obj.enum_values
                && enum_vals.len() == 1
            {
                found_key = Some(key.clone());
                break;
            }
        }

        let key = found_key?;

        match &common_key {
            None => common_key = Some(key),
            Some(ck) if *ck == key => {}
            _ => return None,
        }
    }

    common_key
}

fn is_null_schema(schema: &Schema) -> bool {
    matches!(
        schema,
        Schema::Object(obj) if matches!(
            &obj.instance_type,
            Some(SingleOrVec::Single(t)) if **t == InstanceType::Null
        )
    )
}

// ---------------------------------------------------------------------------
// Topological sort
// ---------------------------------------------------------------------------

fn topological_sort(definitions: &Defs) -> Vec<String> {
    let mut visited = HashSet::new();
    let mut order = Vec::new();

    for name in definitions.keys() {
        topo_visit(name, definitions, &mut visited, &mut order);
    }

    order
}

fn topo_visit(name: &str, defs: &Defs, visited: &mut HashSet<String>, order: &mut Vec<String>) {
    if visited.contains(name) {
        return;
    }
    visited.insert(name.to_string());

    if let Some(schema) = defs.get(name) {
        for dep in collect_refs(schema) {
            if defs.contains_key(dep.as_str()) {
                topo_visit(&dep, defs, visited, order);
            }
        }
    }

    order.push(name.to_string());
}

fn collect_refs(schema: &Schema) -> Vec<String> {
    match schema {
        Schema::Bool(_) => vec![],
        Schema::Object(obj) => collect_refs_object(obj),
    }
}

fn collect_refs_object(obj: &SchemaObject) -> Vec<String> {
    let mut refs = vec![];

    if let Some(ref r) = obj.reference
        && let Some(name) = r.strip_prefix("#/definitions/")
    {
        refs.push(name.to_string());
    }

    if let Some(ref subs) = obj.subschemas {
        for schemas in [&subs.all_of, &subs.any_of, &subs.one_of]
            .into_iter()
            .flatten()
        {
            for s in schemas {
                refs.extend(collect_refs(s));
            }
        }
    }

    if let Some(ref obj_val) = obj.object {
        for prop_schema in obj_val.properties.values() {
            refs.extend(collect_refs(prop_schema));
        }
        if let Some(ref additional) = obj_val.additional_properties {
            refs.extend(collect_refs(additional));
        }
    }

    if let Some(ref arr_val) = obj.array
        && let Some(ref items) = arr_val.items
    {
        match items {
            SingleOrVec::Single(s) => refs.extend(collect_refs(s)),
            SingleOrVec::Vec(v) => {
                for s in v {
                    refs.extend(collect_refs(s));
                }
            }
        }
    }

    refs
}

// ---------------------------------------------------------------------------
// JS/TS formatting helpers
// ---------------------------------------------------------------------------

fn emit_property_key(out: &mut String, name: &str) {
    if is_js_identifier(name) {
        let _ = write!(out, "{name}");
    } else {
        let _ = write!(out, "\"{name}\"");
    }
}

fn is_js_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn emit_json_value(out: &mut String, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => {
            let _ = write!(out, "null");
        }
        serde_json::Value::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        serde_json::Value::Number(n) => {
            let _ = write!(out, "{n}");
        }
        serde_json::Value::String(s) => emit_js_string(out, s),
        serde_json::Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                emit_json_value(out, v);
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    let _ = write!(out, ", ");
                }
                emit_js_string(out, k);
                let _ = write!(out, ": ");
                emit_json_value(out, v);
            }
            out.push('}');
        }
    }
}

fn emit_js_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_schema;

    #[test]
    fn emit_zod_snapshot() {
        let root = config_schema();
        let output = emit_zod(&root);

        // Structural invariants
        assert!(
            output.starts_with("import { z } from \"zod\";\n"),
            "must start with zod import"
        );
        assert!(
            output.contains("export const configFileSchema ="),
            "must export configFileSchema"
        );
        assert!(
            output.contains("export type ConfigFile ="),
            "must export ConfigFile type"
        );
        assert!(
            output.contains("export function defineConfig("),
            "must export defineConfig"
        );

        // All definitions emitted
        for name in root.definitions.keys() {
            assert!(
                output.contains(&format!("const {name} =")),
                "missing definition: {name}"
            );
            assert!(
                output.contains(&format!("export type {name} =")),
                "missing type export: {name}"
            );
        }
    }
}
