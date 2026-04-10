use std::collections::BTreeMap;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new(title: &str) -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        execute!(stdout, SetTitle(title))?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

pub fn pick_branch(
    branches: &[String],
    title: &str,
    current_branch: Option<&str>,
    remote_names: &[String],
) -> Result<Option<String>> {
    let is_remote = |b: &str| -> bool {
        remote_names.iter().any(|r| {
            b.len() > r.len()
                && b.starts_with(r.as_str())
                && b.as_bytes().get(r.len()) == Some(&b'/')
        })
    };

    let mut guard = TerminalGuard::new(title)?;

    let mut filter = String::new();
    let mut cursor = 0usize;

    loop {
        let filtered = branches
            .iter()
            .filter(|b| b.to_lowercase().contains(&filter.to_lowercase()))
            .cloned()
            .collect::<Vec<_>>();

        if cursor >= filtered.len() {
            cursor = filtered.len().saturating_sub(1);
        }

        // Build display items, inserting a separator at the local → remote boundary.
        let mut items: Vec<ListItem> = Vec::new();
        let mut branch_to_display: Vec<usize> = Vec::new();
        let mut has_local = false;
        let mut separator_inserted = false;

        for b in &filtered {
            if is_remote(b) && !separator_inserted && has_local {
                items.push(ListItem::new(Line::from(Span::styled(
                    "── Remote ──",
                    Style::default().fg(Color::DarkGray),
                ))));
                separator_inserted = true;
            }
            if !is_remote(b) {
                has_local = true;
            }

            branch_to_display.push(items.len());
            let is_current = current_branch.is_some_and(|cb| cb == b.as_str());
            if is_current {
                items.push(ListItem::new(format!("> {b}")));
            } else {
                items.push(ListItem::new(format!("  {b}")));
            }
        }

        let display_cursor = branch_to_display.get(cursor).copied().unwrap_or(0);

        guard.terminal.draw(|f| {
            let size = f.area();
            let block = Block::default()
                .title("Select Merge Source Branch")
                .borders(Borders::ALL);
            f.render_widget(block, size);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(1),
                ])
                .margin(1)
                .split(size);

            let filter_line = Paragraph::new(format!("Filter: {filter}"))
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(filter_line, chunks[0]);

            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("Branches"))
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );

            let mut state = ListState::default();
            if !filtered.is_empty() {
                state.select(Some(display_cursor));
            }
            f.render_stateful_widget(list, chunks[1], &mut state);

            render_keybar(
                f,
                chunks[2],
                &[
                    ("Up/Down", "Move"),
                    ("Enter", "Select"),
                    ("Esc", "Cancel"),
                    ("q", "Quit"),
                ],
            );
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Enter => {
                let selected = filtered_get(branches, &filter, cursor);
                if let Some(s) = selected {
                    return Ok(Some(s));
                }
            }
            KeyCode::Up => cursor = cursor.saturating_sub(1),
            KeyCode::Down => {
                let len = branches_filtered_len(branches, &filter);
                if len > 0 {
                    cursor = (cursor + 1).min(len - 1);
                }
            }
            KeyCode::Backspace => {
                filter.pop();
                cursor = 0;
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    filter.push(c);
                    cursor = 0;
                }
            }
            _ => {}
        }
    }
}

fn branches_filtered_len(branches: &[String], filter: &str) -> usize {
    branches
        .iter()
        .filter(|b| b.to_lowercase().contains(&filter.to_lowercase()))
        .count()
}

fn filtered_get(branches: &[String], filter: &str, index: usize) -> Option<String> {
    branches
        .iter()
        .filter(|b| b.to_lowercase().contains(&filter.to_lowercase()))
        .nth(index)
        .cloned()
}

pub fn confirm(prompt: &str, title: &str) -> Result<bool> {
    let mut guard = TerminalGuard::new(title)?;

    loop {
        guard.terminal.draw(|f| {
            let root = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(1)])
                .split(f.area());
            let area = centered_rect(70, 30, root[0]);
            let widget = Paragraph::new(prompt)
                .block(Block::default().title("Confirm").borders(Borders::ALL))
                .wrap(Wrap { trim: true });
            f.render_widget(Clear, area);
            f.render_widget(widget, area);

            render_keybar(f, root[1], &[("Enter/Y", "Yes"), ("Esc/N", "No")]);
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => return Ok(true),
            KeyCode::Char('n') | KeyCode::Esc => return Ok(false),
            _ => {}
        }
    }
}

