//! Ratatui widget for rendering the step graph.

use std::collections::HashMap;

use barnum_types::StepName;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::StepGraph;
use crate::app::{StatusCounts, Viewport};
use crate::theme::TaskStatus;

/// Node dimensions.
const NODE_WIDTH: u16 = 14;
const NODE_HEIGHT: u16 = 3;
const H_SPACING: u16 = 4;
const V_SPACING: u16 = 2;

/// Widget for rendering the step graph DAG.
pub struct GraphWidget<'a> {
    graph: &'a StepGraph,
    status_counts: &'a HashMap<StepName, StatusCounts>,
    selected: Option<&'a StepName>,
    viewport: &'a Viewport,
}

impl<'a> GraphWidget<'a> {
    pub fn new(
        graph: &'a StepGraph,
        status_counts: &'a HashMap<StepName, StatusCounts>,
        selected: Option<&'a StepName>,
        viewport: &'a Viewport,
    ) -> Self {
        Self {
            graph,
            status_counts,
            selected,
            viewport,
        }
    }

    /// Compute screen position for a node given its layer and order.
    fn node_position(&self, layer: u16, order: u16) -> (u16, u16) {
        let x = layer * (NODE_WIDTH + H_SPACING);
        let y = order * (NODE_HEIGHT + V_SPACING);
        (x, y)
    }

    /// Check if a rectangle is visible in the viewport.
    fn is_visible(&self, x: u16, y: u16, width: u16, height: u16, area: Rect) -> bool {
        let scroll_x = self.viewport.scroll_x;
        let scroll_y = self.viewport.scroll_y;

        // Node must be at least partially within the visible area
        x + width > scroll_x
            && x < scroll_x + area.width
            && y + height > scroll_y
            && y < scroll_y + area.height
    }

    /// Render a single node.
    fn render_node(&self, buf: &mut Buffer, area: Rect, node_idx: usize) {
        let node = &self.graph.steps[node_idx];
        let (abs_x, abs_y) = self.node_position(node.layer, node.order);

        // Apply viewport scroll
        let x = abs_x.saturating_sub(self.viewport.scroll_x);
        let y = abs_y.saturating_sub(self.viewport.scroll_y);

        // Check visibility
        if !self.is_visible(abs_x, abs_y, NODE_WIDTH, NODE_HEIGHT, area) {
            return;
        }

        // Clamp to area bounds
        let draw_x = area.x + x;
        let draw_y = area.y + y;

        if draw_x >= area.x + area.width || draw_y >= area.y + area.height {
            return;
        }

        let is_selected = self.selected.is_some_and(|s| s == &node.name);
        let border_style = if is_selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Draw box border
        self.draw_box(buf, draw_x, draw_y, NODE_WIDTH, NODE_HEIGHT, border_style, area);

        // Draw name (centered on line 1)
        let name = node.name.as_str();
        let name_truncated: String = name.chars().take((NODE_WIDTH - 2) as usize).collect();
        let name_x = draw_x + 1;
        let name_y = draw_y + 1;

        if name_y < area.y + area.height && name_x < area.x + area.width {
            let max_width = (area.x + area.width).saturating_sub(name_x) as usize;
            let display_name: String = name_truncated.chars().take(max_width).collect();
            buf.set_string(name_x, name_y, &display_name, Style::default());
        }

        // Draw status badges on line 2 (if we have counts)
        if NODE_HEIGHT >= 3 {
            let badge_y = draw_y + 2;
            if badge_y < area.y + area.height {
                self.render_status_badges(buf, area, name_x, badge_y, &node.name);
            }
        }
    }

    /// Render status count badges for a step.
    fn render_status_badges(&self, buf: &mut Buffer, area: Rect, x: u16, y: u16, step: &StepName) {
        let counts = match self.status_counts.get(step) {
            Some(c) => c,
            None => return,
        };

        let mut offset = 0u16;
        let max_x = area.x + area.width;

        // Show non-zero counts with colored icons
        for (count, status) in [
            (counts.in_flight, TaskStatus::InFlight),
            (counts.completed, TaskStatus::Completed),
            (counts.failed, TaskStatus::Failed),
            (counts.pending, TaskStatus::Pending),
        ] {
            if count > 0 && x + offset < max_x {
                let badge = format!("{}{}", status.icon(), count);
                let style = status.style();
                let available = (max_x - x - offset) as usize;
                let display: String = badge.chars().take(available).collect();
                buf.set_string(x + offset, y, &display, style);
                offset += badge.len() as u16 + 1;
            }
        }
    }

    /// Draw a box outline.
    fn draw_box(&self, buf: &mut Buffer, x: u16, y: u16, width: u16, height: u16, style: Style, area: Rect) {
        let max_x = area.x + area.width;
        let max_y = area.y + area.height;

        // Top border
        if y < max_y && x < max_x {
            buf.set_string(x, y, "┌", style);
        }
        for dx in 1..width.saturating_sub(1) {
            if y < max_y && x + dx < max_x {
                buf.set_string(x + dx, y, "─", style);
            }
        }
        if y < max_y && x + width - 1 < max_x && width > 0 {
            buf.set_string(x + width - 1, y, "┐", style);
        }

        // Side borders
        for dy in 1..height.saturating_sub(1) {
            if y + dy < max_y {
                if x < max_x {
                    buf.set_string(x, y + dy, "│", style);
                }
                if x + width - 1 < max_x && width > 0 {
                    buf.set_string(x + width - 1, y + dy, "│", style);
                }
            }
        }

        // Bottom border
        if height > 1 && y + height - 1 < max_y {
            if x < max_x {
                buf.set_string(x, y + height - 1, "└", style);
            }
            for dx in 1..width.saturating_sub(1) {
                if x + dx < max_x {
                    buf.set_string(x + dx, y + height - 1, "─", style);
                }
            }
            if x + width - 1 < max_x && width > 0 {
                buf.set_string(x + width - 1, y + height - 1, "┘", style);
            }
        }
    }

