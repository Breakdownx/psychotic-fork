//! Messages and status snapshots shared by the interfaces and the worker.

use std::path::PathBuf;

/// Everything the UI can flip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cap {
    /// Switch 1: the client patch that lifts the account-region block.
    ClientPatch,
    /// Re-applies the patch after Antigravity updates itself.
    Watchdog,
    /// The DNS half of the 400 bypass: local relay + NRPT rules.
    Dns,
    /// Our own CONNECT proxy, named by the private env var the patch installs.
    LocalProxy,
    /// The user's own HTTP proxy.
    OwnProxy,
    /// Whether the DNS pool rotates, or only its first enabled member answers.
    DnsRotation,
    // `VpnDetect` is gone (D25): the layer no longer stands down for a tunnel,
    // so there is nothing left for that switch to decide. `settings.vpn_detect`
    // stays in the file so an older build reading it still finds the field.
}

impl Cap {
    /// Windows gates the DNS layer behind UAC. Without admin these do not fail —
    /// the cmdlets carry `-ErrorAction SilentlyContinue` — they quietly do
    /// nothing, which is exactly the way a switch ends up lying.
    pub fn needs_admin(self) -> bool {
        cfg!(target_os = "windows") && matches!(self, Cap::Dns | Cap::Watchdog)
    }

    pub fn title(self) -> &'static str {
        match self {
            Cap::ClientPatch => "Разблокировать вход в аккаунт",
            Cap::Watchdog => "Автопатч Antigravity",
            Cap::Dns => "Обход через DNS",
            Cap::LocalProxy => "Локальный прокси",
            Cap::OwnProxy => "Свой HTTP-прокси",
            Cap::DnsRotation => "Ротация DNS-серверов",
        }
    }
}

/// Where a capability actually is, as opposed to where the user put the switch.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    On,
    Off,
    /// On, but not the whole way — some installs patched and some not, or rules
    /// present with the relay down.
    Partial(String),
    /// **Off**, with a line saying what being off costs. Draws as an off switch.
    ///
    /// Not a flavour of `Partial`, and the difference is a bug that shipped:
    /// `is_on()` is true for `Partial`, so a switch whose off state was described
    /// with one drew itself back **on** the moment the worker's snapshot landed —
    /// «Сверять TLS» and «Ротация» could not be turned off at all, while the log
    /// said they had been. A note is a note; whether the thing is on is a
    /// separate question and now has a separate variant.
    OffNote(String),
    /// Cannot be turned on right now, and the reason is not the user's mistake:
    /// no admin, a foreign proxy that outranks ours, a VPN the layer stood down
    /// for. Drawn as an off switch with the reason next to it.
    Blocked(String),
}

impl State {
    pub fn is_on(&self) -> bool {
        matches!(self, State::On | State::Partial(_))
    }
    pub fn note(&self) -> Option<&str> {
        match self {
            State::Partial(s) | State::Blocked(s) | State::OffNote(s) => Some(s),
            _ => None,
        }
    }
}

/// One Antigravity *kind*, as the paths list draws it.
///
/// There is a row per kind rather than per found install, and it survives the
/// kind not being installed: an empty row with a pencil is how a user says
/// "it is here, you missed it". A list that only shows what was found gives
/// them nowhere to say it.
#[derive(Debug, Clone)]
pub struct InstallRow {
    /// `None` when this kind was neither found nor pointed at by hand.
    pub path: Option<PathBuf>,
    pub label: &'static str,
    /// `None` while it has not been inspected yet — inspecting reads the whole
    /// Language Server binary, so it is not done on the drawing thread.
    pub patched: Option<bool>,
    /// Added by hand through the pencil, rather than found by the scan.
    pub manual: bool,
}

/// What the VPN measurement found, as the indicator draws it.
///
/// The distinction is the whole point and it is a measured one, not a guess: a
/// tunnel being up says nothing about whether *Antigravity* is inside it. Per-app
/// split tunnelling made "a VPN is connected" mean the opposite of what it looks
/// like, and reading it as "skip the rules" left the client in the blocked region
/// with no bypass at all (G29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnSeen {
    /// No tunnel on the machine.
    None,
    /// A tunnel is up, but Antigravity's own sockets do not leave through it.
    NotCarryingClient,
    /// Measured: Antigravity's sockets leave through the tunnel.
    CarryingClient,
    /// Measured: some of them do and some do not — two language servers on
    /// different sides of a split tunnel, or sockets that outlived a rule
    /// change. The half inside faces the gate from wherever the tunnel exits.
    PartlyCarryingClient,
    /// A tunnel is up, and Antigravity's own traffic goes to **our** proxy — so
    /// the socket that faces the gate is the relay's, not the client's.
    ///
    /// The ordinary state for a patched client with the local-proxy route on,
    /// and the one the tool's own success created: excluding `language_server`
    /// from a VPN stopped meaning anything the moment it stopped making gate
    /// connections. Whether the *relay* is in the tunnel is a question this
    /// measurement does not answer (P34).
    ViaLocalProxy,
    /// A tunnel is up and there was nothing to read: Antigravity is not running,
    /// or has not dialled out yet.
    ///
    /// Its own variant because it used to be folded into `NotCarryingClient`,
    /// and that is the same mistake the tool forbids itself elsewhere — treating
    /// an absence of evidence as evidence (the resolver rule in CLAUDE.md). The
    /// window opens before Antigravity is started far more often than not, so
    /// the message every VPN user got was the confident and usually wrong «VPN
    /// активен, но Antigravity идёт мимо него». What the layer *does* is
    /// unchanged: no measurement still means the rules go in (S37).
    Unmeasured,
}

