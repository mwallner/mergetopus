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
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

pub(crate) struct TerminalGuard {
    pub(crate) terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub(crate) fn new(title: &str) -> Result<Self> {
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

/// Render the branch-picker screen. Pure function of its inputs; all filtering,
/// separator-insertion, and cursor adjustment happen inside so the caller only
/// manages `filter` and `cursor` state.
pub(crate) fn render_pick_branch(
    f: &mut ratatui::Frame,
    branches: &[String],
    filter: &str,
    cursor: usize,
    current_branch: Option<&str>,
    remote_names: &[String],
) {
    let is_remote = |b: &str| -> bool {
        remote_names.iter().any(|r| {
            b.len() > r.len()
                && b.starts_with(r.as_str())
                && b.as_bytes().get(r.len()) == Some(&b'/')
        })
    };

    let filtered: Vec<&String> = branches
        .iter()
        .filter(|b| b.to_lowercase().contains(&filter.to_lowercase()))
        .collect();

    let cursor = if cursor >= filtered.len() {
        filtered.len().saturating_sub(1)
    } else {
        cursor
    };

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
}

/// Event-loop variant of [`pick_branch`] that accepts injectable event sources.
pub(crate) fn pick_branch_on_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    branches: &[String],
    current_branch: Option<&str>,
    remote_names: &[String],
    mut poll_event: impl FnMut(Duration) -> Result<bool>,
    mut read_event: impl FnMut() -> Result<Event>,
) -> Result<Option<String>>
where
    B::Error: Send + Sync + 'static,
{
    let mut filter = String::new();
    let mut cursor = 0usize;

    loop {
        terminal.draw(|f| {
            render_pick_branch(f, branches, &filter, cursor, current_branch, remote_names);
        })?;

        if !poll_event(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = read_event()? else {
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

pub fn pick_branch(
    branches: &[String],
    title: &str,
    current_branch: Option<&str>,
    remote_names: &[String],
) -> Result<Option<String>> {
    let mut guard = TerminalGuard::new(title)?;
    pick_branch_on_terminal(
        &mut guard.terminal,
        branches,
        current_branch,
        remote_names,
        |d| Ok(event::poll(d)?),
        || Ok(event::read()?),
    )
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

pub(crate) fn render_confirm(f: &mut ratatui::Frame, prompt: &str) {
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
}

/// Event-loop variant of [`confirm`] that accepts injectable event sources.
/// Useful for unit testing with [`ratatui::backend::TestBackend`].
pub(crate) fn confirm_on_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    prompt: &str,
    mut poll_event: impl FnMut(Duration) -> Result<bool>,
    mut read_event: impl FnMut() -> Result<Event>,
) -> Result<bool>
where
    B::Error: Send + Sync + 'static,
{
    loop {
        terminal.draw(|f| render_confirm(f, prompt))?;

        if !poll_event(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = read_event()? else {
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

pub fn confirm(prompt: &str, title: &str) -> Result<bool> {
    let mut guard = TerminalGuard::new(title)?;
    confirm_on_terminal(
        &mut guard.terminal,
        prompt,
        |d| Ok(event::poll(d)?),
        || Ok(event::read()?),
    )
}

/// Show a prompt with two labeled options the user can toggle with Up/Down.
/// Returns the 0-based index of the chosen option, or `None` if cancelled.
pub(crate) fn render_pick_option(
    f: &mut ratatui::Frame,
    prompt: &str,
    options: &[&str],
    cursor: usize,
) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length((options.len() as u16) + 2),
            Constraint::Length(1),
        ])
        .split(f.area());

    let prompt_area = centered_rect(70, 30, root[0]);
    let widget = Paragraph::new(prompt)
        .block(Block::default().title("Choose").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    f.render_widget(Clear, prompt_area);
    f.render_widget(widget, prompt_area);

    let items: Vec<ListItem> = options
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let prefix = if i == cursor { "> " } else { "  " };
            ListItem::new(format!("{prefix}{label}"))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(Some(cursor));
    f.render_stateful_widget(list, root[1], &mut state);

    render_keybar(
        f,
        root[2],
        &[("Up/Down", "Move"), ("Enter", "Select"), ("Esc", "Cancel")],
    );
}

/// Event-loop variant of [`pick_option`] that accepts injectable event sources.
pub(crate) fn pick_option_on_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    prompt: &str,
    options: &[&str],
    mut poll_event: impl FnMut(Duration) -> Result<bool>,
    mut read_event: impl FnMut() -> Result<Event>,
) -> Result<Option<usize>>
where
    B::Error: Send + Sync + 'static,
{
    let mut cursor = 0usize;

    loop {
        terminal.draw(|f| render_pick_option(f, prompt, options, cursor))?;

        if !poll_event(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = read_event()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Up => cursor = cursor.saturating_sub(1),
            KeyCode::Down => cursor = (cursor + 1).min(options.len().saturating_sub(1)),
            KeyCode::Enter => return Ok(Some(cursor)),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            _ => {}
        }
    }
}

pub fn pick_option(prompt: &str, options: &[&str], title: &str) -> Result<Option<usize>> {
    let mut guard = TerminalGuard::new(title)?;
    pick_option_on_terminal(
        &mut guard.terminal,
        prompt,
        options,
        |d| Ok(event::poll(d)?),
        || Ok(event::read()?),
    )
}

/// Show a scrollable list of `items` (highlighted in red) above a `prompt`,
/// and ask the user to confirm (Enter / y) or cancel (Esc / n).
///
/// The extracted rendering function also updates `scroll` and `max_scroll` for
/// use by the caller's event handler loop.
pub(crate) fn render_confirm_list(
    f: &mut ratatui::Frame,
    items: &[String],
    prompt: &str,
    scroll: &mut usize,
    max_scroll: &mut usize,
) {
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
    *max_scroll = items.len().saturating_sub(visible_rows.max(1));
    *scroll = (*scroll).min(*max_scroll);

    let s = *scroll;
    let title_str = if items.len() > visible_rows {
        format!(
            "Branches to cleanup ({}/{})",
            s + visible_rows.min(items.len().saturating_sub(s)),
            items.len()
        )
    } else {
        format!("Branches to cleanup ({})", items.len())
    };

    let list_items: Vec<ListItem> = items
        .iter()
        .skip(s)
        .take(visible_rows)
        .map(|item| ListItem::new(item.as_str()).style(Style::default().fg(Color::Red)))
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
}

/// Event-loop variant of [`confirm_list`] that accepts injectable event sources.
pub(crate) fn confirm_list_on_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    items: &[String],
    prompt: &str,
    mut poll_event: impl FnMut(Duration) -> Result<bool>,
    mut read_event: impl FnMut() -> Result<Event>,
) -> Result<bool>
where
    B::Error: Send + Sync + 'static,
{
    let mut scroll = 0usize;
    let mut max_scroll = 0usize;

    loop {
        terminal.draw(|f| {
            render_confirm_list(f, items, prompt, &mut scroll, &mut max_scroll);
        })?;

        if !poll_event(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = read_event()? else {
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

pub fn confirm_list(items: &[String], prompt: &str, title: &str) -> Result<bool> {
    let mut guard = TerminalGuard::new(title)?;
    confirm_list_on_terminal(
        &mut guard.terminal,
        items,
        prompt,
        |d| Ok(event::poll(d)?),
        || Ok(event::read()?),
    )
}

/// Render the conflict-selection screen: two-pane layout with assignment
/// markers and optional diff overlay. Overlay scroll values are mutated for
/// use by the caller's event handler.
pub(crate) fn render_select_conflicts(
    f: &mut ratatui::Frame,
    conflicts: &[String],
    slices: &[Vec<String>],
    assignments: &BTreeMap<String, usize>,
    left_cursor: usize,
    right_cursor: usize,
    focus_right: bool,
    overlay: Option<&str>,
    overlay_scroll: &mut usize,
    overlay_max_scroll: &mut usize,
    external_diff_tool: Option<&str>,
) {
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

    if let Some(content) = overlay {
        let area = centered_rect(90, 85, size);
        let visible_rows = area.height.saturating_sub(2) as usize;
        let total_rows = content.lines().count();
        *overlay_max_scroll = total_rows.saturating_sub(visible_rows.max(1));
        *overlay_scroll = (*overlay_scroll).min(*overlay_max_scroll);

        let p = Paragraph::new(content)
            .block(
                Block::default()
                    .title("3-Way Diff (Esc close, Up/Down/PgUp/PgDn/Home/End scroll)")
                    .borders(Borders::ALL),
            )
            .scroll(((*overlay_scroll).min(u16::MAX as usize) as u16, 0))
            .wrap(Wrap { trim: false });
        f.render_widget(Clear, area);
        f.render_widget(p, area);
    }
}

/// Event-loop variant of [`select_conflicts`] that accepts injectable event sources.
pub(crate) fn select_conflicts_on_terminal<B: Backend>(
    terminal: &mut Terminal<B>,
    conflicts: &[String],
    diff_provider: impl Fn(&str) -> Result<String>,
    external_diff_tool: Option<&str>,
    external_diff_runner: impl Fn(&str) -> Result<()>,
    mut poll_event: impl FnMut(Duration) -> Result<bool>,
    mut read_event: impl FnMut() -> Result<Event>,
) -> Result<Option<Vec<Vec<String>>>>
where
    B::Error: Send + Sync + 'static,
{
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

        terminal.draw(|f| {
            render_select_conflicts(
                f,
                conflicts,
                &slices,
                &assignments,
                left_cursor,
                right_cursor,
                focus_right,
                overlay.as_deref(),
                &mut overlay_scroll,
                &mut overlay_max_scroll,
                external_diff_tool,
            );
        })?;

        if !poll_event(Duration::from_millis(200))? {
            continue;
        }

        let Event::Key(key) = read_event()? else {
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

pub fn select_conflicts(
    conflicts: &[String],
    diff_provider: impl Fn(&str) -> Result<String>,
    external_diff_tool: Option<&str>,
    external_diff_runner: impl Fn(&str) -> Result<()>,
    title: &str,
) -> Result<Option<Vec<Vec<String>>>> {
    let mut guard = TerminalGuard::new(title)?;
    select_conflicts_on_terminal(
        &mut guard.terminal,
        conflicts,
        diff_provider,
        external_diff_tool,
        external_diff_runner,
        |d| Ok(event::poll(d)?),
        || Ok(event::read()?),
    )
}

pub(crate) fn render_keybar(f: &mut ratatui::Frame, area: ratatui::layout::Rect, items: &[(&str, &str)]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    /// Shortcut: `key!(Enter)` → `Event::Key(make_key(KeyCode::Enter))`
    macro_rules! key {
        (char $c:expr) => {
            Event::Key(make_key(KeyCode::Char($c)))
        };
        (F($n:expr)) => {
            Event::Key(make_key(KeyCode::F($n)))
        };
        ($code:ident) => {
            Event::Key(make_key(KeyCode::$code))
        };
    }

    /// Build a `FnMut() -> Result<Event>` from an event list.
    /// Replaces the `Cell<u64>` + `match i` boilerplate.
    macro_rules! events {
        ($($event:expr),* $(,)?) => {{
            let mut __iter = vec![$($event),*].into_iter();
            move || Ok(__iter.next().expect("events! exhausted"))
        }};
    }

    fn buffer_lines(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                let mut line = String::new();
                for x in 0..buffer.area.width {
                    if let Some(cell) = buffer.cell((x, y)) {
                        line.push_str(cell.symbol());
                    }
                }
                line
            })
            .collect()
    }

    #[test]
    fn render_keybar_shows_two_keys() {
        let backend = TestBackend::new(50, 3);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                render_keybar(
                    f,
                    Rect::new(0, 0, 50, 3),
                    &[("Up/Down", "Move"), ("Enter", "Select")],
                );
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Up/Down"), "buffer:\n{all}");
        assert!(all.contains("Move"), "buffer:\n{all}");
        assert!(all.contains("Enter"), "buffer:\n{all}");
        assert!(all.contains("Select"), "buffer:\n{all}");
    }

    #[test]
    fn render_keybar_single_key() {
        let backend = TestBackend::new(30, 3);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                render_keybar(f, Rect::new(0, 0, 30, 3), &[("Esc", "Cancel")]);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Esc"), "buffer:\n{all}");
        assert!(all.contains("Cancel"), "buffer:\n{all}");
    }

    #[test]
    fn render_confirm_shows_prompt_and_keys() {
        let backend = TestBackend::new(60, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| render_confirm(f, "Proceed with merge?"))
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Proceed with merge?"), "buffer:\n{all}");
        assert!(all.contains("Confirm"), "buffer:\n{all}");
        assert!(all.contains("Enter/Y"), "buffer:\n{all}");
        assert!(all.contains("Esc/N"), "buffer:\n{all}");
    }

    #[test]
    fn render_confirm_long_prompt_wraps() {
        let backend = TestBackend::new(40, 12);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let long = "This is a very long confirmation prompt that should wrap across multiple lines in the terminal display area";
        terminal
            .draw(|f| render_confirm(f, long))
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        // The key bar should still appear
        assert!(all.contains("Enter/Y"), "buffer:\n{all}");
    }

    // -- confirm_on_terminal state-machine tests --

    fn make_key(code: KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn confirm_enter_accepts() {
        let backend = TestBackend::new(50, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = confirm_on_terminal(
            &mut terminal,
            "Proceed?",
            |_| Ok(true),
            events![key!(Enter)],
        )
        .unwrap();
        assert!(result, "Enter should accept");
    }

    #[test]
    fn confirm_y_accepts() {
        let backend = TestBackend::new(50, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = confirm_on_terminal(
            &mut terminal,
            "Proceed?",
            |_| Ok(true),
            events![key!(char 'y')],
        )
        .unwrap();
        assert!(result, "'y' should accept");
    }

    #[test]
    fn confirm_n_rejects() {
        let backend = TestBackend::new(50, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = confirm_on_terminal(
            &mut terminal,
            "Proceed?",
            |_| Ok(true),
            events![key!(char 'n')],
        )
        .unwrap();
        assert!(!result, "'n' should reject");
    }

    #[test]
    fn confirm_esc_rejects() {
        let backend = TestBackend::new(50, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = confirm_on_terminal(
            &mut terminal,
            "Proceed?",
            |_| Ok(true),
            events![key!(Esc)],
        )
        .unwrap();
        assert!(!result, "Esc should reject");
    }

    #[test]
    fn confirm_ignores_non_key_events() {
        let backend = TestBackend::new(50, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = confirm_on_terminal(
            &mut terminal,
            "Proceed?",
            |_| Ok(true),
            events![Event::Resize(100, 50), key!(Enter)],
        )
        .unwrap();
        assert!(result, "should skip Resize then accept Enter");
    }

    #[test]
    fn confirm_ignores_non_press_key_events() {
        use crossterm::event::{KeyEventKind, KeyEventState};

        let backend = TestBackend::new(50, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = confirm_on_terminal(
            &mut terminal,
            "Proceed?",
            |_| Ok(true),
            events![
                Event::Key(crossterm::event::KeyEvent {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Release,
                    state: KeyEventState::NONE,
                }),
                key!(Enter),
            ],
        )
        .unwrap();
        assert!(result, "should skip Release then accept Press");
    }

    #[test]
    fn confirm_poll_spins_then_accepts() {
        use std::cell::Cell;

        let backend = TestBackend::new(50, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let polls = Cell::new(0u64);
        let result = confirm_on_terminal(
            &mut terminal,
            "Proceed?",
            |_| {
                let p = polls.get();
                polls.set(p + 1);
                Ok(p > 0) // first call returns false (spins), subsequent true
            },
            events![key!(Enter)],
        )
        .unwrap();

        assert!(result, "should eventually accept after spin");
        assert!(polls.get() > 1, "should have polled more than once");
    }

    #[test]
    fn render_pick_option_shows_option_and_keys() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| render_pick_option(f, "Pick one?", &["only"], 0))
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("only"), "buffer:\n{all}");
        assert!(all.contains("Choose"), "buffer:\n{all}");
        assert!(all.contains("Enter"), "buffer:\n{all}");
        assert!(all.contains("Up/Down"), "buffer:\n{all}");
    }

    #[test]
    fn render_pick_option_multiple_options() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| render_pick_option(f, "Select", &["Alpha", "Beta", "Gamma"], 1))
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Alpha"), "buffer:\n{all}");
        assert!(all.contains("Beta"), "buffer:\n{all}");
        assert!(all.contains("Gamma"), "buffer:\n{all}");
    }

    #[test]
    fn render_pick_option_cursor_at_end() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| render_pick_option(f, "Pick", &["A", "B", "C"], 2))
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("C"), "buffer:\n{all}");
        assert!(all.contains("Up/Down"), "buffer:\n{all}");
    }

    // -- pick_option_on_terminal state-machine tests --

    #[test]
    fn pick_option_enter_selects_first() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B", "C"],
            |_| Ok(true),
            events![key!(Enter)],
        )
        .unwrap();
        assert_eq!(result, Some(0), "Enter should select first option");
    }

    #[test]
    fn pick_option_esc_cancels() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B", "C"],
            |_| Ok(true),
            events![key!(Esc)],
        )
        .unwrap();
        assert_eq!(result, None, "Esc should cancel");
    }

    #[test]
    fn pick_option_q_cancels() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B", "C"],
            |_| Ok(true),
            events![key!(char 'q')],
        )
        .unwrap();
        assert_eq!(result, None, "'q' should cancel");
    }

    #[test]
    fn pick_option_down_moves_cursor() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B", "C"],
            |_| Ok(true),
            events![key!(Down), key!(Enter)],
        )
        .unwrap();
        assert_eq!(result, Some(1), "Down then Enter should select second option");
    }

    #[test]
    fn pick_option_up_at_top_stays_at_zero() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B", "C"],
            |_| Ok(true),
            events![key!(Up), key!(Enter)],
        )
        .unwrap();
        assert_eq!(result, Some(0), "Up then Enter should still select first");
    }

    #[test]
    fn pick_option_down_past_end_clamps() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B"],
            |_| Ok(true),
            events![key!(Down), key!(Down), key!(Enter)],
        )
        .unwrap();
        assert_eq!(result, Some(1), "Down twice on 2 options should select last");
    }

    #[test]
    fn pick_option_multiple_down() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B", "C", "D"],
            |_| Ok(true),
            events![key!(Down), key!(Down), key!(Down), key!(Enter)],
        )
        .unwrap();
        assert_eq!(
            result,
            Some(3),
            "Three Downs then Enter should select fourth option"
        );
    }

    #[test]
    fn pick_option_ignores_unknown_keys() {
        let backend = TestBackend::new(50, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let result = pick_option_on_terminal(
            &mut terminal,
            "Pick",
            &["A", "B"],
            |_| Ok(true),
            events![key!(char 'x'), key!(Enter)],
        )
        .unwrap();
        assert_eq!(
            result,
            Some(0),
            "unknown key ignored then Enter should select first"
        );
    }

    #[test]
    fn render_confirm_list_few_items() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let items: Vec<String> = vec!["a.txt".into(), "b.txt".into()];
        let mut scroll = 0;
        let mut max_scroll = 0;
        terminal
            .draw(|f| {
                render_confirm_list(f, &items, "Delete these?", &mut scroll, &mut max_scroll)
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("a.txt"), "buffer:\n{all}");
        assert!(all.contains("b.txt"), "buffer:\n{all}");
        assert!(all.contains("Delete these?"), "buffer:\n{all}");
    }

    #[test]
    fn render_confirm_list_many_items_shows_count() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let items: Vec<String> = (1..=20).map(|i| format!("file{i}.rs")).collect();
        let mut scroll = 0;
        let mut max_scroll = 0;
        terminal
            .draw(|f| {
                render_confirm_list(f, &items, "Go?", &mut scroll, &mut max_scroll)
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Branches to cleanup"), "buffer:\n{all}");
        assert!(all.contains("20)"), "buffer:\n{all}");
        assert!(all.contains("file1.rs"), "buffer:\n{all}");
    }

    #[test]
    fn render_confirm_list_scrolled() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let items: Vec<String> = (1..=20).map(|i| format!("file{i}.rs")).collect();
        let mut scroll = 10;
        let mut max_scroll = 0;
        terminal
            .draw(|f| {
                render_confirm_list(f, &items, "Go?", &mut scroll, &mut max_scroll)
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("file11.rs"), "buffer:\n{all}");
        assert!(all.contains("file18.rs"), "buffer:\n{all}");
    }

    // -- confirm_list_on_terminal state-machine tests --

    #[test]
    fn confirm_list_enter_accepts() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let items: Vec<String> = vec!["a.txt".into(), "b.txt".into()];
        let result = confirm_list_on_terminal(
            &mut terminal,
            &items,
            "Delete?",
            |_| Ok(true),
            events![key!(Enter)],
        )
        .unwrap();
        assert!(result, "Enter should accept");
    }

    #[test]
    fn confirm_list_y_accepts() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let items: Vec<String> = vec!["a.txt".into(), "b.txt".into()];
        let result = confirm_list_on_terminal(
            &mut terminal,
            &items,
            "Delete?",
            |_| Ok(true),
            events![key!(char 'y')],
        )
        .unwrap();
        assert!(result, "'y' should accept");
    }

    #[test]
    fn confirm_list_esc_rejects() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let items: Vec<String> = vec!["a.txt".into(), "b.txt".into()];
        let result = confirm_list_on_terminal(
            &mut terminal,
            &items,
            "Delete?",
            |_| Ok(true),
            events![key!(Esc)],
        )
        .unwrap();
        assert!(!result, "Esc should reject");
    }

    #[test]
    fn confirm_list_n_rejects() {
        let backend = TestBackend::new(60, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let items: Vec<String> = vec!["a.txt".into(), "b.txt".into()];
        let result = confirm_list_on_terminal(
            &mut terminal,
            &items,
            "Delete?",
            |_| Ok(true),
            events![key!(char 'n')],
        )
        .unwrap();
        assert!(!result, "'n' should reject");
    }

    #[test]
    fn render_pick_branch_shows_filter_and_branches() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let branches = vec!["main".into(), "feature/x".into()];
        terminal
            .draw(|f| {
                render_pick_branch(f, &branches, "", 0, None, &[]);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Filter:"), "buffer:\n{all}");
        assert!(all.contains("main"), "buffer:\n{all}");
        assert!(all.contains("feature/x"), "buffer:\n{all}");
        assert!(all.contains("Branches"), "buffer:\n{all}");
    }

    #[test]
    fn render_pick_branch_marks_current() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let branches = vec!["main".into(), "develop".into(), "feature/x".into()];
        terminal
            .draw(|f| {
                render_pick_branch(f, &branches, "", 1, Some("develop"), &[]);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        // Current branch should be marked with `>`
        assert!(all.contains("> develop"), "buffer:\n{all}");
    }

    #[test]
    fn render_pick_branch_respects_filter() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let branches = vec!["main".into(), "feature/x".into(), "fix/y".into()];
        terminal
            .draw(|f| {
                render_pick_branch(f, &branches, "fix", 0, None, &[]);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Filter: fix"), "buffer:\n{all}");
        assert!(all.contains("fix/y"), "buffer:\n{all}");
        // main and feature/x should not appear
        assert!(!all.contains("main"), "filtered out branch in:\n{all}");
        assert!(!all.contains("feature/x"), "filtered out branch in:\n{all}");
    }

    #[test]
    fn render_pick_branch_separator_local_remote() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let branches = vec!["main".into(), "origin/dev".into()];
        terminal
            .draw(|f| {
                render_pick_branch(f, &branches, "", 0, None, &["origin".into()]);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Remote"), "separator should appear:\n{all}");
    }

    // -- pick_branch_on_terminal state-machine tests --

    fn make_pick_branch_branches() -> Vec<String> {
        vec![
            "main".into(),
            "feature/foo".into(),
            "feature/bar".into(),
            "bugfix/baz".into(),
        ]
    }

    #[test]
    fn pick_branch_enter_selects_first() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(Enter)],
        )
        .unwrap();
        assert_eq!(result, Some("main".into()), "Enter should select first branch");
    }

    #[test]
    fn pick_branch_esc_cancels() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(Esc)],
        )
        .unwrap();
        assert_eq!(result, None, "Esc should cancel");
    }

    #[test]
    fn pick_branch_q_cancels() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(char 'q')],
        )
        .unwrap();
        assert_eq!(result, None, "'q' should cancel");
    }

    #[test]
    fn pick_branch_down_moves_cursor() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(Down), key!(Enter)],
        )
        .unwrap();
        assert_eq!(
            result,
            Some("feature/foo".into()),
            "Down then Enter should select second branch"
        );
    }

    #[test]
    fn pick_branch_type_filter_filters() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(char 'f'), key!(Enter)],
        )
        .unwrap();
        assert_eq!(
            result,
            Some("feature/foo".into()),
            "typing 'f' then Enter should select first matching 'feature/foo'"
        );
    }

    #[test]
    fn pick_branch_type_filter_narrows() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(char 'b'), key!(char 'u'), key!(char 'g'), key!(Enter)],
        )
        .unwrap();
        assert_eq!(
            result,
            Some("bugfix/baz".into()),
            "typing 'bug' then Enter should select 'bugfix/baz'"
        );
    }

    #[test]
    fn pick_branch_backspace_clears_filter() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        // type "x" to filter to nothing, Backspace clears, then Enter selects
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(char 'x'), key!(Backspace), key!(Enter)],
        )
        .unwrap();
        assert_eq!(
            result,
            Some("main".into()),
            "type 'x', Backspace, Enter should select first branch (filter cleared)"
        );
    }

    #[test]
    fn pick_branch_enter_with_no_match_continues() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        // type "xyz" which matches nothing, Enter does nothing, then Esc
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(char 'x'), key!(char 'y'), key!(char 'z'), key!(Enter), key!(Esc)],
        )
        .unwrap();
        assert_eq!(result, None, "Enter on empty filter should be ignored, then Esc cancels");
    }

    #[test]
    fn pick_branch_up_clamps_at_zero() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![key!(Up), key!(Enter)],
        )
        .unwrap();
        assert_eq!(
            result,
            Some("main".into()),
            "Up then Enter should still select first branch"
        );
    }

    #[test]
    fn pick_branch_ignores_ctrl_chars() {
        let backend = TestBackend::new(70, 15);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let branches = make_pick_branch_branches();
        let result = pick_branch_on_terminal(
            &mut terminal,
            &branches,
            None,
            &[],
            |_| Ok(true),
            events![
                Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Char('c'),
                    KeyModifiers::CONTROL,
                )),
                key!(Enter),
            ],
        )
        .unwrap();
        assert_eq!(
            result,
            Some("main".into()),
            "Ctrl-C ignored then Enter should select first"
        );
    }

    #[test]
    fn render_select_conflicts_empty() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut overlay_scroll = 0;
        let mut overlay_max_scroll = 0;
        terminal
            .draw(|f| {
                render_select_conflicts(
                    f,
                    &[],
                    &[],
                    &BTreeMap::new(),
                    0,
                    0,
                    false,
                    None,
                    &mut overlay_scroll,
                    &mut overlay_max_scroll,
                    None,
                );
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        // Both panes and key bar should be visible
        assert!(all.contains("Conflicted Files"), "buffer:\n{all}");
        assert!(all.contains("Explicit Slices"), "buffer:\n{all}");
        assert!(all.contains("Tab"), "buffer:\n{all}");
        assert!(all.contains("Pane"), "buffer:\n{all}");
    }

    #[test]
    fn render_select_conflicts_shows_assigned() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let conflicts = vec!["a.txt".into(), "b.txt".into()];
        let mut assignments = BTreeMap::new();
        assignments.insert("a.txt".into(), 0usize);
        let slices = vec![vec!["a.txt".into()]];

        let mut overlay_scroll = 0;
        let mut overlay_max_scroll = 0;
        terminal
            .draw(|f| {
                render_select_conflicts(
                    f,
                    &conflicts,
                    &slices,
                    &assignments,
                    0,
                    0,
                    false,
                    None,
                    &mut overlay_scroll,
                    &mut overlay_max_scroll,
                    None,
                );
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        // Assigned conflict should show [S1]
        assert!(all.contains("[S1] a.txt"), "buffer:\n{all}");
        // Unassigned should show [--]
        assert!(all.contains("[--] b.txt"), "buffer:\n{all}");
        // Slice should show count
        assert!(all.contains("Slice 1 (1 file)"), "buffer:\n{all}");
    }

    #[test]
    fn render_select_conflicts_overlay() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut overlay_scroll = 0;
        let mut overlay_max_scroll = 0;
        terminal
            .draw(|f| {
                render_select_conflicts(
                    f,
                    &["x.txt".into()],
                    &[],
                    &BTreeMap::new(),
                    0,
                    0,
                    false,
                    Some("diff content here"),
                    &mut overlay_scroll,
                    &mut overlay_max_scroll,
                    None,
                );
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        // Overlay key bar items
        assert!(all.contains("diff content here"), "buffer:\n{all}");
        assert!(all.contains("3-Way Diff"), "buffer:\n{all}");
    }

    #[test]
    fn render_select_conflicts_overlay_scroll_computed() {
        let backend = TestBackend::new(80, 25);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        // 100 lines of content — guaranteed longer than visible area
        let content: String = (0..100)
            .map(|i| format!("line {i}\n"))
            .collect();

        let mut overlay_scroll = 0;
        let mut overlay_max_scroll = 0;
        terminal
            .draw(|f| {
                render_select_conflicts(
                    f,
                    &[],
                    &[],
                    &BTreeMap::new(),
                    0,
                    0,
                    false,
                    Some(&content),
                    &mut overlay_scroll,
                    &mut overlay_max_scroll,
                    None,
                );
            })
            .unwrap();

        assert!(
            overlay_max_scroll > 0,
            "max_scroll should be > 0 for long content, got {overlay_max_scroll}"
        );
        assert_eq!(
            overlay_scroll, 0,
            "initial scroll should stay 0 when within bounds"
        );
    }

    #[test]
    fn render_select_conflicts_overlay_scroll_clamped() {
        let backend = TestBackend::new(80, 25);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let content: String = (0..100)
            .map(|i| format!("line {i}\n"))
            .collect();

        let mut overlay_scroll = 9999;
        let mut overlay_max_scroll = 0;
        terminal
            .draw(|f| {
                render_select_conflicts(
                    f,
                    &[],
                    &[],
                    &BTreeMap::new(),
                    0,
                    0,
                    false,
                    Some(&content),
                    &mut overlay_scroll,
                    &mut overlay_max_scroll,
                    None,
                );
            })
            .unwrap();

        assert!(
            overlay_max_scroll > 0,
            "max_scroll should be > 0 for long content"
        );
        assert_eq!(
            overlay_scroll, overlay_max_scroll,
            "scroll should be clamped to max_scroll, got {overlay_scroll} vs {overlay_max_scroll}"
        );
    }

    // -- select_conflicts_on_terminal state-machine tests --

    fn make_conflicts() -> Vec<String> {
        vec!["a.txt".into(), "b.txt".into(), "c.txt".into()]
    }

    #[test]
    fn select_conflicts_esc_cancels() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(Esc)],
        )
        .unwrap();
        assert_eq!(result, None, "Esc should cancel");
    }

    #[test]
    fn select_conflicts_q_cancels() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(char 'q')],
        )
        .unwrap();
        assert_eq!(result, None, "'q' should cancel");
    }

    #[test]
    fn select_conflicts_enter_empty_slices() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(Enter)],
        )
        .unwrap();
        assert_eq!(result, Some(vec![]), "Enter with no slices should return empty vec");
    }

    #[test]
    fn select_conflicts_space_assigns_first_conflict() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(char ' '), key!(Enter)],
        )
        .unwrap();
        let expected = Some(vec![vec!["a.txt".into()]]);
        assert_eq!(result, expected, "Space on first conflict should assign to slice 0");
    }

    #[test]
    fn select_conflicts_space_on_right_pane_does_nothing() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(Tab), key!(char ' '), key!(Tab), key!(char ' '), key!(Enter)],
        )
        .unwrap();
        let expected = Some(vec![vec!["a.txt".into()]]);
        assert_eq!(
            result, expected,
            "Tab right, Space (no-op), Tab left, Space should assign"
        );
    }

    #[test]
    fn select_conflicts_u_unassigns() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(char ' '), key!(char 'u'), key!(Enter)],
        )
        .unwrap();
        let expected = Some(vec![]);
        assert_eq!(
            result, expected,
            "Space then 'u' should leave empty slices"
        );
    }

    #[test]
    fn select_conflicts_u_does_nothing_on_unassigned() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(char 'u'), key!(Enter)],
        )
        .unwrap();
        assert_eq!(result, Some(vec![]), "'u' on unassigned should be no-op");
    }

    #[test]
    fn select_conflicts_n_creates_new_slice() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![
                key!(char ' '), // assign a.txt to 0
                key!(Down),     // -> b.txt
                key!(char ' '), // assign b.txt to 0
                key!(char 'n'), // new slice 1, focus right
                key!(Tab),      // focus left
                key!(Down),     // -> c.txt
                key!(char ' '), // assign c.txt to slice 1
                key!(Enter),
            ],
        )
        .unwrap();
        let expected = Some(vec![
            vec!["a.txt".into(), "b.txt".into()],
            vec!["c.txt".into()],
        ]);
        assert_eq!(result, expected, "Two slices with 'a+b' and 'c'");
    }

    #[test]
    fn select_conflicts_d_drops_slice() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![
                key!(char ' '), // assign a.txt to 0
                key!(char 'n'), // new slice 1
                key!(char 'd'), // drop slice 1
                key!(Enter),
            ],
        )
        .unwrap();
        let expected = Some(vec![vec!["a.txt".into()]]);
        assert_eq!(result, expected, "Space, 'n', 'd', Enter should leave one slice with 'a.txt'");
    }

    #[test]
    fn select_conflicts_tab_switches_focus() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![
                key!(char ' '), // assign a.txt
                key!(Tab),      // focus right
                key!(Tab),      // focus left
                key!(Down),     // -> b.txt
                key!(char ' '), // assign b.txt
                key!(Enter),
            ],
        )
        .unwrap();
        let expected = Some(vec![vec!["a.txt".into(), "b.txt".into()]]);
        assert_eq!(result, expected, "Tab to right and back should work");
    }

    #[test]
    fn select_conflicts_down_moves_cursor() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(Down), key!(char ' '), key!(Enter)],
        )
        .unwrap();
        let expected = Some(vec![vec!["b.txt".into()]]);
        assert_eq!(result, expected, "Down then Space should assign second conflict");
    }

    #[test]
    fn select_conflicts_up_moves_cursor() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("diff".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(Down), key!(Down), key!(Up), key!(char ' '), key!(Enter)],
        )
        .unwrap();
        let expected = Some(vec![vec!["b.txt".into()]]);
        assert_eq!(
            result, expected,
            "Down twice, Up, Space should assign second conflict"
        );
    }

    #[test]
    fn select_conflicts_f3_shows_and_esc_closes_overlay() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| Ok("DIFF CONTENT".into()),
            None,
            |_| Ok(()),
            |_| Ok(true),
            events![key!(F(3)), key!(Esc), key!(Enter)],
        )
        .unwrap();
        let expected = Some(vec![]);
        assert_eq!(
            result, expected,
            "F3 then Esc then Enter should return empty slices"
        );
    }

    #[test]
    fn select_conflicts_f3_with_external_tool_calls_runner() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let conflicts = make_conflicts();
        let called = std::cell::Cell::new(false);
        // F3 with external_diff_tool set — should call runner, not show overlay
        let result = select_conflicts_on_terminal(
            &mut terminal,
            &conflicts,
            |_| panic!("should not be called"),
            Some("tool"),
            |path| {
                called.set(true);
                assert_eq!(path, "a.txt");
                Ok(())
            },
            |_| Ok(true),
            events![key!(F(3)), key!(Enter)],
        )
        .unwrap();
        assert!(called.get(), "external_diff_runner should have been called");
        assert_eq!(result, Some(vec![]), "empty slices after external tool");
    }

    #[test]
    fn render_keybar_many_keys() {
        let backend = TestBackend::new(80, 3);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                render_keybar(
                    f,
                    Rect::new(0, 0, 80, 3),
                    &[
                        ("a", "Alpha"),
                        ("b", "Beta"),
                        ("c", "Gamma"),
                        ("d", "Delta"),
                        ("e", "Epsilon"),
                    ],
                );
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("Alpha"), "buffer:\n{all}");
        assert!(all.contains("Epsilon"), "buffer:\n{all}");
    }
}