/// Show a scrollable list of `items` (highlighted in red) above a `prompt`,
/// and ask the user to confirm (Enter / y) or cancel (Esc / n).
pub fn confirm_list(items: &[String], prompt: &str, title: &str) -> Result<bool> {
    let mut guard = TerminalGuard::new(title)?;
    let mut scroll = 0usize;
    let mut max_scroll = 0usize;

    loop {
        guard.terminal.draw(|f| {
            let size = f.area();
            let root = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(5),
                    Constraint::Length(3),
                    Constraint::Length(1),
                ])
                .split(size);

            let visible_rows = root[0].height.saturating_sub(2) as usize;
            max_scroll = items.len().saturating_sub(visible_rows.max(1));
            scroll = scroll.min(max_scroll);

            let title_str = if items.len() > visible_rows {
                format!(
                    "Branches to cleanup ({}/{})",
                    scroll + visible_rows.min(items.len() - scroll),
                    items.len()
                )
            } else {
                format!("Branches to cleanup ({})", items.len())
            };

            let list_items: Vec<ListItem> = items
                .iter()
                .skip(scroll)
                .take(visible_rows)
                .map(|s| ListItem::new(s.as_str()).style(Style::default().fg(Color::Red)))
                .collect();

            let list = List::new(list_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title_str.as_str()),
            );
            f.render_widget(list, root[0]);

            f.render_widget(
                Paragraph::new(prompt)
                    .block(Block::default().borders(Borders::ALL))
                    .wrap(Wrap { trim: true }),
                root[1],
            );

            render_keybar(
                f,
                root[2],
                &[
                    ("Up/Down", "Scroll"),
                    ("Enter/Y", "Delete"),
                    ("Esc/N", "Cancel"),
                ],
            );
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => return Ok(true),
            KeyCode::Char('n') | KeyCode::Esc => return Ok(false),
            KeyCode::Up => scroll = scroll.saturating_sub(1),
            KeyCode::Down => scroll = (scroll + 1).min(max_scroll),
            _ => {}
        }
    }
}

