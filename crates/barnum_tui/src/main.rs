//! barnum-tui: Terminal dashboard for barnum workflows.

mod app;
mod detail;
mod event;
mod footer;
mod graph;
mod header;
mod log_watcher;
mod task_list;
mod theme;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::Event,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Widget},
};

use app::{AppState, PanelFocus};
use barnum_config::ConfigFile;
use barnum_state::StateLogEntry;
use detail::render::DetailWidget;
use footer::FooterWidget;
use graph::{GraphWidget, StepGraph};
use header::HeaderWidget;
use log_watcher::{LogEvent, LogWatcher};
use task_list::render::TaskListWidget;

#[derive(Parser)]
#[command(name = "barnum-tui", about = "Terminal dashboard for barnum workflows")]
struct Cli {
    /// Path to the barnum config file
    #[arg(long)]
    config: PathBuf,

    /// Path to the state log file
    #[arg(long)]
    state_log: PathBuf,

    /// Replay existing log entries from the beginning
    #[arg(long, default_value_t = false)]
    replay: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load and validate config
    let config_content = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("Failed to read config file: {}", cli.config.display()))?;
    let config: ConfigFile = json5::from_str(&config_content)
        .with_context(|| format!("Failed to parse config file: {}", cli.config.display()))?;
    config.validate().context("Config validation failed")?;

    // Build step graph from config
    let step_graph = StepGraph::from_config(&config);

    // Create app state
    let config_path = cli.config.display().to_string();
    let mut app = AppState::new(config_path);

    // Create log watcher
    let mut watcher = LogWatcher::new(&cli.state_log, cli.replay)
        .with_context(|| format!("Failed to watch state log: {}", cli.state_log.display()))?;

    // Set up terminal
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    // Run the event loop
    let result = run_loop(&mut terminal, &mut app, &step_graph, &mut watcher);

    // Restore terminal
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    step_graph: &StepGraph,
    watcher: &mut LogWatcher,
) -> Result<()> {
    let tick_rate = Duration::from_millis(100);

    loop {
        // 1. Poll log watcher for new events
        for log_event in watcher.poll() {
            match log_event {
                LogEvent::Entry(StateLogEntry::Config(_)) => {
                    // Ignore - we built graph from file
                }
                LogEvent::Entry(StateLogEntry::TaskSubmitted(sub)) => {
                    app.apply_submitted(
                        sub.task_id,
                        sub.step,
                        sub.value.0,
                        sub.parent_id,
                        sub.origin,
                    );
                }
                LogEvent::Entry(StateLogEntry::TaskCompleted(comp)) => {
                    app.apply_completed(comp.task_id, comp.outcome);
                }
                LogEvent::Error(_e) => {
                    // Ignore parse errors for now
                }
            }
        }

        // 2. Render UI
        terminal.draw(|frame| {
            let size = frame.area();

            // Main layout: header + body + footer
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Header
                    Constraint::Min(10),   // Body
                    Constraint::Length(1), // Footer
                ])
                .split(size);

            // Render header
            let header = HeaderWidget::new(app);
            frame.render_widget(header, main_chunks[0]);

            // Body layout: main panels + detail
            let body_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(8),    // Main panels
                    Constraint::Length(8), // Detail
                ])
                .split(main_chunks[1]);

            // Main panels: graph + task list
            let panel_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(35), // Graph
                    Constraint::Percentage(65), // Task list
                ])
                .split(body_chunks[0]);

            // Render graph
            let graph_focused = app.focus == PanelFocus::Graph;
            let graph_block = Block::default()
                .title("Step Graph")
                .borders(Borders::ALL)
                .border_style(if graph_focused {
                    theme::focused_border_style()
                } else {
                    theme::unfocused_border_style()
                });
            let graph_inner = graph_block.inner(panel_chunks[0]);
            frame.render_widget(graph_block, panel_chunks[0]);

            let graph_widget = GraphWidget::new(
                step_graph,
                &app.step_counts,
                app.selected_step.as_ref(),
                &app.graph_viewport,
            );
            frame.render_widget(graph_widget, graph_inner);

            // Render task list
            let visible_tasks = app.visible_tasks();
            let task_list = TaskListWidget::new(
                &visible_tasks,
                app,
                app.focus == PanelFocus::TaskList,
            );
            let mut table_state = app.task_list_state.clone();
            task_list.render_with_state(panel_chunks[1], frame.buffer_mut(), &mut table_state);
            app.task_list_state = table_state;

            // Render detail
            let detail = DetailWidget::new(app, app.focus == PanelFocus::Detail);
            frame.render_widget(detail, body_chunks[1]);

            // Render footer
            let footer = FooterWidget::new(app.focus);
            frame.render_widget(footer, main_chunks[2]);
        })?;

        // 3. Handle input
        if let Some(Event::Key(key)) = event::poll_event(tick_rate) {
            if matches!(event::handle_key(key, app), event::EventResult::Quit) {
                break;
            }
        }
    }

    Ok(())
}
