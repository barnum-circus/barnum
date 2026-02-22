//! Markdown documentation generation for agents.
//!
//! Generates instructions that tell agents what they can do at each step.

use crate::config::{Action, Config, Instructions, SchemaRef, Step};
use std::fmt::Write;

/// Write instructions to a doc string, handling both inline and linked variants.
fn write_instructions(doc: &mut String, action: &Action) {
    let Some(instructions) = action.instructions() else {
        return;
    };
    match instructions {
        Instructions::Inline(text) if !text.is_empty() => {
            writeln!(doc, "{text}").ok();
            writeln!(doc).ok();
        }
        Instructions::Link { link } => {
            writeln!(doc, "*See instructions in `{link}`*").ok();
            writeln!(doc).ok();
        }
        Instructions::Inline(_) => {}
    }
}

/// Generate markdown documentation for a specific step.
///
/// This creates instructions for an agent about what responses are valid.
#[must_use]
pub fn generate_step_docs(step: &Step, config: &Config) -> String {
    let mut doc = String::new();

    // Header with step name
    let name = &step.name;
    writeln!(doc, "# Current Step: {name}").ok();
    writeln!(doc).ok();

    // Step instructions
    write_instructions(&mut doc, &step.action);

    // Valid responses section
    if step.next.is_empty() {
        writeln!(doc, "## Terminal Step").ok();
        writeln!(doc).ok();
        writeln!(doc, "This is a terminal step. Return an empty array: `[]`").ok();
    } else {
        writeln!(doc, "## Valid Responses").ok();
        writeln!(doc).ok();
        writeln!(
            doc,
            "You must return a JSON array of tasks. Each task has `kind` and `value` fields."
        )
        .ok();
        writeln!(doc).ok();
        writeln!(doc, "Valid next steps:").ok();
        writeln!(doc).ok();

        for next_name in &step.next {
            if let Some(next_step) = config.steps.iter().find(|s| &s.name == next_name) {
                writeln!(doc, "### {next_name}").ok();
                writeln!(doc).ok();

                // Show schema info
                match &next_step.value_schema {
                    None => {
                        writeln!(doc, "Accepts any JSON value.").ok();
                        writeln!(doc).ok();
                        writeln!(doc, "```json").ok();
                        writeln!(doc, r#"{{"kind": "{next_name}", "value": <any>}}"#).ok();
                        writeln!(doc, "```").ok();
                    }
                    Some(SchemaRef::Inline(schema)) => {
                        writeln!(doc, "Value must match schema:").ok();
                        writeln!(doc).ok();
                        writeln!(doc, "```json").ok();
                        if let Ok(pretty) = serde_json::to_string_pretty(schema) {
                            writeln!(doc, "{pretty}").ok();
                        }
                        writeln!(doc, "```").ok();
                        writeln!(doc).ok();
                        writeln!(doc, "Example:").ok();
                        writeln!(doc, "```json").ok();
                        writeln!(doc, r#"{{"kind": "{next_name}", "value": {{...}}}}"#).ok();
                        writeln!(doc, "```").ok();
                    }
                    Some(SchemaRef::Link(path)) => {
                        writeln!(doc, "Value must match schema in `{path}`.").ok();
                        writeln!(doc).ok();
                        writeln!(doc, "```json").ok();
                        writeln!(doc, r#"{{"kind": "{next_name}", "value": {{...}}}}"#).ok();
                        writeln!(doc, "```").ok();
                    }
                }
                writeln!(doc).ok();
            }
        }
    }

    doc
}

/// Generate a complete markdown document describing all steps.
#[must_use]
pub fn generate_full_docs(config: &Config) -> String {
    let mut doc = String::new();

    writeln!(doc, "# GSD Task Queue Documentation").ok();
    writeln!(doc).ok();

    // Options summary
    if config.options.timeout.is_some() || config.options.max_retries > 0 {
        writeln!(doc, "## Options").ok();
        writeln!(doc).ok();
        if let Some(timeout) = config.options.timeout {
            writeln!(doc, "- **Timeout**: {timeout} seconds").ok();
        }
        let max_retries = config.options.max_retries;
        if max_retries > 0 {
            writeln!(doc, "- **Max Retries**: {max_retries}").ok();
        }
        writeln!(doc).ok();
    }

    // State diagram (simple text representation)
    writeln!(doc, "## State Diagram").ok();
    writeln!(doc).ok();
    writeln!(doc, "```").ok();
    for step in &config.steps {
        let name = &step.name;
        if step.next.is_empty() {
            writeln!(doc, "{name} (terminal)").ok();
        } else {
            let next = step.next.join(", ");
            writeln!(doc, "{name} -> {next}").ok();
        }
    }
    writeln!(doc, "```").ok();
    writeln!(doc).ok();

    // Detailed step documentation
    writeln!(doc, "## Steps").ok();
    writeln!(doc).ok();

    for step in &config.steps {
        let name = &step.name;
        writeln!(doc, "### {name}").ok();
        writeln!(doc).ok();

        write_instructions(&mut doc, &step.action);

        if step.next.is_empty() {
            writeln!(doc, "**Terminal step** - no further transitions.").ok();
        } else {
            let next = step.next.join(", ");
            writeln!(doc, "**Next steps**: {next}").ok();
        }
        writeln!(doc).ok();
    }

    doc
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn generates_step_docs() {
        let config: Config = serde_json::from_str(
            r#"{
            "steps": [
                {"name": "Start", "action": {"kind": "Pool", "instructions": "Begin here."}, "next": ["End"]},
                {"name": "End", "next": []}
            ]
        }"#,
        )
        .unwrap();

        let docs = generate_step_docs(&config.steps[0], &config);
        assert!(docs.contains("Current Step: Start"));
        assert!(docs.contains("Begin here."));
        assert!(docs.contains("### End"));
    }

    #[test]
    fn generates_terminal_step_docs() {
        let config: Config = serde_json::from_str(
            r#"{
            "steps": [
                {"name": "End", "next": []}
            ]
        }"#,
        )
        .unwrap();

        let docs = generate_step_docs(&config.steps[0], &config);
        assert!(docs.contains("Terminal Step"));
        assert!(docs.contains("empty array"));
    }

    #[test]
    fn generates_full_docs() {
        let config: Config = serde_json::from_str(
            r#"{
            "options": {"timeout": 60, "max_retries": 2},
            "steps": [
                {"name": "Start", "action": {"kind": "Pool", "instructions": "Begin."}, "next": ["End"]},
                {"name": "End", "next": []}
            ]
        }"#,
        )
        .unwrap();

        let docs = generate_full_docs(&config);
        assert!(docs.contains("GSD Task Queue Documentation"));
        assert!(docs.contains("Timeout"));
        assert!(docs.contains("60"));
        assert!(docs.contains("Max Retries"));
        assert!(docs.contains("State Diagram"));
        assert!(docs.contains("Start -> End"));
        assert!(docs.contains("End (terminal)"));
    }
}
