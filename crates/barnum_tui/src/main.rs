//! Terminal dashboard for barnum workflows.

mod app;
mod detail;
mod event;
mod footer;
mod graph;
mod header;
mod log_watcher;
mod task_list;
mod theme;

use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use barnum_config::ConfigFile;
use barnum_state::StateLogEntry;
use clap::Parser;
use crossterm::event::Event;
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders, TableState};
use ratatui::Terminal;

use app::{AppState, PanelFocus};
use detail::render::DetailWidget;
use event::EventResult;
use footer::FooterWidget;
use graph::render::GraphWidget;
use graph::StepGraph;
use header::HeaderWidget;
use log_watcher::{LogEvent, LogWatcher};
use task_list::render::TaskListWidget;
use theme::{focused_border_style, unfocused_border_style};

/// Terminal dashboard for barnum workflows.
#[derive(Parser)]
#[command(name = "barnum-tui", about = "Terminal dashboard for barnum workflows")]
struct Cli {
    /// Path to the workflow config file (JSON/JSONC).
    #[arg(long)]
    config: PathBuf,

    /// Path to the NDJSON state log file.
    #[arg(long)]
    state_log: PathBuf,

    /// Replay mode: read log from beginning instead of tailing.
    #[arg(long, default_value_t = false)]
    replay: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load and validate config.
    let config_text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("failed to read config: {}", cli.config.display()))?;
    let config_file: ConfigFile = json5::from_str(&config_text)
        .with_context(|| format!("failed to parse config: {}", cli.config.display()))?;
    config_file
        .validate()
        .map_err(|e| anyhow::anyhow!("config validation failed: {e:?}"))?;

    // Build the step graph for the DAG panel.
    let step_graph = StepGraph::from_config(&config_file);

    // Create application state.
    let mut app = AppState::new(cli.config.clone());

    // Create log watcher.
    let mut watcher = LogWatcher::new(&cli.state_log, cli.replay)?;

    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the event loop; capture result so we always restore the terminal.
    let result = run_loop(&mut terminal, &mut app, &mut watcher, &step_graph);

    // Restore terminal.
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    watcher: &mut LogWatcher,
    step_graph: &StepGraph,
) -> anyhow::Result<()> {
    let tick_rate = Duration::from_millis(100);
    let mut table_state = TableState::default();

    loop {
        // 1. Poll log watcher for new events.
        for log_event in watcher.poll() {
            match log_event {
                LogEvent::Entry(StateLogEntry::Config(_)) => {
                    // Config entry: already loaded, skip.
                }
                LogEvent::Entry(StateLogEntry::TaskSubmitted(sub)) => {
                    app.apply_submitted(
                        sub.task_id,
                        sub.step,
                        sub.value,
                        sub.parent_id,
                        sub.origin,
                    );
                }
                LogEvent::Entry(StateLogEntry::TaskCompleted(comp)) => {
                    app.apply_completed(comp.task_id, comp.outcome);
                }
                LogEvent::Error(_) => {
                    // Silently skip parse errors for now.
                }
            }
        }

        // Sync table selection state with app state.
        table_state.select(app.task_list_state.selected());

        // 2. Render.
        let visible_tasks = app.visible_tasks();

        terminal.draw(|frame| {
            let size = frame.area();

            // Top-level layout: header(1) + body(fill) + footer(1).
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),  // header
                    Constraint::Min(10),    // body
                    Constraint::Length(1),  // footer
                ])
                .split(size);

            // Header.
            frame.render_widget(HeaderWidget { app }, outer[0]);

            // Body: main_panels(fill) + detail(8 rows).
            let body = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(8),    // main panels
                    Constraint::Length(8), // detail pane
                ])
                .split(outer[1]);

            // Main panels: graph(35%) + task_list(65%).
            let main_panels = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(35), // graph
                    Constraint::Percentage(65), // task list
                ])
                .split(body[0]);

            // Graph panel with border.
            let graph_focused = app.focus == PanelFocus::Graph;
            let graph_border_style = if graph_focused {
                focused_border_style()
            } else {
                unfocused_border_style()
            };
            let graph_block = Block::default()
                .title(" DAG ")
                .borders(Borders::ALL)
                .border_style(graph_border_style);
            let graph_inner = graph_block.inner(main_panels[0]);
            frame.render_widget(graph_block, main_panels[0]);

            let graph_widget = GraphWidget {
                graph: step_graph,
                step_counts: &app.step_counts,
                selected: app.selected_step.as_ref(),
                viewport: &app.graph_viewport,
            };
            frame.render_widget(graph_widget, graph_inner);

            // Task list panel.
            let task_list_focused = app.focus == PanelFocus::TaskList;
            let task_widget = TaskListWidget::new(&visible_tasks, app, task_list_focused);
            task_widget.render_with_state(main_panels[1], frame.buffer_mut(), &mut table_state);

            // Detail pane.
            let detail_focused = app.focus == PanelFocus::Detail;
            let detail_widget = DetailWidget::new(app, detail_focused);
            frame.render_widget(detail_widget, body[1]);

            // Footer.
            frame.render_widget(FooterWidget { focus: app.focus }, outer[2]);
        })?;

        // Sync table state back to app.
        app.task_list_state.select(table_state.selected());

        // 3. Handle input.
        if let Some(Event::Key(key)) = event::poll_event(tick_rate) {
            match event::handle_key(key, app) {
                EventResult::Quit => break,
                EventResult::Continue => {}
            }
        }
    }

    Ok(())
}
