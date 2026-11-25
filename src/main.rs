// The GUI is the product; a console subsystem binary would put a black window
// on screen behind it. The CLI flags that still print (--about/--version) and
// the background modes attach to the parent console instead - see attach_console.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
// The Windows build is the shipping platform and is fully linted. The Linux port
// is partial (phase 1: the client patch; the NRPT/relay/systemd layer is stubbed,
// P7 phase 5), so a large Windows-only surface is legitimately dead code there.
// Silence exactly that noise on non-Windows targets, and only there, so a real
// dead symbol on Windows is still caught.
#![cfg_attr(
    not(target_os = "windows"),
    allow(dead_code, unused_imports, unused_variables, unused_mut)
)]

use std::env;
use std::fs;
use std::path::Path;

mod asar;
mod background;
mod canary;
mod dns;
mod deactivate;
mod sidecar;
mod dns_client;
mod dns_forwarder;
mod doh;
mod dot;
mod egress;
mod elevate;
mod endpoint;
mod gate;
mod gui;
mod hosts_pin;
mod installs;
mod instances;
mod journal;
#[cfg(windows)]
mod launch_guard;
mod ls_log;
#[cfg(target_os = "macos")]
mod macos;
mod net;
#[cfg(not(target_os = "windows"))]
mod netif;
mod ops;
mod patch_binary;
mod patch_ide;
mod portcheck;
mod proxy;
mod resolvers;
mod settings;
mod stale_env;
mod tui;
mod update;
mod utils;
mod watchdog;

use asar::extract_asar;
use patch_binary::patch_all_binaries;
use patch_ide::{is_new_desktop_architecture, patch_desktop, patch_extension_js, patch_ide};

/// Puts a v2.4+ install back into its pristine shape: the extracted
/// `resources/app` from an older patch is removed and `app.asar` is restored.
/// Electron prefers `resources/app` over the archive, so the directory must be
/// gone before the archive is put back.
pub fn restore_pristine_asar(resources: &Path) -> Result<(), String> {
    let app_dir = resources.join("app");
    let app_asar = resources.join("app.asar");
    let asar_bak = resources.join("app.asar.bak");

    // Only touch resources/app when it demonstrably came from an archive.
    // Antigravity IDE ships resources/app as its real, unpacked layout - with
    // neither app.asar nor a backup present there is nothing to restore, and
    // deleting the directory would destroy the install.
    if !asar_bak.exists() && !app_asar.exists() {
        return Ok(());
    }

    if app_dir.exists() {
        fs::remove_dir_all(&app_dir)
            .map_err(|e| format!("не удалось удалить resources\\app: {}", e))?;
    }
    if asar_bak.exists() && !app_asar.exists() {
        fs::rename(&asar_bak, &app_asar)
            .map_err(|e| format!("не удалось восстановить app.asar: {}", e))?;
    }
    Ok(())
}

/// One patched installation and its non-fatal cleanup warnings.
pub struct InstallOutcome {
    pub label: &'static str,
    /// Non-fatal notes — a leftover override that would not come off. The patch
    /// succeeded anyway, so these are not errors.
    pub warnings: Vec<String>,
}

pub fn process_install(install: &Path) -> Result<InstallOutcome, String> {
    // Patch all relevant binaries (Language Server / CLI).
    let bin_summary = patch_all_binaries(install);
    let mut warnings: Vec<String> = Vec::new();

    #[cfg(target_os = "macos")]
    let resources = macos::resources_dir(install);
    #[cfg(not(target_os = "macos"))]
    let resources = install.join("resources");
    let app_dir = resources.join("app");
    let app_asar = resources.join("app.asar");

    if app_asar.exists() {
        // Peek at dist/main.js straight out of the archive. On v2.4+ the shell
        // carries no auth code, so nothing is unpacked and the install stays
        // byte-identical to a fresh one.
        let is_new_arch = asar::read_asar_entry(&app_asar, "dist/main.js")
            .and_then(|b| String::from_utf8(b).ok())
            .map_or(false, |src| is_new_desktop_architecture(&src));

        if is_new_arch {
            // Clean up leftovers from a patch applied before v2.4.
            restore_pristine_asar(&resources)?;
            // The Language Server is the only thing being patched here, so if
            // it did not take there is nothing to report as success.
            if bin_summary.ok == 0 {
                return Err(binary_failure_message(&bin_summary));
            }
            return Ok(InstallOutcome {
                label: "Antigravity Desktop",
                warnings,
            });
        }

        // Older layout: unpack so the JS can be patched.
        if app_dir.exists() {
            let _ = fs::remove_dir_all(&app_dir);
        }
        if !extract_asar(&app_asar, &app_dir) {
            return Err("Ошибка получения доступа к приложению".to_string());
        }
    }

    let ide_js = app_dir.join("out").join("main.js");
    let desktop_js = app_dir.join("dist").join("main.js");

    if ide_js.exists() {
        patch_ide(install, &ide_js)?;
        if let Err(e) = patch_extension_js(install) {
            // Not reported per install: the progress line is one row wide, and
            // the extension patch is cosmetic next to the Language Server one.
            let _ = e;
        }
        // The endpoint is deliberately NOT overridden any more (2.11.0).
        //
        // Pushing the client onto `daily-cloudcode-pa` was a workaround for one
        // fact: nobody substituted `cloudcode-pa`, so the host Antigravity picks
        // for itself had no route (S9/N15). A provider that substitutes it is now
        // measured and first in the pool, so the workaround costs more than it
        // buys - it is a user-visible setting in *their* settings.json that
        // survives our revert paths only if they run one, and it pins a host
        // choice Antigravity is entitled to make differently per build or
        // account. Leave the host alone; route whichever one it asks for.
        //
        // Upgrade duty, same shape as `remove_legacy_ca` (I27): a machine
        // patched by <= 2.10.x still carries the override, and leaving it would
        // silently keep that machine on the old host. Menu 1 takes it back out.
        if let Err(e) = endpoint::remove_ide(install) {
            warnings.push(format!("Прежний оверрайд эндпоинта не снят: {}", e));
        }
        return Ok(InstallOutcome {
            label: "Antigravity IDE",
            warnings,
        });
    } else if desktop_js.exists() {
        let js_patched = patch_desktop(install, &desktop_js)?;
        if !js_patched {
            // v2.4+ unpacked by an older build of this tool: undo the unpack.
            restore_pristine_asar(&resources)?;
        }
        return Ok(InstallOutcome {
            label: "Antigravity Desktop",
            warnings,
        });
    } else if install.join("agy.exe").is_file() || install.join("agy").is_file() {
        if bin_summary.ok == 0 {
            return Err(binary_failure_message(&bin_summary));
        }
        // Not overridden any more, for the reason spelled out in the IDE arm
        // above; here the leftover is an environment variable rather than a
        // settings key, and it outlives a reinstall, so taking it back out
        // matters more, not less.
        if let Err(e) = endpoint::remove_cli() {
            warnings.push(format!("Прежний {} не снят: {}", endpoint::CLI_ENV_VAR, e));
        }
        return Ok(InstallOutcome {
            label: installs::install_label(install),
            warnings,
        });
    }

    Err("Компоненты приложения не найдены".to_string())
}

