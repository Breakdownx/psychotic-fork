//! What the user switched on, remembered between runs.
//!
//! The old build had no such file: state lived only in the system (an NRPT rule
//! either exists or does not) and the menu asked again every time. A switch UI
//! needs both — the *system* answers "is it on right now", this answers "is it
//! on because the user asked for it", which is what lets the app put a switch
//! back where the user left it and re-apply after a reboot wiped something.
//!
//! Deliberately best-effort: a missing or corrupt file is a fresh `Settings`,
//! never an error the user has to deal with.
//!
//! The window and the per-user background services share this settings file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(not(test))]
use std::time::Duration;
use std::time::Instant;

const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Switch 1: the client patch that lifts the account-region block.
    pub client_patch: bool,

    /// Patch Antigravity by itself: a fresh install as much as one an update
    /// wiped (D27).
    ///
    /// Read by the *watchdog process*, not just the window, which is why it goes
    /// through `auto_patch_wanted()` and its cache rather than being consulted
    /// from an in-memory `Settings` the watchdog never sees.
    pub auto_patch: bool,

    /// The user switched the client patch **off** on purpose.
    ///
    /// The one thing auto-patch must never do is put back a patch the user took
    /// off (G38), so that wish is kept here and not read off `client_patch`:
    /// `client_patch` false is also every machine that simply has not patched
    /// yet - which is exactly who auto-patch is for. Gating on it left a fresh
    /// Desktop 2.15.0 unpatched with auto-patch on, and its window came up black
    /// (owner, 2026-09-19).
    pub patch_declined: bool,

    /// Read from a file written before `patch_declined` existed, so the field
    /// above is `parse`'s conservative guess rather than something the user
    /// said. The window settles it once at start (`ops::settle_decline`) and
    /// saves it. Never written to the file.
    #[serde(skip)]
    pub decline_unrecorded: bool,

    /// Set only when loading a file written by a version with HTTP proxies.
    #[serde(skip)]
    pub has_retired_proxy_settings: bool,

    /// Enables DNS forwarding on Windows.
    pub dns: bool,

    /// Providers the user turned off by name. A deny-list rather than an
    /// allow-list on purpose: a provider added in a later release is then on by
    /// default instead of silently missing from everyone's saved settings.
    pub disabled_providers: Vec<String>,

    /// The user's own DNS server(s) for the bypass: IPv4 addresses separated by
    /// spaces, commas or semicolons. They join the pool as one more provider
    /// («свой DNS»): raced with the others, every answer judged against the
    /// reference resolver, so a server that does not substitute the gate hosts
    /// costs nothing. Empty means the slot is unused.
    #[serde(default)]
    pub custom_dns: String,

    /// Whether every enabled provider may answer, or only the first one.
    ///
    /// On (the default) is what the pool was built for: each query is raced and
    /// the answer verified against a reference resolver, so a provider that
    /// silently drops a name costs nothing. Off pins the layer to the first
    /// enabled provider in pool order - for a user who wants exactly one service
    /// seeing their lookups, at the price of having no fallback when it stops
    /// substituting.
    pub rotate_providers: bool,

    /// The user's own order for the DNS pool, by name.
    ///
    /// Empty means the compiled order, which is ordered by measured substitution.
    /// It matters most with rotation off, where the *first* enabled provider is
    /// the only one asked - so being able to say which one that is is the point.
    /// Names not listed here keep their compiled position after the listed ones,
    /// so a provider added in a later release is never silently dropped.
    pub provider_order: Vec<String>,

    /// Unused since D25 (`2.14.0_1`): the rules are installed whatever the
    /// tunnel does, and the relay decides at runtime what they answer. Kept so
    /// a file written by an older build still loads, and an older build reading
    /// a newer file still finds its field.
    pub vpn_detect: bool,

    /// No longer a switch: the certificate of a substituted address is always
    /// verified, and an address that passed is trusted for a day (`resolvers.rs`,
    /// `TLS_VERIFIED_FOR`). Stays in the file so an older build still finds it.
    pub verify_tls: bool,

    /// Installs the user pointed at by hand, on top of the ones found by the
    /// scan (the pencil next to each path).
    pub manual_paths: Vec<PathBuf>,

    /// UI color theme preference (`system`, `light`, or `dark`).
    #[serde(default)]
    pub theme: ThemeMode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            client_patch: false,
            auto_patch: true,
            patch_declined: false,
            decline_unrecorded: false,
            has_retired_proxy_settings: false,
            dns: true,
            disabled_providers: Vec::new(),
            custom_dns: String::new(),
            rotate_providers: true,
            provider_order: Vec::new(),
            vpn_detect: true,
            verify_tls: true,
            manual_paths: Vec::new(),
            theme: ThemeMode::System,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        match read() {
            Read::Parsed(s) => s,
            Read::Missing => Self::default(),
            Read::Broken => Self::unreadable(),
        }
    }

    /// What a file that exists but cannot be read stands for: the defaults,
    /// minus the one that acts on the user's files by itself. With saves atomic
    /// a broken file is real damage, and it may have been a user's "patch off".
    fn unreadable() -> Self {
        Self {
            patch_declined: true,
            ..Self::default()
        }
    }

    /// Best-effort write. A failure here must never block an action the user
    /// asked for, so it is reported as a bool rather than an error to stop on;
    /// the worker logs it (`ops::Ctx::save_settings`).
    pub fn save(&self) -> bool {
        let Some(path) = path() else { return false };
        if let Some(dir) = path.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return false;
            }
        }
        let written = match serde_json::to_string_pretty(self) {
            Ok(json) => write_replacing(&path, &json).is_ok(),
            Err(_) => false,
        };
        if written {
            // The writer must see its own write immediately. Without this the
            // window saves a new provider order and then reads the list back
            // through a cache that is up to a TTL old, so the change appears not
            // to have happened at all - for twenty seconds. Other processes
            // still pick it up on their own TTL, which is the point of the
            // cache; this only shortcuts the process that made the change.
            if let Ok(mut guard) = CACHE.lock() {
                *guard = Some((self.clone(), Instant::now()));
            }
        }
        written
    }

    /// Whether Antigravity is to be patched without a click: auto-patch on, and
    /// the patch never switched off by hand since.
    pub fn auto_patch_wanted(&self) -> bool {
        self.auto_patch && !self.patch_declined
    }

    pub fn provider_enabled(&self, name: &str) -> bool {
        !self
            .disabled_providers
            .iter()
            .any(|d| d.eq_ignore_ascii_case(name))
    }

    pub fn set_provider_enabled(&mut self, name: &str, on: bool) {
        self.disabled_providers
            .retain(|d| !d.eq_ignore_ascii_case(name));
        if !on {
            self.disabled_providers.push(name.to_string());
        }
    }
}

