//! DAG construction from workflow config.
//!
//! Builds a [`StepGraph`] from a [`ConfigFile`], laying out steps in layers
//! for rendering as a left-to-right DAG.

pub mod layout;
pub mod render;

use std::collections::HashMap;

use barnum_config::ConfigFile;
use barnum_types::StepName;

/// A step in the workflow DAG, positioned for rendering.
#[derive(Debug, Clone)]
pub struct StepNode {
    pub name: StepName,
    pub next: Vec<StepName>,
    pub layer: u16,
    pub order: u16,
}

/// Directed acyclic graph of workflow steps, ready for layout and rendering.
#[derive(Debug)]
pub struct StepGraph {
    pub steps: Vec<StepNode>,
    pub edges: Vec<(usize, usize)>,
    pub index_by_name: HashMap<StepName, usize>,
}

impl StepGraph {
    /// Build a `StepGraph` from a workflow config.
    ///
    /// Steps become nodes, `next` references become directed edges.
    /// Layers are assigned via longest-path topological sort, then
    /// nodes within each layer are ordered to minimize edge crossings.
    pub fn from_config(config: &ConfigFile) -> Self {
        let mut steps: Vec<StepNode> = Vec::with_capacity(config.steps.len());
        let mut index_by_name: HashMap<StepName, usize> = HashMap::with_capacity(config.steps.len());

        for (idx, step_file) in config.steps.iter().enumerate() {
            index_by_name.insert(step_file.name.clone(), idx);
            steps.push(StepNode {
                name: step_file.name.clone(),
                next: step_file.next.clone(),
                layer: 0,
                order: 0,
            });
        }

        // Build edges from each step's `next` field.
        let mut edges = Vec::new();
        for (from_idx, step_file) in config.steps.iter().enumerate() {
            for next_name in &step_file.next {
                if let Some(&to_idx) = index_by_name.get(next_name) {
                    edges.push((from_idx, to_idx));
                }
            }
        }

        let mut graph = Self {
            steps,
            edges,
            index_by_name,
        };

        layout::assign_layers(&mut graph);
        layout::order_within_layers(&mut graph);

        graph
    }

    /// Look up a step node by name.
    pub fn get(&self, name: &StepName) -> Option<&StepNode> {
        self.index_by_name.get(name).map(|&idx| &self.steps[idx])
    }

    /// Group step indices by layer, sorted by layer number.
    ///
    /// Returns `layers[layer_idx] = [step_idx, ...]` with nodes sorted
    /// by their `order` within each layer.
    pub fn layers(&self) -> Vec<Vec<usize>> {
        let Some(max_layer) = self.steps.iter().map(|s| s.layer).max() else {
            return Vec::new();
        };
        let mut layers: Vec<Vec<usize>> = vec![Vec::new(); (max_layer as usize) + 1];

        for (idx, step) in self.steps.iter().enumerate() {
            layers[step.layer as usize].push(idx);
        }

        // Sort each layer by the node's order field.
        for layer in &mut layers {
            layer.sort_by_key(|&idx| self.steps[idx].order);
        }

        layers
    }

    /// Total number of steps in the graph.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;

    fn step_json(name: &str, next: &[&str]) -> String {
        let next_json: Vec<String> = next.iter().map(|n| format!("\"{n}\"")).collect();
        format!(
            r#"{{"name": "{name}", "action": {{"kind": "Pool", "instructions": {{"inline": ""}}}}, "next": [{next}]}}"#,
            next = next_json.join(", ")
        )
    }

    fn make_config(steps: &[(&str, &[&str])]) -> ConfigFile {
        let steps_json: Vec<String> = steps
            .iter()
            .map(|(name, next)| step_json(name, next))
            .collect();
        let json = format!(r#"{{"steps": [{}]}}"#, steps_json.join(", "));
        serde_json::from_str(&json).expect("test config should parse")
    }

    #[test]
    fn linear_chain_layers() {
        let config = make_config(&[("A", &["B"]), ("B", &["C"]), ("C", &[])]);
        let graph = StepGraph::from_config(&config);

        assert_eq!(graph.step_count(), 3);
        assert_eq!(graph.get(&StepName::new("A")).map(|n| n.layer), Some(0));
        assert_eq!(graph.get(&StepName::new("B")).map(|n| n.layer), Some(1));
        assert_eq!(graph.get(&StepName::new("C")).map(|n| n.layer), Some(2));
    }

    #[test]
    fn diamond_graph() {
        // A -> B, A -> C, B -> D, C -> D
        let config = make_config(&[
            ("A", &["B", "C"]),
            ("B", &["D"]),
            ("C", &["D"]),
            ("D", &[]),
        ]);
        let graph = StepGraph::from_config(&config);

        assert_eq!(graph.get(&StepName::new("A")).map(|n| n.layer), Some(0));
        assert_eq!(graph.get(&StepName::new("D")).map(|n| n.layer), Some(2));
        assert_eq!(graph.get(&StepName::new("B")).map(|n| n.layer), Some(1));
        assert_eq!(graph.get(&StepName::new("C")).map(|n| n.layer), Some(1));
    }

    #[test]
    fn empty_config() {
        let config = make_config(&[]);
        let graph = StepGraph::from_config(&config);

        assert_eq!(graph.step_count(), 0);
        assert!(graph.layers().is_empty());
    }

    #[test]
    fn single_step() {
        let config = make_config(&[("Solo", &[])]);
        let graph = StepGraph::from_config(&config);

        assert_eq!(graph.step_count(), 1);
        assert_eq!(graph.layers().len(), 1);
        assert_eq!(graph.layers()[0], vec![0]);
    }

    #[test]
    fn self_loop_does_not_block_layering() {
        // Mirrors refactor-workflow: ProcessRefactorList references itself
        // and also feeds into CommitFile.
        let config = make_config(&[
            ("List", &["Analyze"]),
            ("Analyze", &["Process"]),
            ("Process", &["Process", "Commit"]),
            ("Commit", &[]),
        ]);
        let graph = StepGraph::from_config(&config);

        assert_eq!(graph.get(&StepName::new("List")).map(|n| n.layer), Some(0));
        assert_eq!(graph.get(&StepName::new("Analyze")).map(|n| n.layer), Some(1));
        assert_eq!(graph.get(&StepName::new("Process")).map(|n| n.layer), Some(2));
        assert_eq!(graph.get(&StepName::new("Commit")).map(|n| n.layer), Some(3));
    }
}
