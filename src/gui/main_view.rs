//! Desktop views for installation status, management and the local journal.

use eframe::egui;
use std::time::{Duration, Instant};

use super::{theme, widgets, App};
use crate::ops::{Cap, Cmd, InstallRow, Level, State};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Page {
    #[default]
    Overview,
    Installs,
    Log,
}

#[derive(Default)]
pub(super) struct ViewState {
    page: Page,
    log_copied: Option<(Instant, bool)>,
    confirm_deactivation: bool,
}

pub fn view(app: &mut App, ui: &mut egui::Ui) {
    let colors = theme::palette(ui);
    egui::Panel::top("navigation")
        .frame(
            egui::Frame::new()
                .fill(colors.chrome)
                .inner_margin(egui::Margin::symmetric(18, 8)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (page, label) in [
                    (Page::Overview, "Обзор"),
                    (Page::Installs, "Установки"),
                    (Page::Log, "Журнал"),
                ] {
                    let selected = app.main_view.page == page;
                    let tab =
                        egui::Button::selectable(selected, label).min_size(egui::vec2(88.0, 32.0));
                    if ui.add(tab).clicked() && !selected {
                        app.main_view.page = page;
                        app.log_all_selected = false;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme_toggle(app, ui);
                });
            });
        });
    if app.update.is_some() {
        egui::Panel::top("update").show(ui, |ui| app.update_banner(ui));
    }
    if app.main_view.page == Page::Installs {
        footer_panel(colors).show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if deactivation_button(ui, !app.is_busy()).clicked() {
                    app.main_view.confirm_deactivation = true;
                }
            });
        });
    }
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(colors.bg)
                .inner_margin(egui::Margin::symmetric(22, 18)),
        )
        .show(ui, |ui| {
            if let Some(what) = &app.busy {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(what);
                });
                ui.add_space(6.0);
            }
            match app.main_view.page {
                Page::Log => journal(app, ui),
                page => {
                    egui::ScrollArea::vertical()
                        .id_salt(("page", page))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if page == Page::Overview {
                                overview(app, ui);
                            } else {
                                installs(app, ui);
                            }
                        });
                }
            }
        });
    deactivation_dialog(app, ui.ctx());
}

fn strip_frame(colors: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(colors.chrome)
        .inner_margin(egui::Margin::symmetric(16, 5))
}

fn footer_panel(colors: &theme::Palette) -> egui::Panel {
    egui::Panel::bottom("main-footer")
        .exact_size(44.0)
        .frame(strip_frame(colors))
        .show_separator_line(true)
}

fn theme_toggle(app: &mut App, ui: &mut egui::Ui) {
    if let (_, Some(next)) = theme_toggle_button(app.theme, ui) {
        app.theme = next;
        theme::set_preference(ui.ctx(), next);
        app.worker.send(Cmd::SetTheme(next));
    }
}

fn theme_toggle_button(
    current: crate::settings::ThemeMode,
    ui: &mut egui::Ui,
) -> (egui::Response, Option<crate::settings::ThemeMode>) {
    let colors = theme::palette(ui);
    let dark = ui.visuals().dark_mode;
    let icon = if dark { widgets::SUN } else { widgets::MOON };
    let mut hint = if dark {
        "Переключить на светлую тему".to_string()
    } else {
        "Переключить на тёмную тему".to_string()
    };
    if current != crate::settings::ThemeMode::System {
        hint.push_str("\nПравый клик — системная тема");
    }
    let resp = ui
        .add(
            egui::Button::new(
                egui::RichText::new(icon)
                    .font(theme::icon_font(17.0))
                    .color(colors.muted),
            )
            .frame_when_inactive(false)
            .min_size(egui::vec2(32.0, 32.0)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint);
    let next = if resp.clicked() {
        Some(if dark {
            crate::settings::ThemeMode::Light
        } else {
            crate::settings::ThemeMode::Dark
        })
    } else if resp.secondary_clicked() && current != crate::settings::ThemeMode::System {
        Some(crate::settings::ThemeMode::System)
    } else {
        None
    };
    (resp, next)
}

/// Everything the verdict is made of, read off the window's own state.
fn install_name(row: &InstallRow) -> &'static str {
    match row.label {
        "Antigravity 2.0" => "Antigravity · настольное приложение",
        name => name,
    }
}

