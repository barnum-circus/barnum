//! Step graph: DAG construction from config.

mod layout;
mod render;

pub use render::GraphWidget;

use std::collections::HashMap;

use barnum_config::ConfigFile;
use barnum_types::StepName;

/// A node in the step graph.
#[derive(Debug, Clone)]
pub struct StepNode {
    pub name: StepName,
    pub next: Vec<StepName>,
    pub layer: u16,
    pub order: u16,
}

/// DAG representation of workflow steps.
#[derive(Debug)]
pub struct StepGraph {
    pub steps: Vec<StepNode>,
    pub edges: Vec<(usize, usize)>,
    pub index_by_name: HashMap<StepName, usize>,
}

impl StepGraph {
    /// Build a step graph from a config file.
    pub fn from_config(config: &ConfigFile) -> Self {
        let mut steps = Vec::with_capacity(config.steps.len());
        let mut index_by_name = HashMap::with_capacity(config.steps.len());

        // Build nodes
        for (i, step) in config.steps.iter().enumerate() {
            index_by_name.insert(step.name.clone(), i);
            steps.push(StepNode {
                name: step.name.clone(),
                next: step.next.clone(),
                layer: 0,
                order: 0,
            });
        }

        // Build edges
        let mut edges = Vec::new();
        for (from_idx, node) in steps.iter().enumerate() {
            for next_name in &node.next {
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

        // Compute layout
        layout::assign_layers(&mut graph);
        layout::order_within_layers(&mut graph);

        graph
    }

    /// Get a step node by name.
    pub fn get(&self, name: &StepName) -> Option<&StepNode> {
        self.index_by_name.get(name).map(|&i| &self.steps[i])
    }

    /// Get steps grouped by layer, sorted by order within each layer.
    pub fn layers(&self) -> Vec<Vec<usize>> {
        if self.steps.is_empty() {
            return Vec::new();
        }

        let max_layer = self.steps.iter().map(|n| n.layer).max().unwrap_or(0);
        let mut layers: Vec<Vec<usize>> = vec![Vec::new(); (max_layer + 1) as usize];

        for (idx, node) in self.steps.iter().enumerate() {
            layers[node.layer as usize].push(idx);
        }

        // Sort each layer by order
        for layer in &mut layers {
            layer.sort_by_key(|&idx| self.steps[idx].order);
        }

        layers
    }

    /// Total number of steps.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}
