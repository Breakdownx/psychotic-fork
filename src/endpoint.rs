use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::utils::powershell;

// Which CloudCode host the client talks to, and why that is the whole fix.
//
// The region gate lives on the CloudCode API. `cloudcode-pa.googleapis.com` was
// substituted by the unblock resolvers until they dropped it, and their proxies
// refuse it by SNI, so that host cannot be reached through a permitted region
// any more (kb/dns.md). `daily-cloudcode-pa.googleapis.com` is the *same
// service* - its 401 names `service: cloudcode-pa.googleapis.com` - it is still
// substituted by geohide.ru, and geohide's proxy accepts its SNI. So the fix is
// to send the client at the host that still has a route.
//
// Nothing here patches a binary. Each surface already has a supported way to
// choose the endpoint:
//
// - Desktop passes `--cloud_code_endpoint https://daily-cloudcode-pa...` as a
//   literal in app.asar. Already on the working host; nothing to do.
// - IDE builds the argument from `getCloudCodeUrl()`, which returns
//   `cloudCodeUrlOverride` first if the `jetski.cloudCodeUrl` setting is set,
//   and otherwise the production host for any account without GCP terms - i.e.
//   every ordinary user. That setting is what we write.
// - CLI reads the `CLOUD_CODE_URL` environment variable ("UpdateEndpointURL
//   called with CLOUD_CODE_URL: %q" in agy.exe).
//
// Being configuration rather than a patch matters twice over: an app update
// cannot silently undo it the way it undoes the binary rename (G2), and a
// revert is a key removal rather than a byte-level restore.

/// The CloudCode host that still has a substituted route.
pub const DAILY_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";

/// IDE setting that overrides the CloudCode base URL.
pub const IDE_SETTING: &str = "jetski.cloudCodeUrl";

/// Environment variable the CLI reads for the same purpose.
pub const CLI_ENV_VAR: &str = "CLOUD_CODE_URL";