fn patch_label(patched: Option<bool>) -> &'static str {
    match patched {
        Some(true) => "Патч применён",
        Some(false) => "Патч не подтверждён",
        None => "Ещё не проверено",
    }
}

fn status_color(patched: Option<bool>, colors: &theme::Palette) -> egui::Color32 {
    match patched {
        Some(true) => colors.ok,
        Some(false) => colors.warn,
        None => colors.muted,
    }
}

fn overview(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.heading("Состояние установок");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::command(
                ui,
                widgets::REFRESH,
                "Проверить",
                false,
                app.status.is_some() && !app.is_busy(),
            )
            .on_hover_text("Заново проверить состояние локальных файлов")
            .clicked()
            {
                app.busy = Some("Проверка установок".into());
                app.worker.send(Cmd::DeepRefresh);
            }
        });
    });
    ui.add_space(12.0);
    let Some(status) = &app.status else {
        scanning(ui);
        return;
    };
    installation_list(ui, &status.installs);
    if status.patch_blocked {
        ui.add_space(8.0);
        widgets::hint(ui, "macOS ограничила изменение установленного приложения.");
    }
}

fn scanning(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(16.0));
        ui.label("Проверяем установленные приложения…");
    });
}

fn installation_list(ui: &mut egui::Ui, rows: &[InstallRow]) {
    let colors = theme::palette(ui);
    let mut found = rows.iter().filter(|row| row.path.is_some()).peekable();
    if found.peek().is_none() {
        ui.label("Установки Antigravity не найдены.");
        return;
    }
    widgets::card(ui, |ui| {
        for (index, row) in found.enumerate() {
            if index > 0 {
                ui.separator();
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = ui.label(egui::RichText::new(install_name(row)).size(15.0).strong());
                if let Some(path) = &row.path {
                    label.on_hover_text(path.display().to_string());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let color = status_color(row.patched, colors);
                    ui.colored_label(color, patch_label(row.patched));
                    widgets::dot(ui, color);
                });
            });
            ui.add_space(6.0);
        }
    });
}

fn installs(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Управление установками");
    ui.add_space(10.0);
    let Some(status) = &app.status else {
        scanning(ui);
        return;
    };
    let rows = status.installs.clone();
    let state = &status.client_patch;
    let mut on = state.is_on();
    let enabled = !app.is_busy() && !matches!(state, State::Blocked(_));
    let title = "Разблокировка входа";
    if widgets::switch_row(ui, &mut on, enabled, title, |ui| {
        ui.label(title);
    }) {
        app.worker.send(Cmd::Set(Cap::ClientPatch, on));
    }
    if status.patch_blocked {
        widgets::hint(ui, "macOS ограничила изменение установленного приложения.");
    }
    ui.add_space(10.0);
    widgets::card(ui, |ui| {
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                ui.separator();
            }
            ui.push_id(index, |ui| integration_row(app, ui, row));
        }
    });
}

fn section_title(ui: &mut egui::Ui, label: &str) {
    ui.label(egui::RichText::new(label).size(15.0).strong());
}

fn integration_row(app: &mut App, ui: &mut egui::Ui, row: &InstallRow) {
    let colors = theme::palette(ui);
    let enabled = !app.is_busy();
    ui.horizontal(|ui| {
        let label = ui.label(install_name(row));
        if let Some(path) = &row.path {
            label.on_hover_text(path.display().to_string());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let edit = ui.add_enabled(
                enabled,
                egui::Button::new(widgets::icon_text(widgets::FOLDER, "", colors.muted))
                    .frame_when_inactive(false)
                    .min_size(egui::vec2(32.0, 28.0)),
            );
            if edit.on_hover_text("Указать путь вручную").clicked() {
                app.path_dialog = Some(
                    row.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                );
            }
            if let Some(path) = &row.path {
                if row.manual
                    && ui
                        .add_enabled(enabled, egui::Button::new("Отключить"))
                        .clicked()
                {
                    app.worker.send(Cmd::ForgetPath(path.clone()));
                }
                if row.patched == Some(false)
                    && widgets::command(ui, widgets::REPAIR, "Пропатчить", false, enabled).clicked()
                {
                    app.worker.send(Cmd::PatchInstall(path.clone()));
                }
            }
            if row.path.is_some() {
                ui.colored_label(status_color(row.patched, colors), patch_label(row.patched));
            } else {
                ui.colored_label(colors.muted, "Не найдено");
            }
        });
    });
}