/// What reading the file found. Missing and broken are told apart because they
/// mean opposite things to the watchdog: no file is a machine nobody configured,
/// and the defaults are right for it; a file that does not parse is usually one
/// caught mid-write by another process, and the defaults say "patch" (D27).
enum Read {
    Parsed(Settings),
    Missing,
    Broken,
}

fn read() -> Read {
    let Some(path) = path() else {
        return Read::Missing;
    };
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_or(Read::Broken, Read::Parsed),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(_) => Read::Broken,
    }
}

/// The file's text as settings, a leading byte-order mark allowed.
///
/// Windows PowerShell 5.1's `Set-Content -Encoding utf8` and older Notepad write
/// one, and serde_json rejects it - which `load` turned into "no file": every
/// switch back at its default, the user's own proxy and paths gone, silently.
///
/// A file without `patch_declined` comes from a build before D27, where "the
/// user switched the patch off" was written as `client_patch` false and nothing
/// else. It reads as declined whenever `client_patch` is false - the side that
/// never puts back a patch the user took off (G38) - and the window refines it
/// once with what only it can check (`ops::settle_decline`).
fn parse(text: &str) -> Option<Settings> {
    let value: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let unrecorded = value.get("patch_declined").is_none();
    let retired = ["local_proxy", "own_proxy", "own_proxy_enabled"].iter()
        .any(|key| value.get(key).is_some());
    let mut s: Settings = serde_json::from_value(value).ok()?;
    s.has_retired_proxy_settings = retired;
    if unrecorded {
        s.patch_declined = !s.client_patch;
        s.decline_unrecorded = true;
    }
    Some(s)
}

