//! Installation management, footer, and related dialogs.

use eframe::egui;

use super::install_status::{install_name, patch_label, scanning, status_color};
use crate::gui::{theme, widgets, App};
use crate::ops::{Cap, Cmd, InstallRow, State};

fn strip_frame(colors: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(colors.chrome)
        .inner_margin(egui::Margin::symmetric(16, 5))
}

pub(super) fn footer_panel(colors: &theme::Palette) -> egui::Panel {
    egui::Panel::bottom("main-footer")
        .exact_size(44.0)
        .frame(strip_frame(colors))
        .show_separator_line(true)
}

pub(super) fn installs(app: &mut App, ui: &mut egui::Ui) {
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
    if widgets::card(ui, |ui| {
        widgets::switch_row(ui, &mut on, enabled, title, |ui| {
            section_title(ui, title);
        })
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
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let label = ui.label(egui::RichText::new(install_name(row)).strong());
            if let Some(path) = &row.path {
                label.on_hover_text(path.display().to_string());
                ui.label(
                    egui::RichText::new(patch_label(row.patched))
                        .size(12.0)
                        .color(status_color(row.patched, colors)),
                );
            } else {
                widgets::hint(ui, "Не найдено");
            }
        });
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
        });
    });
}

pub(super) fn deactivation_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
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

pub(super) fn deactivation_dialog(app: &mut App, ctx: &egui::Context) {
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
pub(in crate::gui) fn path_dialog(app: &mut App, ctx: &egui::Context) {
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
        let cleaned = crate::installs::clean_input_path(&text);
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
mod tests {
    use super::{deactivation_button, footer_panel};
    use crate::gui::theme;
    use eframe::egui;

    #[test]
    fn deactivation_stays_in_the_footer_while_the_page_scrolls() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut previous = None;
        for offset in [0.0, 500.0] {
            let size = egui::vec2(crate::gui::WIN_W, 350.0);
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
}
