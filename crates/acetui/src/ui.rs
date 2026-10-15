//! TUI rendering (ratatui): one draw function per session phase.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Phase};
use crate::world::{ChatStyle, RADAR_RANGE};

/// Render the whole frame for the current phase.
pub fn draw(f: &mut Frame, app: &App) {
    match app.phase {
        Phase::Idle | Phase::Connecting | Phase::Entering => draw_status(f, app),
        Phase::CharSelect => draw_charselect(f, app),
        Phase::CharCreate => draw_charcreate(f, app),
        Phase::InWorld | Phase::LoggingOut => draw_world(f, app),
    }
    if app.phase == Phase::InWorld {
        if app.world.dead {
            draw_center_box(f, "YOU ARE DEAD", &["Press Enter to chat, q to quit."], Color::Red);
        } else if app.menu {
            draw_center_box(
                f,
                "Menu",
                &["L  Log out of the world", "q  Quit", "Esc  Close"],
                Color::Yellow,
            );
        }
    }
}

// ================================================================== helpers

fn status_title(app: &App) -> String {
    format!("{} — {}@{}", app.server_name, app.account, app.host)
}

fn draw_status(f: &mut Frame, app: &App) {
    let area = f.area();
    let text = vec![
        Line::from(Span::styled("Asheron's Call — TUI client", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::raw(&app.status)),
        Line::from(Span::styled(
            status_title(app),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled("Esc  Quit", Style::default().fg(Color::DarkGray))),
    ];
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn style_for_chat(style: ChatStyle) -> (Color, Color) {
    match style {
        ChatStyle::Speech => (Color::White, Color::Cyan),
        ChatStyle::Own => (Color::Cyan, Color::Cyan),
        ChatStyle::Tell => (Color::Yellow, Color::Yellow),
        ChatStyle::Channel => (Color::Blue, Color::Magenta),
        ChatStyle::System => (Color::DarkGray, Color::Magenta),
        ChatStyle::Combat => (Color::Red, Color::Red),
        ChatStyle::Emote => (Color::Green, Color::Green),
        ChatStyle::Error => (Color::Red, Color::Yellow),
    }
}

// ============================================================== char select

fn draw_charselect(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(3)])
        .split(area);

    let items: Vec<ListItem> = app
        .char_list
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mark = if i == app.char_sel { ">" } else { " " };
            let mut s = format!("{mark} {}", c.name);
            if c.time_since_delete > 0 {
                s.push_str(&format!("  (deleted {}s ago)", c.time_since_delete));
            }
            let style = if i == app.char_sel {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(s).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Characters ({})", app.char_list.len())),
    );
    f.render_widget(list, chunks[0]);

    let hint = Paragraph::new(vec![
        Line::from(Span::raw(&app.status)),
        Line::from(Span::styled(
            "↑/↓ select · Enter enter world · C create · D delete · Esc quit",
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    f.render_widget(hint, chunks[1]);
}

// ============================================================== char create

fn draw_charcreate(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);

    let form = &app.create_form;
    let h = form.heritage();
    let t = form.template();
    let start = h.start_areas.get(form.start_area_idx).copied().unwrap_or(0);
    let start_name = acproto::chargen::starter_area_name(start);
    let sum = form.attr_sum();
    let over = sum > h.attribute_credits;
    let sum_style = if over { Color::Red } else { Color::Green };
    let attr_names = ["Str", "End", "Coord", "Quick", "Focus", "Self"];

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("[{}]", form.name), Style::default().fg(Color::Cyan)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Heritage: ", Style::default().fg(Color::DarkGray)),
        Span::raw(h.name.to_string()),
        Span::styled(
            format!("  ({} attr credits)", h.attribute_credits),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Template: ", Style::default().fg(Color::DarkGray)),
        Span::raw(t.name.to_string()),
        Span::styled("  (Tab to change)", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Gender: ", Style::default().fg(Color::DarkGray)),
        Span::raw(if form.gender == 1 { "Male" } else { "Female" }.to_string()),
        Span::styled("  (PgUp to toggle)", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Start: ", Style::default().fg(Color::DarkGray)),
        Span::raw(start_name.unwrap_or("Unknown").to_string()),
        Span::styled("  (←/→ to change)", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(""));
    for (i, a) in form.attrs.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(format!("{:>5} ", attr_names[i]), Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{a:>3}"),
                Style::default().fg(if *a >= 100 { Color::Yellow } else { Color::White }),
            ),
            Span::styled(format!("  (F{} +5)", i + 1), Style::default().fg(Color::DarkGray)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Attribute sum: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{sum} / {}", h.attribute_credits),
            Style::default().fg(sum_style).add_modifier(Modifier::BOLD),
        ),
    ]));
    if let Some(err) = &form.error {
        lines.push(Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red))));
    }
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Create Character"))
        .wrap(Wrap { trim: false });
    f.render_widget(p, chunks[0]);

    let hint = Paragraph::new(vec![
        Line::from(Span::raw(&app.status)),
        Line::from(Span::styled(
            "type name · ↑/↓ heritage · Tab template · PgUp gender · ←/→ start · F1–F6 +5 attrs · Enter create · Esc back",
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    f.render_widget(hint, chunks[1]);
}

// ===================================================================== world

fn draw_world(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    // Main row: vitals | center(radar+chat) | target/help
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(24),
            Constraint::Ratio(3, 5),
            Constraint::Ratio(2, 5),
        ])
        .split(chunks[0]);

    draw_vitals(f, app, main[0]);

    let center = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(13), Constraint::Min(2)])
        .split(main[1]);
    draw_radar(f, app, center[0]);
    draw_chat(f, app, center[1]);

    draw_targets(f, app, main[2]);

    // Chat input line.
    let input_area = chunks[1];
    let (text, style) = if app.chat_focused {
        let mut s = String::from("> ");
        s.push_str(&app.chat_buf);
        s.push('▌');
        (s, Style::default().fg(Color::Cyan))
    } else {
        (
            "Enter chat · / command · ` combat · T attack · Tab target · q run · Esc menu".to_string(),
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(Paragraph::new(text).style(style), input_area);

    // Status bar.
    let (cell, pos) = app.self_position();
    let block = (cell >> 16) & 0xFFFF;
    let mut bar = vec![
        format!("{}@{}", app.server_name, app.host),
        format!("{}/{}", app.account, app.self_name),
        format!("block {block:04X}"),
        format!("({:.1}, {:.1}, {:.1})", pos.x, pos.y, pos.z),
    ];
    if let Some(ping) = app.ping_ms {
        bar.push(format!("ping {ping:.0}ms"));
    }
    if let Some(net) = app.net.as_ref() {
        bar.push(format!("net {:#06x}", net.net_id() as u32));
    }
    bar.push(if app.combat_mode == acproto::messages::combat_mode::MELEE {
        "MELEE".to_string()
    } else {
        "peace".to_string()
    });
    bar.push(if app.run_lock { "run" } else { "walk" }.to_string());
    if app.crypto_bad > 0 {
        bar.push(format!("!crypto {}", app.crypto_bad));
    }
    let status_text = bar.join(" │ ");
    f.render_widget(
        Paragraph::new(format!("{}  ·  {}", status_text, app.status))
            .style(Style::default().bg(Color::DarkGray).fg(Color::White))
            .wrap(Wrap { trim: false }),
        chunks[2],
    );
}

fn bar_lines(label: &str, cur: Option<u32>, max: Option<u32>) -> Vec<Line<'static>> {
    let c = cur.unwrap_or(0);
    let m = max.unwrap_or(1).max(1);
    let ratio = ((c as f64 / m as f64).clamp(0.0, 1.0)) as f32;
    let color = if ratio > 0.5 {
        Color::Green
    } else if ratio > 0.25 {
        Color::Yellow
    } else {
        Color::Red
    };
    vec![
        Line::from(Span::styled(
            format!("{label}: {c}/{m}"),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "█".repeat((ratio * 20.0).round() as usize),
            Style::default().fg(color),
        )),
    ]
}

fn draw_vitals(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let v = &app.world.vitals;
    let mut lines: Vec<Line> = Vec::new();
    lines.extend(bar_lines("HP", v.health, v.health_max));
    lines.extend(bar_lines("Stamina", v.stamina, v.stamina_max));
    lines.extend(bar_lines("Mana", v.mana, v.mana_max));
    lines.push(Line::from(""));
    let names = ["Str", "End", "Crd", "Quk", "Foc", "Slf"];
    for (i, a) in v.attrs.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(format!("{:>3} ", names[i]), Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{a}")),
        ]));
    }
    if let Some(mv) = app.movement.as_ref() {
        lines.push(Line::from(""));
        let deg = (mv.heading.to_degrees() % 360.0).abs();
        lines.push(Line::from(Span::styled(
            format!("heading {deg:.0}°  {}", if mv.grounded { "grounded" } else { "airborne" }),
            Style::default().fg(Color::DarkGray),
        )));
    }
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Vitals"))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn radar_lines(app: &App, w: usize, h: usize) -> Vec<Line<'static>> {
    let (self_cell, self_pos) = app.self_position();
    let mut grid: Vec<Vec<Option<char>>> = vec![vec![None; w]; h];
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let scale_x = (RADAR_RANGE / (w as f32 / 2.0 - 1.0)).max(0.001);
    let scale_y = (RADAR_RANGE / (h as f32 / 2.0 - 1.0)).max(0.001);
    let self_block = (self_cell >> 16) & 0xFFFF;

    for e in app.world.entities.values() {
        if Some(e.guid) == app.self_guid || e.block() != self_block {
            continue;
        }
        let col = (cx + (e.pos.x - self_pos.x) / scale_x).round() as isize;
        let row = (cy - (e.pos.y - self_pos.y) / scale_y).round() as isize;
        if col < 0 || row < 0 || col as usize >= w || row as usize >= h {
            continue;
        }
        let ch = if e.is_player { 'p' } else { 'o' };
        // Prefer the more informative glyph.
        let slot = &mut grid[row as usize][col as usize];
        if slot.is_none() || *slot == Some('o') {
            *slot = Some(ch);
        }
    }
    if let Some(&target) = app.targets.get(app.world.target_index) {
        if let Some(e) = app.world.entities.get(&target) {
            let col = (cx + (e.pos.x - self_pos.x) / scale_x).round() as isize;
            let row = (cy - (e.pos.y - self_pos.y) / scale_y).round() as isize;
            if col >= 0 && row >= 0 && (col as usize) < w && (row as usize) < h {
                grid[row as usize][col as usize] = Some('T');
            }
        }
    }
    grid[cy as usize][cx as usize] = Some('@');

    let mut out = Vec::with_capacity(h);
    for row in grid.iter() {
        let mut spans = Vec::with_capacity(w);
        for &ch in row.iter() {
            let (c, style) = match ch {
                Some('@') => ('@', Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Some('T') => ('T', Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Some('p') => ('p', Style::default().fg(Color::Green)),
                Some('o') => ('o', Style::default().fg(Color::DarkGray)),
                _ => (' ', Style::default()),
            };
            spans.push(Span::styled(c.to_string(), style));
        }
        out.push(Line::from(spans));
    }
    out
}

fn draw_radar(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let (cell, _) = app.self_position();
    let block = (cell >> 16) & 0xFFFF;
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    if inner_w < 5 || inner_h < 3 {
        return;
    }
    let lines = radar_lines(app, inner_w.min(60), inner_h.min(15));
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Radar  N↑  block {block:04X}")),
    );
    f.render_widget(p, area);
}