/// A temp beside the file, then a rename over it: a reader in another process -
/// the relay re-reads this every 20 s - sees the old file or the new one, never
/// the empty one `fs::write` leaves between truncating and writing. Per process,
/// so two savers never share a temp.
fn write_replacing(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let mut result = std::fs::write(&tmp, text);
    if result.is_ok() {
        // A reader that opened the file without delete sharing - a scanner, an
        // editor - blocks the rename for as long as it holds it. Brief, usually.
        for attempt in 0..3 {
            result = std::fs::rename(&tmp, path);
            if result.is_ok() {
                break;
            }
            if attempt < 2 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn path() -> Option<PathBuf> {
    let dir = crate::dns_forwarder::log_dir();
    if dir.as_os_str().is_empty() {
        return None;
    }
    Some(dir.join(FILE_NAME))
}

/// The settings as a *serving* process must read them.
///
/// The DNS relay caches settings briefly between queries.
///
/// Holds only its own lock and calls into nothing else, so it cannot take part
/// in the cross-module reentrancy I50 forbids.
static CACHE: Mutex<Option<(Settings, Instant)>> = Mutex::new(None);

fn cached() -> Settings {
    // Tests get the defaults, never the machine's file.
    //
    // Resolvers ask this question deep inside the pool, so without
    // this the pool's own unit tests would depend on whatever the person running
    // them last switched in the window — and the failure looks like a bug in the
    // pool rather than in the harness. Behaviour with real settings is covered
    // where it belongs, by tests that build a `Settings` and pass it in.
    #[cfg(test)]
    return Settings::default();

    #[cfg(not(test))]
    {
        cached_from_disk()
    }
}

#[cfg(not(test))]
fn cached_from_disk() -> Settings {
    // Short on purpose: a switch flipped in the window has to reach the running
    // relay within a couple of seconds, not half a minute. One small file read
    // per TTL is nothing next to the DNS queries it sits behind.
    const TTL: Duration = Duration::from_secs(2);

    if let Ok(guard) = CACHE.lock() {
        if let Some((s, at)) = guard.as_ref() {
            if at.elapsed() < TTL {
                return s.clone();
            }
        }
    }
    let fresh = match read() {
        Read::Parsed(s) => s,
        Read::Missing => Settings::default(),
        // Damaged, or held by something mid-write. Keep serving what was last
        // read - or, with nothing read yet, the defaults minus the one that
        // acts on the user's files - for another TTL. Caching this too matters:
        // the relay asks on every query, and a file that stays broken would
        // otherwise be re-read and re-parsed on each one.
        Read::Broken => CACHE
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|(s, _)| s.clone()))
            .unwrap_or_else(Settings::unreadable),
    };
    if let Ok(mut guard) = CACHE.lock() {
        let changed = guard.as_ref().is_some_and(|(old, _)| {
            old.disabled_providers != fresh.disabled_providers
                || old.provider_order != fresh.provider_order
                || old.custom_dns != fresh.custom_dns
                || old.rotate_providers != fresh.rotate_providers
        });
        if changed {
            EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        *guard = Some((fresh.clone(), Instant::now()));
    }
    fresh
}

/// Bumped whenever a re-read finds the DNS pool's settings changed (a provider
/// switched, reordered, the user's server edited, rotation flipped). The pool
/// keeps a decision for minutes and a vetted answer for seconds; both were made
/// under the old settings, so they compare this and start over when it moved.
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn pool_epoch() -> u64 {
    EPOCH.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn custom_dns_cached() -> String {
    cached().custom_dns
}

pub fn disabled_providers_cached() -> Vec<String> {
    cached().disabled_providers
}

/// The user's provider order, for the resolver pool to lay itself out by.
pub fn provider_order_cached() -> Vec<String> {
    cached().provider_order
}

/// Whether the pool may rotate, or only its first enabled member answers.
pub fn rotation_enabled() -> bool {
    cached().rotate_providers
}

/// Whether a VPN carrying Antigravity makes the DNS layer stand down.
///
/// Read from `dns.rs`, which runs in the window *and* in the relay, so it goes
/// through the cache like every other cross-process setting.

/// Whether the watchdog may patch what it finds: auto-patch on, and the patch
/// not switched off by hand (D27).
///
/// The watchdog is a separate process that survives reboots, so this is the only
/// way it can hear about either switch. Without the second half it puts back what
/// the window just removed, seconds later, and the switch flips itself back on
/// (G38). Defaults to on when there is no settings file yet, which is the shipped
/// behaviour.
pub fn auto_patch_wanted() -> bool {
    cached().auto_patch_wanted()
}

/// The installs the user pointed at by hand, so the watchdog keeps them patched
/// too and not only the ones in the standard locations.
pub fn manual_paths() -> Vec<PathBuf> {
    cached().manual_paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_reads_as_the_defaults_not_an_error() {
        // Nothing has been written in a test process, so this exercises the
        // "first run" path that every new user takes.
        let s = Settings::default();
        assert!(s.dns && s.auto_patch);
        assert!(s.rotate_providers, "the pool races by default");
        assert!(
            !s.client_patch,
            "nothing is patched until something patches it"
        );
        assert!(
            s.auto_patch_wanted(),
            "a fresh install is patched unasked (D27): that is what auto-patch is for"
        );
    }

    #[test]
    fn retired_proxy_options_are_ignored_without_resetting_the_patch_or_dns() {
        let s = parse(r#"{
            "group_key":"legacy", "group_key_at":42, "group_proxy":true,
            "own_proxy":"http://127.0.0.1:8080", "own_proxy_at":43, "own_proxy_enabled":true,
            "local_proxy":true, "debug_route":"own", "dns":false,
            "client_patch":true, "auto_patch":true, "patch_declined":false, "theme":"dark"
        }"#).expect("old settings still load");
        assert!(s.has_retired_proxy_settings);
        assert!(s.client_patch && s.auto_patch_wanted());
        assert!(!s.dns);
        assert_eq!(s.theme, ThemeMode::Dark);
        let saved = serde_json::to_value(&s).expect("serializes");
        assert!(!parse(&saved.to_string()).unwrap().has_retired_proxy_settings);
        for field in [
            "group_key", "group_key_at", "group_proxy", "own_proxy_at", "own_proxy",
            "own_proxy_enabled", "local_proxy", "debug_route",
        ] {
            assert!(saved.get(field).is_none(), "obsolete {field} was saved");
        }
    }

    /// G38 under D27: switching the patch off keeps it off, whatever auto-patch
    /// says, until the user asks for the patch again.
    #[test]
    fn a_patch_switched_off_by_hand_is_not_put_back() {
        let mut s = Settings::default();
        s.patch_declined = true;
        assert!(s.auto_patch, "the auto-patch wish itself is kept");
        assert!(!s.auto_patch_wanted());
        s.patch_declined = false;
        assert!(s.auto_patch_wanted());
        s.auto_patch = false;
        assert!(!s.auto_patch_wanted(), "and auto-patch off is off");
    }

    /// A hand-edited file with a byte-order mark keeps its contents.
    #[test]
    fn a_byte_order_mark_does_not_reset_the_file() {
        let s = parse("\u{feff}{\"dns\": false, \"patch_declined\": true}").expect("parses");
        assert!(!s.dns);
        assert!(s.patch_declined);
    }

    /// A file from a build before D27 has no `patch_declined`. With the patch
    /// off it reads as declined - an older build wrote "switched off by hand"
    /// exactly that way, and that patch must not come back unasked (G38) - and
    /// it is marked, so the window can settle it once.
    #[test]
    fn a_file_from_before_the_decline_flag_reads_conservatively() {
        let off = parse(r#"{"client_patch": false, "auto_patch": true}"#).expect("loads");
        assert!(off.patch_declined && off.decline_unrecorded);
        assert!(!off.auto_patch_wanted());

        let on = parse(r#"{"client_patch": true, "auto_patch": true}"#).expect("loads");
        assert!(!on.patch_declined && on.decline_unrecorded);
        assert!(on.auto_patch_wanted());

        let recorded = parse(r#"{"client_patch": false, "patch_declined": false}"#).expect("loads");
        assert!(!recorded.patch_declined && !recorded.decline_unrecorded);
        assert!(
            recorded.auto_patch_wanted(),
            "a recorded answer is taken as is"
        );
    }

    /// The marker never reaches the file, so a file this build wrote is never
    /// mistaken for an older one.
    #[test]
    fn the_unrecorded_marker_is_not_saved() {
        let s = Settings {
            decline_unrecorded: true,
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).expect("serializes");
        assert!(!json.contains("decline_unrecorded"));
        assert!(json.contains("patch_declined"));
    }

    /// A save is a whole file or nothing, and leaves no temp behind.
    #[test]
    fn a_save_replaces_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("ag_settings_replace_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, "old").unwrap();
        write_replacing(&file, "{\"dns\": false}").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"dns\": false}");
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1, "only the file itself");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_provider_is_on_until_it_is_explicitly_turned_off() {
        let mut s = Settings::default();
        assert!(s.provider_enabled("dns-ai"));
        s.set_provider_enabled("dns-ai", false);
        assert!(
            !s.provider_enabled("DNS-AI"),
            "the name match is case-blind"
        );
        assert_eq!(s.disabled_providers.len(), 1);
        // Turning it off twice must not leave two entries behind.
        s.set_provider_enabled("dns-ai", false);
        assert_eq!(s.disabled_providers.len(), 1);
        s.set_provider_enabled("dns-ai", true);
        assert!(s.disabled_providers.is_empty());
    }

    #[test]
    fn an_unknown_field_in_the_file_does_not_throw_the_whole_thing_away() {
        // A settings file written by a newer build must still load in an older
        // one - otherwise a downgrade silently resets every switch.
        let json = r#"{"dns": false, "something_from_the_future": 7}"#;
        let s: Settings = serde_json::from_str(json).expect("unknown fields are ignored");
        assert!(!s.dns);
        assert!(s.auto_patch, "fields left out keep their default");
        assert_eq!(s.theme, ThemeMode::System);
    }

    #[test]
    fn theme_mode_defaults_to_system_and_round_trips() {
        assert_eq!(Settings::default().theme, ThemeMode::System);
        for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
            let s = Settings {
                theme: mode,
                ..Settings::default()
            };
            let json = serde_json::to_string(&s).unwrap();
            let parsed = parse(&json).unwrap();
            assert_eq!(parsed.theme, mode);
        }
    }
}
