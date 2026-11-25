//! The switch layer: every capability the window can turn on or off, and the
//! one worker thread that does it.
//!
//! The console build expressed this as numbered menu items, each a script that
//! printed as it went. A switch needs three things a menu item never did: an
//! honest *enable*, an *undo* that is its exact mirror, and a read-only *state*
//! query so the switch can be drawn where the system actually is rather than
//! where the user last left it.
//!
//! ## What is load-bearing here
//!
//! The order of the steps, not the steps. Every sequence below is a documented
//! bug fix and is marked with the invariant it encodes:
//!
//! * **I5** — the relay comes up *before* the NRPT rules, because the rules name
//!   it as the first nameserver.
//! * **I20** — nothing may be re-patching while an unpatch runs, so both
//!   watchdogs stop first and are put back afterwards.
//! * **I39** — the legacy CA comes out *last*, and only once the relay is
//!   current; pulling it from under an old relay kills every gate request.
//! * **I45** — an undo path has no fast exit. Every step runs, each is judged on
//!   its own, failures are collected. `?` between undo steps is the bug.
//!
//! ## Threading
//!
//! One worker, one queue, for everything that touches the system. Not for
//! tidiness: `setup_dns_nrpt` is remove-then-add with no lock, `enable`/`disable`
//! both drive the same image name, and two patch runs race the same files. A
//! second worker would interleave them into a half-installed state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};

use crate::settings::Settings;
use crate::utils::is_admin;
use crate::{background, dns, endpoint, legacy_proxy, patch_binary};

mod protocol;

pub use protocol::{Cap, Cmd, Event, InstallRow, Level, ProviderRow, State, Status, VpnSeen};

pub struct Worker {
    tx: Sender<Cmd>,
}

impl Worker {
    pub fn send(&self, cmd: Cmd) {
        // A closed worker means the app is going down; there is nothing useful
        // to do about it and nothing to tell the user.
        let _ = self.tx.send(cmd);
    }
}

/// Starts the worker. `wake` is called after every event so the UI thread
/// repaints — egui sleeps until something asks it not to.
pub fn spawn(events: Sender<Event>, wake: Box<dyn Fn() + Send>) -> Worker {
    let (tx, rx) = mpsc::channel::<Cmd>();
    // The worker keeps a sender of its own, for the one job it hands to a
    // thread and gets an answer back from (`RemeasureVpn`). It means the
    // channel never disconnects, so the loop does not end when the window drops
    // its `Worker` — which costs nothing here, because the window closing is the
    // process exiting.
    let own = tx.clone();
    std::thread::Builder::new()
        .name("ops".to_string())
        .spawn(move || run_worker(rx, own, events, wake))
        .ok();
    Worker { tx }
}

// ---------------------------------------------------------------------------
// The worker loop
// ---------------------------------------------------------------------------

struct Ctx {
    events: Sender<Event>,
    /// Shared with `Busy`, which lives on this thread only - hence `Rc`.
    wake: Rc<dyn Fn() + Send>,
    settings: Settings,
    /// What the last deep pass found, per install.
    ///
    /// A shallow refresh must not throw this away: reading a 150 MB binary is
    /// what makes the answer expensive, and without a memory every cheap refresh
    /// would blank the switch back to "not checked yet" — which reads as the
    /// patch having come undone.
    patched_seen: HashMap<PathBuf, bool>,
    /// Last VPN measurement. `egress::detect` drives PowerShell and is capped at
    /// a minute, so it is taken on the deep pass and after anything that could
    /// change it — never on a plain refresh.
    vpn: Option<VpnSeen>,
    /// The last snapshot that was actually asked of the system.
    ///
    /// What makes `Scan::Settings` possible: a switch that only writes a field of
    /// `settings.json` cannot have changed a scheduled task, an NRPT rule or an
    /// environment variable, so re-asking Windows about all three is pure latency
    /// — and it was about two seconds of it per flip, which is what «слишком
    /// много времени» was.
    last: Option<Status>,
    /// The raw pair `read_dns` needs, kept from the last system scan so the DNS
    /// row can be recomputed for a new `vpn_detect` without going near PowerShell.
    dns_probe: (bool, bool),
}

/// The busy marker of one command; clears it on drop (`Ctx::busy_with`). Holds
/// its own handles rather than borrowing `Ctx`, which the command needs `&mut`.
struct Busy {
    events: Sender<Event>,
    wake: Rc<dyn Fn() + Send>,
}

impl Drop for Busy {
    fn drop(&mut self) {
        let _ = self.events.send(Event::Busy(None));
        (self.wake)();
    }
}

/// How much of the system a refresh is allowed to ask about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// Everything, including reading every Language Server binary end to end and
    /// re-measuring where the client's traffic leaves.
    Deep,
    /// Everything the system can be asked cheaply: tasks, rules, the proxy
    /// variable. Not the binaries.
    System,
    /// Nothing from the system. The last snapshot with the settings-derived
    /// fields recomputed — instant, and correct exactly for the switches that
    /// change nothing but a line in `settings.json`.
    Settings,
}

impl Ctx {
    fn log(&self, level: Level, msg: impl Into<String>) {
        let _ = self.events.send(Event::Log(level, msg.into()));
        (self.wake)();
    }
    fn busy(&self, what: Option<&str>) {
        let _ = self.events.send(Event::Busy(what.map(|s| s.to_string())));
        (self.wake)();
    }
    /// Shows `what` as the running job until the returned guard is dropped, so
    /// every way out of a command - `continue` and `return` included - clears it.
    fn busy_with(&self, what: &str) -> Busy {
        self.busy(Some(what));
        Busy {
            events: self.events.clone(),
            wake: Rc::clone(&self.wake),
        }
    }
    /// Writes `settings.json`. A failed write never stops the action that
    /// changed the setting, but it is said: otherwise the switch reads as saved
    /// and quietly comes back after a restart.
    fn save_settings(&self) {
        if !self.settings.save() {
            self.log(
                Level::Warn,
                "Не удалось сохранить настройки: изменение действует до перезапуска анлокера.",
            );
        }
    }
}

