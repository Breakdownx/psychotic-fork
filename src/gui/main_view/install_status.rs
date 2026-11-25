//! Shared installation labels and scanning state.

use eframe::egui;

use crate::gui::{theme, widgets};
use crate::ops::InstallRow;

/// Everything the verdict is made of, read off the window's own state.
pub(super) fn install_name(row: &InstallRow) -> &'static str {
    match row.label {
        "Antigravity 2.0" => "Antigravity · настольное приложение",
        name => name,
    }
}

pub(super) fn patch_label(patched: Option<bool>) -> &'static str {
    match patched {
        Some(true) => "Патч применён",
        Some(false) => "Патч не подтверждён",
        None => "Ещё не проверено",
    }
}

pub(super) fn status_color(patched: Option<bool>, colors: &theme::Palette) -> egui::Color32 {
    match patched {
        Some(true) => colors.ok,
        Some(false) => colors.warn,
        None => colors.muted,
    }
}

pub(super) fn scanning(ui: &mut egui::Ui) {
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::Spinner::new()
                    .size(16.0)
                    .color(theme::palette(ui).accent),
            );
            ui.label("Проверяем установленные приложения…");
        });
    });
}