fn draw_chat(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    // `chat.lines()` is newest-first; take the last `inner_h` (oldest of those) and flip.
    let mut visible: Vec<&crate::world::ChatLine> = app.world.chat.lines().take(inner_h.max(1)).collect();
    visible.reverse();
    let mut out: Vec<Line> = Vec::with_capacity(visible.len());
    for line in visible.iter() {
        let (text_color, sender_color) = style_for_chat(line.style);
        out.push(Line::from(vec![
            Span::styled(
                format!("[{}]", line.sender),
                Style::default().fg(sender_color),
            ),
            Span::styled(format!(" {}", line.text), Style::default().fg(text_color)),
        ]));
    }
    let p = Paragraph::new(out)
        .block(Block::default().borders(Borders::ALL).title("Chat"))
        .wrap(Wrap { trim: true });
    f.render_widget(p, area);
}

fn draw_targets(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if let Some(&target) = app.targets.get(app.world.target_index) {
        let (cell, pos) = app.self_position();
        let e = app.world.entities.get(&target);
        let name = e.and_then(|e| e.name.clone()).unwrap_or_else(|| format!("#{target}"));
        lines.push(Line::from(Span::styled(
            format!("Target: {name}"),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
        if let Some(e) = e {
            if let (Some(hp), Some(max)) = (e.health, e.health_max) {
                lines.extend(bar_lines("HP", Some(hp), Some(max)));
            }
            let d = ((e.pos.x - pos.x).powi(2) + (e.pos.y - pos.y).powi(2)).sqrt();
            lines.push(Line::from(Span::styled(
                format!("distance {d:.1}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!("block {}", (e.map(|e| e.cell).unwrap_or(cell) >> 16) & 0xFFFF), Style::default().fg(Color::DarkGray))));
    } else {
        lines.push(Line::from(Span::styled(
            "No target (Tab to select).",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(""));
    for (i, &g) in app.targets.iter().enumerate() {
        let name = app
            .world
            .entities
            .get(&g)
            .and_then(|e| e.name.clone())
            .unwrap_or_else(|| format!("#{g}"));
        let mark = if i == app.world.target_index { ">" } else { " " };
        lines.push(Line::from(Span::styled(
            format!("{mark} {name}"),
            Style::default().fg(if i == app.world.target_index { Color::Yellow } else { Color::White }),
        )));
    }
    if app.targets.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no entities in range)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Target"))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

// ==================================================================== overlays

fn draw_center_box(f: &mut Frame, title: &str, lines: &[&str], title_color: Color) {
    let area = f.area();
    let width = lines.iter().map(|l| l.len()).max().unwrap_or(title.len()).min(40) as u16 + 4;
    let height = (lines.len() + 1) as u16 + 2;
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let rect = ratatui::layout::Rect { x: area.x + x, y: area.y + y, width, height };
    let content: Vec<Line> = lines.iter().map(|l| Line::from(Span::raw(l.to_string()))).collect();
    let p = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(title.to_string(), Style::default().fg(title_color)))
            .style(Style::default().bg(Color::Black)),
    );
    f.render_widget(p, rect);
    f.set_cursor_position((rect.x + 1, rect.y + 1));
}
