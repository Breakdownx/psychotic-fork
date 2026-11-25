//! Installation status overview and its banner.

use eframe::egui;

use super::install_status::{install_name, patch_label, scanning, status_color};
use crate::gui::{theme, widgets, App};
use crate::ops::{Cmd, InstallRow};

pub(super) fn overview(app: &mut App, ui: &mut egui::Ui) {
    let colors = theme::palette(ui);
    let (banner, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 124.0),
        egui::Sense::hover(),
    );
    paint_overview_banner(ui, banner, colors);
    let inset = egui::vec2(20.0, 12.0);
    let mut controls = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                banner.min + inset,
                egui::pos2(banner.center().x + 58.0, banner.max.y - inset.y),
            ))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    controls.spacing_mut().item_spacing.y = 4.0;
    controls.heading("Состояние установок");
    if let Some(status) = &app.status {
        let mut found = status
            .installs
            .iter()
            .filter(|row| row.path.is_some())
            .peekable();
        if found.peek().is_some() {
            let total = found.clone().count();
            let patched = found.filter(|row| row.patched == Some(true)).count();
            controls.label(
                egui::RichText::new(format!("Патч подтверждён для {patched} из {total}"))
                    .size(13.0)
                    .color(colors.muted),
            );
        }
    }
    controls.add_space(3.0);
    if widgets::command(
        &mut controls,
        widgets::REFRESH,
        "Проверить",
        true,
        app.status.is_some() && !app.is_busy(),
    )
    .on_hover_text("Заново проверить состояние локальных файлов")
    .clicked()
    {
        app.busy = Some("Проверка установок".into());
        app.worker.send(Cmd::DeepRefresh);
    }
    ui.add_space(5.0);
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

/// The offset contours repeat the illuminated arch on the lock's face.
fn paint_overview_banner(ui: &egui::Ui, rect: egui::Rect, colors: &theme::Palette) {
    let painter = ui.painter_at(rect);
    let cut = 12.0;
    let corners = [
        rect.left_top(),
        rect.right_top() - egui::vec2(cut, 0.0),
        rect.right_top() + egui::vec2(0.0, cut),
        rect.right_bottom(),
        rect.left_bottom() + egui::vec2(cut, 0.0),
        rect.left_bottom() - egui::vec2(0.0, cut),
    ];
    let left = colors.card.lerp_to_gamma(colors.accent, 0.06);
    let right = colors.card.lerp_to_gamma(colors.violet, 0.16);
    let mut mesh = egui::Mesh::default();
    for point in corners {
        mesh.colored_vertex(
            point,
            left.lerp_to_gamma(right, (point.x - rect.left()) / rect.width()),
        );
    }
    for i in 1..5 {
        mesh.add_triangle(0, i, i + 1);
    }
    // A single mesh also keeps the linework inexpensive on software renderers.
    let origin = rect.right_top() + egui::vec2(-232.0, 14.0);
    for layer in 0..12 {
        let depth = layer as f32 / 11.0;
        let first = mesh.vertices.len() as u32;
        for step in 0..=48 {
            let t = step as f32 / 48.0;
            let phase = (std::f32::consts::PI * t).sin();
            let x = t * 205.0 + depth * 19.0;
            let y = 78.0 + depth * 22.0 - phase.powi(3) * (76.0 - depth * 16.0);
            let point = origin + egui::vec2(x, y);
            let tint = colors.accent.lerp_to_gamma(colors.violet, t);
            let ink = tint.gamma_multiply(if layer == 0 {
                0.94
            } else {
                0.55 - depth * 0.24
            });
            let half_width = if layer == 0 { 1.15 } else { 0.56 };
            mesh.colored_vertex(point - egui::vec2(0.0, half_width), ink);
            mesh.colored_vertex(point + egui::vec2(0.0, half_width), ink);
            if step > 0 {
                let i = first + step * 2;
                mesh.add_triangle(i - 2, i - 1, i);
                mesh.add_triangle(i, i - 1, i + 1);
            }
        }
    }
    painter.add(mesh);
    painter.add(egui::Shape::closed_line(
        corners.to_vec(),
        egui::Stroke::new(1.0, colors.line),
    ));
}

fn installation_list(ui: &mut egui::Ui, rows: &[InstallRow]) {
    let colors = theme::palette(ui);
    let mut found = rows.iter().filter(|row| row.path.is_some()).peekable();
    if found.peek().is_none() {
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(widgets::FOLDER)
                        .font(theme::icon_font(18.0))
                        .color(colors.muted),
                );
                ui.label("Установки Antigravity не найдены.");
            });
        });
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
                    egui::Frame::new()
                        .fill(colors.card.lerp_to_gamma(color, 0.09))
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                widgets::dot(ui, color);
                                ui.label(
                                    egui::RichText::new(patch_label(row.patched))
                                        .size(13.0)
                                        .color(color),
                                );
                            });
                        });
                });
            });
            ui.add_space(6.0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::installation_list;
    use crate::gui::theme;
    use crate::ops::InstallRow;
    use eframe::egui;

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
            theme::apply(&ctx);
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
}