    /// Render an edge between two nodes.
    fn render_edge(&self, buf: &mut Buffer, area: Rect, from_idx: usize, to_idx: usize) {
        let from = &self.graph.steps[from_idx];
        let to = &self.graph.steps[to_idx];

        let (from_x, from_y) = self.node_position(from.layer, from.order);
        let (to_x, to_y) = self.node_position(to.layer, to.order);

        // Edge starts at right side of from node, ends at left side of to node
        let start_x = from_x + NODE_WIDTH;
        let start_y = from_y + NODE_HEIGHT / 2;
        let end_x = to_x;
        let end_y = to_y + NODE_HEIGHT / 2;

        let edge_style = Style::default().fg(Color::DarkGray);

        // Apply viewport scroll
        let scroll_x = self.viewport.scroll_x;
        let scroll_y = self.viewport.scroll_y;

        if start_y == end_y {
            // Straight horizontal line
            for x in start_x..end_x {
                let screen_x = x.saturating_sub(scroll_x) + area.x;
                let screen_y = start_y.saturating_sub(scroll_y) + area.y;
                if screen_x < area.x + area.width && screen_y < area.y + area.height && screen_x >= area.x && screen_y >= area.y {
                    buf.set_string(screen_x, screen_y, "─", edge_style);
                }
            }
            // Arrow at end
            let arrow_x = end_x.saturating_sub(1).saturating_sub(scroll_x) + area.x;
            let arrow_y = end_y.saturating_sub(scroll_y) + area.y;
            if arrow_x < area.x + area.width && arrow_y < area.y + area.height && arrow_x >= area.x && arrow_y >= area.y {
                buf.set_string(arrow_x, arrow_y, "▶", edge_style);
            }
        } else {
            // L-shaped edge: horizontal -> vertical -> horizontal
            let mid_x = start_x + (end_x - start_x) / 2;

            // First horizontal segment
            for x in start_x..mid_x {
                let screen_x = x.saturating_sub(scroll_x) + area.x;
                let screen_y = start_y.saturating_sub(scroll_y) + area.y;
                if screen_x < area.x + area.width && screen_y < area.y + area.height && screen_x >= area.x && screen_y >= area.y {
                    buf.set_string(screen_x, screen_y, "─", edge_style);
                }
            }

            // Vertical segment
            let (min_y, max_y) = if start_y < end_y { (start_y, end_y) } else { (end_y, start_y) };
            for y in min_y..=max_y {
                let screen_x = mid_x.saturating_sub(scroll_x) + area.x;
                let screen_y = y.saturating_sub(scroll_y) + area.y;
                if screen_x < area.x + area.width && screen_y < area.y + area.height && screen_x >= area.x && screen_y >= area.y {
                    buf.set_string(screen_x, screen_y, "│", edge_style);
                }
            }

            // Second horizontal segment
            for x in mid_x + 1..end_x {
                let screen_x = x.saturating_sub(scroll_x) + area.x;
                let screen_y = end_y.saturating_sub(scroll_y) + area.y;
                if screen_x < area.x + area.width && screen_y < area.y + area.height && screen_x >= area.x && screen_y >= area.y {
                    buf.set_string(screen_x, screen_y, "─", edge_style);
                }
            }

            // Corner pieces
            let corner_x = mid_x.saturating_sub(scroll_x) + area.x;
            let corner_start_y = start_y.saturating_sub(scroll_y) + area.y;
            let corner_end_y = end_y.saturating_sub(scroll_y) + area.y;

            if corner_x < area.x + area.width && corner_x >= area.x {
                if corner_start_y < area.y + area.height && corner_start_y >= area.y {
                    let corner = if start_y < end_y { "┐" } else { "┘" };
                    buf.set_string(corner_x, corner_start_y, corner, edge_style);
                }
                if corner_end_y < area.y + area.height && corner_end_y >= area.y {
                    let corner = if start_y < end_y { "└" } else { "┌" };
                    buf.set_string(corner_x, corner_end_y, corner, edge_style);
                }
            }

            // Arrow at end
            let arrow_x = end_x.saturating_sub(1).saturating_sub(scroll_x) + area.x;
            let arrow_y = end_y.saturating_sub(scroll_y) + area.y;
            if arrow_x < area.x + area.width && arrow_y < area.y + area.height && arrow_x >= area.x && arrow_y >= area.y {
                buf.set_string(arrow_x, arrow_y, "▶", edge_style);
            }
        }
    }
}

impl Widget for GraphWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Render edges first (behind nodes)
        for &(from, to) in &self.graph.edges {
            self.render_edge(buf, area, from, to);
        }

        // Render nodes on top
        for idx in 0..self.graph.steps.len() {
            self.render_node(buf, area, idx);
        }
    }
}