/// True while a VPN measurement is out at a thread. One at a time: the watcher
/// can ask again before the last answer is back, and two PowerShell probes
/// racing produce the same answer twice at twice the cost.
static MEASURING_VPN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn run_worker(
    rx: Receiver<Cmd>,
    own: Sender<Cmd>,
    events: Sender<Event>,
    wake: Box<dyn Fn() + Send>,
) {
    let mut ctx = Ctx {
        events,
        wake: Rc::from(wake),
        settings: Settings::load(),
        patched_seen: HashMap::new(),
        vpn: None,
        last: None,
        dns_probe: (false, false),
    };

    settle_decline(&mut ctx);
    startup_housekeeping(&mut ctx);

    // Inspect installed binaries before reporting their initial patch status.
    push_status(&mut ctx, Scan::Deep);

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Stop => return,
            Cmd::SetTheme(mode) => {
                if ctx.settings.theme != mode {
                    ctx.settings.theme = mode;
                    ctx.save_settings();
                }
            }
            Cmd::UiReady => {
                // Re-read binary patch status if auto-patch changed an install.
                if auto_patch_found(&mut ctx) {
                    push_status(&mut ctx, Scan::Deep);
                } else {
                    push_status(&mut ctx, Scan::System);
                }
                ctx.busy(None);
            }
            // Off the queue, not on it. The measurement drives PowerShell and
            // takes seconds; on the queue, a user flipping a switch in the
            // middle of one would watch it snap back until it finished — G42's
            // lesson, applied to a job nobody asked for. No `busy` marker
            // either, for the same reason: nothing the user is waiting on is
            // running.
            Cmd::RemeasureVpn => {
                use std::sync::atomic::Ordering;
                if !MEASURING_VPN.swap(true, Ordering::SeqCst) {
                    let back = own.clone();
                    let spawned = std::thread::Builder::new()
                        .name("vpn-probe".to_string())
                        .spawn(move || {
                            let vpn = measure_vpn();
                            let relay = cfg!(windows) && background::is_enabled() && background::is_running();
                            MEASURING_VPN.store(false, Ordering::SeqCst);
                            let _ = back.send(Cmd::Probed { vpn, relay });
                        });
                    if spawned.is_err() {
                        // Or the flag would stay raised and this window would
                        // never measure again.
                        MEASURING_VPN.store(false, Ordering::SeqCst);
                    }
                }
            }
            Cmd::Probed { vpn, relay } => {
                let before = ctx.vpn;
                ctx.vpn = Some(vpn);
                if before.is_some() && before != ctx.vpn {
                    if let Some(line) = vpn_change_line(vpn) {
                        ctx.log(Level::Info, line);
                    }
                }
                // The raw half of `dns_probe`, refreshed without re-asking
                // Windows about rules: the relay is the one of the two that
                // stops on its own.
                ctx.dns_probe.1 = relay;
                push_status(&mut ctx, Scan::Settings);
            }
            Cmd::DeepRefresh => {
                let _busy = ctx.busy_with("Проверка установок");
                push_status(&mut ctx, Scan::Deep);
            }
            Cmd::Set(cap, on) => {
                let _busy = ctx.busy_with(cap.title());
                apply(&mut ctx, cap, on);
                ctx.save_settings();
                push_status(&mut ctx, scan_after(cap));
                // Turning auto-patch on is asking for Antigravity to be patched;
                // waiting for the watchdog's next poll would show the switch on
                // and the install still grey.
                if cap == Cap::Watchdog && on && auto_patch_found(&mut ctx) {
                    push_status(&mut ctx, Scan::Deep);
                }
            }
            Cmd::AddPath(p) => {
                if !ctx.settings.manual_paths.contains(&p) {
                    ctx.settings.manual_paths.push(p);
                    ctx.save_settings();
                }
                #[cfg(windows)]
                if is_admin()
                    && ctx.settings.auto_patch_wanted()
                    && background::is_watchdog_enabled()
                {
                    let _ = crate::launch_guard::uninstall();
                }
                let _busy = ctx.busy_with("Проверка установок");
                push_status(&mut ctx, Scan::Deep);
                // The path the user just pointed at is an install found at last.
                if auto_patch_found(&mut ctx) {
                    push_status(&mut ctx, Scan::Deep);
                }
            }
            Cmd::PatchInstall(p) => {
                let _busy = ctx.busy_with("Патч");
                let label = crate::installs::install_label(&p);
                if !patch_found(&mut ctx, vec![(label, p)]) {
                    ctx.log(Level::Warn, format!("{}: патч не записан, см. выше", label));
                }
                push_status(&mut ctx, Scan::Deep);
            }
            Cmd::ForgetPath(p) => {
                ctx.settings.manual_paths.retain(|x| x != &p);
                ctx.save_settings();
                push_status(&mut ctx, Scan::System);
            }
            Cmd::SetCustomDns(text) => {
                set_custom_dns(&mut ctx, &text);
                ctx.save_settings();
                push_status(&mut ctx, Scan::Settings);
            }
            Cmd::EnableAll => {
                let _busy = ctx.busy_with("Включаю");
                enable_all(&mut ctx);
                ctx.save_settings();
                push_status(&mut ctx, Scan::Deep);
            }
            Cmd::Deactivate => {
                let _busy = ctx.busy_with("Полная деактивация");
                if cfg!(windows) && !is_admin() {
                    ctx.log(Level::Err, "Для полной деактивации запустите анлокер от администратора.");
                    continue;
                }
                ctx.settings.client_patch = false;
                ctx.settings.auto_patch = false;
                ctx.settings.patch_declined = true;
                ctx.settings.dns = false;
                if !ctx.settings.save() {
                    ctx.log(Level::Err, "Не удалось сохранить отключение автопатча; откат не начат.");
                    continue;
                }
                let errors = crate::deactivate::run(&ctx.settings.manual_paths);
                if errors.is_empty() {
                    let _ = ctx.events.send(Event::Deactivated);
                    (ctx.wake)();
                    return;
                }
                for error in errors {
                    ctx.log(Level::Err, error);
                }
                ctx.log(Level::Warn, "Деактивация не завершена. Устраните ошибки и повторите.");
                push_status(&mut ctx, Scan::Deep);
            }
            Cmd::Repair => {
                let _busy = ctx.busy_with("Чиню службу обхода");
                repair(&mut ctx);
                ctx.save_settings();
                push_status(&mut ctx, Scan::System);
            }
            Cmd::ReorderProviders(order) => {
                ctx.settings.provider_order = order;
                ctx.save_settings();
                ctx.log(
                    Level::Info,
                    match first_enabled_provider(&ctx.settings) {
                        Some(n) => format!("Порядок DNS изменён, первый — {}", n),
                        None => "Порядок DNS изменён.".to_string(),
                    },
                );
                // The order is ours alone — nothing on the machine changed, so
                // there is nothing on the machine to re-read.
                push_status(&mut ctx, Scan::Settings);
            }
            Cmd::SetProvider(name, on) => {
                ctx.settings.set_provider_enabled(&name, on);
                // Never let the last one go: an empty pool is a dead DNS layer
                // that still draws as installed.
                if all_providers_disabled(&ctx.settings) {
                    ctx.settings.set_provider_enabled(&name, true);
                    ctx.log(
                        Level::Warn,
                        "Нельзя отключить все DNS-серверы — хотя бы один должен остаться.",
                    );
                } else {
                    ctx.log(
                        Level::Info,
                        format!(
                            "DNS-сервер {} {}",
                            name,
                            if on {
                                "включён"
                            } else {
                                "отключён"
                            }
                        ),
                    );
                }
                ctx.save_settings();
                // The file is written and the relay re-reads it within seconds.
                // The NRPT rules name only the relay, never a provider, so there
                // is nothing on the machine to rewrite.
                push_status(&mut ctx, Scan::Settings);
            }
        }
    }
}

/// Retire the services installed by previous versions before removing the
/// environment that pointed at them. The Windows DNS task shares its old
/// executable with the embedded proxy, so a registered DNS task needs upgrading.
fn retire_proxy_installation(ctx: &mut Ctx) -> Result<(), String> {
    let registered = background::is_enabled();
    let had_proxy_env = endpoint::proxy_env_value().is_some();
    let dir = crate::dns_forwarder::log_dir();
    #[cfg(not(windows))]
    let old_service = registered || background::is_running();
    #[cfg(windows)]
    let old_service = background::relay_is_outdated()
        && (registered || (background::is_running() && relay_reporting()));
    if !old_service && !had_proxy_env && !ctx.settings.has_retired_proxy_settings
        && !dir.join("upstream.txt").exists() && !dir.join("proxy_port").exists()
    {
        return Ok(());
    }
    #[cfg(not(windows))]
    if old_service {
        background::disable()?;
        if background::is_enabled() || background::is_running() {
            return Err("Не удалось остановить прежнюю службу прокси.".into());
        }
    }
    #[cfg(windows)]
    if old_service {
        if !is_admin() {
            return Err("Для удаления прежнего прокси из службы DNS нужны права администратора.".into());
        }
        if registered && ctx.settings.dns {
            background::ensure_running()?;
        } else {
            background::disable()?;
            if ctx.settings.auto_patch_wanted() {
                background::ensure_installed()?;
                background::enable_watchdog()?;
            }
        }
    }
    endpoint::remove_proxy(&legacy_proxy::proxy_url(), &legacy_proxy::ca_cert_path().to_string_lossy())?;
    if had_proxy_env {
        ctx.log(Level::Info, "Перезапустите открытые клиенты Antigravity, чтобы они забыли прежний адрес прокси.");
    }
    for name in ["upstream.txt", "proxy_port"] {
        if let Err(error) = std::fs::remove_file(dir.join(name)) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(format!("Не удалось удалить прежний файл {name}: {error}"));
            }
        }
    }
    if ctx.settings.has_retired_proxy_settings {
        if ctx.settings.auto_patch_wanted() && background::is_watchdog_enabled() {
            #[cfg(windows)]
            {
                if !is_admin() {
                    return Err("Для обновления прежней службы автопатча нужны права администратора.".into());
                }
                background::ensure_installed()?;
            }
            background::enable_watchdog()?;
        }
        if !ctx.settings.save() {
            return Err("Не удалось удалить прежние настройки прокси.".into());
        }
        ctx.settings.has_retired_proxy_settings = false;
    }
    Ok(())
}

