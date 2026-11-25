//! Controls and Lucide glyphs shared by the native screens.

use eframe::egui::{self, CornerRadius, Sense, Stroke};

use super::theme;

// Glyphs from lucide-static 0.468.0, subset to these code points.
pub const POWER: &str = "\u{e143}";
pub const COPY: &str = "\u{e0a2}";
pub const MOON: &str = "\u{e121}";
pub const SUN: &str = "\u{e17b}";
pub const FOLDER: &str = "\u{e246}";
pub const REPAIR: &str = "\u{e1b0}";
pub const REFRESH: &str = "\u{e148}";

/// A restrained reflected edge, shared by the graphite surfaces.
fn satin_edge(ui: &egui::Ui, rect: egui::Rect, inset: f32, enabled: bool) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let alpha = match (ui.visuals().dark_mode, enabled) {
        (true, true) => 65,
        (true, false) => 25,
        (false, true) => 150,
        (false, false) => 70,
    };
    let reflection = if ui.visuals().dark_mode {
        let violet = theme::palette(ui).violet;
        egui::Color32::from_rgba_unmultiplied(violet.r(), violet.g(), violet.b(), alpha)
    } else {
        egui::Color32::from_white_alpha(alpha)
    };
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + inset, rect.top() + 1.5),
            egui::pos2(rect.right() - inset, rect.top() + 1.5),
        ],
        Stroke::new(1.0, reflection),
    );
}

pub fn icon_text(icon: &str, label: &str, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        icon,
        0.0,
        egui::TextFormat::simple(theme::icon_font(15.0), color),
    );
    if !label.is_empty() {
        job.append(
            label,
            8.0,
            egui::TextFormat::simple(egui::FontId::proportional(13.5), color),
        );
    }
    job
}

pub fn command(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    highlighted: bool,
    enabled: bool,
) -> egui::Response {
    let colors = theme::palette(ui);
    let (ink, fill, border) = if !enabled {
        (colors.muted, colors.sunken, colors.line)
    } else if highlighted {
        (colors.on_accent, colors.accent, colors.accent)
    } else {
        (colors.text, colors.sunken, colors.line)
    };
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(icon_text(icon, label, ink))
            .fill(fill)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(theme::RADIUS_SMALL))
            .min_size(egui::vec2(0.0, 32.0)),
    );
    satin_edge(ui, response.rect, 7.0, enabled);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response
}

/// A compact on/off switch with a satin track and inset thumb.
///
/// Every capability in this tool is a switch: flipping it off *undoes* the
/// thing rather than hiding a menu item (the old build had separate "disable"
/// entries, which is the state living in two places). `enabled = false` draws
/// the switch dimmed and swallows clicks — used when the action needs admin and
/// the process is not elevated.
pub fn switch(ui: &mut egui::Ui, on: &mut bool, enabled: bool, label: &str) -> egui::Response {
    let size = egui::vec2(38.0, 26.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(size, sense);

    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *on, label)
    });

    if ui.is_rect_visible(rect) {
        let colors = theme::palette(ui);
        let rect = rect.shrink2(egui::vec2(3.0, 4.0));
        let how = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
        let track = if *on {
            colors
                .accent
                .gamma_multiply(if enabled { 1.0 } else { 0.4 })
        } else {
            colors.chrome
        };
        let knob = if *on && enabled {
            colors.on_accent
        } else {
            colors.muted
        };

        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(11), track);
        painter.rect_stroke(
            rect,
            CornerRadius::same(11),
            Stroke::new(
                1.0,
                if *on && enabled {
                    colors.accent_text
                } else if !*on {
                    colors.muted
                } else {
                    colors.line
                },
            ),
            egui::StrokeKind::Inside,
        );
        satin_edge(ui, rect, 9.0, enabled);
        let r = rect.height() / 2.0 - 3.0;
        let cx = egui::lerp((rect.left() + r + 3.0)..=(rect.right() - r - 3.0), how);
        painter.circle_filled(egui::pos2(cx, rect.center().y), r, knob);
        painter.circle_stroke(
            egui::pos2(cx, rect.center().y),
            r,
            Stroke::new(0.75, colors.line),
        );
    }

    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// The switch target and the gap separating it from the row label.
pub const SWITCH_COLUMN: f32 = 46.0;

/// A row of text with a switch pinned to its right.
///
/// The text column is *allocated first, at a fixed width* rather than being left
/// to take what it wants. A wrapped label given the whole row claims all of it,
/// and the switch is then laid out past the right edge — drawn, but under the
/// text or off the card entirely. Every row that pairs prose with a switch has
/// to go through here for that reason; doing it by hand is how one row gets
/// missed.
///
/// Returns whether the switch was flipped.
pub fn switch_row(
    ui: &mut egui::Ui,
    on: &mut bool,
    enabled: bool,
    label: &str,
    text: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let text_w = (ui.available_width() - SWITCH_COLUMN).max(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(text_w, 0.0),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                ui.set_max_width(text_w);
                text(ui);
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = switch(ui, on, enabled, label).changed();
        });
    });
    changed
}

/// A titled block. Everything on the main screen lives in one of these.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let shown = egui::Frame::new()
        .fill(theme::palette(ui).card)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::same(14))
        .stroke(Stroke::new(1.0, theme::palette(ui).line))
        .show(ui, |ui| {
            // Cards line up only if they are all the same width; sized to their
            // contents, a short one sits visibly narrower than its neighbour.
            ui.set_min_width(ui.available_width());
            add(ui)
        });
    satin_edge(ui, shown.response.rect, f32::from(theme::RADIUS), true);
    shown.inner
}

/// The one filled button per screen — the action the user came to press.
pub fn primary(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let colors = theme::palette(ui);
    let btn = egui::Button::new(
        egui::RichText::new(text)
            .color(if enabled {
                theme::palette(ui).on_accent
            } else {
                theme::palette(ui).muted
            })
            .size(15.0),
    )
    .fill(if enabled {
        colors.accent
    } else {
        colors.sunken
    })
    .stroke(Stroke::new(
        1.0,
        if enabled {
            colors.accent_text
        } else {
            colors.line
        },
    ))
    .corner_radius(CornerRadius::same(theme::RADIUS_SMALL))
    .min_size(egui::vec2(0.0, 34.0));

    let response = ui.add_enabled(enabled, btn);
    satin_edge(ui, response.rect, 7.0, enabled);
    response
}

/// A quiet outlined button: everything that is not *the* action.
pub fn ghost(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let btn = egui::Button::new(egui::RichText::new(text).color(theme::palette(ui).text))
        .fill(theme::palette(ui).sunken)
        .stroke(Stroke::new(1.0, theme::palette(ui).line))
        .corner_radius(CornerRadius::same(theme::RADIUS_SMALL));
    let response = ui.add(btn);
    satin_edge(ui, response.rect, 7.0, ui.is_enabled());
    response
}

/// Small grey explanatory text — the line under a switch that says what it does.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .color(theme::palette(ui).muted)
            .size(12.5),
    );
}

/// A coloured status dot, for install rows and provider rows.
pub fn dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let halo = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 35);
        ui.painter().circle_filled(rect.center(), 5.0, halo);
        ui.painter().circle_filled(rect.center(), 4.0, color);
    }
}
