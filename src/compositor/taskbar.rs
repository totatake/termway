use super::window::ManagedWindow;

#[derive(Debug, Clone)]
pub struct TaskbarChip {
    pub window_id: usize,
    pub start_col: u16,
    pub end_col: u16,
}

#[derive(Debug, Clone, Default)]
pub struct Taskbar {
    pub chips: Vec<TaskbarChip>,
}

impl Taskbar {
    pub fn new() -> Self {
        Self {
            chips: Vec::new(),
        }
    }

    /// Builds the bottom row taskbar chip string and records chip click bounds
    pub fn build(
        &mut self,
        windows: &[ManagedWindow],
        enable_wm: bool,
        backend_name: &str,
        term_cols: u16,
    ) -> Option<String> {
        if !enable_wm || windows.is_empty() {
            self.chips.clear();
            return None;
        }

        let overlay_len = backend_name.len() + 30;
        let overlay_col = term_cols
            .saturating_sub(overlay_len as u16)
            .saturating_add(1)
            .max(1);
        let max_cols = overlay_col.saturating_sub(2);
        if max_cols < 6 {
            self.chips.clear();
            return None;
        }

        let mut chips = Vec::new();
        let mut out = String::new();
        let mut cur_col: u16 = 2;

        let mut sorted_windows = windows.to_vec();
        sorted_windows.sort_by_key(|w| w.id);

        for win in &sorted_windows {
            let raw_title = if win.title.trim().is_empty() {
                "Window"
            } else {
                win.title.trim()
            };
            let short_title: String = raw_title.chars().take(8).collect();
            let state_suffix = if win.is_minimized { " _" } else { "" };
            let chip_label = format!("[{}: {}{}]", win.id, short_title, state_suffix);
            let chip_len = chip_label.chars().count() as u16;

            if cur_col + chip_len > max_cols {
                break;
            }

            // Colors:
            // Minimized: cyan bg (\x1b[46;30m)
            // Active: blue bg (\x1b[44;37;1m)
            // Inactive: dark gray bg (\x1b[100;37m)
            let color_code = if win.is_minimized {
                "\x1b[46;30m"
            } else if win.active {
                "\x1b[44;37;1m"
            } else {
                "\x1b[100;37m"
            };

            out.push_str(&format!(" {}{}\x1b[0m", color_code, chip_label));

            chips.push(TaskbarChip {
                window_id: win.id,
                start_col: cur_col,
                end_col: cur_col + chip_len - 1,
            });

            cur_col += chip_len + 1;
        }

        self.chips = chips;
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Finds the window ID of the taskbar chip clicked at `col`, if any.
    pub fn find_chip_at_col(&self, col: u16) -> Option<usize> {
        self.chips
            .iter()
            .find(|chip| col >= chip.start_col && col <= chip.end_col)
            .map(|chip| chip.window_id)
    }
}
