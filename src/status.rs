//! `--status`: what the window's deep scan shows, printed once - for a terminal,
//! a script, or a bug report.
//!
//! Nothing here patches, kills or reconfigures anything: it reads the binaries
//! and asks the service manager. The one write is macOS's App Management probe
//! (`macos::write_blocked_by_app_management`), made only next to a binary whose
//! state disagrees with the settings, and removed at once.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::patch_binary::{self, FileState};
use crate::utils::mask_path;

pub const FLAG: &str = "--status";

/// `println!` that a closed pipe (`--status | head`) does not turn into a panic:
/// the reader has what it wanted, so the rest is simply dropped.
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

pub fn requested() -> bool {
    std::env::args().skip(1).any(|a| a == FLAG)
}

/// Prints the report. The exit code says the outcome to a script: 0 when every
/// install found is fully patched, 1 when one is not, 2 when none was found.
pub fn run() -> i32 {
    let settings = crate::settings::Settings::load();
    say!("Antigravity Unlocker {}", env!("CARGO_PKG_VERSION"));
    say!();

    let mut installs: Vec<PathBuf> = settings.manual_paths.clone();
    for path in crate::installs::discover_installs_fast() {
        if !installs.contains(&path) {
            installs.push(path);
        }
    }

    let mut all_patched = true;
    if installs.is_empty() {
        say!("Установки Antigravity не найдены.");
    }
    for install in &installs {
        let state = patch_binary::inspect_install(install);
        say!(
            "{} — {}",
            crate::installs::install_label(install),
            mask_path(&install.display().to_string())
        );
        if state.is_empty() {
            say!("  бинарники Language Server / CLI не найдены");
            all_patched = false;
        }
        for (file, file_state) in &state.files {
            say!("  {:<22} {}", describe(file_state), relative(file, install));
            // A write is only wanted where the file and the settings disagree:
            // unpatched while the patch is on, or patched while it is off.
            #[cfg(target_os = "macos")]
            {
                let wanted = match file_state {
                    FileState::Unpatched => settings.client_patch || settings.auto_patch_wanted(),
                    FileState::Patched => !settings.client_patch,
                    _ => false,
                };
                if wanted && crate::macos::write_blocked_by_app_management(file) {
                    let what = if *file_state == FileState::Patched {
                        "снять патч"
                    } else {
                        "записать"
                    };
                    say!(
                        "  {:<22} macOS не даст {what}: включите {} в «Управлении приложениями»",
                        "",
                        crate::macos::app_management_grantee()
                    );
                }
            }
        }
        all_patched &= state.fully_patched();
        say!();
    }

    say!(
        "Автопатч: {}",
        if settings.auto_patch_wanted() {
            "включён"
        } else if settings.patch_declined {
            "выключен вручную"
        } else {
            "выключен"
        }
    );
    say!("Служба автопатча: {}", service_state());

    if installs.is_empty() {
        2
    } else if all_patched {
        0
    } else {
        1
    }
}

fn describe(state: &FileState) -> String {
    match state {
        FileState::Patched => "патч применён".to_string(),
        FileState::Unpatched => "не пропатчен".to_string(),
        FileState::SignatureMissing => "сигнатура не найдена".to_string(),
        FileState::Unreadable(e) => format!("не читается ({e})"),
    }
}

/// `file` as a path inside `install` when it is one, masked otherwise (the CLI
/// under `~/.gemini/bin` is an install of its own).
fn relative(file: &Path, install: &Path) -> String {
    file.strip_prefix(install)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| mask_path(&file.display().to_string()))
}

#[cfg(target_os = "macos")]
fn service_state() -> &'static str {
    match (
        crate::background::is_watchdog_enabled(),
        crate::background::is_watchdog_running(),
    ) {
        (true, true) => "установлена, работает",
        (true, false) => "установлена, не запущена",
        (false, true) => "запущена без plist (осталась от прошлого запуска)",
        (false, false) => "не установлена",
    }
}

#[cfg(not(target_os = "macos"))]
fn service_state() -> &'static str {
    if crate::background::is_watchdog_enabled() {
        "установлена"
    } else {
        "не установлена"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_inside_the_install_is_shown_relative_to_it() {
        let inst = Path::new("/Applications/Antigravity.app/Contents");
        let file = inst.join("Resources/bin/language_server");
        assert_eq!(relative(&file, inst), "Resources/bin/language_server");
    }

    #[test]
    fn every_state_reads_as_words() {
        assert_eq!(describe(&FileState::Patched), "патч применён");
        assert!(describe(&FileState::Unreadable("EPERM".into())).contains("EPERM"));
    }
}
