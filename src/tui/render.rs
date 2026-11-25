use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::model::scenario::{CheckState, Scenario, ScenarioStatus, StepStatus, VerifyMode};
use crate::model::store::TreeEntry;
use crate::tui::app::{App, ChildrenSummary, FocusPane};
use crate::watcher::drift::DriftSummary;

pub fn render(frame: &mut Frame, app: &App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(frame.area());

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
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
        .title(format!(" Scenarios ({}) ", app.entries.len()));

    if app.entries.is_empty() {
        let empty =
            Paragraph::new("シナリオがありません。\n`daihon register` で登録してください。")
                .block(block)
                .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|entry| {
            let sc = &entry.loaded.scenario;
            let (badge, color) = if entry.blocked_by_parent.is_some() {
                ("親待", Color::DarkGray)
            } else {
                scenario_badge(sc.front_matter.status)
            };
            let passed_steps = sc
                .steps
                .iter()
                .filter(|s| matches!(s.status, StepStatus::Passed | StepStatus::ReadyForManual))
                .count();
            let total_steps = sc.steps.len();

            let mut spans = Vec::new();
            if entry.depth > 0 {
                let indent = "  ".repeat(entry.depth - 1);
                let branch = if entry.is_last_sibling {
                    "└─ "
                } else {
                    "├─ "
                };
                spans.push(Span::styled(
                    format!("{indent}{branch}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            spans.push(Span::styled(
                format!("[{badge}]"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(format!(" {} ", sc.front_matter.priority)));
            spans.push(Span::styled(
                format!("{passed_steps}/{total_steps} "),
                Style::default().fg(Color::DarkGray),
            ));
            if sc.is_togaki() {
                spans.push(Span::styled(
                    "[ト書き] ",
                    Style::default().fg(Color::Yellow),
                ));
            }
            if entry.blocked_by_parent.is_none()
                && let Some(drift) = app.drift_for(&sc.front_matter.id)
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

    let Some(entry) = app.selected_entry() else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Detail ");
        frame.render_widget(
            Paragraph::new("シナリオを選択してください").block(block),
            area,
        );
        return;
    };

    let sc = &entry.loaded.scenario;
    let kind_prefix = if sc.is_togaki() { "[ト書き] " } else { "" };
    let title = format!(
        " {kind_prefix}[{}] {} ",
        sc.front_matter.priority, sc.front_matter.title
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let children_summary = app.children_summary_for(&sc.front_matter.id);
    let lines = build_detail_lines(
        entry,
        app.drift_for(&sc.front_matter.id),
        &children_summary,
    );
    let detail = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));

    frame.render_widget(detail, area);
}

fn build_detail_lines(
    entry: &TreeEntry,
    drift: Option<&DriftSummary>,
    children: &ChildrenSummary,
) -> Vec<Line<'static>> {
    let sc: &Scenario = &entry.loaded.scenario;
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

    if sc.is_togaki() {
        let parent_id = sc.front_matter.parent.as_deref().unwrap_or("-");
        let mut togaki_spans = vec![
            Span::styled("種別: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("ト書き (Togaki)", Style::default().fg(Color::Yellow)),
            Span::raw(format!("   親シナリオ: {parent_id}")),
        ];
        if let Some(step_num) = sc.front_matter.attach_step {
            togaki_spans.push(Span::raw(format!(" (Step {step_num})")));
        }
        lines.push(Line::from(togaki_spans));
    } else if children.total > 0 {
        lines.push(Line::from(vec![
            Span::styled("紐づくト書き: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                "{} 件（通過 {} / 停止 {} / 未検証 {}）",
                children.total, children.passed, children.failed, children.pending
            )),
        ]));
    }

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

    if let Some(ref blocker) = entry.blocked_by_parent {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!(
                "⏳ 親シナリオ `{blocker}` が未通過のため待機中（親が通過すると自動で見張り対象になります）"
            ),
            Style::default().fg(Color::Yellow),
        )));
    } else if let Some(d) = drift
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
