// The GUI is the product; a console subsystem binary would put a black window
// on screen behind it. The CLI flags that still print (--about/--version) and
// the background modes attach to the parent console instead - see attach_console.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
// Every target is linted. Code only one platform calls carries its own
// `cfg_attr(..., allow(dead_code))` (or the module does, for the Windows-only
// DNS layer), so a symbol that is dead everywhere still shows up.

use std::env;

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
mod legacy_proxy;
mod ls_log;
#[cfg(target_os = "macos")]
mod macos;
mod net;
#[cfg(not(target_os = "windows"))]
mod netif;
mod ops;
mod patch_binary;
mod patch_ide;
mod patch_install;
mod portcheck;
mod resolvers;
mod settings;
mod stale_env;
mod status;
mod tui;
mod update;
mod utils;
mod watchdog;

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
            "about" | "license" | "licence" | "version" | "v" | "cli" | "status"
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

    // Read-only and one-shot, so before `close_others`: asking for a status
    // must not close the window the user is looking at.
    if status::requested() {
        std::process::exit(status::run());
    }

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
