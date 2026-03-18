//! Layout algorithms: layer assignment and barycenter ordering.

use std::collections::{HashMap, HashSet, VecDeque};

use super::StepGraph;

/// Assign layers using longest-path algorithm via Kahn's topological sort.
/// Source nodes (in-degree 0) get layer 0. Each child gets max(parent_layers) + 1.
pub fn assign_layers(graph: &mut StepGraph) {
    if graph.steps.is_empty() {
        return;
    }

    // Compute in-degree for each node
    let mut in_degree: Vec<usize> = vec![0; graph.steps.len()];
    for &(_, to) in &graph.edges {
        in_degree[to] += 1;
    }

    // Build adjacency list for forward traversal
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); graph.steps.len()];
    for &(from, to) in &graph.edges {
        children[from].push(to);
    }

    // Initialize source nodes (in-degree 0) with layer 0
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (idx, &deg) in in_degree.iter().enumerate() {
        if deg == 0 {
            graph.steps[idx].layer = 0;
            queue.push_back(idx);
        }
    }

    // Process in topological order
    let mut processed_in_degree = in_degree.clone();
    while let Some(node_idx) = queue.pop_front() {
        let node_layer = graph.steps[node_idx].layer;

        for &child_idx in &children[node_idx] {
            // Child's layer is at least parent's layer + 1
            let new_layer = node_layer + 1;
            if new_layer > graph.steps[child_idx].layer {
                graph.steps[child_idx].layer = new_layer;
            }

            // Decrement in-degree and enqueue if all parents processed
            processed_in_degree[child_idx] -= 1;
            if processed_in_degree[child_idx] == 0 {
                queue.push_back(child_idx);
            }
        }
    }
}

/// Order nodes within layers using barycenter heuristic.
/// 4 passes alternating forward/backward to minimize edge crossings.
pub fn order_within_layers(graph: &mut StepGraph) {
    if graph.steps.is_empty() {
        return;
    }

    // Build parent/child maps
    let mut parents: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();

    for &(from, to) in &graph.edges {
        children.entry(from).or_default().push(to);
        parents.entry(to).or_default().push(from);
    }

    // Get nodes grouped by layer
    let max_layer = graph.steps.iter().map(|n| n.layer).max().unwrap_or(0) as usize;
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); max_layer + 1];
    for (idx, node) in graph.steps.iter().enumerate() {
        layers[node.layer as usize].push(idx);
    }

    // Initialize order within each layer
    for (order, layer) in layers.iter().enumerate() {
        for (pos, &node_idx) in layer.iter().enumerate() {
            graph.steps[node_idx].order = pos as u16;
            let _ = order; // silence unused warning
        }
    }

    // 4 passes: forward, backward, forward, backward
    for pass in 0..4 {
        if pass % 2 == 0 {
            // Forward pass: order by average parent position
            for layer_idx in 1..=max_layer {
                order_layer_by_neighbors(graph, &layers[layer_idx], &parents, &layers[layer_idx - 1]);
                update_layer_order(graph, &layers[layer_idx]);
            }
        } else {
            // Backward pass: order by average child position
            for layer_idx in (0..max_layer).rev() {
                order_layer_by_neighbors(graph, &layers[layer_idx], &children, &layers[layer_idx + 1]);
                update_layer_order(graph, &layers[layer_idx]);
            }
        }

        // Rebuild layers with new order
        for layer in &mut layers {
            layer.sort_by_key(|&idx| graph.steps[idx].order);
        }
    }
}

/// Order a layer's nodes by the average position of their neighbors in the reference layer.
fn order_layer_by_neighbors(
    graph: &mut StepGraph,
    layer: &[usize],
    neighbor_map: &HashMap<usize, Vec<usize>>,
    _ref_layer: &[usize],
) {
    // Compute barycenter for each node
    let mut barycenters: Vec<(usize, f64)> = Vec::with_capacity(layer.len());

    for &node_idx in layer {
        let neighbors = neighbor_map.get(&node_idx);
        let barycenter = match neighbors {
            Some(ns) if !ns.is_empty() => {
                let sum: u32 = ns.iter().map(|&n| u32::from(graph.steps[n].order)).sum();
                f64::from(sum) / ns.len() as f64
            }
            _ => f64::from(graph.steps[node_idx].order), // Keep current position
        };
        barycenters.push((node_idx, barycenter));
    }

    // Sort by barycenter
    barycenters.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Assign new order
    for (new_order, &(node_idx, _)) in barycenters.iter().enumerate() {
        graph.steps[node_idx].order = new_order as u16;
    }
}

/// Update order values after sorting.
fn update_layer_order(graph: &mut StepGraph, layer: &[usize]) {
    let mut sorted: Vec<usize> = layer.to_vec();
    sorted.sort_by_key(|&idx| graph.steps[idx].order);

    for (new_order, &node_idx) in sorted.iter().enumerate() {
        graph.steps[node_idx].order = new_order as u16;
    }
}
