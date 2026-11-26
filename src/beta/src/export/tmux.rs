//! The tmux status-bar heuristics of the video export.
//!
//! They stay in beta because they decide how beta draws pixels, not how the
//! terminal parses bytes.

use termstate::TerminalState;

#[derive(Debug, Clone)]
pub struct TmuxLayout {
    pub status_bar_row: usize,
    pub content_height: usize,
}

pub fn is_status_bar_row(state: &TerminalState, row_index: usize) -> bool {
    // Check if this row index is the tmux status bar
    if let Some(layout) = detect_tmux_layout(state) {
        row_index == layout.status_bar_row
    } else {
        false
    }
}

pub fn detect_tmux_layout(state: &TerminalState) -> Option<TmuxLayout> {
    let grid = &state.grid;
    if grid.is_empty() {
        return None;
    }

    // Check for tmux status bar - usually at the bottom
    let last_row = grid.len() - 1;
    if last_row > 0 {
        let status_row = &grid[last_row];

        // Check if this row has characteristics of a tmux status bar.
        // Only match on specific known tmux status bar colors, not any non-default color.
        let has_tmux_green_bg = status_row.iter().any(|cell| {
            matches!(
                cell.bg_color,
                (0, 255, 0) | (85, 255, 85) | (0, 128, 0) | (80, 250, 123)
            )
        });

        // Also check for full-width inverted row (white bg / black fg) with bracket chars
        let has_inverted_status = {
            let inverted_count = status_row
                .iter()
                .filter(|cell| cell.bg_color == (255, 255, 255) && cell.fg_color == (0, 0, 0))
                .count();
            let has_brackets = status_row.iter().any(|cell| matches!(cell.ch, '[' | ']'));
            inverted_count > status_row.len() / 2 && has_brackets
        };

        if has_tmux_green_bg || has_inverted_status {
            return Some(TmuxLayout {
                status_bar_row: last_row,
                content_height: last_row,
            });
        }
    }

    None
}

pub fn is_valid_cursor_position(state: &TerminalState, tmux_layout: Option<&TmuxLayout>) -> bool {
    // Check if cursor is within valid content area considering tmux layout
    let max_row = if let Some(layout) = tmux_layout {
        layout.content_height
    } else {
        state.height
    };

    state.cursor_y < max_row && state.cursor_x < state.width
}

pub fn adjust_coordinates_for_tmux(
    x: usize,
    y: usize,
    tmux_layout: Option<&TmuxLayout>,
) -> (usize, usize) {
    // Adjust coordinates to account for tmux layout
    if let Some(layout) = tmux_layout {
        // Ensure we don't render outside the content area
        let adjusted_y = if y >= layout.content_height {
            layout.content_height - 1
        } else {
            y
        };
        (x, adjusted_y)
    } else {
        (x, y)
    }
}

pub fn protect_status_bar_area(state: &mut TerminalState) {
    // Ensure status bar area is protected from application content
    if let Some(layout) = detect_tmux_layout(state) {
        let status_row = layout.status_bar_row;

        // Clear any non-status content that might have been written to the status bar
        if status_row < state.grid.len() {
            let row = &mut state.grid[status_row];

            // Check if this looks like application content rather than status bar
            let non_status_chars = row
                .iter()
                .filter(|cell| {
                    // Look for patterns that suggest this is application content
                    matches!(cell.ch, 'A'..='Z' | 'a'..='z' | '0'..='9') &&
                cell.bg_color == (0, 0, 0) && // Default background
                cell.fg_color == (255, 255, 255) // Default foreground
                })
                .count();

            // If more than 20% of the status bar looks like application content, clear it
            if non_status_chars > row.len() / 5 {
                for cell in row {
                    if cell.bg_color == (0, 0, 0) && cell.fg_color == (255, 255, 255) {
                        // Only clear cells that look like application content
                        cell.ch = ' ';
                        cell.bg_color = (0, 128, 0); // Set to status bar green
                        cell.fg_color = (0, 0, 0); // Black text
                    }
                }
            }
        }
    }
}

pub fn debug_status_bar(state: &TerminalState) {
    // Debug function to analyze status bar content
    if let Some(layout) = detect_tmux_layout(state) {
        let status_row = layout.status_bar_row;
        if status_row < state.grid.len() {
            eprintln!("Status bar analysis (row {}):", status_row);
            let row = &state.grid[status_row];

            for (i, cell) in row.iter().enumerate().take(20) {
                // Show first 20 chars
                if cell.ch != ' ' || cell.bg_color != (0, 0, 0) {
                    eprintln!(
                        "  [{}] char='{}' fg=({},{},{}) bg=({},{},{})",
                        i,
                        cell.ch,
                        cell.fg_color.0,
                        cell.fg_color.1,
                        cell.fg_color.2,
                        cell.bg_color.0,
                        cell.bg_color.1,
                        cell.bg_color.2
                    );
                }
            }
        }
    }
}
