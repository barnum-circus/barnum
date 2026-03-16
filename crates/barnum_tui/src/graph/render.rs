//! Widget for rendering the step DAG in a terminal.
//!
//! [`GraphWidget`] implements [`ratatui::widgets::Widget`], drawing nodes as
//! 14x3 boxes with status badges and edges using Unicode box-drawing characters.

use std::collections::HashMap;

use barnum_types::StepName;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use super::StepGraph;
use crate::app::{StatusCounts, Viewport};
use crate::theme;

/// Node box dimensions.
const NODE_WIDTH: u16 = 14;
const NODE_HEIGHT: u16 = 3;

/// Horizontal gap between layers.
const LAYER_GAP_X: u16 = 4;
/// Vertical gap between nodes in the same layer.
const NODE_GAP_Y: u16 = 2;

/// Edge drawing characters.
const EDGE_HORIZONTAL: &str = "\u{2500}"; // ─
const EDGE_VERTICAL: &str = "\u{2502}";   // │
const EDGE_ARROW: &str = "\u{25B6}";      // ▶
const EDGE_CORNER_DOWN: &str = "\u{256D}"; // ╭
const EDGE_CORNER_UP: &str = "\u{2570}";   // ╰

/// Renderable DAG widget.
///
/// Borrows all state it needs; stateless between frames.
pub struct GraphWidget<'a> {
    pub graph: &'a StepGraph,
    pub step_counts: &'a HashMap<StepName, StatusCounts>,
    pub selected: Option<&'a StepName>,
    pub viewport: &'a Viewport,
}

impl<'a> GraphWidget<'a> {
    /// Compute the (x, y) position for a node given its layer and order.
    fn node_position(&self, layer: u16, order: u16) -> (i32, i32) {
        let x = i32::from(layer) * i32::from(NODE_WIDTH + LAYER_GAP_X);
        let y = i32::from(order) * i32::from(NODE_HEIGHT + NODE_GAP_Y);
        (x, y)
    }

    /// Apply viewport offset to get screen coordinates.
    fn to_screen(&self, world_x: i32, world_y: i32) -> (i32, i32) {
        (world_x - self.viewport.scroll_x, world_y - self.viewport.scroll_y)
    }

