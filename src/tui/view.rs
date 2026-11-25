//! Terminal layout, drawing, and display formatting.

use std::time::Duration;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::status::{self, Action, Tone};
use super::{report, App, Input, Row};
use crate::journal::format_line;
use crate::ops::{Cap, Level, State};
use crate::update;

/// Below this the layout cannot hold the card, the list and the journal.
const MIN_W: u16 = 50;
const MIN_H: u16 = 18;

// Paint both foreground and background so the same palette stays readable in
// light and dark terminal profiles. Status colours retain their usual meaning.
mod palette {
    use ratatui::style::Color;

    pub const BG: Color = Color::Rgb(24, 28, 37);
    pub const SURFACE: Color = Color::Rgb(32, 38, 49);
    pub const SELECTED: Color = Color::Rgb(44, 56, 72);
    pub const TEXT: Color = Color::Rgb(230, 235, 243);
    pub const MUTED: Color = Color::Rgb(166, 178, 197);
    pub const BORDER: Color = Color::Rgb(111, 128, 151);
    pub const CYAN: Color = Color::Rgb(105, 210, 225);
    pub const VIOLET: Color = Color::Rgb(181, 161, 241);
    pub const OK: Color = Color::Rgb(125, 211, 167);
    pub const WARN: Color = Color::Rgb(239, 198, 120);
    pub const ERROR: Color = Color::Rgb(245, 143, 151);
}

impl App {
    /// What the selected row does, for the line under the list.
    fn row_hint(&self, row: Row) -> String {
        match row {
            Row::Action(a) => format!("Enter — {}.", a.label()),
            Row::Cap(cap) => status::switch_text(cap).1.to_string(),
            Row::Bypass => status::BYPASS_TEXT.1.to_string(),
            Row::Install(i) => {
                let manual = self
                    .status
                    .as_ref()
                    .and_then(|s| s.installs.get(i))
                    .is_some_and(|r| r.manual);
                let full = self.install_path(i).unwrap_or_default();
                let mut s = "Enter — указать путь вручную".to_string();
                if manual {
                    s.push_str(", x — убрать указанный путь");
                }
                if !full.is_empty() {
                    s.push_str(&format!(". {full}"));
                }
                s
            }
            Row::Advanced => "Детали обхода: DNS-серверы и ротация.".into(),
            Row::Provider(_) => {
                "Пробел — включить или выключить сервер, +/- — выше или ниже в списке.".into()
            }
            Row::CustomDnsText => {
                "Enter — задать свой DNS: IPv4, https://… (DoH) или tls://… (DoT) через пробел."
                    .into()
            }
            Row::Report => {
                "Всё, что нужно, чтобы понять, почему Antigravity отвечает или нет — одним текстом."
                    .into()
            }
        }
    }

