//! The desktop GUI.
//!
//! A compact native window for patch controls and the journal.
//! Nothing here blocks — system work
//! goes to the `ops` worker and comes back as status snapshots and log lines.

mod icon;
mod main_view;
pub(crate) mod renderer;
mod theme;
mod widgets;

use eframe::egui;
use std::sync::mpsc::{channel, Receiver};

use crate::journal::Journal;
use crate::ops::{self, Cmd, Event, Status, Worker};
use crate::settings::Settings;
use crate::update::{self, ReleaseInfo};

const WIN_W: f32 = 680.0;
const WIN_H: f32 = 435.0;

#[derive(Debug, PartialEq, Eq)]
enum Screen {
    Main,
    Deactivated,
}

pub struct App {
    screen: Screen,

    /// Raised once a newer release is on GitHub and retained across pages.
    update: Option<ReleaseInfo>,
    update_rx: Receiver<ReleaseInfo>,

    worker: Worker,
    events: Receiver<Event>,
    status: Option<Status>,

    log: Journal,
    /// Ctrl+A over the journal. Our own, because egui's label selection is per
    /// galley and the journal is one label per line.
    log_all_selected: bool,
    busy: Option<String>,
    main_view: main_view::ViewState,
    path_dialog: Option<String>,
    path_dialog_error: Option<String>,
    /// Frames drawn so far, up to the few it takes to call the renderer proven
    /// (`renderer::confirm`).
    frames: u8,
    theme: crate::settings::ThemeMode,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            renderer::watch_device(&rs.device);
        }

        let (up_tx, update_rx) = channel();
        let up_ctx = cc.egui_ctx.clone();
        update::spawn_watch(up_tx, Box::new(move || up_ctx.request_repaint()));

        // The worker wakes the UI itself: egui sleeps until something asks it to
        // repaint, so an event that only lands in a channel is an event the user
        // never sees.
        let (ev_tx, events) = channel();
        let ctx = cc.egui_ctx.clone();
        let worker = ops::spawn(ev_tx, Box::new(move || ctx.request_repaint()));

        // The worker owns settings changes; the GUI reads the saved theme.
        let settings = Settings::load();
        theme::set_preference(&cc.egui_ctx, settings.theme);
        worker.send(Cmd::UiReady);
        Self {
            screen: Screen::Main,
            update: None,
            update_rx,
            worker,
            events,
            status: None,
            log: Journal::default(),
            log_all_selected: false,
            busy: None,
            main_view: main_view::ViewState::default(),
            path_dialog: None,
            path_dialog_error: None,
            frames: 0,
            theme: settings.theme,
        }
    }

    fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// The update banner remains available above the main controls.
    fn update_banner(&self, ui: &mut egui::Ui) {
        let Some(rel) = &self.update else { return };
        let label = format!("⬆  Доступна новая версия — {}", rel.display_version());
        let colors = theme::palette(ui);
        let btn = egui::Button::new(egui::RichText::new(label).color(colors.on_accent))
            .fill(colors.accent)
            .corner_radius(egui::CornerRadius::same(theme::RADIUS_SMALL))
            .min_size(egui::vec2(ui.available_width(), 32.0));
        if ui.add(btn).clicked() {
            // Always /latest, never the tag URL from the API response: the point
            // is to land the user on whatever is newest when they click, not on
            // the release this process happened to see hours ago.
            crate::utils::open_url(&update::releases_latest_url());
        }
        ui.add_space(10.0);
    }

    fn drain_events(&mut self) {
        while let Ok(rel) = self.update_rx.try_recv() {
            self.update = Some(rel);
        }
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Log(level, line) => self.log.push(level, line),
                Event::Status(status) => self.status = Some(*status),
                Event::Busy(what) => self.busy = what,
                Event::Deactivated => {
                    self.busy = None;
                    self.screen = Screen::Deactivated;
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // By the third pass two frames have been presented, which is where a
        // renderer that was going to fail at the swap chain has failed. egui
        // would otherwise sleep after the first one, so it is asked for more.
        if self.frames < 3 {
            if self.frames == 1 {
                renderer::dev_break_frame();
            }
            self.frames += 1;
            ui.ctx().request_repaint();
        } else if self.frames == 3 {
            self.frames += 1;
            renderer::confirm();
        }
        self.drain_events();

        match self.screen {
            Screen::Main => {
                main_view::view(self, ui);
                main_view::path_dialog(self, &ui.ctx().clone());
            }
            Screen::Deactivated => {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(theme::palette(ui).bg)
                            .inner_margin(22),
                    )
                    .show(ui, |ui| {
                        widgets::card(ui, |ui| {
                            ui.heading("Анлокер полностью деактивирован");
                            ui.label("Патчи, обход ошибки 400 и фоновые службы удалены.");
                            if widgets::primary(ui, "Закрыть", true).clicked() {
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
            }
        }
    }
}

/// Opens the window. Returns only when the user closes it.
///
/// Renderers are tried down `renderer::CHAIN` — on Windows DirectX 12, then
/// DirectX 12 on the software rasteriser, then OpenGL — starting from whatever
/// the last start on this machine learned (`renderer::first`). An error moves
/// to the next one in this process; a panic or a crash inside a driver moves to
/// it on the next start (see `renderer`). Falling back costs nothing at runtime
/// and is the difference between "it starts on other PCs" and a support thread.
pub fn run() -> Result<(), String> {
    fn options(kind: renderer::Kind) -> eframe::NativeOptions {
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([WIN_W, WIN_H])
                // Width is fixed: the layout is built for one width, and
                // only the height is the user's to change.
                .with_min_inner_size([WIN_W, 350.0])
                .with_max_inner_size([WIN_W, 4000.0])
                .with_maximize_button(false)
                .with_title(title())
                .with_icon(icon::window_icon()),
            renderer: kind.renderer(),
            wgpu_options: kind.wgpu_options(),
            ..Default::default()
        }
    }

    renderer::install();
    let mut failures: Vec<String> = Vec::new();
    let mut kind = Some(renderer::first());
    while let Some(k) = kind {
        renderer::attempting(k);
        match eframe::run_native(
            &title(),
            options(k),
            Box::new(|cc| Ok(Box::new(App::new(cc)))),
        ) {
            Ok(()) => return Ok(()),
            // No display to open a window on at all: no X server or Wayland
            // compositor, or one this user may not use (`sudo` — "Authorization
            // required"). Not a renderer's failure, so the next one would fail
            // the same way, and winit would refuse it anyway: an event loop
            // cannot be created twice in one process ("EventLoop can't be
            // recreated", which is all the second attempt used to report).
            Err(e @ eframe::Error::WinitEventLoop(_)) => {
                renderer::forget();
                return Err(format!(
                    "нет доступа к графическому дисплею: {}",
                    without_source_location(&e.to_string())
                ));
            }
            Err(e) => failures.push(format!("{}: {e}", k.label())),
        }
        kind = k.next();
    }
    // Nothing opened, and every renderer said why rather than crashing: that
    // says more about this start (a session with no desktop) than about the
    // machine, so the next start gets the whole chain again.
    renderer::forget();
    Err(format!("не удалось открыть окно ({})", failures.join("; ")))
}

/// winit's `os error at <file>.rs:<line>: <what>` is for its own developers;
/// the user gets `<what>`.
fn without_source_location(msg: &str) -> &str {
    msg.find(".rs:")
        .and_then(|i| msg[i..].find(": ").map(|j| &msg[i + j + 2..]))
        .unwrap_or(msg)
}

fn title() -> String {
    format!("Antigravity Unlocker {}", update::current_version())
}

#[cfg(test)]
mod tests {
    use super::without_source_location;

    #[test]
    fn a_winit_error_loses_the_build_machine_path_and_keeps_what_happened() {
        assert_eq!(
            without_source_location(
                "os error at /cargo/registry/src/index.crates.io-1949cf8c6b5b557f/winit-0.30.13/src/platform_impl/linux/mod.rs:788: Failed to open connection to X server"
            ),
            "Failed to open connection to X server"
        );
        assert_eq!(without_source_location("no display"), "no display");
    }
}
