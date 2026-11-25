//! The window icon.
//!
//! `icon.ico` is already compiled into the exe as a Windows resource by
//! `build.rs`/winres — that is what File Explorer and the taskbar shortcut show.
//! The *window's* icon is a separate thing: winit sets it on the window class,
//! and with nothing supplied the title bar and Alt-Tab get a blank.
//!
//! The ICO's six layers are all PNG-compressed, and decoding PNG means an
//! inflate implementation — a dependency, for one 64×64 image. So the 64×64
//! layer is unpacked to raw RGBA once, at development time, and included as
//! bytes. `assets/icon_64.rgba` is 64·64·4 and is regenerated from `icon.ico`
//! whenever the icon changes; nothing decodes anything at runtime.

use eframe::egui;

const ICON_W: u32 = 64;
const ICON_H: u32 = 64;
const ICON_RGBA: &[u8; (ICON_W * ICON_H * 4) as usize] =
    include_bytes!("../../assets/icon_64.rgba");

pub fn unlocker_texture(ctx: &egui::Context) -> egui::TextureHandle {
    ctx.load_texture(
        "unlocker",
        egui::ColorImage::from_rgba_unmultiplied([ICON_W as usize, ICON_H as usize], ICON_RGBA),
        egui::TextureOptions::LINEAR,
    )
}

const ANTIGRAVITY_SIZE: usize = 48;
const ANTIGRAVITY_RGBA: &[u8] = include_bytes!("../../assets/antigravity_48.rgba");

/// The Antigravity product badge shown next to the group heading in Integration.
pub fn antigravity_image() -> Option<egui::ColorImage> {
    if ANTIGRAVITY_RGBA.len() != ANTIGRAVITY_SIZE * ANTIGRAVITY_SIZE * 4 {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [ANTIGRAVITY_SIZE, ANTIGRAVITY_SIZE],
        ANTIGRAVITY_RGBA,
    ))
}

/// The embedded array checks the RGBA size at compile time.
pub fn window_icon() -> egui::IconData {
    egui::IconData {
        rgba: ICON_RGBA.to_vec(),
        width: ICON_W,
        height: ICON_H,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_baked_icon_is_the_size_it_claims() {
        let icon = window_icon();
        assert_eq!(icon.width, ICON_W);
        assert_eq!(icon.height, ICON_H);
        assert_eq!(icon.rgba.len(), (ICON_W * ICON_H * 4) as usize);
        // A fully transparent blob would draw as nothing at all.
        assert!(
            icon.rgba.chunks_exact(4).any(|px| px[3] != 0),
            "the icon is entirely transparent"
        );
    }

    #[test]
    fn the_antigravity_icon_is_the_size_it_claims() {
        let img = antigravity_image().expect("antigravity asset must match 48x48 RGBA");
        assert_eq!(img.size, [ANTIGRAVITY_SIZE, ANTIGRAVITY_SIZE]);
        assert!(
            img.pixels.iter().any(|px| px.a() != 0),
            "the antigravity icon is entirely transparent"
        );
    }
}
