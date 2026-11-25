use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::model::scenario::{CheckState, Scenario, ScenarioStatus, StepStatus, VerifyMode};
use crate::model::store::TreeEntry;
use crate::tui::app::{App, ChildrenSummary, DraftTarget, FocusPane, ModalState};
use crate::watcher::drift::DriftSummary;

pub fn render(frame: &mut Frame, app: &App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(frame.area());

    if matches!(app.modal, ModalState::None) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(vertical[0]);

        render_scenario_list(frame, app, columns[0]);
        render_scenario_detail(frame, app, columns[1]);
    } else {
        frame.render_widget(Clear, vertical[0]);
        render_modal(frame, app, vertical[0]);
    }
    render_footer(frame, app, vertical[1]);
}

fn render_scenario_list(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == FocusPane::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let (scenario_count, togaki_count) = app.counts();
    let list_title = if togaki_count > 0 {
        format!(" Scenarios ({scenario_count}) / Togaki ({togaki_count}) ")
    } else {
        format!(" Scenarios ({scenario_count}) ")
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(list_title);

    if app.entries.is_empty() {
        let empty =
            Paragraph::new("シナリオがありません。\n`daihon register` または `n` キーで登録してください。")
                .block(block)
                .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let inner_width = block.inner(area).width as usize;
    let avail_width = inner_width.saturating_sub(2).max(8);

    let items: Vec<ListItem> = app
        .entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_selected = idx == app.selected;
            let muted_color = if is_selected {
                Color::Gray
            } else {
                Color::DarkGray
            };

            let sc = &entry.loaded.scenario;
            let (badge, raw_color) = if entry.blocked_by_parent.is_some() {
                ("親待", Color::DarkGray)
            } else {
                scenario_badge(sc.front_matter.status)
            };
            let color = if raw_color == Color::DarkGray && is_selected {
                Color::Gray
            } else {
                raw_color
            };
            let passed_steps = sc
                .steps
                .iter()
                .filter(|s| matches!(s.status, StepStatus::Passed | StepStatus::ReadyForManual))
                .count();
            let total_steps = sc.steps.len();

            let mut header_spans = Vec::new();
            if entry.depth > 0 {
                let indent = "  ".repeat(entry.depth - 1);
                let branch = if entry.is_last_sibling {
                    "└─ "
                } else {
                    "├─ "
                };
                header_spans.push(Span::styled(
                    format!("{indent}{branch}"),
                    Style::default().fg(muted_color),
                ));
            }
            header_spans.push(Span::styled(
                format!("[{badge}]"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
            header_spans.push(Span::raw(format!(" {} ", sc.front_matter.priority)));
            header_spans.push(Span::styled(
                format!("{passed_steps}/{total_steps} "),
                Style::default().fg(muted_color),
            ));
            if sc.is_togaki() {
                header_spans.push(Span::styled(
                    "[ト書き] ",
                    Style::default().fg(Color::Yellow),
                ));
            }
            if entry.blocked_by_parent.is_none()
                && let Some(drift) = app.drift_for(&sc.front_matter.id)
                && drift.is_drifting
            {
                header_spans.push(Span::styled(
                    format!("[DRIFT x{}] ", drift.consecutive_same_stop),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ));
            }

            let header_line = Line::from(header_spans.clone());
            let title_text = sc.front_matter.title.clone();
            let title_width = Span::raw(title_text.as_str()).width();

            if header_line.width() + title_width <= avail_width {
                header_spans.push(Span::raw(title_text));
                ListItem::new(Line::from(header_spans))
            } else {
                let mut item_lines = wrap_styled_line(header_line, avail_width, "");
                let title_indent = if entry.depth > 0 {
                    format!("{}   ", "  ".repeat(entry.depth - 1))
                } else {
                    "  ".to_string()
                };
                let effective_indent = if Span::raw(title_indent.as_str()).width() + 8 <= avail_width
                {
                    title_indent
                } else {
                    String::new()
                };
                let title_line = Line::from(Span::raw(format!("{effective_indent}{title_text}")));
                item_lines.extend(wrap_styled_line(title_line, avail_width, &effective_indent));
                ListItem::new(item_lines)
            }
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
    let title = format!(" {kind_prefix}[{}] Detail ", sc.front_matter.priority);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner_width = block.inner(area).width as usize;
    let children_summary = app.children_summary_for(&sc.front_matter.id);
    let raw_lines = build_detail_lines(
        entry,
        app.drift_for(&sc.front_matter.id),
        &children_summary,
    );
    let wrapped_lines = wrap_lines_for_width(raw_lines, inner_width);
    let detail = Paragraph::new(wrapped_lines)
        .block(block)
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

    lines.push(Line::from(Span::styled(
        sc.front_matter.title.clone(),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(""));

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
    ]));
    lines.push(Line::from(vec![
        Span::styled("ID: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(sc.front_matter.id.clone()),
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
    let narrow = area.width < 96;
    let help = match &app.modal {
        ModalState::None if narrow => {
            "j/k:移動 0-3:優先度 t:ト書き n:新規 Tab:切替 r:再読込 q:終了"
        }
        ModalState::None => {
            "↑/↓(j/k): 選択  0-3: 優先度  t: ト書き追加  n: 新規シナリオ  Tab: ペイン切替  r: 再読込  q: 終了"
        }
        ModalState::Input { .. } => "自然言語で入力  Enter: 生成  Esc: キャンセル",
        ModalState::Generating { .. } => "登録エージェントで構造化中...  Esc: キャンセル",
        ModalState::Preview { .. } if narrow => "y/Enter:保存 0-3:P0-P3 j/k:移動 n/Esc:戻",
        ModalState::Preview { .. } => {
            "y/Enter: 保存  0-3: 優先度変更(P0..P3)  ↑/↓(j/k): スクロール  n/Esc: キャンセル"
        }
    };
    let text = match (&app.modal, &app.status_message) {
        (ModalState::None, Some(msg)) => format!("{help}  |  {msg}"),
        (ModalState::None, None) if !narrow => format!("{help}  |  dir: {}", app.dir.display()),
        _ => help.to_string(),
    };
    let footer = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, area);
}

fn render_modal(frame: &mut Frame, app: &App, body_area: Rect) {
    match &app.modal {
        ModalState::None => {}
        ModalState::Input { target, input } => {
            let area = centered_rect(76, 11, body_area);
            frame.render_widget(Clear, area);

            let (title, subtitle, placeholder) = match target {
                DraftTarget::Togaki {
                    parent_id,
                    parent_title,
                } => (
                    format!(" ト書き（Togaki）を追加 — 親: {parent_id} "),
                    format!("親シナリオ: {parent_title}"),
                    "例: 遅延が 50ms 以内に収まること / ローカル素材を引き渡せること",
                ),
                DraftTarget::NewScenario => (
                    " 新規シナリオ（台本）を登録 ".to_string(),
                    "最初にドッグフーディングしたい大きな使い方を自然言語で書いてください".to_string(),
                    "例: MacBook Air でアプリを開いて Ubuntu の Blender で編集作業が行える",
                ),
            };

            let outer = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title);
            let inner = outer.inner(area);
            frame.render_widget(outer, area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2),
                    Constraint::Length(3),
                    Constraint::Min(1),
                ])
                .split(inner);

            frame.render_widget(
                Paragraph::new(subtitle)
                    .style(Style::default().fg(Color::Yellow))
                    .wrap(Wrap { trim: true }),
                chunks[0],
            );

            let input_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White))
                .title(" 入力 ");
            let input_inner = input_block.inner(chunks[1]);

            let (display_line, cursor_offset) = if input.is_empty() {
                (
                    Line::from(Span::styled(
                        placeholder,
                        Style::default().fg(Color::DarkGray),
                    )),
                    0u16,
                )
            } else {
                let line = Line::from(input.as_str());
                let w = line.width() as u16;
                (line, w)
            };

            let scroll_x = if input_inner.width > 1 && cursor_offset >= input_inner.width {
                cursor_offset - input_inner.width + 1
            } else {
                0
            };

            frame.render_widget(
                Paragraph::new(display_line)
                    .block(input_block)
                    .scroll((0, scroll_x)),
                chunks[1],
            );

            // Position hardware cursor inside the input box so macOS Japanese IME popup aligns cleanly.
            let cursor_x = input_inner
                .x
                .saturating_add(cursor_offset.saturating_sub(scroll_x))
                .min(input_inner.right().saturating_sub(1));
            frame.set_cursor_position(Position::new(cursor_x, input_inner.y));

            frame.render_widget(
                Paragraph::new("Enter: エージェントで構造化プレビュー   Esc: キャンセル")
                    .style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
        ModalState::Generating {
            target,
            input,
            tick,
        } => {
            const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let spin = SPINNER[*tick % SPINNER.len()];
            let area = centered_rect(68, 8, body_area);
            frame.render_widget(Clear, area);

            let heading = match target {
                DraftTarget::Togaki { parent_id, .. } => {
                    format!(" ト書き生成中 (親: {parent_id}) ")
                }
                DraftTarget::NewScenario => " シナリオ生成中 ".to_string(),
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(heading);

            let lines = vec![
                Line::raw(""),
                Line::from(vec![
                    Span::styled(
                        format!(" {spin} "),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("登録エージェントでステップと OK 条件を構造化しています..."),
                ]),
                Line::raw(""),
                Line::from(Span::styled(
                    format!(" 入力: {input}"),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    " Esc: キャンセル",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            frame.render_widget(
                Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
                area,
            );
        }
        ModalState::Preview {
            target,
            proposal,
            priority,
            scroll,
        } => {
            let area = centered_rect(86, body_area.height.saturating_sub(2).max(12), body_area);
            frame.render_widget(Clear, area);

            let heading = match target {
                DraftTarget::Togaki { parent_id, .. } => {
                    format!(" ト書きプレビュー確認 — 親: {parent_id} ")
                }
                DraftTarget::NewScenario => " シナリオプレビュー確認 ".to_string(),
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .title(heading);
            let inner = block.inner(area);
            frame.render_widget(block, area);

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(1)])
                .split(inner);

            let mut lines = Vec::new();
            lines.push(Line::from(vec![
                Span::styled("優先度: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("{priority}"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "  （推奨: {} — {}）",
                    proposal.priority_proposal.suggested, proposal.priority_proposal.rationale
                )),
            ]));

            if proposal.overlaps.is_empty() {
                lines.push(Line::from(Span::styled(
                    "重複チェック: 既存との重なりなし",
                    Style::default().fg(Color::Green),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "重複チェック（重なり候補あり）:",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )));
                for ov in &proposal.overlaps {
                    lines.push(Line::from(Span::styled(
                        format!("  - {} ({}): {}", ov.existing_id, ov.existing_title, ov.reason),
                        Style::default().fg(Color::Yellow),
                    )));
                }
            }

            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "--- 生成される Markdown ---",
                Style::default().fg(Color::Cyan),
            )));
            let mut preview_sc = proposal.scenario.clone();
            preview_sc.front_matter.priority = *priority;
            for md_line in preview_sc.render_new_markdown().lines() {
                lines.push(Line::raw(md_line.to_string()));
            }

            let wrapped = wrap_lines_for_width(lines, chunks[0].width as usize);
            frame.render_widget(
                Paragraph::new(wrapped).scroll((*scroll, 0)),
                chunks[0],
            );
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "[y/Enter] 保存   [0/1/2/3] P0..P3変更   [j/k] スクロール   [n/Esc] キャンセル",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ))),
                chunks[1],
            );
        }
    }
}

