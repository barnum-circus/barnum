//! DAG layout algorithms for step graph positioning.
//!
//! - [`assign_layers`]: Longest-path layering via Kahn's topological sort.
//! - [`order_within_layers`]: Barycenter heuristic to reduce edge crossings.

use super::StepGraph;

/// Assign each node a layer using longest-path layering.
///
/// Sources (nodes with no incoming edges) start at layer 0.
/// Each child's layer = max(parent layers) + 1.
///
/// Uses Kahn's algorithm for topological ordering, computing longest
/// path distances as we go.
pub fn assign_layers(graph: &mut StepGraph) {
    let n = graph.steps.len();
    if n == 0 {
        return;
    }

    // Compute in-degree for each node, ignoring self-loops (they don't
    // affect topological order and would prevent the node from ever
    // reaching in-degree 0).
    let mut in_degree = vec![0u32; n];
    for &(from, to) in &graph.edges {
        if from != to {
            in_degree[to] += 1;
        }
    }

    // Build adjacency list (forward edges, excluding self-loops).
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(from, to) in &graph.edges {
        if from != to {
            adjacency[from].push(to);
        }
    }

    // Initialize: sources get layer 0, seed the queue.
    let mut layer = vec![0u16; n];
    let mut queue: Vec<usize> = Vec::new();
    for (idx, &deg) in in_degree.iter().enumerate() {
        if deg == 0 {
            queue.push(idx);
        }
    }

    // Process in topological order, propagating longest path.
    let mut head = 0;
    while head < queue.len() {
        let node = queue[head];
        head += 1;

        for &child in &adjacency[node] {
            let candidate = layer[node] + 1;
            if candidate > layer[child] {
                layer[child] = candidate;
            }
            in_degree[child] -= 1;
            if in_degree[child] == 0 {
                queue.push(child);
            }
        }
    }

    // Write layers back to nodes.
    for (idx, &l) in layer.iter().enumerate() {
        graph.steps[idx].layer = l;
    }
}

/// Order nodes within each layer to minimize edge crossings.
///
/// Uses the barycenter heuristic: for each node, compute the average
/// position of its connected nodes in the adjacent layer, then sort
/// by that value. Runs 4 passes, alternating forward and backward.
pub fn order_within_layers(graph: &mut StepGraph) {
    let n = graph.steps.len();
    if n == 0 {
        return;
    }

    let layers = graph.layers();
    let layer_count = layers.len();
    if layer_count <= 1 {
        // Single layer or empty: just assign order = position.
        for (i, step) in graph.steps.iter_mut().enumerate() {
            step.order = layers.get(step.layer as usize)
                .and_then(|l| l.iter().position(|&idx| idx == i))
                .unwrap_or(0) as u16;
        }
        return;
    }

    // Build adjacency lists in both directions for barycenter lookups.
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut backward: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(from, to) in &graph.edges {
        forward[from].push(to);
        backward[to].push(from);
    }

    // Working order: position of each node within its layer.
    let mut position = vec![0u16; n];
    for layer in &layers {
        for (pos, &idx) in layer.iter().enumerate() {
            position[idx] = pos as u16;
        }
    }

    // 4 passes alternating forward/backward.
    for pass in 0..4 {
        if pass % 2 == 0 {
            // Forward pass: for each layer (left to right), order nodes
            // by barycenter of their predecessors.
            for layer_idx in 1..layer_count {
                order_layer_by_barycenter(
                    &layers[layer_idx],
                    &backward,
                    &mut position,
                );
            }
        } else {
            // Backward pass: for each layer (right to left), order nodes
            // by barycenter of their successors.
            for layer_idx in (0..layer_count - 1).rev() {
                order_layer_by_barycenter(
                    &layers[layer_idx],
                    &forward,
                    &mut position,
                );
            }
        }
    }

    // Write final ordering back.
    for (idx, &pos) in position.iter().enumerate() {
        graph.steps[idx].order = pos;
    }
}

/// Reorder a single layer's nodes by the barycenter of their neighbors
/// in the adjacent layer.
///
/// `layer_nodes` - indices of nodes in the layer being reordered.
/// `neighbors` - adjacency list (forward or backward depending on direction).
/// `positions` - current position of every node; read for neighbor lookup,
///   updated in-place for this layer's nodes.
fn order_layer_by_barycenter(
    layer_nodes: &[usize],
    neighbors: &[Vec<usize>],
    positions: &mut [u16],
) {
    if layer_nodes.is_empty() {
        return;
    }

    // Compute barycenter for each node using current positions (read-only snapshot).
    let mut barycenters: Vec<(usize, f64)> = layer_nodes
        .iter()
        .map(|&idx| {
            let nbrs = &neighbors[idx];
            if nbrs.is_empty() {
                // No neighbors: keep current position as tiebreaker.
                (idx, f64::from(positions[idx]))
            } else {
                let sum: f64 = nbrs.iter().map(|&n| f64::from(positions[n])).sum();
                (idx, sum / nbrs.len() as f64)
            }
        })
        .collect();

    // Sort by barycenter, stable to preserve existing order on ties.
    barycenters.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Assign new positions.
    for (pos, &(idx, _)) in barycenters.iter().enumerate() {
        positions[idx] = pos as u16;
    }
}
