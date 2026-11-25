//! Shared palettes and typography for the native window.

use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub struct Palette {
    pub bg: Color32,
    pub chrome: Color32,
    pub card: Color32,
    pub sunken: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub violet: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub soft: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const LIGHT: Palette = Palette {
    bg: rgb(0xeef2f6),
    chrome: rgb(0xf9fbfd),
    card: rgb(0xffffff),
    sunken: rgb(0xf3f6fa),
    line: rgb(0xc8d3de),
    text: rgb(0x192534),
    muted: rgb(0x516374),
    accent: rgb(0x006879),
    violet: rgb(0x7052a5),
    accent_text: rgb(0x006575),
    on_accent: rgb(0xffffff),
    soft: rgb(0xd4ecf0),
    ok: rgb(0x257346),
    warn: rgb(0x875018),
    bad: rgb(0xa22b34),
};

const DARK: Palette = Palette {
    bg: rgb(0x0e121a),
    chrome: rgb(0x151a25),
    card: rgb(0x191f2b),
    sunken: rgb(0x101620),
    line: rgb(0x354052),
    text: rgb(0xecf3fc),
    muted: rgb(0xa7b7c9),
    accent: rgb(0x69e4ee),
    violet: rgb(0xac96f6),
    accent_text: rgb(0x7fe9f1),
    on_accent: rgb(0x10232a),
    soft: rgb(0x203b47),
    ok: rgb(0x7ae0ad),
    warn: rgb(0xf6c681),
    bad: rgb(0xff9da7),
};

pub fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &DARK
    } else {
        &LIGHT
    }
}

pub const RADIUS: u8 = 8;
pub const RADIUS_SMALL: u8 = 6;

/// Installs Segoe UI Semibold as the UI face, when the machine has it.
///
/// Loaded from the system font directory rather than embedded: Segoe UI is
/// licensed to Windows, not redistributable, so shipping the file inside the exe
/// would be a licensing problem. Reading the copy the user already has is not.
///
/// egui's own fonts stay behind it as fallbacks, so a glyph Segoe lacks still
/// renders and a machine without the file (any Linux box, a stripped Windows
/// image) simply keeps the default face instead of showing nothing.
fn install_font(ctx: &egui::Context) {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
    // Semibold first, as asked; plain Segoe UI is the near-miss worth taking
    // before falling all the way back to Ubuntu-Light.
    let candidates = ["seguisb.ttf", "segoeui.ttf"];
    let segoe = candidates.iter().find_map(|f| {
        let path = std::path::Path::new(&dir).join("Fonts").join(f);
        std::fs::read(&path).ok().map(|b| (*f, b))
    });

    let mut fonts = egui::FontDefinitions::default();
    if let Some((name, bytes)) = segoe {
        fonts.font_data.insert(
            name.to_string(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, name.to_string());
    }
    fonts.font_data.insert(
        "Lucide".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/lucide-panel.ttf"
        ))),
    );
    fonts.families.insert(
        egui::FontFamily::Name("Lucide".into()),
        vec!["Lucide".into()],
    );
    ctx.set_fonts(fonts);
}

pub fn icon_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("Lucide".into()))
}

/// Updates the context's theme preference from persisted settings or user toggle.
pub fn set_preference(ctx: &egui::Context, mode: crate::settings::ThemeMode) {
    let pref = match mode {
        crate::settings::ThemeMode::System => egui::ThemePreference::System,
        crate::settings::ThemeMode::Light => egui::ThemePreference::Light,
        crate::settings::ThemeMode::Dark => egui::ThemePreference::Dark,
    };
    ctx.set_theme(pref);
}

/// Applies the theme to a context. Called once at startup.
///
/// egui ships Ubuntu-Light and Hack as its default fonts and both carry the
/// Cyrillic block, so a Russian UI renders even when `install_font` finds
/// no matching system font.
pub fn apply(ctx: &egui::Context) {
    install_font(ctx);

    for (theme, colors, mut visuals) in [
        (egui::Theme::Dark, &DARK, egui::Visuals::dark()),
        (egui::Theme::Light, &LIGHT, egui::Visuals::light()),
    ] {
        visuals.panel_fill = colors.bg;
        visuals.window_fill = colors.card;
        visuals.window_stroke = Stroke::new(1.0, colors.line);
        visuals.extreme_bg_color = colors.sunken;
        visuals.faint_bg_color = colors.card;
        visuals.override_text_color = Some(colors.text);
        visuals.hyperlink_color = colors.accent_text;
        let radius = CornerRadius::same(RADIUS_SMALL);
        for (widget, fill, border) in [
            (
                &mut visuals.widgets.noninteractive,
                colors.card,
                colors.line,
            ),
            (&mut visuals.widgets.inactive, colors.sunken, colors.line),
            (&mut visuals.widgets.hovered, colors.card, colors.accent),
            (&mut visuals.widgets.active, colors.soft, colors.accent),
            (&mut visuals.widgets.open, colors.soft, colors.accent),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = Stroke::new(1.0, border);
            widget.corner_radius = radius;
        }
        visuals.selection.bg_fill = colors.soft;
        visuals.selection.stroke = Stroke::new(1.0, colors.accent);
        ctx.set_visuals_of(theme, visuals);
    }
    ctx.set_theme(egui::ThemePreference::System);

    ctx.all_styles_mut(|style| {
        use egui::{FontFamily::Proportional, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Heading, FontId::new(24.0, Proportional)),
            (TextStyle::Body, FontId::new(14.5, Proportional)),
            (TextStyle::Button, FontId::new(14.5, Proportional)),
            (TextStyle::Small, FontId::new(12.5, Proportional)),
            (
                TextStyle::Monospace,
                FontId::new(13.0, egui::FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.interact_size.y = 26.0;
        // Always visible and taking its own column, not floating over the
        // content: a bar that only appears on hover leaves no sign that there is
        // more below, and this window is taller than it fits.
        style.spacing.scroll.floating = false;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
    });
}