/// The one thing that runs on its own, before the user has touched anything.
///
/// `refresh_pinned_hosts` exists because the host routes that keep our queries
/// off a VPN survive a reboot only while the network stays the same. It is also
/// the one call that can turn a switch off by itself: with the client measured
/// inside a tunnel it strips the rules outright (G29). In a console that was
/// invisible and harmless; in a window it reads as the tool spontaneously
/// disabling itself, so the flip is watched for and explained.
fn startup_housekeeping(ctx: &mut Ctx) {
    if let Err(error) = retire_proxy_installation(ctx) {
        ctx.log(Level::Warn, error);
    }
    #[cfg(windows)]
    let _ = crate::launch_guard::uninstall();
    if !cfg!(target_os = "windows") || !is_admin() {
        return;
    }
    #[cfg(windows)]
    match crate::sidecar::reap_orphans() {
        Ok(n) if n > 0 => ctx.log(Level::Info, format!("Закрыто осиротевших процессов Antigravity: {n}")),
        Ok(_) => {},
        Err(e) => ctx.log(Level::Warn, format!("Не удалось убрать фоновые процессы: {e}")),
    }
    let before = dns::is_nrpt_applied();
    dns::refresh_pinned_hosts();
    dns::invalidate_cache();
    if before && !dns::is_nrpt_applied() {
        ctx.log(
            Level::Warn,
            "Antigravity идёт через активный VPN — правила DNS сняты, чтобы не перебивать \
             ваш туннель. Выключите VPN и включите «Обход через DNS» заново.",
        );
    }
}

/// Whether the provider can be asked at all: everything can, except the user's
/// own slot while it is empty.
fn provider_available(s: &Settings, name: &str) -> bool {
    name != crate::resolvers::CUSTOM_NAME || !s.custom_dns.trim().is_empty()
}

/// What the window draws: switched on **and** there is something to ask.
fn provider_on(s: &Settings, name: &str) -> bool {
    s.provider_enabled(name) && provider_available(s, name)
}

fn all_providers_disabled(s: &Settings) -> bool {
    crate::resolvers::provider_names()
        .iter()
        .all(|n| !provider_on(s, n))
}

/// The one that answers when rotation is off: first in pool order that the user
/// has not switched off. Pool order is by measured substitution, so its head is
/// the one most likely to still be substituting.
fn first_enabled_provider(s: &Settings) -> Option<&'static str> {
    crate::resolvers::ordered_provider_names()
        .into_iter()
        .find(|n| provider_on(s, n))
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// How much of the system has to be re-read after `cap` was flipped.
///
/// The rule is "what could this switch have changed", not "how important is it":
/// System capabilities re-read service and environment state; the client patch
/// additionally requires inspecting the binaries.
fn scan_after(cap: Cap) -> Scan {
    match cap {
        // The one action that changes what is *inside* the binaries, so the one
        // that has to pay for re-reading them.
        Cap::ClientPatch => Scan::Deep,
        Cap::Watchdog | Cap::Dns | Cap::DnsRotation => {
            Scan::System
        }
    }
}

fn push_status(ctx: &mut Ctx, scan: Scan) {
    // The measurement answers "do the client's own sockets leave through a
    // tunnel", which no switch in this window changes — only what we then do
    // about it. So it is taken on the deep pass and when it has never been
    // taken, and a settings flip inherits it rather than paying a PowerShell
    // round trip to be told the same thing.
    if scan == Scan::Deep || ctx.vpn.is_none() {
        let vpn = measure_vpn();
        ctx.vpn = Some(vpn);
    }

    let status = match scan {
        Scan::Settings => match ctx.last.clone() {
            Some(prev) => settings_only_status(ctx, prev),
            // Nothing to reuse yet (a flip before the first snapshot landed):
            // ask properly rather than invent a snapshot.
            None => read_status(ctx, false),
        },
        Scan::System => read_status(ctx, false),
        Scan::Deep => read_status(ctx, true),
    };

    if scan == Scan::Deep {
        ctx.patched_seen = status
            .installs
            .iter()
            .filter_map(|r| match (&r.path, r.patched) {
                (Some(p), Some(v)) => Some((p.clone(), v)),
                _ => None,
            })
            .collect();
    }
    ctx.last = Some(status.clone());
    let _ = ctx.events.send(Event::Status(Box::new(status)));
    (ctx.wake)();
}

/// The last snapshot with every settings-derived field recomputed, and nothing
/// asked of the machine.
///
/// The DNS row is included because it is settings-derived *in part*: whether a
/// measured tunnel blocks it depends on `vpn_detect`. It is rebuilt from the raw
/// pair the last system scan kept, so a settings-only flip updates the row
/// it governs without going near a scheduled task.
fn settings_only_status(ctx: &Ctx, prev: Status) -> Status {
    let (rules, relay) = ctx.dns_probe;
    Status {
        dns: dns_state(
            prev.admin,
            rules,
            relay,
            ctx.vpn,
            ctx.settings.vpn_detect,
            ctx.settings.dns,
        ),
        // Not `..prev`: `Cmd::Probed` refreshes this behind the window's back,
        // and a row carried forward from the last system scan would say a relay
        // that died an hour ago is still running.
        relay_running: relay,
        rules,
        vpn: ctx.vpn,
        ..prev
    }
    .with_settings_switches(&ctx.settings)
}

impl Status {
    /// The settings switches and the provider list they order. Written once so
    /// `read_status` and `settings_only_status` cannot drift apart — that would produce
    /// a switch that answers differently depending on which path drew it.
    fn with_settings_switches(mut self, s: &Settings) -> Self {
        self.custom_dns_text = s.custom_dns.clone();
        self.dns_rotation = if s.rotate_providers {
            State::On
        } else {
            // Naming the one that will answer is the whole point of switching
            // rotation off, so the switch says which it is rather than leaving
            // the user to count down the list.
            State::OffNote(match first_enabled_provider(s) {
                Some(name) => format!("используется только {}", name),
                None => "ни один сервер не включён".to_string(),
            })
        };
        self.providers = crate::resolvers::ordered_provider_names()
            .iter()
            .map(|n| ProviderRow {
                name: (*n).to_string(),
                enabled: provider_on(s, n),
                available: provider_available(s, n),
            })
            .collect();
        self
    }
}

/// The switch note when a macOS `.app` install cannot be patched because macOS
/// App Management refuses the write. Short on purpose — it sits under the
/// «Разблокировка входа» switch at 12 px; the status card carries the full
/// steps.
const APP_MGMT_SWITCH_NOTE: &str =
    "macOS не даёт изменить Antigravity — включите «Управление приложениями»";

fn read_status(ctx: &mut Ctx, deep: bool) -> Status {
    let admin = is_admin();

    // Every probe below spawns a helper process — two scheduled-task queries, a
    // tasklist, up to three reads of the user environment — and they are
    // independent of one another. Run in series they added up to seconds per
    // switch; run at once the refresh costs the slowest one. They only read, so
    // there is nothing here for them to race over.
    let settings = &ctx.settings;
    let patched_seen = &ctx.patched_seen;
    let (installs, watchdog, dns_probe, relay_outdated) =
        std::thread::scope(|scope| {
            let installs = scope.spawn(move || collect_installs(settings, deep, patched_seen));
            let watchdog = scope.spawn(move || read_watchdog(admin));
            let dns_probe = scope.spawn(probe_dns);
            let relay_outdated = scope.spawn(background::relay_is_outdated);
            (
                installs.join().unwrap_or_default(),
                watchdog.join().unwrap_or(State::Off),
                dns_probe.join().unwrap_or((false, false)),
                relay_outdated.join().unwrap_or(false),
            )
        });
    ctx.dns_probe = dns_probe;

    // macOS: an install left unpatched because its `.app` bundle is
    // App-Management protected is the one case where "пропатчены не все
    // установки" is a dead end — the button that patches it fails the same way.
    // Detect it so the card and the switch can say what macOS actually wants,
    // probing only the installs that are still unpatched.
    #[cfg(target_os = "macos")]
    let patch_blocked = installs.iter().any(|r| {
        r.patched == Some(false)
            && r.path.as_deref().is_some_and(|p| {
                crate::patch_binary::binary_targets(p)
                    .iter()
                    .any(|t| crate::macos::write_blocked_by_app_management(t))
            })
    });
    #[cfg(not(target_os = "macos"))]
    let patch_blocked = false;

    let patched: Vec<bool> = installs.iter().filter_map(|i| i.patched).collect();
    let client_patch = if patched.is_empty() {
        // Not inspected yet, or nothing found. Fall back to what the user asked
        // for rather than claiming knowledge we do not have.
        if ctx.settings.client_patch {
            State::Partial("состояние ещё не проверено".into())
        } else {
            State::Off
        }
    } else if patched.iter().all(|p| *p) {
        State::On
    } else if patched.iter().any(|p| *p) {
        // Some patched, some not: still "on-ish", so the switch stays on, but the
        // note names the real blocker when there is one.
        State::Partial(if patch_blocked { APP_MGMT_SWITCH_NOTE } else { "пропатчены не все установки" }.into())
    } else if patch_blocked {
        // None patched and the write is refused: off, but with the reason, so the
        // switch is not a silent dead end.
        State::OffNote(APP_MGMT_SWITCH_NOTE.into())
    } else {
        State::Off
    };

    Status {
        admin,
        client_patch,
        watchdog,
        dns: dns_state(
            admin,
            dns_probe.0,
            dns_probe.1,
            ctx.vpn,
            ctx.settings.vpn_detect,
            ctx.settings.dns,
        ),
        custom_dns_text: ctx.settings.custom_dns.clone(),
        vpn: ctx.vpn,
        dns_rotation: State::Off,
        relay_outdated,
        relay_running: dns_probe.1,
        rules: dns_probe.0,
        providers: Vec::new(),
        installs,
        patch_blocked,
    }
    .with_settings_switches(&ctx.settings)
}