/// Where the IDE keeps its user settings.
///
/// Derived from `product.json` rather than hardcoded: the folder is named after
/// `nameShort`, so a rebranded or renamed build still resolves correctly.
pub fn ide_settings_path(install: &Path) -> Option<PathBuf> {
    let product = install.join("resources").join("app").join("product.json");
    let text = fs::read_to_string(product).ok()?;
    let name = Regex::new(r#""nameShort"\s*:\s*"([^"]+)""#)
        .ok()?
        .captures(&text)?
        .get(1)?
        .as_str()
        .to_string();
    // The user-config root is `%APPDATA%` on Windows and `~/.config` on Linux -
    // the same VS Code layout underneath (`<root>/<nameShort>/User/settings.json`).
    #[cfg(target_os = "windows")]
    let root = PathBuf::from(std::env::var("APPDATA").ok()?);
    #[cfg(not(target_os = "windows"))]
    let root = {
        // An empty XDG_CONFIG_HOME (elevation can pass it through blank) is "unset",
        // not a relative root - fall back to ~/.config in that case.
        match std::env::var("XDG_CONFIG_HOME") {
            Ok(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
            _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
        }
    };
    Some(root.join(name).join("User").join("settings.json"))
}

/// Rewrites `key` to `value`, leaving every other byte of the file alone.
///
/// A settings file is JSONC - comments and trailing commas are legal - and it
/// is the user's, not ours. Parsing and re-serialising would silently drop
/// their comments and reorder their keys, so this edits the text in place, the
/// same rule `hosts_pin` follows for the hosts file.
///
/// Unused since 2.11.0 removed the endpoint override (D11); kept because it is
/// the write half of the pair `remove_key` belongs to, its eight tests are what
/// pin that JSONC discipline (I18), and restoring an override - or writing any
/// other IDE setting - would otherwise start by rewriting it from scratch.
/// Delete it if a release passes with nothing needing to write a settings key.
#[allow(dead_code)]
fn upsert_key(text: &str, key: &str, value: &str) -> Result<String, String> {
    let existing = Regex::new(&format!(r#""{}"\s*:\s*"[^"]*""#, regex::escape(key)))
        .map_err(|_| "неверный шаблон настройки".to_string())?;
    let entry = format!("\"{}\": \"{}\"", key, value);

    if existing.is_match(text) {
        return Ok(existing.replace(text, entry.as_str()).into_owned());
    }
    if text.trim().is_empty() {
        return Ok(format!("{{\n    {}\n}}\n", entry));
    }

    let cut = text
        .rfind('}')
        .ok_or_else(|| "settings.json без закрывающей скобки".to_string())?;
    let head = text[..cut].trim_end();
    let mut out = String::with_capacity(text.len() + entry.len() + 8);
    out.push_str(head);
    // An empty object needs no separator; anything else does, unless the user
    // already left a trailing comma.
    if !head.ends_with('{') && !head.ends_with(',') {
        out.push(',');
    }
    out.push_str("\n    ");
    out.push_str(&entry);
    out.push('\n');
    out.push_str(&text[cut..]);
    Ok(out)
}

/// Drops `key`, taking the separating comma with it so the file stays valid.
fn remove_key(text: &str, key: &str) -> Result<String, String> {
    let escaped = regex::escape(key);
    // As a later entry it carries a comma in front of it; as the only or first
    // entry, the comma (if any) follows.
    let trailing = Regex::new(&format!(r#",\s*"{}"\s*:\s*"[^"]*""#, escaped))
        .map_err(|_| "неверный шаблон настройки".to_string())?;
    if trailing.is_match(text) {
        return Ok(trailing.replace(text, "").into_owned());
    }
    let alone = Regex::new(&format!(r#""{}"\s*:\s*"[^"]*"\s*,?\s*"#, escaped))
        .map_err(|_| "неверный шаблон настройки".to_string())?;
    Ok(alone.replace(text, "").into_owned())
}

// `apply_ide`/`apply_cli` were deleted in 2.11.0 together with the policy they
// served: the client is no longer pushed onto `daily-cloudcode-pa`, because the
// host it picks for itself is routed now (see `patch_install::process_install`). The two
// `remove_*` below stay, and are called from menu 1 as well as the undo paths -
// a machine patched by an earlier build still carries the override, and nothing
// else would ever take it back off.

/// Takes our override back out of the IDE's settings.
///
/// Scoped to **our** value, the same rule `remove_cli` follows. Before 2.11.0
/// this stripped the key whatever it held, which was harmless while only we ever
/// wrote it; now that menu 1 calls it on every run, a user who points
/// `jetski.cloudCodeUrl` somewhere themselves would have it silently deleted on
/// the next patch. Deleting somebody's own configuration is the worse of the two
/// failures (G20), so a value we did not write is left alone.
pub fn remove_ide(install: &Path) -> Result<(), String> {
    let Some(path) = ide_settings_path(install) else {
        return Ok(());
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(());
    };
    if !text.contains(DAILY_ENDPOINT) {
        return Ok(());
    }
    let updated = remove_key(&text, IDE_SETTING)?;
    fs::write(&path, updated).map_err(|e| format!("не записать {}: {}", path.display(), e))
}

/// Removes the variable, but only when it still holds the value we wrote: a
/// user (or another tool) may have pointed it somewhere themselves.
pub fn remove_cli() -> Result<(), String> {
    if current_cli_endpoint().as_deref() != Some(DAILY_ENDPOINT) {
        return Ok(());
    }
    let script = format!(
        "[Environment]::SetEnvironmentVariable('{}',$null,'User')",
        CLI_ENV_VAR
    );
    powershell(&script).ok_or_else(|| "не удалось удалить переменную среды".to_string())?;
    Ok(())
}

/// The variable that points the language server at the local gate proxy.
///
/// **Not** `HTTPS_PROXY`. It is a private name that only the patched binaries
/// read, because `patch_binary` renames the lower-case literal in their
/// `httpproxy.getEnvAny("HTTPS_PROXY", "https_proxy")` to exactly this. The full
/// argument is on `patch_binary::PROXY_VAR_NEW`; the short version is that the
/// variable has to be user-wide (nothing spawns the language server for us) and
/// a user-wide `HTTPS_PROXY` put every program on the machine through
/// `127.0.0.1:53129`. Same reach for the language server, no reach at all for
/// anything else.
pub const PROXY_ENV_VAR: &str = crate::patch_binary::PROXY_VAR_NEW;

/// What builds up to `2.11.0_4` wrote into the User environment, and what every
/// machine upgrading from one still carries. Removed on sight - but only when
/// the value is one of ours (I45) - by `remove_legacy_proxy_env`.
///
/// `HTTPS_PROXY` doubles as the name a *user's own* proxy lives under, which is
/// why removal is value-scoped.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const LEGACY_PROXY_ENV_VAR: &str = "HTTPS_PROXY";
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const NO_PROXY_ENV_VAR: &str = "NO_PROXY";
/// Node keeps its own bundled trust store and does not read the Windows one, so
/// installing the CA as a system root is not enough: Antigravity's extension
/// host is Node, it calls the gate host itself, and it answered `self signed
/// certificate in certificate chain` until this was set. Points at the same
/// per-machine CA that is already in the user's root store, so it widens nothing
/// that was not already trusted.
pub const NODE_CA_ENV_VAR: &str = "NODE_EXTRA_CA_CERTS";
/// The companion `NO_PROXY` those builds wrote, kept only so the value can be
/// recognised as ours and taken back off.
///
/// Nothing writes it any more, and nothing needs to: `httpproxy.useProxy`
/// bypasses `localhost` and every loopback address unconditionally, in the
/// library, so the one thing this value protected - the language server's own
/// gRPC on `127.0.0.1` - was never at risk. It existed for the Node side, which
/// only ever saw it because the variable was user-wide.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const NO_PROXY_VALUE: &str = "127.0.0.1,localhost,::1";

/// Removes the HTTPS_PROXY/NO_PROXY values written by old builds.
/// Standard proxy variables are removed only when their values identify ours.
/// Returns whether anything was removed; user-owned values are preserved.
#[cfg(target_os = "windows")]
pub fn remove_legacy_proxy_env(url: &str) -> Result<bool, String> {
    let (drop_proxy, drop_no_proxy) = legacy_removals(
        current_env(LEGACY_PROXY_ENV_VAR).as_deref(),
        current_env(NO_PROXY_ENV_VAR).as_deref(),
        url,
    );
    let mut removed = false;
    let mut trouble: Vec<String> = Vec::new();

    if drop_proxy {
        match clear_env(LEGACY_PROXY_ENV_VAR) {
            Ok(()) => removed = true,
            Err(e) => trouble.push(e),
        }
    }
    if drop_no_proxy {
        match clear_env(NO_PROXY_ENV_VAR) {
            Ok(()) => removed = true,
            Err(e) => trouble.push(e),
        }
    }

    if trouble.is_empty() {
        Ok(removed)
    } else {
        Err(trouble.join("; "))
    }
}

/// Which of the two legacy variables this tool may take off, given what they
/// currently hold. Pure, so the one rule that can destroy a user's configuration
/// is testable without an environment.
///
/// `NO_PROXY` goes **only alongside** ours, and that condition is the whole
/// safety of this step. It carries no address to recognise, so the only evidence
/// it is ours is the exact value - and `127.0.0.1,localhost,::1` is an entirely
/// ordinary thing for someone to set for themselves. Since `_5` this runs on the
/// *patch* path and at every relay start, not only on an undo, so an
/// unconditional match would delete that user's variable, and delete it again
/// after every reboot they re-set it. Our `HTTPS_PROXY` sitting beside it is the
/// evidence that the pair was written by an older build of this tool.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn legacy_removals(https: Option<&str>, no_proxy: Option<&str>, url: &str) -> (bool, bool) {
    let drop_proxy = https.is_some_and(|v| is_ours_on_any_port(v, url));
    (drop_proxy, drop_proxy && no_proxy == Some(NO_PROXY_VALUE))
}

/// Removes value-matched legacy proxy variables from the running user manager.
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn remove_legacy_proxy_env(url: &str) -> Result<bool, String> {
    use std::process::Command;
    let Ok(out) = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
    else {
        return Ok(false);
    };
    let env = String::from_utf8_lossy(&out.stdout);
    let mut removed = false;
    for name in [
        LEGACY_PROXY_ENV_VAR,
        "https_proxy",
        NO_PROXY_ENV_VAR,
        "no_proxy",
    ] {
        let prefix = format!("{}=", name);
        let Some(value) = env
            .lines()
            .find_map(|l| l.strip_prefix(prefix.as_str()))
            .map(str::trim)
        else {
            continue;
        };
        // `NO_PROXY` only once one of ours has already been recognised, for the
        // reason spelled out in the Windows arm: the value is a plausible thing
        // for a user to have set. The proxy names come first in the list, so
        // `removed` is already decided by the time it is reached.
        let ours = if name.eq_ignore_ascii_case(NO_PROXY_ENV_VAR) {
            removed && value == NO_PROXY_VALUE
        } else {
            is_ours_on_any_port(value, url)
        };
        if !ours {
            continue;
        }
        // `.success()`, not `.is_ok()`: the latter is true for any process that
        // merely started, so a failed unset would be reported as a removal.
        if Command::new("systemctl")
            .args(["--user", "unset-environment", name])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            removed = true;
        }
    }
    Ok(removed)
}

// The shell-file hook is Linux's (`remove_shell_hooks`); the tests run everywhere.
#[cfg_attr(any(target_os = "windows", target_os = "macos"), allow(dead_code))]
const SHELL_BEGIN: &str = "# >>> ag-unlocker >>>";
#[cfg_attr(any(target_os = "windows", target_os = "macos"), allow(dead_code))]
const SHELL_END: &str = "# <<< ag-unlocker <<<";

/// `text` with our block replaced by `block` (or removed when `block` is None).
/// Returns None when nothing would change.
#[cfg_attr(any(target_os = "windows", target_os = "macos"), allow(dead_code))]
fn rewrite_shell_block(text: &str, block: Option<&str>) -> Option<String> {
    let out = match (text.find(SHELL_BEGIN), text.find(SHELL_END)) {
        (Some(b), Some(e)) if e > b => {
            let end = e + SHELL_END.len();
            let end = if text[end..].starts_with('\n') {
                end + 1
            } else {
                end
            };
            format!("{}{}{}", &text[..b], block.unwrap_or(""), &text[end..])
        }
        _ => match block {
            Some(blk) => {
                let sep = if text.is_empty() || text.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                format!("{text}{sep}{blk}")
            }
            None => return None,
        },
    };
    (out != text).then_some(out)
}

/// Shell files that reach what `environment.d` does not: `~/.profile` (lightdm
/// and login shells) and `~/.bashrc` (a terminal's non-login shell). `.profile`
/// is created when missing; `.bashrc` only edited if the user has one.
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn shell_files(home: &Path) -> [(PathBuf, bool); 2] {
    [(home.join(".profile"), true), (home.join(".bashrc"), false)]
}

/// Takes our block back out of the shell files (revert).
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn remove_shell_hooks() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    for (file, _) in shell_files(&home) {
        if let Ok(text) = fs::read_to_string(&file) {
            if let Some(new) = rewrite_shell_block(&text, None) {
                let _ = fs::write(&file, new);
            }
        }
    }
}

/// `~/.config/environment.d/ag-unlocker.conf`, honouring `XDG_CONFIG_HOME`.
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn environment_d_path() -> Result<PathBuf, String> {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var("HOME").map_err(|_| "HOME не задан".to_string())?)
            .join(".config"),
    };
    Ok(base.join("environment.d").join("ag-unlocker.conf"))
}

/// Removes them, but only while they still hold what we wrote - a user may have
/// a proxy of their own, and taking that away would be worse than any bug here.
#[cfg(target_os = "windows")]
pub fn remove_proxy(url: &str, ca_path: &str) -> Result<(), String> {
    // Every variable is judged and removed on its own, and nothing stops at the
    // first failure. A half-removed proxy is worse than either state: the value
    // left behind names a loopback port whose listener the revert has just
    // deleted, so every program that honours it loses the network. That was
    // reported from a real machine as "не работает выход в интернет".
    let mut trouble: Vec<String> = Vec::new();

    // Unconditional, unlike everything else here: `PROXY_ENV_VAR` is a name only
    // this tool ever writes, so any value under it is ours whatever port it
    // names. That matters for the day the listener moves off 53129 (P26) - a
    // value-scoped removal would leave the old one behind forever.
    if current_env(PROXY_ENV_VAR).is_some() {
        if let Err(e) = clear_env(PROXY_ENV_VAR) {
            trouble.push(e);
        }
    }
    // The user-wide variables of builds up to `2.11.0_4`, value-scoped because
    // `HTTPS_PROXY` is also where a user's own proxy lives.
    if let Err(e) = remove_legacy_proxy_env(url) {
        trouble.push(e);
    }
    if let Err(e) = clear_node_ca(ca_path) {
        trouble.push(e);
    }

    if trouble.is_empty() {
        Ok(())
    } else {
        Err(trouble.join("; "))
    }
}

/// Linux: delete the `environment.d` drop-in and unset the live session vars.
/// Only ever removes our own file (the path is ours by construction) and our own
/// variable; the standard names an older build exported are unset by value, so a
/// proxy the user set another way survives the revert.
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn remove_proxy(url: &str, _ca_path: &str) -> Result<(), String> {
    use std::process::Command;
    remove_shell_hooks();
    if let Ok(path) = environment_d_path() {
        if fs::remove_file(&path).is_ok() {
            // Measured (WSL, systemd user manager): a variable the environment.d
            // generator loaded at the last reload survives `unset-environment`,
            // so every program started afterwards still got the address of a
            // proxy that is gone. A reload re-reads environment.d without our file.
            Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status()
                .ok();
        }
    }
    Command::new("systemctl")
        .args(["--user", "unset-environment", PROXY_ENV_VAR])
        .status()
        .ok();
    remove_legacy_proxy_env(url).ok();
    Ok(())
}

// ---- macOS (S80): the launchd environment is the session manager. An app
// started from the Dock or Finder inherits what `launchctl setenv` put there;
// the `com.agunlocker.env` login agent sets it again after a reboot. Only
// `PROXY_ENV_VAR`, for the same reason as on Linux (D19).

/// No build ever wrote the standard names on a Mac, so there is nothing
/// legacy to migrate; a user's own `HTTPS_PROXY` is not ours to touch.
#[cfg(target_os = "macos")]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn remove_legacy_proxy_env(_url: &str) -> Result<bool, String> {
    Ok(false)
}

#[cfg(target_os = "macos")]
pub fn remove_proxy(_url: &str, _ca_path: &str) -> Result<(), String> {
    crate::macos::unpersist_env(PROXY_ENV_VAR);
    Ok(())
}

/// The process environment, then launchd's, then the login agent's plist.
#[cfg(target_os = "macos")]
fn current_env(name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(name) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    crate::macos::getenv(name).or_else(|| crate::macos::persisted_env(name))
}

/// Whether an `HTTPS_PROXY` value is one this tool wrote.
///
/// Compared on the **address**, not on the exact string. A value that has been
/// through a settings dialog, a shell or another tool's rewrite can differ from
/// what we wrote by a trailing slash or the case of the scheme and still be
/// ours - and an exact comparison used to bail on that, leaving the variable
/// pointing at a port that no longer answers.
///
/// Safe because the address is loopback and a fixed port that only this tool
/// listens on: a proxy the user chose for themselves never names it. Their own
/// value is left alone, which matters more than removing ours.
/// Ours at all: the listener's current address, or the default port it had
/// before the relay had to move it (`legacy_proxy::port`). For "is it someone else's"
/// and "may we take it off" - a value naming the old port is still ours to
/// remove, never a foreign proxy to stand aside for. "Is it ours *and still
/// current*" is `is_our_proxy_value`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn is_ours_on_any_port(value: &str, url: &str) -> bool {
    is_our_proxy_value(value, url) || is_our_proxy_value(value, &crate::legacy_proxy::default_url())
}

fn is_our_proxy_value(value: &str, url: &str) -> bool {
    let strip = |s: &str| {
        s.trim()
            .trim_end_matches('/')
            .to_ascii_lowercase()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string()
    };
    let ours = strip(url);
    // Equality, not `contains`: a test caught `socks5://127.0.0.1:53129x`
    // passing a substring check, and `…:531290` would have too. Removing a
    // proxy that is not ours is the worse of the two failures here.
    !ours.is_empty() && strip(value) == ours
}

/// Drops `NODE_EXTRA_CA_CERTS` if it still holds `ca_path`, leaving a value the
/// user set themselves alone.
///
/// This also supports cleanup of older builds that installed a local CA.
pub fn clear_node_ca(ca_path: &str) -> Result<(), String> {
    if current_env(NODE_CA_ENV_VAR).as_deref() == Some(ca_path) {
        clear_env(NODE_CA_ENV_VAR)?;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn clear_env(name: &str) -> Result<(), String> {
    let script = format!(
        "[Environment]::SetEnvironmentVariable('{}',$null,'User')", name
    );
    powershell(&script).ok_or_else(|| format!("не удалось удалить {}", name))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn clear_env(_name: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn current_env(name: &str) -> Option<String> {
    let out = powershell(&format!(
        "[Environment]::GetEnvironmentVariable('{}','User')",
        name
    ))?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// On Linux reads the process environment, the systemd user manager session,
/// or the persistent environment.d drop-in.
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn current_env(name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(name) {
        let trimmed = v.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    if let Ok(out) = std::process::Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
    {
        let env_str = String::from_utf8_lossy(&out.stdout);
        let prefix = format!("{}=", name);
        if let Some(val) = env_str.lines().find_map(|l| l.strip_prefix(&prefix)) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    if let Ok(path) = environment_d_path() {
        if let Ok(content) = fs::read_to_string(&path) {
            let prefix = format!("{}=", name);
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('#') {
                    continue;
                }
                if let Some(val) = trimmed.strip_prefix(&prefix) {
                    let val_trimmed = val.trim().to_string();
                    if !val_trimmed.is_empty() {
                        return Some(val_trimmed);
                    }
                }
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn current_cli_endpoint() -> Option<String> {
    let out = powershell(&format!(
        "[Environment]::GetEnvironmentVariable('{}','User')",
        CLI_ENV_VAR
    ))?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(not(target_os = "windows"))]
fn current_cli_endpoint() -> Option<String> {
    None
}

/// The value of our proxy variable as the user environment has it right now.
/// Read-only; `None` when it is not set.
pub fn proxy_env_value() -> Option<String> {
    current_env(PROXY_ENV_VAR)
}

/// True when the variable is set AND names our own local listener rather than
/// something the user configured themselves.
#[allow(dead_code)]
pub fn proxy_env_is_ours() -> bool {
    let ours = crate::legacy_proxy::proxy_url();
    proxy_env_value().is_some_and(|v| is_our_proxy_value(&v, &ours))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shell block is added once, is idempotent, keeps the user's own lines
    /// and comes out again exactly as it went in.
    #[test]
    fn the_shell_block_round_trips() {
        let block = format!("{SHELL_BEGIN}\nx\n{SHELL_END}\n");
        let user = "export A=1\nalias l='ls'";
        let added = rewrite_shell_block(user, Some(&block)).unwrap();
        assert!(added.starts_with("export A=1\nalias l='ls'\n# >>>"));
        assert_eq!(rewrite_shell_block(&added, Some(&block)), None);
        let removed = rewrite_shell_block(&added, None).unwrap();
        assert_eq!(removed, format!("{user}\n"));
        assert_eq!(rewrite_shell_block("", None), None);
    }

    /// The bug users hit: a revert that left `HTTPS_PROXY` naming a port it had
    /// just deleted, so nothing on the machine could reach the network. Part of
    /// it was an exact string comparison - a value that had been through a
    /// settings dialog or another tool came back with a trailing slash or a
    /// different case and was not recognised as ours.
    #[test]
    fn our_proxy_value_is_recognised_however_it_was_written_back() {
        let url = "http://127.0.0.1:53129";
        for shape in [
            "http://127.0.0.1:53129",
            "http://127.0.0.1:53129/",
            "HTTP://127.0.0.1:53129",
            "  http://127.0.0.1:53129  ",
            "127.0.0.1:53129",
            "https://127.0.0.1:53129",
        ] {
            assert!(
                is_our_proxy_value(shape, url),
                "should be ours: {:?}",
                shape
            );
        }
    }

    /// `NO_PROXY` is the one variable this tool can delete without being able to
    /// recognise it: the value is all there is, and `127.0.0.1,localhost,::1` is
    /// something a user may well have set. Since `_5` the migration runs on the
    /// patch path and at every relay start, so an unconditional match would take
    /// it off a machine that had never run this tool - and again after every
    /// reboot they put it back. It goes only in company.
    #[test]
    fn no_proxy_is_only_removed_beside_a_proxy_value_of_ours() {
        let url = "http://127.0.0.1:53129";
        let ours = Some("http://127.0.0.1:53129");
        let theirs = Some("http://corp.example:3128");
        let np = Some(NO_PROXY_VALUE);

        // The pair an older build wrote: both go.
        assert_eq!(legacy_removals(ours, np, url), (true, true));
        // Never used this tool, but happens to have that NO_PROXY: nothing goes.
        assert_eq!(legacy_removals(None, np, url), (false, false));
        // Their own proxy plus that NO_PROXY: still nothing.
        assert_eq!(legacy_removals(theirs, np, url), (false, false));
        // Ours, but a NO_PROXY they changed: only ours goes.
        assert_eq!(
            legacy_removals(ours, Some("127.0.0.1,corp.example"), url),
            (true, false)
        );
        // Ours alone.
        assert_eq!(legacy_removals(ours, None, url), (true, false));
    }

    /// The other half, and the more important one: a proxy the user chose for
    /// themselves must survive our revert untouched. Removing someone else's
    /// proxy is a worse failure than leaving ours behind.
    #[test]
    fn a_proxy_the_user_chose_is_never_removed() {
        let url = "http://127.0.0.1:53129";
        for theirs in [
            "http://127.0.0.1:1371",
            "http://proxy.example.com:8080",
            "http://10.0.0.1:53129",
            "socks5://127.0.0.1:53129x",
            "",
        ] {
            assert!(
                !is_our_proxy_value(theirs, url),
                "must be left alone: {:?}",
                theirs
            );
        }
    }

    const REAL: &str = "{\n    \"workbench.colorTheme\": \"Solarized Dark\",\n    \"securecoder.enabled\": true\n}";

    #[test]
    fn the_key_is_added_and_the_rest_kept_byte_for_byte() {
        let out = upsert_key(REAL, IDE_SETTING, DAILY_ENDPOINT).expect("edited");
        assert!(
            out.contains("\"jetski.cloudCodeUrl\": \"https://daily-cloudcode-pa.googleapis.com\"")
        );
        // Every line the user had must survive, unchanged apart from the comma
        // that now separates it from ours.
        assert!(out.contains("\"workbench.colorTheme\": \"Solarized Dark\""));
        assert!(out.contains("\"securecoder.enabled\": true"));
        assert!(
            serde_json::from_str::<serde_json::Value>(&out).is_ok(),
            "{}",
            out
        );
    }

    #[test]
    fn writing_twice_changes_nothing_the_second_time() {
        let once = upsert_key(REAL, IDE_SETTING, DAILY_ENDPOINT).expect("edited");
        let twice = upsert_key(&once, IDE_SETTING, DAILY_ENDPOINT).expect("edited");
        assert_eq!(once, twice);
    }

    /// A value the user (or an older build) left behind must be corrected, not
    /// duplicated - two entries for one key is invalid JSON.
    #[test]
    fn a_stale_value_is_replaced_not_duplicated() {
        let stale = REAL.replace(
            "\"securecoder.enabled\": true",
            "\"securecoder.enabled\": true,\n    \"jetski.cloudCodeUrl\": \"https://cloudcode-pa.googleapis.com\"",
        );
        let out = upsert_key(&stale, IDE_SETTING, DAILY_ENDPOINT).expect("edited");
        assert_eq!(out.matches(IDE_SETTING).count(), 1);
        assert!(out.contains(DAILY_ENDPOINT));
        assert!(
            serde_json::from_str::<serde_json::Value>(&out).is_ok(),
            "{}",
            out
        );
    }

    #[test]
    fn an_empty_or_missing_file_becomes_a_valid_object() {
        for text in ["", "   \n"] {
            let out = upsert_key(text, IDE_SETTING, DAILY_ENDPOINT).expect("created");
            let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
            assert_eq!(parsed[IDE_SETTING], DAILY_ENDPOINT);
        }
    }

    #[test]
    fn an_empty_object_gets_no_stray_comma() {
        let out = upsert_key("{}", IDE_SETTING, DAILY_ENDPOINT).expect("edited");
        assert!(
            serde_json::from_str::<serde_json::Value>(&out).is_ok(),
            "{}",
            out
        );
        assert!(!out.contains("{,"));
    }

    /// The user's own trailing comma is legal JSONC and must not produce two.
    #[test]
    fn a_trailing_comma_is_not_doubled() {
        let out =
            upsert_key("{\n    \"a\": \"b\",\n}", IDE_SETTING, DAILY_ENDPOINT).expect("edited");
        assert!(!out.contains(",,"), "{}", out);
    }

    #[test]
    fn a_file_without_a_closing_brace_is_refused() {
        assert!(upsert_key("not json at all", IDE_SETTING, DAILY_ENDPOINT).is_err());
    }

    #[test]
    fn removal_restores_valid_json_without_our_key() {
        for original in [REAL, "{}", "{\n    \"a\": \"b\"\n}"] {
            let with = upsert_key(original, IDE_SETTING, DAILY_ENDPOINT).expect("edited");
            let without = remove_key(&with, IDE_SETTING).expect("removed");
            assert!(!without.contains(IDE_SETTING), "{}", without);
            assert!(
                serde_json::from_str::<serde_json::Value>(&without).is_ok(),
                "{}",
                without
            );
        }
    }

    /// Removing ours must not take the user's settings with it.
    #[test]
    fn removal_keeps_everything_else() {
        let with = upsert_key(REAL, IDE_SETTING, DAILY_ENDPOINT).expect("edited");
        let without = remove_key(&with, IDE_SETTING).expect("removed");
        let parsed: serde_json::Value = serde_json::from_str(&without).expect("valid json");
        assert_eq!(parsed["workbench.colorTheme"], "Solarized Dark");
        assert_eq!(parsed["securecoder.enabled"], true);
    }

    #[test]
    fn removing_a_key_that_is_not_there_is_a_no_op() {
        assert_eq!(remove_key(REAL, IDE_SETTING).expect("no-op"), REAL);
    }

    /// The endpoint has to be the host that is actually still substituted; the
    /// production one is exactly what stopped working.
    #[test]
    fn the_endpoint_is_the_daily_host() {
        assert!(DAILY_ENDPOINT.starts_with("https://daily-cloudcode-pa."));
        assert!(!DAILY_ENDPOINT.contains("sandbox"));
    }

    /// Against the real install. It used to also assert that the build still
    /// reads `jetski.cloudCodeUrl`; nothing writes that key since 2.11.0, so the
    /// question left is the one `remove_ide` depends on - does the path resolve
    /// to the folder the IDE actually reads, so an override left by an earlier
    /// build is found and taken out.
    #[test]
    #[ignore = "needs a real Antigravity IDE install; run with --ignored"]
    fn finds_the_real_ide_settings_file() {
        let install = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA"))
            .join("Programs")
            .join("Antigravity IDE");
        if !install.exists() {
            println!("no IDE install at {}", install.display());
            return;
        }
        let path = ide_settings_path(&install).expect("settings path");
        println!("settings: {} (exists: {})", path.display(), path.exists());
        assert!(
            path.ends_with("User\\settings.json"),
            "unexpected shape: {}",
            path.display()
        );
    }

    #[test]
    fn proxy_env_queries_run_without_panic() {
        let _ = proxy_env_value();
        let _ = proxy_env_is_ours();
    }
}
