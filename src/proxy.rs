//! Compatibility helpers for removing proxy state created by older builds.
//! This module does not run or connect to an HTTP proxy.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use rustls::ClientConfig;
use crate::utils::no_window;

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

pub fn default_url() -> String {
    url_at(DEFAULT_PORT)
}
pub fn proxy_url() -> String {
    url_at(port())
}
pub fn url_at(port: u16) -> String {
    format!("http://{}:{}", LISTEN_IP, port)
}

const CA_NAME: &str = "AG Unlocker local CA";
pub fn ca_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("AGUnlocker")
}

pub fn ca_cert_path() -> PathBuf {
    ca_dir().join("ca.pem")
}

fn ca_key_path() -> PathBuf {
    ca_dir().join("ca.key")
}

pub fn untrust_ca() {
    let mut cmd = std::process::Command::new("certutil");
    cmd.args(["-user", "-delstore", "Root", CA_NAME]);
    no_window(&mut cmd).output().ok();
    fs::remove_file(ca_cert_path()).ok();
    fs::remove_file(ca_key_path()).ok();
}

pub fn probe_config() -> Arc<ClientConfig> {
    upstream_config()
}

fn upstream_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let mut cfg = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(cfg)
    })
    .clone()
}

pub fn is_gate_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "cloudcode-pa.googleapis.com"
        || h == "daily-cloudcode-pa.googleapis.com"
        || h == "aicode.googleapis.com"
}