    /// Render a single node box into the buffer.
    fn render_node(
        &self,
        buf: &mut Buffer,
        area: Rect,
        node_idx: usize,
    ) {
        let node = &self.graph.steps[node_idx];
        let (world_x, world_y) = self.node_position(node.layer, node.order);
        let (sx, sy) = self.to_screen(world_x, world_y);

        // Clip: skip if entirely outside the render area.
        let ax = i32::from(area.x);
        let ay = i32::from(area.y);
        let aw = i32::from(area.width);
        let ah = i32::from(area.height);

        if sx + i32::from(NODE_WIDTH) <= ax
            || sy + i32::from(NODE_HEIGHT) <= ay
            || sx >= ax + aw
            || sy >= ay + ah
        {
            return;
        }

        let is_selected = self.selected.is_some_and(|s| *s == node.name);
        let border_style = if is_selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        // Draw the 14x3 box.
        // Row 0: top border    ┌────────────┐
        // Row 1: name (center) │  StepName  │
        // Row 2: bottom border └────────────┘
        for row in 0..NODE_HEIGHT {
            for col in 0..NODE_WIDTH {
                let px = sx + i32::from(col);
                let py = sy + i32::from(row);

                if px < ax || py < ay || px >= ax + aw || py >= ay + ah {
                    continue;
                }

                let cell = buf.cell_mut((px as u16, py as u16));
                let Some(cell) = cell else { continue };

                let ch = match (row, col) {
                    // Corners
                    (0, 0) => "\u{250C}",                          // ┌
                    (0, c) if c == NODE_WIDTH - 1 => "\u{2510}",   // ┐
                    (2, 0) => "\u{2514}",                          // └
                    (2, c) if c == NODE_WIDTH - 1 => "\u{2518}",   // ┘
                    // Horizontal borders
                    (0 | 2, _) => "\u{2500}",                      // ─
                    // Vertical borders
                    (1, 0) => "\u{2502}",                          // │
                    (1, c) if c == NODE_WIDTH - 1 => "\u{2502}",   // │
                    // Interior
                    _ => " ",
                };

                cell.set_symbol(ch);
                cell.set_style(border_style);
            }
        }

        // Write the step name centered in row 1.
        let name = node.name.as_str();
        let max_name_len = (NODE_WIDTH - 2) as usize; // minus borders
        let display_name = if name.len() > max_name_len {
            &name[..max_name_len]
        } else {
            name
        };
        let name_offset = ((max_name_len - display_name.len()) / 2) as i32 + 1;

        let name_style = if is_selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        for (i, ch) in display_name.chars().enumerate() {
            let px = sx + name_offset + i as i32;
            let py = sy + 1;
            if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                    cell.set_char(ch);
                    cell.set_style(name_style);
                }
            }
        }

        // Render status badges below the box (row 3) if we have counts.
        if let Some(counts) = self.step_counts.get(&node.name) {
            let badge_y = sy + i32::from(NODE_HEIGHT);
            if badge_y >= ay && badge_y < ay + ah {
                self.render_status_badge(buf, area, sx + 1, badge_y, counts);
            }
        }
    }

    /// Render compact status badges: colored count numbers.
    fn render_status_badge(
        &self,
        buf: &mut Buffer,
        area: Rect,
        sx: i32,
        sy: i32,
        counts: &StatusCounts,
    ) {
        let ax = i32::from(area.x);
        let aw = i32::from(area.width);

        let badges: Vec<(u32, Color, &str)> = vec![
            (counts.completed, theme::COLOR_COMPLETED, theme::ICON_COMPLETED),
            (counts.in_flight, theme::COLOR_IN_FLIGHT, theme::ICON_IN_FLIGHT),
            (counts.pending, theme::COLOR_PENDING, theme::ICON_PENDING),
            (counts.failed, theme::COLOR_FAILED, theme::ICON_FAILED),
            (counts.retried, theme::COLOR_RETRIED, theme::ICON_RETRIED),
        ];

        let mut x = sx;
        for (count, color, icon) in badges {
            if count == 0 {
                continue;
            }
            let text = format!("{icon}{count}");
            let style = Style::default().fg(color);

            for ch in text.chars() {
                if x >= ax && x < ax + aw {
                    if let Some(cell) = buf.cell_mut((x as u16, sy as u16)) {
                        cell.set_char(ch);
                        cell.set_style(style);
                    }
                }
                x += 1;
            }
            x += 1; // space between badges
        }
    }

    /// Render a single edge between two nodes.
    fn render_edge(
        &self,
        buf: &mut Buffer,
        area: Rect,
        from_idx: usize,
        to_idx: usize,
    ) {
        let from = &self.graph.steps[from_idx];
        let to = &self.graph.steps[to_idx];

        let (from_x, from_y) = self.node_position(from.layer, from.order);
        let (to_x, to_y) = self.node_position(to.layer, to.order);

        // Edge starts at right side of source, ends at left side of target.
        let start_x = from_x + i32::from(NODE_WIDTH);
        let start_y = from_y + 1; // middle row
        let end_x = to_x - 1;    // one cell before target left border
        let end_y = to_y + 1;     // middle row

        let ax = i32::from(area.x);
        let ay = i32::from(area.y);
        let aw = i32::from(area.width);
        let ah = i32::from(area.height);

        let edge_style = Style::default().fg(Color::DarkGray);

        if start_y == end_y {
            // Straight horizontal edge.
            for x in start_x..=end_x {
                let (px, py) = self.to_screen(x, start_y);
                if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                    if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                        cell.set_symbol(EDGE_HORIZONTAL);
                        cell.set_style(edge_style);
                    }
                }
            }
            // Arrow at the end.
            let (px, py) = self.to_screen(end_x + 1, end_y);
            if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                    cell.set_symbol(EDGE_ARROW);
                    cell.set_style(edge_style);
                }
            }
        } else {
            // L-shaped edge: horizontal from source, vertical, horizontal to target.
            let mid_x = start_x + (end_x - start_x) / 2;
            let going_down = end_y > start_y;

            // Horizontal segment from source to mid_x.
            for x in start_x..=mid_x {
                let (px, py) = self.to_screen(x, start_y);
                if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                    if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                        cell.set_symbol(EDGE_HORIZONTAL);
                        cell.set_style(edge_style);
                    }
                }
            }

            // Corner at turn from horizontal to vertical.
            {
                let corner = if going_down { EDGE_CORNER_DOWN } else { EDGE_CORNER_UP };
                let (px, py) = self.to_screen(mid_x, start_y);
                if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                    if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                        cell.set_symbol(corner);
                        cell.set_style(edge_style);
                    }
                }
            }

            // Vertical segment.
            let (y_min, y_max) = if going_down {
                (start_y + 1, end_y - 1)
            } else {
                (end_y + 1, start_y - 1)
            };
            for y in y_min..=y_max {
                let (px, py) = self.to_screen(mid_x, y);
                if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                    if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                        cell.set_symbol(EDGE_VERTICAL);
                        cell.set_style(edge_style);
                    }
                }
            }

            // Corner at turn from vertical to horizontal.
            let corner2_symbol = if going_down { EDGE_CORNER_UP } else { EDGE_CORNER_DOWN };
            let (px, py) = self.to_screen(mid_x, end_y);
            if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                    cell.set_symbol(corner2_symbol);
                    cell.set_style(edge_style);
                }
            }

            // Horizontal segment from mid_x to target.
            for x in (mid_x + 1)..=end_x {
                let (px, py) = self.to_screen(x, end_y);
                if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                    if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                        cell.set_symbol(EDGE_HORIZONTAL);
                        cell.set_style(edge_style);
                    }
                }
            }

            // Arrow at the end.
            let (px, py) = self.to_screen(end_x + 1, end_y);
            if px >= ax && py >= ay && px < ax + aw && py < ay + ah {
                if let Some(cell) = buf.cell_mut((px as u16, py as u16)) {
                    cell.set_symbol(EDGE_ARROW);
                    cell.set_style(edge_style);
                }
            }
        }
    }
}

impl Widget for GraphWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Render edges first so nodes paint on top.
        for &(from, to) in &self.graph.edges {
            self.render_edge(buf, area, from, to);
        }

        // Render nodes on top.
        for idx in 0..self.graph.steps.len() {
            self.render_node(buf, area, idx);
        }
    }
}