/// One log line as it is drawn — and as it is copied, so what lands on the
/// clipboard is what was on screen.
fn log_line(level: Level, line: &str) -> String {
    if level == Level::Step {
        format!("— {line}")
    } else {
        line.to_string()
    }
}

fn journal_text(log: &[(Level, String)]) -> String {
    log.iter()
        .map(|(level, line)| log_line(*level, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn journal(app: &mut App, ui: &mut egui::Ui) {
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
                let copied = crate::utils::set_clipboard_text(&journal_text(&app.log));
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
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("journal-lines")
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.log.is_empty() {
                widgets::hint(ui, "Пусто");
            }
            let colors = theme::palette(ui);
            let fill = ui.visuals().selection.bg_fill;
            for (level, line) in &app.log {
                let color = match level {
                    Level::Ok => colors.ok,
                    Level::Warn => colors.warn,
                    Level::Err => colors.bad,
                    Level::Step => colors.text,
                    Level::Info => colors.muted,
                };
                let mut text = egui::RichText::new(log_line(*level, line))
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
            ui.ctx().copy_text(journal_text(&app.log));
        }
        // Any click, or Escape, gives the selection up — otherwise the next
        // Ctrl+C anywhere in the window would still copy the journal.
        let dismissed = ui.input(|i| i.pointer.any_pressed() || i.key_pressed(egui::Key::Escape));
        if dismissed {
            app.log_all_selected = false;
        }
    }
}

fn deactivation_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let colors = theme::palette(ui);
    ui.add_enabled(
        enabled,
        egui::Button::new(widgets::icon_text(
            widgets::POWER,
            "Полная деактивация",
            colors.muted,
        ))
        .frame_when_inactive(false)
        .min_size(egui::vec2(0.0, 32.0)),
    )
    .on_hover_text("Снять патчи, выключить обход ошибки 400 и удалить фоновые службы")
}

fn deactivation_dialog(app: &mut App, ctx: &egui::Context) {
    if !app.main_view.confirm_deactivation {
        return;
    }
    let dialog = egui::Modal::new(egui::Id::new("confirm-deactivation")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        section_title(ui, "Полная деактивация");
        ui.label("Antigravity будет закрыт. Патчи будут сняты, обход выключен, фоновые службы и данные анлокера удалены.");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let confirm = widgets::command(ui, widgets::POWER, "Деактивировать", false, !app.is_busy()).clicked();
            let cancel = ui.button("Отмена").clicked();
            (confirm, cancel)
        }).inner
    });
    let (confirm, cancel) = dialog.inner;
    if confirm {
        app.busy = Some("Полная деактивация".into());
        app.worker.send(Cmd::Deactivate);
    }
    if confirm || cancel || dialog.should_close() {
        app.main_view.confirm_deactivation = false;
    }
}

