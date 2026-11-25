//! Navigation theme control.

use eframe::egui;

use crate::gui::{theme, widgets, App};
use crate::ops::Cmd;

pub(super) fn theme_toggle(app: &mut App, ui: &mut egui::Ui) {
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

#[cfg(test)]
mod tests {
    use super::theme_toggle_button;
    use crate::gui::theme;
    use eframe::egui;

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