fn char_display_width(ch: char) -> usize {
    let mut buf = [0u8; 4];
    let s: &str = ch.encode_utf8(&mut buf);
    Span::raw(s).width()
}

/// Splits a string into wrappable tokens:
/// - Contiguous ASCII graphic runs (words/numbers/flags like `Studio`, `500ms`, `--target`) stay intact when they fit.
/// - Non-ASCII characters (Japanese/CJK) wrap cleanly character-by-character.
/// - Spaces and `\n` are emitted as individual tokens.
fn tokenize_for_wrap(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch.is_ascii_graphic() {
            let mut end = start + ch.len_utf8();
            while let Some(&(next_idx, next_ch)) = chars.peek() {
                if next_ch.is_ascii_graphic() {
                    end = next_idx + next_ch.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(&text[start..end]);
        } else {
            tokens.push(&text[start..start + ch.len_utf8()]);
        }
    }
    tokens
}

/// Lossless, Unicode-display-width-aware styled line wrapper.
/// Keeps ASCII words/numbers together when possible while wrapping CJK smoothly without
/// ratatui's WordWrapper character-dropping bugs.
fn wrap_styled_line(
    line: Line<'static>,
    max_width: usize,
    continuation_indent: &str,
) -> Vec<Line<'static>> {
    let max_width = max_width.max(4);
    let indent_width = Span::raw(continuation_indent).width();
    let effective_indent = if !continuation_indent.is_empty() && indent_width + 4 <= max_width {
        continuation_indent
    } else {
        ""
    };
    let effective_indent_width = Span::raw(effective_indent).width();
    let continuation_capacity = max_width.saturating_sub(effective_indent_width).max(4);

    if line.spans.is_empty() {
        return vec![Line::raw("")];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width: usize = 0;

    let push_wrapped_line = |out: &mut Vec<Line<'static>>,
                             current_spans: &mut Vec<Span<'static>>,
                             current_width: &mut usize| {
        out.push(Line::from(std::mem::take(current_spans)));
        if !effective_indent.is_empty() {
            current_spans.push(Span::raw(effective_indent.to_string()));
            *current_width = effective_indent_width;
        } else {
            *current_width = 0;
        }
    };

    for span in line.spans {
        let style = span.style;
        let mut buf = String::new();
        for token in tokenize_for_wrap(&span.content) {
            if token == "\n" {
                if !buf.is_empty() {
                    current_spans.push(Span::styled(std::mem::take(&mut buf), style));
                }
                out.push(Line::from(std::mem::take(&mut current_spans)));
                current_width = 0;
                continue;
            }

            let tok_width = Span::raw(token).width();
            let line_has_content = if out.is_empty() {
                current_width > 0
            } else {
                current_width > effective_indent_width
            };

            if line_has_content && current_width + tok_width > max_width {
                if token == " " {
                    if !buf.is_empty() {
                        current_spans.push(Span::styled(std::mem::take(&mut buf), style));
                    }
                    push_wrapped_line(&mut out, &mut current_spans, &mut current_width);
                    continue;
                }
                if tok_width <= continuation_capacity {
                    if !buf.is_empty() {
                        current_spans.push(Span::styled(std::mem::take(&mut buf), style));
                    }
                    push_wrapped_line(&mut out, &mut current_spans, &mut current_width);
                    buf.push_str(token);
                    current_width += tok_width;
                    continue;
                }
            }

            if current_width + tok_width <= max_width {
                buf.push_str(token);
                current_width += tok_width;
            } else {
                // Token itself is wider than a single line: fall back to char-by-char wrapping.
                for ch in token.chars() {
                    let w = char_display_width(ch);
                    if current_width > 0 && current_width + w > max_width {
                        if !buf.is_empty() {
                            current_spans.push(Span::styled(std::mem::take(&mut buf), style));
                        }
                        push_wrapped_line(&mut out, &mut current_spans, &mut current_width);
                    }
                    buf.push(ch);
                    current_width += w;
                }
            }
        }
        if !buf.is_empty() {
            current_spans.push(Span::styled(buf, style));
        }
    }

    if !current_spans.is_empty() || out.is_empty() {
        out.push(Line::from(current_spans));
    }

    out
}

fn wrap_lines_for_width(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    let max_width = max_width.max(4);
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let first_text = line
            .spans
            .first()
            .map(|s| s.content.as_ref())
            .unwrap_or("");
        let second_text = line
            .spans
            .get(1)
            .map(|s| s.content.as_ref())
            .unwrap_or("");
        let continuation = if first_text.starts_with("      └─ ") {
            "         "
        } else if first_text == "  " && second_text.starts_with('[') {
            "      "
        } else {
            ""
        };
        out.extend(wrap_styled_line(line, max_width, continuation));
    }
    out
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let h = height.min(area.height.saturating_sub(2)).max(3);
    let vertical_pad = area.height.saturating_sub(h) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vertical_pad),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(area);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);

    horizontal[1]
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
