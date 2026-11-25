//! Desktop views for installation status, management and the local journal.

use eframe::egui;
use std::time::Instant;

mod install_status;
mod installations;
mod journal;
mod overview;
mod theme_controls;

use super::{theme, App};
use installations::{deactivation_button, deactivation_dialog, footer_panel, installs};
use journal::journal;
use overview::overview;
use theme_controls::theme_toggle;

pub(super) use installations::path_dialog;

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
    brand_icon: Option<egui::TextureHandle>,
    log_copied: Option<(Instant, bool)>,
    confirm_deactivation: bool,
}

pub fn view(app: &mut App, ui: &mut egui::Ui) {
    let colors = theme::palette(ui);
    egui::Panel::top("navigation")
        .frame(
            egui::Frame::new()
                .fill(colors.chrome)
                .inner_margin(egui::Margin::symmetric(20, 9)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let icon = app
                    .main_view
                    .brand_icon
                    .get_or_insert_with(|| super::icon::unlocker_texture(ui.ctx()));
                ui.add(
                    egui::Image::new((icon.id(), egui::vec2(28.0, 28.0)))
                        .alt_text("Antigravity Unlocker"),
                );
                ui.label(egui::RichText::new("Antigravity").size(18.0).strong());
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(4.0);
                for (page, label) in [
                    (Page::Overview, "Обзор"),
                    (Page::Installs, "Установки"),
                    (Page::Log, "Журнал"),
                ] {
                    let selected = app.main_view.page == page;
                    let ink = if selected {
                        colors.accent_text
                    } else {
                        colors.muted
                    };
                    let tab = ui
                        .add(
                            egui::Button::selectable(
                                selected,
                                egui::RichText::new(label).color(ink),
                            )
                            .frame_when_inactive(false)
                            .min_size(egui::vec2(82.0, 32.0)),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if selected {
                        let bar = egui::Rect::from_center_size(
                            tab.rect.center_bottom() - egui::vec2(0.0, 1.0),
                            egui::vec2(24.0, 2.0),
                        );
                        ui.painter().rect_filled(bar, 1, colors.accent);
                    }
                    if tab.clicked() && !selected {
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