/// The pencil dialog: type or paste a folder, we resolve it to an install root.
pub fn path_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(mut text) = app.path_dialog.take() else {
        return;
    };
    let mut keep_open = true;
    let mut submit = false;

    egui::Window::new("Путь к Antigravity")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            widgets::hint(
                ui,
                "Папка установки Antigravity, IDE или CLI. Можно указать вложенную — \
                 корень будет найден сам.",
            );
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(f32::INFINITY)
                    .hint_text("C:\\Users\\...\\Programs\\Antigravity"),
            );
            if let Some(err) = &app.path_dialog_error {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(err)
                        .color(theme::palette(ui).bad)
                        .size(12.5),
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if widgets::primary(ui, "Добавить", !text.trim().is_empty()).clicked() {
                    submit = true;
                }
                if widgets::ghost(ui, "Отмена").clicked() {
                    keep_open = false;
                }
            });
        });

    if submit {
        let cleaned = crate::clean_input_path(&text);
        match crate::ops::resolve_manual_path(std::path::Path::new(&cleaned)) {
            Some(root) => {
                app.worker.send(Cmd::AddPath(root));
                app.path_dialog_error = None;
                keep_open = false;
            }
            None => {
                app.path_dialog_error =
                    Some("По этому пути установка Antigravity не найдена.".into());
            }
        }
    }

    if keep_open {
        app.path_dialog = Some(text);
    } else {
        app.path_dialog_error = None;
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    #[test]
    fn deactivation_stays_in_the_footer_while_the_page_scrolls() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut previous = None;
        for offset in [0.0, 500.0] {
            let size = egui::vec2(super::super::WIN_W, 350.0);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |root| {
                let colors = theme::palette(root);
                let button = footer_panel(colors)
                    .show(root, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            deactivation_button(ui, true).rect
                        })
                        .inner
                    })
                    .inner;
                assert!(button.right() >= size.x - 20.0);
                assert!(button.bottom() >= size.y - 12.0);
                if let Some(previous) = previous {
                    assert_eq!(button, previous);
                }
                previous = Some(button);
                egui::CentralPanel::default().show(root, |ui| {
                    let scroll = egui::ScrollArea::vertical()
                        .vertical_scroll_offset(offset)
                        .show(ui, |ui| {
                            ui.label("Scrollable body");
                            ui.add_space(2000.0);
                        });
                    assert!(scroll.inner_rect.bottom() < button.top());
                    assert_eq!(scroll.state.offset.y, offset);
                });
            });
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn overview_shows_each_detected_install_and_its_own_status() {
        use std::path::PathBuf;
        let row = |label, path: Option<&str>, patched| InstallRow {
            label,
            path: path.map(PathBuf::from),
            patched,
            manual: false,
        };
        let rows = [
            row("Antigravity 2.0", Some("Desktop"), Some(false)),
            row("Antigravity CLI", Some("cli"), Some(true)),
            row("Antigravity IDE", None, None),
            row("Custom", Some("extra"), None),
        ];

        fn drawn_text(rows: &[InstallRow]) -> Vec<String> {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| installation_list(ui, rows));
            });
            let labels = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            labels
        }
        let labels = drawn_text(&rows);
        let position = |label: &str| labels.iter().position(|s| s == label).unwrap();
        assert_eq!(
            position("Antigravity · настольное приложение") + 1,
            position("Патч не подтверждён")
        );
        assert_eq!(position("Antigravity CLI") + 1, position("Патч применён"));
        assert_eq!(position("Custom") + 1, position("Ещё не проверено"));
        assert!(!labels.iter().any(|s| s == "Antigravity IDE"));
        assert!(drawn_text(&rows[2..3])
            .iter()
            .any(|s| s == "Установки Antigravity не найдены."));
    }

    #[test]
    fn journal_copy_includes_every_line_in_display_order() {
        let log = [
            (Level::Step, "First".into()),
            (Level::Info, "Middle".into()),
            (Level::Err, "Last".into()),
        ];
        assert_eq!(journal_text(&log), "— First\nMiddle\nLast");
    }

    #[test]
    fn theme_toggle_switches_between_dark_and_light_and_resets_to_system() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut mode = crate::settings::ThemeMode::System;

        let initial_dark = ctx.theme() == egui::Theme::Dark;
        let expected_next = if initial_dark {
            crate::settings::ThemeMode::Light
        } else {
            crate::settings::ThemeMode::Dark
        };

        let size = egui::vec2(200.0, 80.0);
        let mut time = 0.0;
        let run_step = |ctx: &egui::Context,
                        time: &mut f64,
                        mode: &mut crate::settings::ThemeMode,
                        events: Vec<egui::Event>| {
            *time += 0.1;
            let mut center = egui::Pos2::ZERO;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(*time),
                events,
                ..Default::default()
            };
            let out = ctx.run_ui(input, |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    let (resp, next) = theme_toggle_button(*mode, ui);
                    center = resp.rect.center();
                    if let Some(next) = next {
                        *mode = next;
                        theme::set_preference(ui.ctx(), next);
                    }
                });
            });
            out.drop_without_applying_deltas();
            center
        };

        let button_center = run_step(&ctx, &mut time, &mut mode, Vec::new());
        for pressed in [true, false] {
            run_step(
                &ctx,
                &mut time,
                &mut mode,
                vec![
                    egui::Event::PointerMoved(button_center),
                    egui::Event::PointerButton {
                        pos: button_center,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(mode, expected_next);
        assert_eq!(
            ctx.theme(),
            match expected_next {
                crate::settings::ThemeMode::Dark => egui::Theme::Dark,
                crate::settings::ThemeMode::Light => egui::Theme::Light,
                crate::settings::ThemeMode::System => unreachable!(),
            }
        );

        for pressed in [true, false] {
            run_step(
                &ctx,
                &mut time,
                &mut mode,
                vec![
                    egui::Event::PointerMoved(button_center),
                    egui::Event::PointerButton {
                        pos: button_center,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(mode, crate::settings::ThemeMode::System);
    }
}