#[cfg(target_os = "windows")]
fn measure_vpn() -> VpnSeen {
    use crate::egress::ClientEgress;
    let egress = crate::egress::detect();
    let reading = crate::egress::read();
    if !egress.as_ref().is_some_and(|e| e.vpn_active) {
        return VpnSeen::None;
    }
    // One reading, both halves — the same call `egress::vpn_verdict` makes for
    // the DNS layer, so the row this colours and the decision that layer takes
    // cannot be looking at different data. What is *reported* is the
    // measurement itself, not the verdict derived from it: "stand the layer
    // down" and "the client is outside the tunnel" are different questions, and
    // reading the first as the second is what made the indicator claim a
    // measurement nobody had taken.
    match reading.client {
        ClientEgress::Tunnel => VpnSeen::CarryingClient,
        ClientEgress::Mixed => VpnSeen::PartlyCarryingClient,
        ClientEgress::Physical => VpnSeen::NotCarryingClient,
        ClientEgress::Unknown => VpnSeen::Unmeasured,
    }
}

#[cfg(not(target_os = "windows"))]
fn measure_vpn() -> VpnSeen {
    if crate::egress::detect().is_some_and(|e| e.vpn_active) {
        VpnSeen::Unmeasured
    } else {
        VpnSeen::None
    }
}

/// The one line the journal gets when the answer changes. An `Unmeasured` is
/// not a change worth a line: it says only that Antigravity stopped running.
fn vpn_change_line(now: VpnSeen) -> Option<&'static str> {
    match now {
        VpnSeen::None => Some("VPN отключён."),
        VpnSeen::CarryingClient => Some(
            "Трафик Antigravity идёт через VPN.",
        ),
        VpnSeen::NotCarryingClient => Some("Трафик Antigravity идёт мимо VPN."),
        VpnSeen::PartlyCarryingClient => {
            Some("Часть трафика Antigravity пошла через VPN, часть идёт мимо.")
        }
        VpnSeen::Unmeasured => None,
    }
}

/// Windows registers the watcher as a machine task, which takes UAC; on Linux
/// it is a user unit and anyone may have it. Without this the «Включить всё»
/// path and the patch switch skipped auto-patch for every non-root user, and the
/// row stayed off for good.
fn watchdog_allowed(admin: bool) -> bool {
    admin || !cfg!(target_os = "windows")
}

fn read_watchdog(admin: bool) -> State {
    if background::is_watchdog_enabled() {
        State::On
    } else if !admin && cfg!(target_os = "windows") {
        State::Blocked("нужны права администратора".into())
    } else {
        State::Off
    }
}

/// The two facts about the DNS layer that only the machine can answer: are our
/// NRPT rules in, and is the relay task both registered and running.
///
/// Split from the verdict below so a settings-only refresh can rebuild the row
/// from the last measurement instead of asking Windows again (`Scan::Settings`).
fn probe_dns() -> (bool, bool) {
    let rules = dns::is_nrpt_applied();
    let relay = cfg!(windows) && background::is_enabled() && background::is_running() && relay_reporting();
    (rules, relay)
}

/// Whether the relay itself is alive, and not only a process of its name.
///
/// `is_running` matches `ag_dns.exe` in the task list, and the watchdog is a
/// second process with that same image name - so a relay that died on startup
/// still read as running for as long as its watchdog lived (P54). A field
/// report printed both halves of that contradiction in one paste: «Служба:
/// запущена» over «Записи нет — служба не запущена или старая». The relay
/// writes `gate.json` from its first moment (`gate::note_started`) and on every
/// warm pass; the watchdog never writes it.
///
/// The Windows DNS service publishes this heartbeat.
fn relay_reporting() -> bool {
    !cfg!(target_os = "windows") || crate::gate::read().is_some_and(|r| !r.is_stale())
}

fn dns_state(
    admin: bool,
    rules: bool,
    relay: bool,
    vpn: Option<VpnSeen>,
    detect_on: bool,
    wanted: bool,
) -> State {
    // No VPN branch any more (D25): the layer does not stand down for a tunnel,
    // so a client measured inside one says nothing about whether the rules are
    // doing their job. The parameters stay so the callers keep one signature.
    let _ = (vpn, detect_on);
    match (rules, relay) {
        (true, true) => State::On,
        // The rules name the relay first (I5). Rules without it resolve through
        // the fallback providers only, which works but is not what was asked
        // for — and it is exactly what a crashed relay looks like.
        (true, false) => State::Partial("правила стоят, служба не запущена".into()),
        // A service that runs for the sake of a route (own proxy, exits)
        // is not a half-installed DNS layer: the DNS switch is off, and drawing
        // it as `Partial` - which reads as on - made it impossible to switch off
        // while any route kept the service up.
        (false, true) if !wanted => State::Off,
        (false, true) => State::Partial("служба работает, правил нет".into()),
        (false, false) => {
            if !admin && cfg!(target_os = "windows") {
                State::Blocked("нужны права администратора".into())
            } else {
                State::Off
            }
        }
    }
}

/// The kinds, in the order the window lists them. VS Code is the Antigravity
/// extension's backend in `~/.gemini/bin` (`install_label`).
const INSTALL_KINDS: [&str; 4] = [
    "Antigravity 2.0",
    "Antigravity IDE",
    "Antigravity CLI",
    "Antigravity для VS Code",
];