    fn row_item(&self, row: Row, width: u16) -> ListItem<'static> {
        let busy = self.busy.is_some();
        match row {
            Row::Action(a) => ListItem::new(Line::from(vec![
                Span::styled("▶ ", Style::new().fg(palette::CYAN)),
                Span::styled(
                    a.label().to_string(),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
            ])),
            Row::Cap(cap) => {
                let state = self
                    .status
                    .as_ref()
                    .map(|s| s.get(cap).clone())
                    .unwrap_or(State::Off);
                let indent = if matches!(cap, Cap::ClientPatch | Cap::Watchdog) {
                    ""
                } else {
                    "  "
                };
                switch_line(indent, status::switch_text(cap).0, &state, busy, width)
            }
            Row::Bypass => {
                let on = self.status.as_ref().is_some_and(|s| s.bypass_on());
                let state = if on { State::On } else { State::Off };
                switch_line("", status::BYPASS_TEXT.0, &state, busy, width)
            }
            Row::Install(i) => {
                let Some(r) = self.status.as_ref().and_then(|s| s.installs.get(i)) else {
                    return ListItem::new("");
                };
                let (dot, color, what) = match (&r.path, r.patched) {
                    (None, _) => (
                        "○",
                        palette::MUTED,
                        "не найдено — Enter: указать путь".to_string(),
                    ),
                    (Some(p), patched) => (
                        "●",
                        match patched {
                            Some(true) => palette::OK,
                            Some(false) => palette::MUTED,
                            None => palette::MUTED,
                        },
                        crate::utils::mask_path(&p.display().to_string()),
                    ),
                };
                let tail = match (&r.path, r.patched) {
                    (Some(_), Some(true)) => " · пропатчен",
                    (Some(_), Some(false)) => " · не пропатчен",
                    (Some(_), None) => " · проверяю…",
                    (None, _) => "",
                };
                ListItem::new(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(format!("{dot} "), Style::new().fg(color)),
                    Span::raw(format!("{}  ", r.label)),
                    Span::styled(what, Style::new().fg(palette::MUTED)),
                    Span::styled(tail, Style::new().fg(color)),
                ]))
            }
            Row::Advanced => ListItem::new(Line::styled(
                format!(
                    "{} Настройки для опытных",
                    if self.advanced { "▾" } else { "▸" }
                ),
                Style::new().fg(palette::VIOLET),
            )),
            Row::Provider(i) => {
                let Some(p) = self.status.as_ref().and_then(|s| s.providers.get(i)) else {
                    return ListItem::new("");
                };
                let state = if p.enabled { State::On } else { State::Off };
                let name = format!("{}. {}", i + 1, status::provider_name(&p.name));
                switch_line("      ", &name, &state, busy, width)
            }
            Row::CustomDnsText => {
                let text = self.custom_dns_text();
                let shown = if text.is_empty() {
                    "не задан".to_string()
                } else {
                    text
                };
                ListItem::new(Line::from(vec![
                    Span::raw("      Свой DNS: "),
                    Span::styled(shown, Style::new().fg(palette::MUTED)),
                ]))
            }
            Row::Report => ListItem::new(Line::from(vec![
                Span::styled("⎘ ", Style::new().fg(palette::CYAN)),
                Span::raw("Сохранить отчёт"),
            ])),
        }
    }

    // -----------------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------------

    pub(super) fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        f.render_widget(
            Block::new().style(Style::new().fg(palette::TEXT).bg(palette::BG)),
            area,
        );
        if area.width < MIN_W || area.height < MIN_H {
            f.render_widget(
                Paragraph::new(format!(
                    "Окно терминала слишком маленькое: нужно хотя бы {MIN_W}×{MIN_H} (сейчас {}×{}).",
                    area.width, area.height
                ))
                .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        self.draw_main(f, area);
    }

    fn title_line(&self) -> Line<'static> {
        let mut spans = vec![Span::styled(
            format!(" Antigravity Unlocker 2 v{} ", update::current_version()),
            Style::new().fg(palette::CYAN).add_modifier(Modifier::BOLD),
        )];
        if let Some(rel) = &self.update {
            spans.push(Span::styled(
                format!(
                    " ⬆ новая версия {}: {} ",
                    rel.display_version(),
                    update::releases_latest_url()
                ),
                Style::new()
                    .fg(palette::BG)
                    .bg(palette::VIOLET)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Line::from(spans)
    }

    fn draw_main(&mut self, f: &mut Frame, area: Rect) {
        let facts = self.facts();
        let headline = facts.as_ref().map(status::headline);

        // The card's height follows its text, so a long detail is never cut.
        let inner_w = area.width.saturating_sub(4);
        let card_h = match &headline {
            Some(h) => 2 + 1 + wrapped_height(&h.detail, inner_w) + u16::from(self.busy.is_some()),
            None => 3,
        }
        .min(area.height / 3);
        let vpn_h = if facts.as_ref().is_some_and(|f| f.relay_uses_vpn) {
            wrapped_height(status::VPN_TRAFFIC_NOTICE, area.width)
        } else {
            0
        };
        let log_h = if area.height >= 32 { 9 } else { 6 };
        let log_h = log_h.min(area.height.saturating_sub(8 + card_h + vpn_h));
        let [top, card, vpn, list, hint, journal, keys] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(card_h),
            Constraint::Length(vpn_h),
            Constraint::Min(4),
            Constraint::Length(2),
            Constraint::Length(log_h),
            Constraint::Length(1),
        ])
        .areas(area);

        f.render_widget(
            Paragraph::new(self.title_line()).style(Style::new().bg(palette::SURFACE)),
            top,
        );
        self.draw_card(f, card, headline.as_ref());
        if vpn_h != 0 {
            f.render_widget(
                Paragraph::new(status::VPN_TRAFFIC_NOTICE)
                    .style(Style::new().fg(palette::WARN))
                    .wrap(Wrap { trim: true }),
                vpn,
            );
        }

        let rows = self.rows();
        let (at, _) = self.cursor(&rows);
        self.list.select(Some(at));
        let items: Vec<ListItem> = rows.iter().map(|r| self.row_item(*r, list.width)).collect();
        let list_widget = List::new(items)
            .style(Style::new().fg(palette::TEXT).bg(palette::BG))
            .highlight_style(
                Style::new()
                    .bg(palette::SELECTED)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        f.render_stateful_widget(list_widget, list, &mut self.list);

        let selected = self.list.selected().and_then(|i| rows.get(i).copied());
        let hint_text = selected.map(|r| self.row_hint(r)).unwrap_or_default();
        f.render_widget(
            Paragraph::new(hint_text)
                .style(Style::new().fg(palette::MUTED))
                .wrap(Wrap { trim: true }),
            hint,
        );

        self.draw_journal(f, journal);
        self.draw_keys(f, keys);

        if self.input.is_some() {
            self.draw_input(f, area);
        }
    }

    fn draw_card(&self, f: &mut Frame, area: Rect, headline: Option<&status::Headline>) {
        let Some(h) = headline else {
            f.render_widget(
                Paragraph::new("Проверяю систему…")
                    .style(Style::new().fg(palette::MUTED).bg(palette::SURFACE))
                    .block(
                        Block::new()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::new().fg(palette::CYAN)),
                    ),
                area,
            );
            return;
        };
        let color = match h.tone {
            Tone::Ok => palette::OK,
            Tone::Wait => palette::CYAN,
            Tone::Action => palette::ERROR,
            Tone::Off => palette::MUTED,
        };
        let mut lines = vec![Line::from(vec![
            Span::styled("● ", Style::new().fg(color)),
            Span::styled(h.title.clone(), Style::new().add_modifier(Modifier::BOLD)),
        ])];
        lines.push(Line::raw(h.detail.clone()));
        if let Some(what) = &self.busy {
            lines.push(Line::styled(
                format!("⏳ {what}"),
                Style::new().fg(palette::WARN),
            ));
        }
        f.render_widget(
            Paragraph::new(lines)
                .style(Style::new().fg(palette::TEXT).bg(palette::SURFACE))
                .wrap(Wrap { trim: true })
                .block(
                    Block::new()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::new().fg(color)),
                ),
            area,
        );
    }

    fn draw_journal(&self, f: &mut Frame, area: Rect) {
        let block = Block::new()
            .borders(Borders::TOP)
            .title(Span::styled(" Журнал ", Style::new().fg(palette::VIOLET)))
            .border_style(Style::new().fg(palette::BORDER));
        let inner = block.inner(area);
        f.render_widget(block, area);
        // Newest at the bottom; only as many as fit once wrapped.
        let mut lines: Vec<Line> = Vec::new();
        let mut used = 0u16;
        for (level, text) in self.log.iter().rev() {
            let shown = format_line(*level, text);
            let h = wrapped_height(&shown, inner.width);
            if used + h > inner.height {
                break;
            }
            used += h;
            let color = match level {
                Level::Ok => palette::OK,
                Level::Warn => palette::WARN,
                Level::Err => palette::ERROR,
                Level::Step => palette::CYAN,
                Level::Info => palette::TEXT,
            };
            lines.push(Line::styled(shown, Style::new().fg(color)));
        }
        lines.reverse();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn draw_keys(&mut self, f: &mut Frame, area: Rect) {
        if let Some((text, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(6) {
                f.render_widget(
                    Paragraph::new(text.clone()).style(Style::new().fg(palette::CYAN)),
                    area,
                );
                return;
            }
            self.toast = None;
        }
        let offered = match self.facts().and_then(|f| status::headline(&f).action) {
            Some(Action::EnableAll) => " · a — включить всё",
            Some(Action::Repair) => " · r — починить",
            Some(Action::Verify) => " · v — пройти верификацию",
            _ => "",
        };
        let report = if report::ENABLED {
            " · o — отчёт"
        } else {
            ""
        };
        let keys = format!("↑↓ выбор · Пробел/Enter — вкл/выкл{offered}{report} · q — выход");
        f.render_widget(
            Paragraph::new(keys).style(Style::new().fg(palette::MUTED)),
            area,
        );
    }

    fn draw_input(&self, f: &mut Frame, area: Rect) {
        let (title, hint, text, error) = match &self.input {
            Some(Input::Path { text, error }) => (
                " Путь к Antigravity ",
                "Папка установки Antigravity, IDE или CLI. Можно указать вложенную — корень будет найден сам.",
                text,
                error.as_deref(),
            ),
            Some(Input::CustomDns { text }) => (
                " Свой DNS ",
                "IPv4-адреса DNS-серверов через пробел. Сервер участвует в пуле наравне с остальными, ответ сверяется с эталонным. Пустая строка — убрать.",
                text,
                None,
            ),
            None => return,
        };
        let w = area.width.saturating_sub(8).min(90);
        // Borders, the wrapped hint, a blank line, the field, the error, the keys.
        let h = 2 + wrapped_height(hint, w.saturating_sub(2)) + 3 + u16::from(error.is_some());
        let popup = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + area.height.saturating_sub(h) / 2,
            width: w,
            height: h,
        };
        let mut lines = vec![
            Line::styled(hint, Style::new().fg(palette::MUTED)),
            Line::raw(""),
            Line::from(vec![
                Span::raw("› "),
                Span::raw(text.clone()),
                Span::styled("▏", Style::new().fg(palette::CYAN)),
            ]),
        ];
        if let Some(e) = error {
            lines.push(Line::styled(e.to_string(), Style::new().fg(palette::ERROR)));
        }
        lines.push(Line::styled(
            "Enter — применить · Esc — отмена",
            Style::new().fg(palette::MUTED),
        ));
        f.render_widget(Clear, popup);
        f.render_widget(
            Paragraph::new(lines)
                .style(Style::new().fg(palette::TEXT).bg(palette::SURFACE))
                .wrap(Wrap { trim: false })
                .block(
                    Block::new()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::new().fg(palette::VIOLET))
                        .title(Span::styled(title, Style::new().fg(palette::CYAN))),
                ),
            popup,
        );
    }
}

