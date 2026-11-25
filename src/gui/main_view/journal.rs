//! Journal rendering, clipboard text, and keyboard selection.

use eframe::egui;
use std::time::{Duration, Instant};

use crate::gui::{theme, widgets, App};
use crate::journal::format_line;
use crate::ops::Level;

pub(super) fn journal(app: &mut App, ui: &mut egui::Ui) {
    // Consume select-all/copy before egui's per-label selection sees the event.
    log_keys(app, ui);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Журнал").size(24.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::command(
                ui,
                widgets::COPY,
                "Скопировать журнал",
                false,
                !app.log.is_empty(),
            )
            .clicked()
            {
                let copied = crate::utils::set_clipboard_text(&app.log.text());
                app.main_view.log_copied = Some((Instant::now(), copied));
            }
            if let Some((at, copied)) = app.main_view.log_copied {
                let show_for = Duration::from_secs(4);
                if at.elapsed() < show_for {
                    let colors = theme::palette(ui);
                    let (text, color) = if copied {
                        ("Журнал скопирован", colors.ok)
                    } else {
                        ("Не удалось скопировать", colors.bad)
                    };
                    ui.label(egui::RichText::new(text).size(12.0).color(color));
                    ui.ctx()
                        .request_repaint_after(show_for.saturating_sub(at.elapsed()));
                }
            }
        });
    });
    ui.add_space(3.0);
    let colors = theme::palette(ui);
    egui::Frame::new()
        .fill(colors.sunken)
        .stroke(egui::Stroke::new(1.0, colors.line))
        .corner_radius(theme::RADIUS)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            egui::ScrollArea::vertical()
                .id_salt("journal-lines")
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if app.log.is_empty() {
                        widgets::hint(ui, "Записей пока нет.");
                    }
                    let colors = theme::palette(ui);
                    let fill = ui.visuals().selection.bg_fill;
                    for (level, line) in app.log.iter() {
                        let color = match level {
                            Level::Ok => colors.ok,
                            Level::Warn => colors.warn,
                            Level::Err => colors.bad,
                            Level::Step => colors.violet,
                            Level::Info => colors.muted,
                        };
                        let mut text = egui::RichText::new(format_line(*level, line))
                            .monospace()
                            .color(color)
                            .size(13.0);
                        if *level == Level::Step {
                            text = text.strong();
                        }
                        if app.log_all_selected {
                            text = text.background_color(fill);
                        }
                        ui.label(text);
                    }
                });
        });
}

/// Ctrl+A over the journal, then Ctrl+C.
///
/// Both work on a Russian layout, and that is not an accident of this code:
/// egui-winit resolves a key as `logical.or(physical)`, so with a Cyrillic
/// layout the logical key («ф», «с») maps to nothing and the *physical* A and C
/// are used instead. What this adds is the select-all, which egui has no notion
/// of across a pile of separate labels — so it is our own flag, drawn as a
/// selection behind every line and copied as one block.
fn log_keys(app: &mut App, ui: &mut egui::Ui) {
    // While a text field has the keyboard, Ctrl+A belongs to that field.
    let typing = ui.memory(|m| m.focused()).is_some();

    if !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A)) {
        app.log_all_selected = !app.log.is_empty();
    }

    if app.log_all_selected {
        // Only while our own selection is up, so a selection made with the mouse
        // is still copied by egui's label machinery rather than overwritten with
        // the whole journal.
        let copy = ui.input_mut(|i| {
            let asked = i
                .events
                .iter()
                .any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut));
            if asked {
                i.events
                    .retain(|e| !matches!(e, egui::Event::Copy | egui::Event::Cut));
            }
            asked
        });
        if copy {
            ui.ctx().copy_text(app.log.text());
        }
        // Any click, or Escape, gives the selection up — otherwise the next
        // Ctrl+C anywhere in the window would still copy the journal.
        let dismissed = ui.input(|i| i.pointer.any_pressed() || i.key_pressed(egui::Key::Escape));
        if dismissed {
            app.log_all_selected = false;
        }
    }
}