#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub name: String,
    pub enabled: bool,
    /// False only for the «свой DNS» slot while the user has put nothing in it:
    /// drawn off and not switchable.
    pub available: bool,
}

/// A whole-system snapshot. Produced on the worker, rendered on the UI thread.
#[derive(Debug, Clone)]
pub struct Status {
    pub admin: bool,
    pub installs: Vec<InstallRow>,
    pub client_patch: State,
    pub watchdog: State,
    pub dns: State,
    pub local_proxy: State,
    pub own_proxy: State,
    pub own_proxy_configured: bool,
    pub dns_rotation: State,
    /// What the last measurement saw. `None` means it has not been taken yet;
    /// taking it spawns PowerShell, so it is not done on every refresh.
    pub vpn: Option<VpnSeen>,
    pub own_proxy_text: String,
    pub custom_dns_text: String,
    pub relay_outdated: bool,
    /// Whether the relay task is registered *and* running. The raw fact, kept
    /// beside the `dns` row that already folds it into a verdict: the gate strip
    /// has to say "there is nobody to catch the next 400" without re-deriving it
    /// from a string meant for a switch.
    pub relay_running: bool,
    /// Whether our NRPT rules are in. The raw fact, beside `relay_running`, for
    /// the same reason: a running service with no rules is a bypass nobody's
    /// queries ever reach, and the status card has to be able to say so.
    pub rules: bool,
    pub providers: Vec<ProviderRow>,
    /// Where **our own** service's connections leave. `None` when there was
    /// nothing to read, or no tunnel to read it against.
    ///
    /// The half the client stopped being able to answer (G46), and the one that
    /// decides whether a provider serving only Russian addresses will serve us
    /// at all. Reported, never acted on: the stand-down decision is still the
    /// client's (D13, P34).
    pub relay_egress: Option<crate::egress::ClientEgress>,
    /// macOS only: an install was found unpatched and its binary cannot be
    /// written because the `.app` bundle is App-Management protected. The card
    /// says what to do about it instead of the bare "пропатчены не все
    /// установки"; always false off macOS.
    pub patch_blocked: bool,
}

impl Status {
    pub fn get(&self, cap: Cap) -> &State {
        match cap {
            Cap::ClientPatch => &self.client_patch,
            Cap::Watchdog => &self.watchdog,
            Cap::Dns => &self.dns,
            Cap::LocalProxy => &self.local_proxy,
            Cap::OwnProxy => &self.own_proxy,
            Cap::DnsRotation => &self.dns_rotation,
        }
    }

    /// The bypass master switch. Derived, never stored: it is on when any part
    /// of the bypass is, which keeps one truth instead of two.
    pub fn bypass_on(&self) -> bool {
        self.dns.is_on() || self.local_proxy.is_on() || self.own_proxy.is_on()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Step,
    Info,
    Ok,
    Warn,
    Err,
}

/// Worker → UI.
pub enum Event {
    Log(Level, String),
    Status(Box<Status>),
    /// `Some(what)` while an action runs, `None` when the queue drains. The UI
    /// dims the switches in between so a second click cannot queue a race.
    Busy(Option<String>),
    ProxySaved(Cap, Result<(), String>),
    Deactivated,
}

/// UI → worker.
pub enum Cmd {
    /// The UI is up. Auto-patch may now act on the initial scan. Sent once at
    /// startup.
    UiReady,
    /// Persist the user's UI theme preference.
    SetTheme(crate::settings::ThemeMode),
    /// Ask the system again where the client's traffic leaves.
    ///
    /// Its own command because it is the one measurement that goes stale on its
    /// own — a user connects a VPN, or starts Antigravity, and nothing in this
    /// window was touched. Sent by the gate watcher, which knows when it is
    /// worth paying for (`gate`), and answered here because `ops` owns the
    /// measurement the DNS layer's decision is made from (I59).
    RemeasureVpn,
    /// The answer to the above, coming back from the thread it was taken on.
    ///
    /// It carries the relay's liveness too, because that is the other fact the
    /// gate strip shows and the other one that goes stale on its own — a relay
    /// can die while the window sits open, and until this arrived nothing
    /// re-read it short of the user flipping a switch. Both probes are cheap
    /// process spawns and neither belongs on the worker's queue.
    Probed {
        vpn: VpnSeen,
        relay: bool,
        relay_egress: Option<crate::egress::ClientEgress>,
    },
    /// Re-read the system *including* whether each install is patched, which
    /// means reading every Language Server binary end to end.
    #[allow(dead_code)]
    DeepRefresh,
    Set(Cap, bool),
    AddPath(PathBuf),
    ForgetPath(PathBuf),
    /// The «Пропатчить» button of one install row.
    PatchInstall(PathBuf),
    SetOwnProxy {
        text: String,
        enable: bool,
    },
    SetCustomDns(String),
    SetProvider(String, bool),
    /// The whole pool in the order the user dragged it into.
    ReorderProviders(Vec<String>),
    /// Everything on, in the order that works: the patch, auto-patch, then the
    /// bypass. The one button a user who is not a programmer needs.
    EnableAll,
    /// Puts the service back the way the switches say it should be: reinstalls
    /// the relay when it is missing, stopped or older than this build, rewrites
    /// the rules, restores the proxy variable.
    Repair,
    Deactivate,
    #[allow(dead_code)]
    Stop,
}
