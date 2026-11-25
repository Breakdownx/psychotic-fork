//! What older builds left behind when they ran a local HTTP proxy: the address
//! they pointed clients at and, on Windows, the CA they trusted. Only used to
//! recognise and remove that state; nothing here runs or connects to a proxy.

use std::fs;
use std::path::PathBuf;

pub const LISTEN_IP: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 53129;

/// Last recorded port, used only to recognize old environment values on removal.
pub fn port() -> u16 {
    fs::read_to_string(crate::dns_forwarder::log_dir().join("proxy_port"))
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_PORT)
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn default_url() -> String {
    url_at(DEFAULT_PORT)
}
pub fn proxy_url() -> String {
    url_at(port())
}
pub fn url_at(port: u16) -> String {
    format!("http://{}:{}", LISTEN_IP, port)
}

#[cfg(target_os = "windows")]
const CA_NAME: &str = "AG Unlocker local CA";
pub fn ca_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("AGUnlocker")
}

pub fn ca_cert_path() -> PathBuf {
    ca_dir().join("ca.pem")
}

#[cfg(target_os = "windows")]
fn ca_key_path() -> PathBuf {
    ca_dir().join("ca.key")
}

/// Takes the legacy CA out of the user's root store and deletes its files.
/// Windows only: the CA was only ever trusted through `certutil`, and outside
/// Windows `LOCALAPPDATA` is unset, so `ca_dir` would be relative to the cwd.
#[cfg(target_os = "windows")]
pub fn untrust_ca() {
    let mut cmd = std::process::Command::new("certutil");
    cmd.args(["-user", "-delstore", "Root", CA_NAME]);
    crate::utils::no_window(&mut cmd).output().ok();
    fs::remove_file(ca_cert_path()).ok();
    fs::remove_file(ca_key_path()).ok();
}

/// Takes the legacy CA out (`endpoint::clear_node_ca`, then the root store),
/// without a word of output. Returns whether anything was actually taken out.
///
/// Both guards are load-bearing and neither is cosmetic (I39): nothing to remove
/// is the common case, and pulling the root from under a relay that is still an
/// older build makes every gate request die as `BadCertificate`.
#[cfg(target_os = "windows")]
pub fn remove_legacy_ca_quiet() -> bool {
    let cert = ca_cert_path();
    if !cert.exists() {
        return false;
    }
    if crate::background::relay_is_outdated() {
        return false;
    }
    crate::endpoint::clear_node_ca(&cert.to_string_lossy()).ok();
    untrust_ca();
    true
}

/// Outside Windows no build ever trusted a CA, so there is nothing to remove.
#[cfg(not(target_os = "windows"))]
pub fn remove_legacy_ca_quiet() -> bool {
    false
}
