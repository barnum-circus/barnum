//! Markdown documentation generation for agents.
//!
//! Generates instructions that tell agents what they can do at each step.

use crate::resolved::{Config, Step};
use std::fmt::Write;

/// Generate a complete markdown document describing all steps.
#[must_use]
pub fn generate_full_docs(config: &Config) -> String {
    let mut doc = String::new();

    writeln!(doc, "# Barnum Task Queue Documentation").ok();
    writeln!(doc).ok();

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
        write_step(&mut doc, step);
    }

    doc
}

/// Write documentation for a single step.
fn write_step(doc: &mut String, step: &Step) {
    let name = &step.name;
    writeln!(doc, "### {name}").ok();
    writeln!(doc).ok();

    if step.next.is_empty() {
        writeln!(doc, "**Terminal step** - no further transitions.").ok();
    } else {
        let next = step.next.join(", ");
        writeln!(doc, "**Next steps**: {next}").ok();
    }
    writeln!(doc).ok();
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::config::ConfigFile;
    use std::path::Path;

    #[test]
    fn generates_full_docs() {
        let config_file: ConfigFile = serde_json::from_str(
            r#"{
            "steps": [
                {"name": "Start", "action": {"kind": "Command", "params": {"script": "echo '[]'"}}, "next": ["End"]},
                {"name": "End", "action": {"kind": "Command", "params": {"script": "echo '[]'"}}, "next": []}
            ]
        }"#,
        )
        .unwrap();

        let config = config_file.resolve(Path::new("."));
        let docs = generate_full_docs(&config);
        assert!(docs.contains("Barnum Task Queue Documentation"));
        assert!(docs.contains("State Diagram"));
        assert!(docs.contains("Start -> End"));
        assert!(docs.contains("End (terminal)"));
    }
}
