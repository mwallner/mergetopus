use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::TerminalGuard;

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

#[derive(Clone, Copy, PartialEq)]
enum StepStatus {
    Pending,
    Running,
    Done,
    Error,
}

struct StepState {
    label: String,
    status: StepStatus,
}

pub struct ProgressStep {
    pub label: String,
    pub action: Box<dyn FnOnce() -> Result<()> + Send + 'static>,
}

/// Run a sequence of blocking operations with a TUI progress overlay.
///
/// Each step is executed sequentially. While a step runs, an animated spinner
/// is displayed next to its label. When all steps complete (or one fails), the
/// screen stays visible until the user presses Enter, Esc, or Space.
pub fn run_progress(title: &str, steps: Vec<ProgressStep>) -> Result<()> {
    let mut guard = TerminalGuard::new(title)?;
    run_progress_on_terminal(
        &mut guard.terminal,
        title,
        steps,
        |d| Ok(event::poll(d)?),
        || Ok(event::read()?),
    )
}

/// Core progress logic — generic over Backend + event source for testability.
pub(crate) fn run_progress_on_terminal<B: Backend>(
    terminal: &mut ratatui::Terminal<B>,
    title: &str,
    steps: Vec<ProgressStep>,
    mut poll_event: impl FnMut(Duration) -> Result<bool>,
    mut read_event: impl FnMut() -> Result<Event>,
) -> Result<()>
where
    <B as Backend>::Error: Send + Sync + 'static,
{
    let total = steps.len();
    if total == 0 {
        return Ok(());
    }

    let states = Arc::new(Mutex::new(
        steps
            .iter()
            .map(|s| StepState {
                label: s.label.clone(),
                status: StepStatus::Pending,
            })
            .collect::<Vec<_>>(),
    ));

    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut error_msg = String::new();
    let mut cancelled = false;

    for (i, step) in steps.into_iter().enumerate() {
        {
            let mut s = states.lock().unwrap();
            s[i].status = StepStatus::Running;
        }

        let step_result: Arc<Mutex<Option<Result<()>>>> = Arc::new(Mutex::new(None));
        let step_result_clone = step_result.clone();

        let join_handle = thread::spawn(move || {
            let result = (step.action)();
            *step_result_clone.lock().unwrap() = Some(result);
        });

        let mut frame = 0u64;
        loop {
            terminal.draw(|f| {
                let states = states.lock().unwrap();
                render_progress(
                    f, &states, frame, title, completed, total, failed, &error_msg, cancelled,
                );
            })?;

            {
                let guard = step_result.lock().unwrap();
                if guard.is_some() {
                    break;
                }
            }

            if poll_event(Duration::from_millis(80))? {
                if let Event::Key(key) = read_event()? {
                    if key.kind == KeyEventKind::Press && key.code == KeyCode::Esc {
                        cancelled = true;
                        break;
                    }
                }
            }

            frame = frame.wrapping_add(1);
        }

        let step_result_value = match join_handle.join() {
            Ok(()) => step_result.lock().unwrap().take(),
            Err(panic_err) => {
                let msg = panic_err
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic_err.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "worker thread panicked".to_string());
                Some(Err(anyhow::anyhow!("{}", msg)))
            }
        };

        match step_result_value {
            Some(Ok(())) => {
                let mut s = states.lock().unwrap();
                s[i].status = StepStatus::Done;
                completed += 1;
            }
            Some(Err(e)) => {
                let mut s = states.lock().unwrap();
                s[i].status = StepStatus::Error;
                error_msg = format!("{:#}", e);
                failed += 1;
            }
            None => {
                let mut s = states.lock().unwrap();
                s[i].status = StepStatus::Error;
                error_msg = "step did not produce a result".to_string();
                failed += 1;
            }
        }

        if cancelled || failed > 0 {
            break;
        }
    }

    // Final state — wait for dismissal
    loop {
        terminal.draw(|f| {
            let states = states.lock().unwrap();
            render_progress(
                f, &states, 0, title, completed, total, failed, &error_msg, cancelled,
            );
        })?;

        if poll_event(Duration::from_millis(200))? {
            if let Event::Key(key) = read_event()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') => break,
                        _ => {}
                    }
                }
            }
        }
    }

    if failed > 0 {
        anyhow::bail!("{}", error_msg);
    }
    if cancelled {
        anyhow::bail!("operation cancelled by user");
    }

    Ok(())
}