fn binary_failure_message(summary: &patch_binary::BinarySummary) -> String {
    if summary.total() == 0 {
        "Бинарник Language Server / CLI не найден в этой установке".to_string()
    } else if let Some(err) = &summary.last_error {
        err.clone()
    } else {
        "Сигнатура в Language Server не найдена — вероятно, вышла новая версия Antigravity"
            .to_string()
    }
}

/// The same removal without a word of output, for the window.
///
/// Both guards are load-bearing and neither is cosmetic (I39): nothing to remove
/// is the common case, and pulling the root from under a relay that is still an
/// older build makes every gate request die as `BadCertificate`. Returns whether
/// anything was actually taken out.
pub fn remove_legacy_ca_quiet() -> bool {
    let cert = proxy::ca_cert_path();
    if !cert.exists() {
        return false;
    }
    if background::relay_is_outdated() {
        return false;
    }
    endpoint::clear_node_ca(&cert.to_string_lossy()).ok();
    proxy::untrust_ca();
    true
}

fn main() {
    if env::args().skip(1).any(|a| {
        matches!(a.as_str(), "--proxy" | "--proxy-service" | "--dns-service")
            || (!cfg!(windows) && a == background::FORWARDER_FLAG)
    }) {
        utils::attach_parent_console();
        eprintln!("Режим службы недоступен в этой сборке.");
        std::process::exit(2);
    }
    #[cfg(windows)]
    if env::args_os().nth(1).is_some_and(|a| a == launch_guard::FLAG) {
        launch_guard::run();
        return;
    }
    if env::args().skip(1).any(|a| {
        matches!(
            a.trim_start_matches('-').to_ascii_lowercase().as_str(),
            "about" | "license" | "licence" | "version" | "v" | "cli"
        )
    }) {
        utils::attach_parent_console();
    }

    if env::args().any(|a| a == background::FORWARDER_FLAG) {
        dns_forwarder::detach_console();
        watchdog::start();
        if let Err(e) = dns_forwarder::run() {
            dns_forwarder::log_fatal(&e);
            std::process::exit(1);
        }
        return;
    }

    if env::args().any(|a| a == background::WATCHDOG_FLAG) {
        dns_forwarder::detach_console();
        watchdog::run_forever();
        return;
    }

    // Root's half of patching a root-owned install (`elevate`): started by
    // pkexec from the window, patches the one install named and exits.
    {
        let args: Vec<String> = env::args().collect();
        if let Some(i) = args.iter().position(|a| a == elevate::PATCH_ROOT_FLAG) {
            elevate::run_helper(args.get(i + 1).cloned());
        }
        if let Some(i) = args.iter().position(|a| a == elevate::UNPATCH_ROOT_FLAG) {
            elevate::run_unpatch_helper(args.get(i + 1).cloned());
        }
    }

    canary::handle_cli_flags();

    // A window of an older release left open would keep re-applying its own
    // state under this one.
    instances::close_others();

    if tui::requested() {
        run_tui(None);
        return;
    }
    // A server over SSH: a terminal, and nothing to draw a window into.
    #[cfg(not(target_os = "windows"))]
    if tui::only_terminal() {
        run_tui(None);
        return;
    }

    if let Err(e) = gui::run() {
        // No renderer opened a window. The terminal UI carries every switch the
        // window does, so it is offered instead of a dead end: on Windows in a
        // console of its own, on Linux only when started from a terminal (a
        // double-clicked launcher has none, and gets the message instead).
        let note = format!("Окно не открылось ({e}) — работаю в терминале.");
        #[cfg(target_os = "windows")]
        run_tui(Some(note));
        #[cfg(not(target_os = "windows"))]
        {
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() {
                run_tui(Some(note));
            } else {
                utils::message_box("Antigravity Unlocker", &e);
                std::process::exit(1);
            }
        }
    }
}

fn run_tui(note: Option<String>) {
    if let Err(e) = tui::run(note) {
        utils::message_box("Antigravity Unlocker", &e);
        std::process::exit(1);
    }
}