pub fn select_conflicts(
    conflicts: &[String],
    diff_provider: impl Fn(&str) -> Result<String>,
    external_diff_tool: Option<&str>,
    external_diff_runner: impl Fn(&str) -> Result<()>,
    title: &str,
) -> Result<Option<Vec<Vec<String>>>> {
    let mut guard = TerminalGuard::new(title)?;

    let mut assignments: BTreeMap<String, usize> = BTreeMap::new();
    let mut slices: Vec<Vec<String>> = Vec::new();
    let mut left_cursor = 0usize;
    let mut right_cursor = 0usize;
    let mut focus_right = false;

    let mut overlay: Option<String> = None;
    let mut overlay_scroll = 0usize;
    let mut overlay_max_scroll = 0usize;

    loop {
        if left_cursor >= conflicts.len() {
            left_cursor = conflicts.len().saturating_sub(1);
        }
        if right_cursor >= slices.len() {
            right_cursor = slices.len().saturating_sub(1);
        }

        guard.terminal.draw(|f| {
            let size = f.area();
            let root = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(10), Constraint::Length(1)])
                .split(size);

            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(root[0]);

            let left_items = conflicts
                .iter()
                .map(|path| {
                    let mark = assignments
                        .get(path)
                        .map(|slice_idx| format!("[S{}]", slice_idx + 1))
                        .unwrap_or_else(|| "[--]".to_string());
                    ListItem::new(Line::from(vec![Span::raw(format!("{mark} {path}"))]))
                })
                .collect::<Vec<_>>();
            let left = List::new(left_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Conflicted Files"),
                )
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(if focus_right {
                            Color::DarkGray
                        } else {
                            Color::Cyan
                        })
                        .add_modifier(Modifier::BOLD),
                );

            let mut left_state = ListState::default();
            if !conflicts.is_empty() {
                left_state.select(Some(left_cursor));
            }
            f.render_stateful_widget(left, panes[0], &mut left_state);

            let right_items = slices
                .iter()
                .enumerate()
                .map(|(idx, group)| {
                    let title = format!(
                        "Slice {} ({} file{})",
                        idx + 1,
                        group.len(),
                        if group.len() == 1 { "" } else { "s" }
                    );
                    let preview = if group.is_empty() {
                        "".to_string()
                    } else {
                        format!(" : {}", group.join(", "))
                    };
                    ListItem::new(format!("{title}{preview}"))
                })
                .collect::<Vec<_>>();
            let right = List::new(right_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Explicit Slices"),
                )
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(if focus_right {
                            Color::Cyan
                        } else {
                            Color::DarkGray
                        })
                        .add_modifier(Modifier::BOLD),
                );

            let mut right_state = ListState::default();
            if !slices.is_empty() {
                right_state.select(Some(right_cursor));
            }
            f.render_stateful_widget(right, panes[1], &mut right_state);

            let key_items = if overlay.is_some() {
                vec![
                    ("Up/Down", "Scroll"),
                    ("PgUp/PgDn", "Fast"),
                    ("Home/End", "Top/Bottom"),
                    ("Esc", "Close"),
                ]
            } else {
                let f3_action = external_diff_tool.unwrap_or("3-way");
                vec![
                    ("Tab", "Pane"),
                    ("n", "NewSlice"),
                    ("Space", "Assign"),
                    ("u", "Unassign"),
                    ("d", "DropSlice"),
                    ("F3", f3_action),
                    ("Enter", "Apply"),
                    ("Esc", "Cancel"),
                ]
            };
            render_keybar(f, root[1], &key_items);

            if let Some(content) = &overlay {
                let area = centered_rect(90, 85, size);
                let visible_rows = area.height.saturating_sub(2) as usize;
                let total_rows = content.lines().count();
                overlay_max_scroll = total_rows.saturating_sub(visible_rows.max(1));
                overlay_scroll = overlay_scroll.min(overlay_max_scroll);

                let p = Paragraph::new(content.as_str())
                    .block(
                        Block::default()
                            .title("3-Way Diff (Esc close, Up/Down/PgUp/PgDn/Home/End scroll)")
                            .borders(Borders::ALL),
                    )
                    .scroll((overlay_scroll as u16, 0))
                    .wrap(Wrap { trim: false });
                f.render_widget(Clear, area);
                f.render_widget(p, area);
            }
        })?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        if overlay.is_some() {
            match key.code {
                KeyCode::Esc => {
                    overlay = None;
                    overlay_scroll = 0;
                    overlay_max_scroll = 0;
                }
                KeyCode::Up => overlay_scroll = overlay_scroll.saturating_sub(1),
                KeyCode::Down => overlay_scroll = (overlay_scroll + 1).min(overlay_max_scroll),
                KeyCode::PageUp => overlay_scroll = overlay_scroll.saturating_sub(20),
                KeyCode::PageDown => overlay_scroll = (overlay_scroll + 20).min(overlay_max_scroll),
                KeyCode::Home => overlay_scroll = 0,
                KeyCode::End => overlay_scroll = overlay_max_scroll,
                _ => {}
            }
            continue;
        }

        match key.code {
            KeyCode::Char('q') => return Ok(None),
            KeyCode::Esc => return Ok(None),
            KeyCode::Tab => {
                focus_right = !focus_right;
            }
            KeyCode::Up => {
                if focus_right {
                    right_cursor = right_cursor.saturating_sub(1);
                } else {
                    left_cursor = left_cursor.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                let len = if focus_right {
                    slices.len()
                } else {
                    conflicts.len()
                };
                if len > 0 {
                    if focus_right {
                        right_cursor = (right_cursor + 1).min(len - 1);
                    } else {
                        left_cursor = (left_cursor + 1).min(len - 1);
                    }
                }
            }
            KeyCode::Char('n') => {
                slices.push(Vec::new());
                right_cursor = slices.len() - 1;
                focus_right = true;
            }
            KeyCode::Char(' ') => {
                if !focus_right {
                    if slices.is_empty() {
                        slices.push(Vec::new());
                        right_cursor = 0;
                    }

                    if let Some(path) = conflicts.get(left_cursor) {
                        if let Some(old_idx) = assignments.get(path).copied() {
                            if let Some(old) = slices.get_mut(old_idx) {
                                old.retain(|p| p != path);
                                old.sort();
                            }
                        }

                        if let Some(target) = slices.get_mut(right_cursor) {
                            if !target.iter().any(|p| p == path) {
                                target.push(path.clone());
                                target.sort();
                            }
                            assignments.insert(path.clone(), right_cursor);
                        }
                    }
                }
            }
            KeyCode::Char('u') => {
                if let Some(path) = conflicts.get(left_cursor) {
                    if let Some(old_idx) = assignments.remove(path) {
                        if let Some(old) = slices.get_mut(old_idx) {
                            old.retain(|p| p != path);
                            old.sort();
                        }
                    }
                }
            }
            KeyCode::Char('d') => {
                if !slices.is_empty() && right_cursor < slices.len() {
                    for path in &slices[right_cursor] {
                        assignments.remove(path);
                    }
                    slices.remove(right_cursor);

                    let mut normalized: BTreeMap<String, usize> = BTreeMap::new();
                    for (idx, group) in slices.iter_mut().enumerate() {
                        group.sort();
                        group.dedup();
                        for path in group.iter() {
                            normalized.insert(path.clone(), idx);
                        }
                    }
                    assignments = normalized;

                    if right_cursor >= slices.len() {
                        right_cursor = slices.len().saturating_sub(1);
                    }
                }
            }
            KeyCode::F(3) => {
                if let Some(path) = conflicts.get(left_cursor) {
                    if external_diff_tool.is_some() {
                        external_diff_runner(path)?;
                    } else {
                        overlay = Some(diff_provider(path)?);
                        overlay_scroll = 0;
                        overlay_max_scroll = 0;
                    }
                }
            }
            KeyCode::Enter => {
                let mut out = slices
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>();
                for group in &mut out {
                    group.sort();
                    group.dedup();
                }
                return Ok(Some(out));
            }
            _ => {}
        }
    }
}

fn render_keybar(f: &mut ratatui::Frame, area: ratatui::layout::Rect, items: &[(&str, &str)]) {
    let mut spans = Vec::new();
    for (idx, (key, action)) in items.iter().enumerate() {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {action} "),
            Style::default().fg(Color::White).bg(Color::DarkGray),
        ));

        if idx + 1 < items.len() {
            spans.push(Span::styled(" ", Style::default().bg(Color::Blue)));
        }
    }

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Blue));
    f.render_widget(bar, area);
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