fn render_progress(
    f: &mut ratatui::Frame,
    states: &[StepState],
    frame: u64,
    title: &str,
    completed: usize,
    total: usize,
    failed: usize,
    error_msg: &str,
    cancelled: bool,
) {
    let size = f.area();
    let block = Block::default().title(title).borders(Borders::ALL);
    f.render_widget(&block, size);

    let inner = block.inner(size);

    let mut rows = Vec::new();
    rows.push(Constraint::Min(1));
    rows.push(Constraint::Length(states.len() as u16));
    if !error_msg.is_empty() {
        rows.push(Constraint::Length(std::cmp::min(
            error_msg.lines().count() as u16,
            5,
        )));
    }
    rows.push(Constraint::Length(2));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(rows)
        .split(inner);

    let steps_area = chunks[1];
    let summary_idx = if !error_msg.is_empty() { 3 } else { 2 };
    let summary_area = chunks[summary_idx];

    let spinner_idx = frame as usize % SPINNER.len();
    let spinner = SPINNER[spinner_idx];

    let step_lines: Vec<Line> = states
        .iter()
        .map(|s| {
            let (icon, style) = match s.status {
                StepStatus::Pending => ("  ", Style::default().fg(Color::DarkGray)),
                StepStatus::Running => {
                    let icon_str = format!("{} ", spinner);
                    return Line::from(Span::styled(
                        format!("{}{}", icon_str, s.label),
                        Style::default().fg(Color::Cyan),
                    ));
                }
                StepStatus::Done => ("✓ ", Style::default().fg(Color::Green)),
                StepStatus::Error => ("✗ ", Style::default().fg(Color::Red)),
            };
            Line::from(Span::styled(format!("{}{}", icon, s.label), style))
        })
        .collect();

    let steps_para = Paragraph::new(step_lines);
    f.render_widget(steps_para, steps_area);

    if !error_msg.is_empty() {
        let error_para = Paragraph::new(error_msg)
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: false });
        f.render_widget(error_para, chunks[2]);
    }

    let summary: String = if failed > 0 {
        "Operation failed \u{2014} press Enter or Esc to continue.".into()
    } else if cancelled {
        "Operation cancelled \u{2014} press Enter or Esc to continue.".into()
    } else {
        let remaining = total - completed - failed;
        format!(
            "\u{2713} {completed} completed \u{00b7} {remaining} remaining \u{00b7} {failed} failed"
        )
    };

    let summary_line = Paragraph::new(Line::from(Span::styled(
        &summary,
        Style::default().fg(if failed > 0 || cancelled {
            Color::Red
        } else {
            Color::White
        }),
    )));
    f.render_widget(summary_line, summary_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use std::cell::Cell;

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
    fn render_pending_step() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let states = [StepState {
                    label: "pending test".into(),
                    status: StepStatus::Pending,
                }];
                render_progress(f, &states, 0, "Title", 0, 1, 0, "", false);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("pending test"), "no label in:\n{all}");
        assert!(all.contains("0 completed"), "no summary in:\n{all}");
    }

    #[test]
    fn render_running_step_shows_spinner() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let states = [StepState {
                    label: "running test".into(),
                    status: StepStatus::Running,
                }];
                render_progress(f, &states, 3, "Title", 1, 2, 0, "", false);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        // SPINNER[3] = '⠸'
        assert!(all.contains('⠸'), "no spinner in:\n{all}");
        assert!(all.contains("running test"), "no label in:\n{all}");
    }

    #[test]
    fn render_done_step_shows_checkmark() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let states = [StepState {
                    label: "done test".into(),
                    status: StepStatus::Done,
                }];
                render_progress(f, &states, 0, "Title", 1, 1, 0, "", false);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains('\u{2713}'), "no checkmark in:\n{all}");
        assert!(all.contains("1 completed"), "no summary in:\n{all}");
    }

    #[test]
    fn render_error_step_shows_x_and_message() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let states = [StepState {
                    label: "error test".into(),
                    status: StepStatus::Error,
                }];
                render_progress(
                    f, &states, 0, "Title", 0, 1, 1, "something failed", false,
                );
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains('\u{2717}'), "no X in:\n{all}");
        assert!(all.contains("something failed"), "no error msg in:\n{all}");
        assert!(
            all.contains("Operation failed"),
            "no failure summary in:\n{all}"
        );
    }

    #[test]
    fn render_cancelled_summary() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let states = [StepState {
                    label: "cancelled test".into(),
                    status: StepStatus::Pending,
                }];
                render_progress(f, &states, 0, "Title", 0, 1, 0, "", true);
            })
            .unwrap();

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(
            all.contains("Operation cancelled"),
            "no cancel summary in:\n{all}"
        );
    }

    #[test]
    fn run_progress_all_steps_ok() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let steps = vec![
            ProgressStep {
                label: "first".into(),
                action: Box::new(|| Ok(())),
            },
            ProgressStep {
                label: "second".into(),
                action: Box::new(|| Ok(())),
            },
        ];

        let polls = Cell::new(0u64);
        let result = run_progress_on_terminal(
            &mut terminal,
            "Test",
            steps,
            |_| {
                let n = polls.get() + 1;
                polls.set(n);
                Ok(n > 5)
            },
            events![
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ],
        );

        assert!(result.is_ok(), "expected Ok, got {result:?}");

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains("2 completed"), "bad summary in:\n{all}");
        assert!(all.contains('\u{2713}'), "no checkmark in:\n{all}");
    }

    #[test]
    fn run_progress_step_fails() {
        let backend = TestBackend::new(60, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let steps = vec![
            ProgressStep {
                label: "good".into(),
                action: Box::new(|| Ok(())),
            },
            ProgressStep {
                label: "bad".into(),
                action: Box::new(|| Err(anyhow::anyhow!("oops"))),
            },
            ProgressStep {
                label: "never runs".into(),
                action: Box::new(|| Ok(())),
            },
        ];

        let polls = Cell::new(0u64);
        let result = run_progress_on_terminal(
            &mut terminal,
            "Test",
            steps,
            |_| {
                let n = polls.get() + 1;
                polls.set(n);
                Ok(n > 5)
            },
            events![
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ],
        );

        assert!(result.is_err(), "expected Err, got {result:?}");

        let lines = buffer_lines(terminal.backend().buffer());
        let all = lines.join("\n");
        assert!(all.contains('\u{2717}'), "no X in:\n{all}");
        assert!(all.contains("oops"), "no error msg in:\n{all}");
        assert!(all.contains("Operation failed"), "bad summary in:\n{all}");
    }
}
