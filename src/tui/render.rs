use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::model::scenario::{CheckState, Scenario, ScenarioStatus, StepStatus, VerifyMode};
use crate::tui::app::{App, FocusPane};
use crate::watcher::drift::DriftSummary;

pub fn render(frame: &mut Frame, app: &App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(frame.area());

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(vertical[0]);

    render_scenario_list(frame, app, columns[0]);
    render_scenario_detail(frame, app, columns[1]);
    render_footer(frame, app, vertical[1]);
}

fn render_scenario_list(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == FocusPane::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(format!(" Scenarios ({}) ", app.scenarios.len()));

    if app.scenarios.is_empty() {
        let empty = Paragraph::new("シナリオがありません。\n`daihon register` で登録してください。")
            .block(block)
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = app
        .scenarios
        .iter()
        .map(|loaded| {
            let sc = &loaded.scenario;
            let (badge, color) = scenario_badge(sc.front_matter.status);
            let passed_steps = sc
                .steps
                .iter()
                .filter(|s| matches!(s.status, StepStatus::Passed | StepStatus::ReadyForManual))
                .count();
            let total_steps = sc.steps.len();

            let mut spans = vec![
                Span::styled(
                    format!("[{badge}]"),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {} ", sc.front_matter.priority)),
                Span::styled(
                    format!("{passed_steps}/{total_steps} "),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if let Some(drift) = app.drift_for(&sc.front_matter.id)
                && drift.is_drifting
            {
                spans.push(Span::styled(
                    format!("[DRIFT x{}] ", drift.consecutive_same_stop),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::raw(sc.front_matter.title.clone()));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.selected));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut state);
}

fn render_scenario_detail(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == FocusPane::Detail {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let Some(loaded) = app.selected_scenario() else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Detail ");
        frame.render_widget(Paragraph::new("シナリオを選択してください").block(block), area);
        return;
    };

    let sc = &loaded.scenario;
    let title = format!(
        " [{}] {} ",
        sc.front_matter.priority, sc.front_matter.title
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let lines = build_detail_lines(sc, app.drift_for(&sc.front_matter.id));
    let detail = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));

    frame.render_widget(detail, area);
}

fn build_detail_lines(sc: &Scenario, drift: Option<&DriftSummary>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let (badge, color) = scenario_badge(sc.front_matter.status);

    let checked_info = match (
        sc.front_matter.last_checked_at.as_deref(),
        sc.front_matter.last_checked_by.as_deref(),
    ) {
        (Some(at), Some(by)) => format!("{at} ({by})"),
        (Some(at), None) => at.to_string(),
        (None, Some(by)) => format!("- ({by})"),
        (None, None) => "未実行".to_string(),
    };

    lines.push(Line::from(vec![
        Span::styled("状態: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("[{badge}] {}", sc.front_matter.status),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("   ID: {}", sc.front_matter.id)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("最終確認: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(checked_info),
    ]));

    if !sc.front_matter.tags.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("タグ: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(sc.front_matter.tags.join(", ")),
        ]));
    }

    if !sc.front_matter.hosts.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("対象ホスト: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(sc.front_matter.hosts.join(", ")),
        ]));
    }

    if let Some(d) = drift
        && d.is_drifting
    {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("⚠ ドリフト検知: {}", d.reason),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "概要",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    for ov_line in sc.overview.lines() {
        lines.push(Line::raw(ov_line.to_string()));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "ステップ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));

    for step in &sc.steps {
        let (st_badge, st_color) = step_badge(step.status);
        lines.push(Line::from(vec![
            Span::styled(
                format!("Step {} [{st_badge}] ", step.number),
                Style::default().fg(st_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                step.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));

        if let Some(ref summary) = step.summary {
            lines.push(Line::from(Span::styled(
                format!("  {summary}"),
                Style::default().fg(Color::DarkGray),
            )));
        }

        for check in &step.checks {
            let (mark_style, mode_label) = (
                match check.state {
                    CheckState::Passed => Style::default().fg(Color::Green),
                    CheckState::Failed => {
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                    }
                    CheckState::Pending => Style::default().fg(Color::DarkGray),
                },
                match check.mode {
                    VerifyMode::Auto => "auto",
                    VerifyMode::Manual => "manual",
                },
            );

            let mode_color = match check.mode {
                VerifyMode::Auto => Color::Blue,
                VerifyMode::Manual => Color::Yellow,
            };

            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("[{}] ", check.state.marker()), mark_style),
                Span::styled(format!("[{mode_label}] "), Style::default().fg(mode_color)),
                Span::styled(
                    format!("{}: ", check.id),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(check.description.clone()),
            ]));

            if let Some(ref reason) = check.reason {
                lines.push(Line::from(Span::styled(
                    format!("      └─ {reason}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
        lines.push(Line::raw(""));
    }

    lines
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let help = "↑/↓(j/k): 選択/スクロール  Tab: ペイン切替  r: 再読込  q: 終了";
    let text = match &app.status_message {
        Some(msg) => format!("{help}  |  {msg}"),
        None => format!("{help}  |  dir: {}", app.dir.display()),
    };
    let footer = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

pub fn scenario_badge(status: ScenarioStatus) -> (&'static str, Color) {
    match status {
        ScenarioStatus::Passed => ("PASS", Color::Green),
        ScenarioStatus::Failed => ("FAIL", Color::Red),
        ScenarioStatus::ReadyForManual => ("WAIT", Color::Yellow),
        ScenarioStatus::InProgress => ("RUN ", Color::Cyan),
        ScenarioStatus::Pending => ("TODO", Color::DarkGray),
    }
}

pub fn step_badge(status: StepStatus) -> (&'static str, Color) {
    match status {
        StepStatus::Passed => ("PASS", Color::Green),
        StepStatus::Failed => ("FAIL", Color::Red),
        StepStatus::ReadyForManual => ("WAIT", Color::Yellow),
        StepStatus::InProgress => ("RUN ", Color::Cyan),
        StepStatus::Pending => ("TODO", Color::DarkGray),
    }
}