fn collect_installs(
    settings: &Settings,
    deep: bool,
    seen: &HashMap<PathBuf, bool>,
) -> Vec<InstallRow> {
    // A path the user pointed at wins over one the scan found: pointing at it is
    // what they do when the scan picked the wrong copy.
    let mut chosen: Vec<(PathBuf, bool)> = settings
        .manual_paths
        .iter()
        .map(|p| (p.clone(), true))
        .collect();
    for path in crate::installs::discover_installs_fast() {
        if !chosen.iter().any(|(p, _)| p == &path) {
            chosen.push((path, false));
        }
    }

    let mut rows: Vec<InstallRow> = Vec::new();
    for label in INSTALL_KINDS {
        let hit = chosen
            .iter()
            .find(|(p, _)| crate::installs::install_label(p) == label)
            .cloned();
        rows.push(match hit {
            Some((path, manual)) => InstallRow {
                label,
                patched: None,
                manual,
                path: Some(path),
            },
            None => InstallRow {
                label,
                patched: None,
                manual: false,
                path: None,
            },
        });
    }
    // Anything the user pointed at that does not answer to one of the three
    // names still gets a row - it is theirs, and dropping it silently would look
    // like the pencil did nothing.
    for (path, manual) in chosen {
        if !manual || rows.iter().any(|r| r.path.as_ref() == Some(&path)) {
            continue;
        }
        rows.push(InstallRow {
            label: crate::installs::install_label(&path),
            patched: None,
            manual: true,
            path: Some(path),
        });
    }

    for row in &mut rows {
        let Some(path) = row.path.clone() else {
            continue;
        };
        if deep {
            let state = patch_binary::inspect_install(&path);
            row.patched = if state.is_empty() {
                None
            } else {
                Some(state.fully_patched())
            };
        } else {
            row.patched = seen.get(&path).copied();
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// Applying a switch
// ---------------------------------------------------------------------------

fn apply(ctx: &mut Ctx, cap: Cap, on: bool) {
    if cap.needs_admin() && !is_admin() {
        ctx.log(
            Level::Err,
            "Для операции нужны права администратора.",
        );
        return;
    }

    match (cap, on) {
        (Cap::ClientPatch, true) => {
            // From the click, unlike the line below: the user wants the patch
            // whether or not an install was found to take it, and auto-patch
            // puts it on whatever turns up later (D27).
            ctx.settings.patch_declined = false;
            // Set from the result, not from the click: a run that found no
            // install patched nothing, and persisting `true` there would leave
            // the switch reading On for ever on a machine with no Antigravity.
            ctx.settings.client_patch = enable_client_patch(ctx);
            // Switching the patch off took the watchdog task down with it. The
            // relay's own watchdog thread acts again the moment the decline is
            // gone, so without the task back the switch would read off while
            // patching went on. `set_watchdog` no longer starts the relay, so
            // this does not depend on the bypass being wanted.
            if watchdog_allowed(is_admin())
                && ctx.settings.auto_patch_wanted()
                && !background::is_watchdog_enabled()
            {
                set_watchdog(ctx, true);
            }
        }
        (Cap::ClientPatch, false) => {
            ctx.settings.client_patch = false;
            // The one wish auto-patch has to respect (G38). Saved before
            // anything is unpatched, so a watchdog reading the file mid-run
            // already sees it.
            ctx.settings.patch_declined = true;
            if !ctx.settings.save() {
                ctx.log(
                    Level::Warn,
                    "Настройки не сохранились (файл занят?) — автопатч может вернуть патч. Выключите и «Автопатч».",
                );
            }
            disable_client_patch(ctx);
        }
        (Cap::Watchdog, on) => {
            ctx.settings.auto_patch = on;
            // Auto-patch switched on is the newer wish, and it means "keep
            // Antigravity patched" - a patch declined earlier would otherwise
            // leave the switch on and the watchdog doing nothing.
            if on {
                ctx.settings.patch_declined = false;
            }
            set_watchdog(ctx, on);
        }
        (Cap::Dns, _) if !cfg!(windows) => return,
        (Cap::Dns, on) => {
            ctx.settings.dns = on;
            if on {
                enable_dns(ctx)
            } else {
                disable_dns(ctx)
            }
        }
        (Cap::DnsRotation, on) => {
            ctx.settings.rotate_providers = on;
            ctx.save_settings();
            reapply_dns_rules(ctx);
            ctx.log(
                Level::Ok,
                if on {
                    "Ротация включена — запрос идёт ко всем включённым серверам, ответ сверяется с эталонным."
                        .to_string()
                } else {
                    match first_enabled_provider(&ctx.settings) {
                        Some(name) => format!(
                            "Ротация выключена — используется только {}. Запасных серверов не будет.",
                            name
                        ),
                        None => "Ротация выключена, но все серверы отключены.".to_string(),
                    }
                },
            );
        }
    }
}

// --- the two buttons --------------------------------------------------------

/// «Включить всё». Each part only if it is not on already, and in the order the
/// switches themselves need: the patch first (it closes Antigravity, so nothing
/// is holding a file), then the Windows DNS relay (the rules name it), and auto-patch last,
/// because it runs out of the relay's install directory.
fn enable_all(ctx: &mut Ctx) {
    let admin = is_admin() || !cfg!(target_os = "windows");
    // "Everything" includes the patch on installs that do not exist yet.
    ctx.settings.patch_declined = false;
    // Only when it is not already on everywhere: patching closes Antigravity,
    // and closing it for a no-op is exactly the kind of surprise this button
    // must not have.
    if !patch_is_on(ctx) {
        apply(ctx, Cap::ClientPatch, true);
    }
    let needs_dns = cfg!(windows)
        && !(dns::is_nrpt_applied() && background::is_running() && !background::relay_is_outdated());
    if needs_dns {
        if !admin {
            // Said only when it is actually missing: the card offers this button
            // for a patch that is off too, with the bypass already running.
            ctx.log(
                Level::Warn,
                "Обход ошибки 400 не включён: нужны права администратора.",
            );
            return;
        }
        apply(ctx, Cap::Dns, true);
    }
    // Whether or not the patch found anything to patch: auto-patch is what
    // patches the Antigravity installed after this click (D27).
    if watchdog_allowed(admin) && !background::is_watchdog_enabled() {
        apply(ctx, Cap::Watchdog, true);
    }
    ctx.log(
        Level::Ok,
        "Готово. Откройте Antigravity и напишите что-нибудь в чат — здесь появится подтверждение, что модель ответила.",
    );
}

fn patch_is_on(ctx: &Ctx) -> bool {
    ctx.last
        .as_ref()
        .is_some_and(|s| s.client_patch == State::On)
}

/// «Починить». The service is the part that can stop on its own - a crash the
/// task's restart policy gave up on, a relay older than this build, an antivirus
/// that ate the copy in `%ProgramData%` - and every one of those is fixed by
/// installing it again and putting its rules and variable back.
fn repair(ctx: &mut Ctx) {
    if cfg!(target_os = "windows") && !is_admin() {
        ctx.log(
            Level::Err,
            "Для операции нужны права администратора.",
        );
        return;
    }
    if !cfg!(windows) {
        return;
    }
    ctx.settings.dns = true;
    // `ensure_running` reinstalls only a copy that differs from this build;
    // a relay that is merely stopped or outdated needs the full `enable`.
    if let Err(e) = background::enable() {
        ctx.log(Level::Err, format!("Служба обхода не запустилась: {}", e));
        return;
    }
    ctx.log(Level::Ok, "Служба обхода переустановлена и запущена.");
    enable_dns(ctx);
}

// --- client patch ----------------------------------------------------------

/// Settles `patch_declined` for a settings file an older build wrote (D27).
///
/// Before D27 a patch switched off by hand left two traces: `client_patch`
/// false, and the standalone watchdog task removed (`disable_client_patch`).
/// `parse` could only read the first and so took every patch-off file as
/// declined. The task is the second trace, and only this side can ask for it:
/// with it still registered the user saw auto-patch *on*, and it may act -
/// which is the owner's own machine (G55). Written back at once, so the relay
/// reads the settled answer and this runs once per machine.
fn settle_decline(ctx: &mut Ctx) {
    if !ctx.settings.decline_unrecorded {
        return;
    }
    ctx.settings.patch_declined = !ctx.settings.client_patch && !background::is_watchdog_enabled();
    ctx.settings.decline_unrecorded = false;
    ctx.save_settings();
}

/// Auto-patch from the window (D27): every install the last deep scan found
/// unpatched is patched now, without a click, while auto-patch is on and the
/// patch was not switched off by hand. True when something was written, so the
/// caller knows the dots need a deep re-read.
///
/// The watchdog's own primitive, not `enable_client_patch`: that one closes
/// every Antigravity process up front, which is right for a button the user
/// pressed and wrong for something nobody asked for this second. This one
/// closes a process only when a binary it holds cannot be written otherwise -
/// and Desktop restarts its language server by itself and reloads the window,
/// which is what brings a black window back to life.
fn auto_patch_found(ctx: &mut Ctx) -> bool {
    // Only while the switch is drawn on («при включенной опции»). Its state is
    // the standalone task, which a user without admin rights cannot have and
    // which `disable_dns` takes down with the relay («вернётся вместе с
    // обходом»); patching behind a switch that reads off is the surprise the
    // switch exists to rule out.
    let switch_on = ctx.last.as_ref().is_some_and(|s| s.watchdog.is_on());
    if !switch_on || !ctx.settings.auto_patch_wanted() {
        return false;
    }
    let found: Vec<(&'static str, PathBuf)> = ctx
        .last
        .as_ref()
        .map(|s| {
            s.installs
                .iter()
                .filter(|r| r.patched == Some(false))
                .filter_map(|r| Some((r.label, r.path.clone()?)))
                .collect()
        })
        .unwrap_or_default();
    patch_found(ctx, found)
}

/// Patches the given installs without closing anything. The shared body of
/// auto-patch and of the row's «Пропатчить» button, which is what a user is left
/// with when the automatic pass could not write.
fn patch_found(ctx: &mut Ctx, found: Vec<(&'static str, PathBuf)>) -> bool {
    if found.is_empty() {
        return false;
    }

    ctx.busy(Some("Автопатч"));
    let mut wrote = false;
    for (label, inst) in found {
        // Never a password prompt nobody asked for: the automatic pass leaves a
        // root-owned install to the button, which asks.
        if crate::elevate::needs_root(&inst) {
            ctx.log(
                Level::Warn,
                format!(
                    "Автопатч: {} — установка принадлежит root. Нажмите «Включить всё» и введите пароль.",
                    label
                ),
            );
            continue;
        }
        let mut patched = 0usize;
        let mut unknown = 0usize;
        let mut failed: Option<String> = None;
        for (_, outcome) in crate::watchdog::patch_now(&inst) {
            match outcome {
                patch_binary::RepatchOutcome::Repatched(_) => patched += 1,
                patch_binary::RepatchOutcome::AlreadyPatched => {}
                patch_binary::RepatchOutcome::SignatureMissing => unknown += 1,
                patch_binary::RepatchOutcome::Failed(e)
                | patch_binary::RepatchOutcome::Deferred(e) => failed = Some(e),
            }
        }
        let where_ = crate::utils::mask_path(&inst.display().to_string());
        if patched > 0 {
            wrote = true;
            ctx.log(
                Level::Ok,
                format!("Автопатч: {} — патч наложен ({})", label, where_),
            );
        }
        if let Some(e) = failed {
            ctx.log(
                Level::Warn,
                format!(
                    "Автопатч: {} — не удалось записать ({}). Закройте Antigravity и нажмите «Включить всё».",
                    label, e
                ),
            );
        } else if unknown > 0 && patched == 0 {
            // The watchdog's rule (I19): a build this patcher does not know is
            // left exactly as it is, so Antigravity shows its own error.
            ctx.log(
                Level::Warn,
                format!(
                    "Автопатч: {} — эта сборка Antigravity анлокеру ещё не знакома, файлы не тронуты. Нужна новая версия анлокера. [{}]",
                    label,
                    patch_binary::describe_targets(&inst)
                ),
            );
        }
    }
    if wrote {
        // What the switch falls back to before the next deep read, and what an
        // older build reading this file takes as the user's wish.
        ctx.settings.client_patch = true;
        ctx.save_settings();
    }
    // Left raised: every caller re-reads the installs next and lowers it after.
    wrote
}

fn enable_client_patch(ctx: &mut Ctx) -> bool {
    ctx.log(Level::Step, "Патч клиента Antigravity");
    patch_binary::kill_affected_processes();

    let installs = crate::installs::find_all_installs();
    let manual: Vec<PathBuf> = ctx.settings.manual_paths.clone();
    let mut targets = installs;
    for p in manual {
        if !targets.contains(&p) {
            targets.push(p);
        }
    }

    if targets.is_empty() {
        ctx.log(
            Level::Err,
            "Установки Antigravity не найдены. Укажите путь вручную (карандаш).",
        );
        return false;
    }

    let mut ok = 0usize;
    for inst in &targets {
        // A root-owned Linux install: only this write is done as root, behind
        // the desktop's password prompt (`elevate`).
        if crate::elevate::needs_root(inst) {
            let where_ = crate::utils::mask_path(&inst.display().to_string());
            ctx.log(
                Level::Step,
                format!("{where_} принадлежит root — запрашиваю пароль для записи патча"),
            );
            match crate::elevate::patch_as_root(inst) {
                Ok(done) => {
                    ok += 1;
                    ctx.log(Level::Ok, format!("{} — {}", done.label, where_));
                    for w in done.warnings {
                        ctx.log(Level::Warn, w);
                    }
                }
                Err(e) => ctx.log(Level::Err, format!("{where_} — {e}")),
            }
            continue;
        }
        match crate::patch_install::process_install(inst) {
            Ok(outcome) => {
                ok += 1;
                ctx.log(
                    Level::Ok,
                    format!(
                        "{} — {}",
                        outcome.label,
                        crate::utils::mask_path(&inst.display().to_string())
                    ),
                );
                for w in outcome.warnings {
                    ctx.log(Level::Warn, w);
                }
            }
            Err(e) => ctx.log(
                Level::Err,
                format!(
                    "{} — {}",
                    crate::utils::mask_path(&inst.display().to_string()),
                    e
                ),
            ),
        }
    }

    if ok == 0 {
        ctx.log(Level::Err, "Ни одна установка не пропатчена.");
        return false;
    }

    // I39: the legacy CA comes out last, and `remove_legacy_ca` itself refuses
    // while the relay is still an older build.
    legacy_proxy::remove_legacy_ca_quiet();

    ctx.log(Level::Ok, format!("Пропатчено установок: {}", ok));
    true
}

fn disable_client_patch(ctx: &mut Ctx) {
    ctx.log(Level::Step, "Снятие патча клиента");

    // I20: a live watchdog treats an unpatch as an update and puts the patch
    // straight back. Both of them stop first — the standalone task and the one
    // that runs inside the relay — and are restored below to whatever the user
    // still has switched on.
    // The relay comes back if the user still wants the DNS bypass. The watchdog
    // does NOT: a watchdog is a process whose whole job is to put the patch back.
    // Restoring it here undid the unpatch about four seconds later and flipped
    // the switch on again. The relay's own watchdog thread comes back with the
    // relay and stays idle, because `patch_declined` is already on disk - the
    // caller saved it before calling this, and the relay reads the file fresh
    // when it starts (a save after the relay was up would lose the race to its
    // first two polls). `settings.auto_patch` keeps its value, so the watchdog
    // returns the moment the patch does.
    let want_relay = cfg!(windows) && ctx.settings.dns;
    background::disable_watchdog();
    if let Err(e) = background::disable() {
        ctx.log(Level::Warn, format!("Служба DNS не остановлена: {}", e));
    }

    patch_binary::kill_affected_processes();

    let mut targets = crate::installs::find_all_installs();
    for p in ctx.settings.manual_paths.clone() {
        if !targets.contains(&p) {
            targets.push(p);
        }
    }

    // I45: every step runs, each judged on its own. No `?` between them.
    for inst in &targets {
        let where_ = crate::utils::mask_path(&inst.display().to_string());
        // Each file is judged on its own (I45), and a failure is said out loud:
        // a count alone turned "one of two files could not be written" into a
        // clean revert, and the user kept a half-patched install. A root-owned
        // install (`/opt`) is reverted through pkexec, as it was patched: written
        // from here it only ever failed with `Permission denied`.
        // Asked only when there is something to take off: a clean root-owned
        // install does not deserve a password prompt.
        let patched_here = patch_binary::inspect_install(inst)
            .files
            .iter()
            .any(|(_, st)| *st == patch_binary::FileState::Patched);
        let done = if patched_here && crate::elevate::needs_root(inst) {
            ctx.log(
                Level::Info,
                format!("{} — установка принадлежит root, запрашиваю пароль", where_),
            );
            match crate::elevate::unpatch_as_root(inst) {
                Ok(d) => d,
                Err(e) => {
                    ctx.log(Level::Err, format!("{} — {}", where_, e));
                    continue;
                }
            }
        } else {
            crate::elevate::revert_files(inst)
        };
        for e in &done.errors {
            ctx.log(Level::Err, format!("{} — {}", where_, e));
        }
        for w in &done.warnings {
            ctx.log(Level::Warn, format!("{} — {}", where_, w));
        }
        let reverted = done.files;
        if let Err(e) = endpoint::remove_ide(inst) {
            ctx.log(Level::Warn, format!("Оверрайд эндпоинта не снят: {}", e));
        }
        ctx.log(
            Level::Ok,
            format!("{} — возвращено файлов: {}", where_, reverted),
        );
    }
    if let Err(e) = endpoint::remove_cli() {
        ctx.log(Level::Warn, format!("Переменная CLI не снята: {}", e));
    }

    // The proxy variable is only read by a patched binary, so an unpatched
    // machine must not keep it: it would sit in the user environment forever,
    // naming a port nothing listens on.
    clear_legacy_proxy(ctx);

    if want_relay {
        if let Err(e) = background::ensure_running() {
            ctx.log(
                Level::Warn,
                format!("Служба DNS не поднялась обратно: {}", e),
            );
        }
    }
}

// --- watchdog --------------------------------------------------------------

fn set_watchdog(ctx: &mut Ctx, on: bool) {
    if !on {
        background::disable_watchdog();
        ctx.log(Level::Ok, "Автовосстановление патча отключено.");
        return;
    }
    // The task launches the copy in %ProgramData%, so it has to be there - but
    // only the file: starting the relay here switched the 400 bypass on for a
    // user who asked for auto-patch alone.
    if let Err(e) = background::ensure_installed() {
        ctx.log(
            Level::Warn,
            format!("Не удалось установить копию для автопатча: {}", e),
        );
    }
    match background::enable_watchdog() {
        Ok(()) => ctx.log(Level::Ok, "Автовосстановление патча включено."),
        Err(e) => ctx.log(Level::Err, format!("Не удалось включить: {}", e)),
    }
}

// --- DNS -------------------------------------------------------------------

fn enable_dns(ctx: &mut Ctx) {
    ctx.log(Level::Step, "Обход через DNS");

    // I5: the rules name the relay as their first nameserver, so it has to be
    // answering before they are written.
    if let Err(e) = background::ensure_running() {
        ctx.log(Level::Err, format!("Служба DNS не запустилась: {}", e));
        return;
    }
    ctx.log(Level::Ok, "Локальная служба DNS запущена.");

    match dns::setup_dns_nrpt() {
        Ok(outcome) => {
            dns::invalidate_cache();
            // The rules are in; what the client cached before them is not ours
            // to keep (G75). The relay does the same at its own start.
            dns::flush_client_cache();
            if outcome.stood_down_for_vpn {
                // Not a failure, and not "on" either: with the client measured
                // inside a tunnel the rules would override the resolver the user
                // turned on, and the substituted address is reached through the
                // tunnel anyway (D13, G26, G29).
                ctx.log(
                    Level::Warn,
                    "Обнаружен активный VPN, через который идёт Antigravity — \
                     правила DNS не создавались. Выключите VPN и включите переключатель заново.",
                );
            } else {
                ctx.log(Level::Ok, "Правила DNS установлены.");
            }
            if let Some(note) = dns::outcome_note(&outcome) {
                ctx.log(Level::Info, strip_ansi(&note));
            }
        }
        Err(e) => ctx.log(Level::Err, format!("Правила DNS не установлены: {}", e)),
    }

    if ctx.settings.auto_patch_wanted() && !background::is_watchdog_enabled() {
        set_watchdog(ctx, true);
    }
}

fn disable_dns(ctx: &mut Ctx) {
    ctx.log(Level::Step, "Отключение обхода через DNS");

    // I45: both steps run regardless of what the first one did.
    dns::remove_dns_nrpt();
    dns::invalidate_cache();
    ctx.log(Level::Ok, "Правила DNS сняты.");
    stop_relay(ctx);
    clear_legacy_proxy(ctx);
    restart_stale_holders(ctx);
}

/// Image names of the Antigravity shells: restarting one is what gives its
/// language server an environment without `AG_LS_PROXY`.
const AG_SHELLS: [&str; 2] = ["Antigravity.exe", "Antigravity IDE.exe"];

/// Editors the Antigravity extension runs in (S95): its `agy` backend inherits
/// the editor's environment, so the editor is what has to be restarted. Not
/// closed for the user - unsaved work there is not ours to risk. Everything else
/// started at logon holds the variable too (measured: dozens of processes) and
/// never reads it, so it is not worth a line.
const EXTENSION_HOSTS: [&str; 5] = [
    "Code.exe",
    "Code - Insiders.exe",
    "Cursor.exe",
    "Windsurf.exe",
    "VSCodium.exe",
];

/// Restarts the Antigravity windows whose environment still names the proxy
/// that was just switched off, and names every other program that holds it.
fn restart_stale_holders(ctx: &mut Ctx) {
    let holders = crate::stale_env::holders(endpoint::PROXY_ENV_VAR);
    if holders.is_empty() {
        return;
    }
    let mut shells: Vec<std::path::PathBuf> = Vec::new();
    let mut others: Vec<String> = Vec::new();
    for h in &holders {
        if AG_SHELLS.iter().any(|s| s.eq_ignore_ascii_case(&h.name)) {
            if let Some(p) = &h.path {
                if !shells.contains(p) {
                    shells.push(p.clone());
                }
            }
        } else if EXTENSION_HOSTS
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&h.name))
            && !others.contains(&h.name)
        {
            others.push(h.name.clone());
        }
    }
    if !shells.is_empty() {
        ctx.log(
            Level::Step,
            "Перезапуск Antigravity: он запомнил адрес выключенного прокси",
        );
        patch_binary::kill_affected_processes();
        for path in &shells {
            launch_clean(path, endpoint::PROXY_ENV_VAR);
        }
        ctx.log(
            Level::Ok,
            "Antigravity перезапущен и обращается к Google напрямую.",
        );
    }
    // Only when the extension's backend is actually running: every editor
    // started while the bypass was on holds the variable, used or not.
    let agy_runs = holders
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("agy.exe"));
    if agy_runs && !others.is_empty() {
        ctx.log(
            Level::Info,
            format!(
                "Расширение Antigravity помнит адрес выключенного прокси — перезапустите {}.",
                others.join(", ")
            ),
        );
    }
}

/// Starts `path` without `var` in its environment. From an elevated window it
/// goes through `explorer.exe`, so it runs unelevated with the user's current
/// environment, which no longer has the variable.
fn launch_clean(path: &std::path::Path, var: &str) {
    #[cfg(target_os = "windows")]
    if is_admin() {
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.arg(path);
        if crate::utils::no_window(&mut cmd).spawn().is_ok() {
            return;
        }
    }
    let mut cmd = std::process::Command::new(path);
    cmd.env_remove(var);
    let _ = cmd.spawn();
}

/// Takes the relay service down for good, with auto-patch kept.
///
/// The standalone watchdog task launches the copy of this exe that
/// `background::disable` is about to delete. It is taken down first and put
/// back after, on a copy of its own: auto-patch is not part of the bypass,
/// and switching the bypass off must not switch it off too.
fn stop_relay(ctx: &mut Ctx) {
    let restore_watchdog = ctx.settings.auto_patch_wanted();
    if restore_watchdog {
        background::disable_watchdog();
    }
    match background::disable() {
        Ok(()) => ctx.log(Level::Ok, "Локальная служба DNS остановлена."),
        Err(e) => ctx.log(
            Level::Warn,
            format!("Служба остановлена не полностью: {}", e),
        ),
    }
    if restore_watchdog {
        set_watchdog(ctx, true);
    }
}

/// Re-writes the NRPT rules when the pool they name has changed.
///
/// A rule lists one address per enabled provider, so switching a provider off —
/// or turning rotation off, which narrows the list to one — changes what Windows
/// is pointed at. Without this the switch looks like it did nothing: the pool
/// stops asking that provider, but the machine keeps it as a nameserver until
/// the DNS switch is toggled off and on again.
/// Returns whether it actually went near the system — the caller uses that to
/// decide whether the following refresh has anything new to read.
fn reapply_dns_rules(ctx: &mut Ctx) -> bool {
    if !dns::is_nrpt_applied() {
        return false;
    }
    if !is_admin() && cfg!(target_os = "windows") {
        ctx.log(
            Level::Warn,
            "Список DNS сохранён, но правила не переписаны — нужны права администратора.",
        );
        return false;
    }
    match dns::setup_dns_nrpt() {
        Ok(_) => {
            dns::invalidate_cache();
            dns::flush_client_cache();
            ctx.log(Level::Ok, "Правила DNS переписаны под новый список.");
        }
        Err(e) => ctx.log(Level::Warn, format!("Правила DNS не переписаны: {}", e)),
    }
    true
}

fn clear_legacy_proxy(ctx: &mut Ctx) {
    let url = legacy_proxy::proxy_url();
    // I45 again: `remove_proxy` collects its own failures and joins them, so a
    // half-failed removal still takes off everything it can.
    match endpoint::remove_proxy(&url, "") {
        Ok(()) => ctx.log(Level::Ok, "Локальный прокси отключён."),
        Err(e) => ctx.log(Level::Warn, format!("Снято не полностью: {}", e)),
    }
}

/// Saves the user's own DNS server(s): IPv4 addresses, `https://` (DoH) and
/// `tls://` (DoT) servers. Anything else is refused with the reason, and the
/// slot stays as it was.
fn set_custom_dns(ctx: &mut Ctx, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        ctx.settings.custom_dns.clear();
        ctx.log(Level::Ok, "Свой DNS убран.");
        return;
    }
    let found = match crate::resolvers::parse_custom_dns(text) {
        Ok(found) if !found.is_empty() => found,
        Ok(_) => return,
        Err(e) => {
            ctx.log(
                Level::Err,
                format!(
                    "{} Принимаются IPv4-адрес (77.88.8.8), DoH (https://dns.example.com/dns-query)                      и DoT (tls://dns.example.com), через пробел.",
                    e
                ),
            );
            return;
        }
    };
    ctx.settings.custom_dns = found.iter().map(|s| s.text()).collect::<Vec<_>>().join(" ");
    ctx.settings
        .set_provider_enabled(crate::resolvers::CUSTOM_NAME, true);
    ctx.log(
        Level::Ok,
        format!(
            "Свой DNS сохранён ({}). Он участвует в пуле наравне с остальными: ответ сверяется с эталонным.",
            ctx.settings.custom_dns
        ),
    );
}

/// Strips SGR escapes from a string built for a terminal.
///
/// `dns::outcome_note` returns text with colour codes baked in — it was written
/// to be printed. Rendering it in a window would show the raw `ESC[33m`.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Drop until the final byte of the sequence (a letter).
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a path looks like an install root, for the manual "pencil" dialog.
pub fn resolve_manual_path(raw: &Path) -> Option<PathBuf> {
    crate::installs::resolve_install_root(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_colours_do_not_reach_the_window() {
        let coloured = "\x1b[33mвнимание\x1b[0m\x1b[92m: правило";
        assert_eq!(strip_ansi(coloured), "внимание: правило");
    }

    #[test]
    fn a_capability_that_edits_system_policy_asks_for_admin_on_windows() {
        // The trap this guards: without elevation `remove_dns_nrpt` and
        // `background::disable` do not fail, they silently do nothing — so an
        // unelevated OFF switch would render as off while the rules stayed.
        assert_eq!(Cap::Dns.needs_admin(), cfg!(target_os = "windows"));
        assert!(!Cap::ClientPatch.needs_admin(), "the patch needs no UAC");
    }

    #[test]
    fn with_rotation_off_the_one_that_answers_is_the_first_one_left_on() {
        let names = crate::resolvers::provider_names();
        assert!(names.len() >= 2, "the test needs a pool to narrow");

        let mut s = Settings::default();
        assert_eq!(first_enabled_provider(&s), Some(names[0]));

        // Turning the head off must move the choice down the list, not leave it
        // pointing at a provider that is switched off.
        s.set_provider_enabled(names[0], false);
        assert_eq!(first_enabled_provider(&s), None, "an empty custom slot cannot answer");
        s.custom_dns = "192.0.2.53".into();
        assert_eq!(first_enabled_provider(&s), Some(crate::resolvers::CUSTOM_NAME));

        for n in &names {
            s.set_provider_enabled(n, false);
        }
        assert_eq!(first_enabled_provider(&s), None);
        assert!(all_providers_disabled(&s));
    }

    #[test]
    fn a_partial_state_still_reads_as_on() {
        // Half-patched must not draw as off: the user would flip it on and get
        // a no-op on the installs that are already done.
        assert!(State::Partial("x".into()).is_on());
        assert!(!State::Blocked("нет прав".into()).is_on());
        assert!(!State::Off.is_on());
    }

    /// The bug this pins: «Сверять TLS» and «Ротация» described their *off*
    /// state with a note, the note was a `Partial`, and `Partial` reads as on —
    /// so the switch sprang back the moment the worker's snapshot arrived and
    /// neither could be turned off, while the log said they had been.
    #[test]
    fn a_switch_that_is_off_with_a_note_is_off() {
        let mut s = Settings::default();
        s.rotate_providers = false;
        let st = blank_status().with_settings_switches(&s);

        assert!(!st.dns_rotation.is_on());
        // The note itself must survive — it is what the row says underneath.
        assert!(st.dns_rotation.note().is_some());

        let on = blank_status().with_settings_switches(&Settings::default());
        assert!(on.dns_rotation.is_on());
    }

    /// A settings-only refresh must produce the same switches a full one does,
    /// or a flip would answer differently depending on which path drew it.
    #[test]
    fn the_cheap_refresh_and_the_full_one_agree_about_the_settings_switches() {
        let mut s = Settings::default();
        s.rotate_providers = false;
        let a = blank_status().with_settings_switches(&s);
        let b = blank_status().with_settings_switches(&s);
        assert_eq!(a.dns_rotation, b.dns_rotation);
        assert_eq!(a.providers.len(), crate::resolvers::provider_names().len());
    }

    /// A switch that only writes `settings.json` must not send the worker back
    /// to Windows for scheduled tasks and environment variables.
    #[test]
    fn only_the_switches_that_touch_the_system_pay_for_a_system_scan() {
        assert_eq!(scan_after(Cap::Dns), Scan::System);
        assert_eq!(scan_after(Cap::ClientPatch), Scan::Deep);
    }

    /// A service kept up for a route is not a half-installed DNS layer: with the
    /// DNS switch off it reads off, with it on it is still a fault.
    #[test]
    fn a_service_running_for_a_route_does_not_pin_the_dns_switch_on() {
        let off = dns_state(true, false, true, None, true, false);
        assert!(!off.is_on());
        let broken = dns_state(true, false, true, None, true, true);
        assert!(broken.is_on());
    }

    /// Live: what a switch actually costs, which is the complaint this was built
    /// to answer («слишком много времени»). Prints, asserts only the shape.
    ///
    ///     cargo test what_a_refresh_costs -- --ignored --nocapture
    #[test]
    #[ignore = "times real helper processes; run on Windows with --ignored"]
    fn what_a_refresh_costs() {
        let (tx, _rx) = mpsc::channel();
        let mut ctx = Ctx {
            events: tx,
            wake: Rc::new(|| {}),
            settings: Settings::load(),
            patched_seen: HashMap::new(),
            vpn: Some(VpnSeen::None),
            last: None,
            dns_probe: (false, false),
        };

        let t = std::time::Instant::now();
        let full = read_status(&mut ctx, false);
        let system = t.elapsed();
        ctx.last = Some(full);

        let t = std::time::Instant::now();
        let _ = settings_only_status(&ctx, ctx.last.clone().unwrap());
        let settings = t.elapsed();

        println!("Scan::System   {:?}", system);
        println!("Scan::Settings {:?}", settings);
        assert!(
            settings < system,
            "the cheap path is not cheaper: {:?} vs {:?}",
            settings,
            system
        );
    }

    fn blank_status() -> Status {
        Status {
            admin: true,
            installs: Vec::new(),
            client_patch: State::Off,
            watchdog: State::Off,
            dns: State::Off,
            dns_rotation: State::Off,
            vpn: None,
            custom_dns_text: String::new(),
            relay_outdated: false,
            relay_running: false,
            rules: false,
            providers: Vec::new(),
            patch_blocked: false,
        }
    }

    /// The regression the `Unmeasured` variant exists for: a tunnel is up and
    /// the client has not dialled out, which is what a window opened before
    /// Antigravity almost always sees. Reading that as "the client is outside
    /// the tunnel" is the indicator claiming a measurement nobody took.
    #[test]
    fn a_tunnel_with_nothing_to_read_is_not_a_client_outside_it() {
        use crate::egress::ClientEgress;
        let seen = |client| match client {
            ClientEgress::Tunnel => VpnSeen::CarryingClient,
            ClientEgress::Mixed => VpnSeen::PartlyCarryingClient,
            ClientEgress::Physical => VpnSeen::NotCarryingClient,
            ClientEgress::Unknown => VpnSeen::Unmeasured,
        };
        assert_eq!(seen(ClientEgress::Unknown), VpnSeen::Unmeasured);
        assert_ne!(seen(ClientEgress::Unknown), VpnSeen::NotCarryingClient);
        // And an unmeasured tunnel must not block the DNS row: it takes evidence
        // to stand down, not the absence of it (S37).
        assert_eq!(
            dns_state(true, true, true, Some(VpnSeen::Unmeasured), true, true),
            State::On
        );
        // A client in a tunnel no longer blocks the row either (D25).
        assert_eq!(
            dns_state(true, true, true, Some(VpnSeen::CarryingClient), true, true),
            State::On
        );
    }
}