/// One switch as a line: `[вкл ] Title — note`.
fn switch_line(
    indent: &str,
    title: &str,
    state: &State,
    busy: bool,
    width: u16,
) -> ListItem<'static> {
    let (badge, color) = match state {
        State::On => ("[вкл ]", palette::OK),
        State::Partial(_) => ("[вкл~]", palette::WARN),
        State::Off | State::OffNote(_) => ("[выкл]", palette::MUTED),
        State::Blocked(_) => ("[ -- ]", palette::WARN),
    };
    let color = if busy { palette::MUTED } else { color };
    let mut spans = vec![
        Span::raw(indent.to_string()),
        Span::styled(badge, Style::new().fg(color)),
        Span::raw(format!(" {title}")),
    ];
    if let Some(note) = state.note() {
        let used = (indent.chars().count() + 7 + title.chars().count() + 3) as u16;
        let room = width.saturating_sub(used + 2) as usize;
        if room > 8 {
            let note: String = if note.chars().count() > room {
                note.chars()
                    .take(room - 1)
                    .chain(std::iter::once('…'))
                    .collect()
            } else {
                note.to_string()
            };
            let note_color = if matches!(state, State::Blocked(_)) {
                palette::WARN
            } else {
                palette::MUTED
            };
            spans.push(Span::styled(
                format!(" — {note}"),
                Style::new().fg(note_color),
            ));
        }
    }
    ListItem::new(Line::from(spans))
}

/// Lines `text` takes when wrapped to `width` columns — close enough to what
/// `Paragraph` does to size a box around it.
fn wrapped_height(text: &str, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    text.split('\n')
        .map(|line| {
            let n = line.chars().count();
            (n.max(1) + width - 1) / width
        })
        .sum::<usize>()
        .min(usize::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_line_counts_every_row_it_takes() {
        assert_eq!(wrapped_height("", 10), 1);
        assert_eq!(wrapped_height("abcdefghij", 10), 1);
        assert_eq!(wrapped_height("abcdefghijk", 10), 2);
        assert_eq!(wrapped_height("ab\ncd", 10), 2);
        // Cyrillic is one column per letter, not one per byte.
        assert_eq!(wrapped_height("ключключкл", 10), 1);
    }
}
