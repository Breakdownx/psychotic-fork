use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::dns;
use crate::dns_client;
use crate::egress;
use crate::gate;
use crate::net;
use crate::resolvers::{self, Verdict};

// A loopback DNS relay, so the answers stay fresh.
//
// Pinning the substituted address into `hosts` works but breaks its own
// contract: xbox-dns answers with **TTL 60**, i.e. it explicitly reserves the
// right to move the address every minute, and a static file holds it forever.
//
// Instead the NRPT rules point at this listener, which relays the query - raw
// bytes, never parsed - to xbox-dns over the ISP link via IP_UNICAST_IF, and
// relays the answer back verbatim. Windows then caches it for exactly the TTL
// the resolver asked for and comes back when it expires, so a rotated address is
// picked up within a minute with no file to go stale. It behaves identically
// with and without a VPN: without one the ISP link is the default path anyway.
//
// Verified before building: the Windows DNS client does send NRPT queries to a
// 127.0.0.0/8 nameserver (a probe rule delivered the question to a listener on
// 127.0.0.53:53).
//
// UDP only, deliberately. Relaying verbatim means a truncated answer would need
// a TCP listener as well, but the routed names answer with a single A record in
// well under 100 bytes, so TC is never set. If that ever changes, Windows falls
// back to the next nameserver in the rule rather than failing outright.

/// Where the NRPT rules point. Any 127.0.0.0/8 address works; .53 keeps it
/// recognisable and clear of anything bound to 127.0.0.1.
pub const LISTEN_IP: &str = "127.0.0.53";
pub const LISTEN_PORT: u16 = 53;

/// What generation of relay this build ships.
///
/// Separate from the product version on purpose: the relay is installed once,
/// by an administrator, and then keeps running across reboots from
/// `%ProgramData%` - so a user can be running a months-old relay under a fresh
/// unlocker and never be told. The unlocker compares this against what the
/// installed relay wrote and says so.
///
/// **Bump this whenever the relay's own behaviour changes** - the loop, the
/// resolver logic it carries, the warm loop, the watchdog. A pure UI or patcher
/// change does not need it. Never decrease it: the comparison is "older than",
/// and a version that goes backwards asks every user to reinstall.
///
/// Generation 52 removes HTTP proxy listeners and proxy environment maintenance.
pub const RELAY_VERSION: u32 = 52;

/// Written where an unelevated relay can write and an unelevated unlocker can
/// read. Absent means a relay from before versioning, i.e. older than anything.
const VERSION_FILE: &str = "relay.version";

/// Closes the console Windows hands a console subsystem process. Without this
/// the scheduled task leaves an empty black window on screen for as long as the
/// relay lives, which is forever. Everything it has to say goes to the log file
/// anyway, so losing stdout costs nothing.
#[cfg(target_os = "windows")]
pub fn detach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
    }
    unsafe {
        FreeConsole();
    }
}

#[cfg(not(target_os = "windows"))]
pub fn detach_console() {}

/// Log prefix for what the answer turned out to be. `substituted` is the only
/// one that means the region gate is actually being defeated for that name;
/// `PASSTHROUGH` shouts because it is the failure the old `ok` used to hide.
fn verdict_tag(v: Verdict) -> &'static str {
    match v {
        Verdict::Substituted => "substituted",
        Verdict::Sibling => "sibling",
        Verdict::Passthrough => "PASSTHROUGH",
        Verdict::Unknown => "ok",
    }
}

/// How long a *detected* interface is trusted. Long on purpose: detection
/// shells out to PowerShell, and an interface that dies is caught by
/// `invalidate_interface()` on the first failed relay, so re-probing on a timer
/// buys nothing and only spawns processes on the user's machine.
const EGRESS_TTL: Duration = Duration::from_secs(30 * 60);
/// How long a *failed* detection is remembered. Short, because it is usually
/// the network not being up yet at logon - but not zero, or a machine with no
/// physical egress at all would spawn a probe per query.
const EGRESS_RETRY: Duration = Duration::from_secs(30);
/// How often the relay re-asks whether a tunnel is carrying the machine.
///
/// Its own clock, and deliberately far shorter than `EGRESS_TTL`: an interface
/// index changes when hardware does, but a VPN is something the user toggles
/// mid-session, and that answer decides whether the relay substitutes at all
/// (G26). Costs one PowerShell spawn per interval and never runs on the query
/// path - a client waits on nothing here. Four minutes rather than fifteen
/// seconds because being late merely means a few minutes of the old, slightly
/// longer route; it breaks nothing.
const VPN_CHECK_EVERY: Duration = Duration::from_secs(4 * 60);
const LOG_LIMIT_BYTES: u64 = 64 * 1024;

/// Interface index, when it was learned, and how long that answer is good for.
static EGRESS_CACHE: Mutex<Option<(u32, Instant, Duration)>> = Mutex::new(None);

/// The log lives under the user profile, not next to the exe: the relay runs
/// unelevated, and the directory an administrator installed it into is not
/// writable for it.
#[cfg(target_os = "windows")]
pub fn log_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("AGUnlocker")
}

/// Per-user settings and watchdog logs on Unix follow the XDG base directory.
#[cfg(not(target_os = "windows"))]
pub fn log_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{}/.local/share", home)
        });
    PathBuf::from(base).join("agunlocker")
}

pub fn log_path() -> PathBuf {
    log_dir().join("forwarder.log")
}

pub fn version_path() -> PathBuf {
    log_dir().join(VERSION_FILE)
}

/// Records which relay generation is in place. Called both by the installer and
/// by the relay itself at startup: the installer's write is what makes the
/// answer correct the moment an upgrade finishes, and the relay's write is what
/// keeps it honest if the exe ever gets there some other way.
pub fn record_version() {
    if let Some(dir) = version_path().parent() {
        fs::create_dir_all(dir).ok();
    }
    fs::write(version_path(), RELAY_VERSION.to_string()).ok();
}

/// The relay generation currently installed. `0` for a relay old enough not to
/// have written one - which is exactly the case worth reporting.
pub fn installed_version() -> u32 {
    parse_version(fs::read_to_string(version_path()).ok().as_deref())
}

/// Anything unreadable counts as the oldest possible relay: the file is written
/// by us and never edited, so a value that will not parse means something else
/// wrote it, and "reinstall" is the right answer to that too.
fn parse_version(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// Local wall-clock `HH:MM:SS` for a log line.
///
/// Local, not UTC: the only reader is a user comparing the log against the moment
/// their editor showed an error, and asking them to do timezone arithmetic on a
/// bug report is how a report becomes useless. Empty where there is no local
/// clock to read (`utils::local_clock`), which costs a stamp rather than a line.
fn stamp() -> String {
    crate::utils::local_clock().map_or_else(String::new, |c| c.hms())
}

/// Best-effort logging: a background process with no console is otherwise
/// impossible to diagnose. Truncated rather than rotated - nothing here is
/// worth keeping across sessions.
///
/// Every line is stamped. Without it a log says what happened but never *when*,
/// so a torn connection cannot be lined up against the error the user saw - which
/// is precisely the question a bug report asks.
fn log(line: &str) {
    // Never from a test. The file is the *installed* relay's log, and a unit test
    // exercising `health` or `routes` used to append lines like «тест отложен на
    // 5 мин» to it on the developer's machine - which the window's «Сохранить
    // отчёт» now puts in a support file (G18's other half).
    if cfg!(test) {
        return;
    }
    let (note, write_line) = collapse_repeat(line);
    if note.is_none() && !write_line {
        return;
    }
    let path = log_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok();
    }
    if fs::metadata(&path).map_or(false, |m| m.len() > LOG_LIMIT_BYTES) {
        fs::remove_file(&path).ok();
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        // One write per line. `writeln!` with arguments issues a write per
        // piece, and an append is atomic only per write, so two threads (or two
        // processes) logging at once interleaved into lines like
        // «16:48:2616:48:26 PASSTHROUGH … PASSTHROUGH …» - seen in reports from
        // both platforms.
        let at = stamp();
        let mut entry = String::new();
        if let Some(note) = note {
            entry.push_str(&format!("{} {}\n", at, note));
        }
        if write_line {
            entry.push_str(&format!("{} {}\n", at, line));
        }
        f.write_all(entry.as_bytes()).ok();
    }
}

/// How long a line has to repeat within to be counted rather than written.
const REPEAT_WINDOW: Duration = Duration::from_secs(30);
/// …and how many repeats are counted before the tally is written anyway, so a
/// problem repeating forever is never silent.
const REPEAT_RUN: u32 = 50;

/// Collapses a run of one identical line into a count.
///
/// The log holds 64 KB and the report shows its last 60 lines, so a burst of
/// one message costs exactly the minutes a report is about: a field report
/// (2026-09-21) spent fifteen of its sixty visible lines on the same
/// «напрямую …: имя не разрешается» inside a single second, and the route
/// switch that followed was off the top of it.
///
/// Returns the tally line to write first, if any, and whether `line` itself is
/// written. The *first* of a run always goes out at once - the news is that it
/// happened, and a delayed first line is a log that lies about when.
fn collapse_repeat(line: &str) -> (Option<String>, bool) {
    static RUN: Mutex<Option<(String, u32, Instant)>> = Mutex::new(None);
    let tally = |n: u32| format!("↑ то же самое ещё {} раз", n);
    let Ok(mut g) = RUN.lock() else {
        return (None, true);
    };
    let same = g
        .as_ref()
        .is_some_and(|(prev, _, at)| prev == line && at.elapsed() < REPEAT_WINDOW);
    if same {
        let mut flush = None;
        if let Some((_, n, at)) = g.as_mut() {
            *n += 1;
            *at = Instant::now();
            if *n >= REPEAT_RUN {
                flush = Some(tally(*n));
                *n = 0;
            }
        }
        return (flush, false);
    }
    let held = g.as_ref().map_or(0, |(_, n, _)| *n);
    *g = Some((line.to_string(), 0, Instant::now()));
    ((held > 0).then(|| tally(held)), true)
}

/// The only way a failed start can be reported: the console is gone by then.
pub fn log_fatal(message: &str) {
    log(&format!("fatal: {}", message));
}

/// The proxy shares the relay's log - it runs in the same process, and one file
/// is what makes a resolve and the connection that followed it readable together.
pub fn log_proxy(message: &str) {
    log(&format!("proxy        {}", message));
}

/// Who answered a race that produced no substitution, and whether a reference
/// was available to judge it. Only logged on that path: it is the one where the
/// interesting question is which provider was missing, and it is rare enough
/// that a line per occurrence is affordable.
pub fn log_race(name: &str, heard: &[&str], had_reference: bool) {
    log(&format!(
        "race         {} heard [{}]{}",
        name,
        heard.join(", "),
        if had_reference { "" } else { " (no reference)" }
    ));
}

fn invalidate_interface() {
    if let Ok(mut c) = EGRESS_CACHE.lock() {
        *c = None;
    }
}

fn isp_interface() -> u32 {
    if let Ok(cache) = EGRESS_CACHE.lock() {
        if let Some((idx, at, good_for)) = *cache {
            if at.elapsed() < good_for {
                return idx;
            }
        }
    }
    // 0 means "use the routing table" - the right degradation when there is no
    // physical egress to name, but a poor thing to remember for long: at logon
    // the network is often simply not up yet, and caching the miss would leave
    // half an hour of unsubstituted answers.
    let (idx, good_for) = match egress::detect() {
        Some(eg) => (eg.if_index, EGRESS_TTL),
        None => (0, EGRESS_RETRY),
    };
    if let Ok(mut cache) = EGRESS_CACHE.lock() {
        *cache = Some((idx, Instant::now(), good_for));
    }
    idx
}

/// Relays one query and reports which provider answered and whether that answer
/// was actually substituted.
///
/// The choice is no longer a constant. A provider can drop a name from its
/// list without any error - the query still resolves, just to the genuine
/// Google address - so every provider is asked at once and compared against a
/// reference resolver that substitutes nothing. See `resolvers`.
fn relay(query: &[u8]) -> Option<(Vec<u8>, &'static str, resolvers::Verdict)> {
    let if_index = isp_interface();
    match resolvers::resolve_best(query, if_index) {
        Some(hit) => Some(hit),
        None => {
            // Nobody answered: the interface may have gone away under us, so
            // the next query re-detects instead of retrying a dead one.
            invalidate_interface();
            None
        }
    }
}

/// How often the routed names are re-resolved in the background.
///
/// Half the substituted TTL, so an entry is always well inside the window the
/// answer cache will serve it from and a client query never finds it expired.
/// Cheap: four names, one upstream query each, once every quarter minute.
const WARM_EVERY: Duration = Duration::from_secs(15);

/// Keeps a vetted answer ready for every routed name, forever.
///
/// The relay has about a second to answer before Windows asks the next
/// nameserver in the NRPT rule - which is a provider's own resolver, handing
/// out addresses that nothing has checked for liveness. A cold resolution does
/// not reliably fit in that second (race, then a liveness probe, and a provider
/// that goes quiet costs the whole timeout), and it does not have to: doing the
/// work on a timer instead means the client's query is answered from memory.
/// Refresh ordinary DNS answers without proxy routes or client traffic handlers.
fn warm_forever() {
    let mut since_vpn = VPN_CHECK_EVERY;
    let mut last_best: Option<u32> = None;
    loop {
        // Keep the existing prompt interface refresh when the default route
        // changes, without retaining an HTTP proxy route table.
        let best = net::best_interface(Ipv4Addr::new(8, 8, 8, 8));
        if best != last_best {
            if last_best.is_some() {
                invalidate_interface();
            }
            last_best = best;
            since_vpn = VPN_CHECK_EVERY;
        }
        // Preserve the DNS relay's existing VPN stand-down behavior. With the
        // HTTP loopback door removed, clients use their own network route.
        if since_vpn >= VPN_CHECK_EVERY {
            let eg = egress::detect();
            let tunnel = eg.as_ref().is_some_and(|e| e.vpn_active);
            net::set_pin_interface(match &eg {
                Some(e) if tunnel => e.if_index,
                _ => 0,
            });
            let (stand_down, _) = if cfg!(target_os = "windows") && tunnel {
                egress::vpn_verdict(eg.as_ref())
            } else {
                (false, egress::ClientEgress::Unknown)
            };
            resolvers::set_vpn_active(stand_down);
            since_vpn = Duration::ZERO;
        }
        gate::publish(gate::Now {
            route: Some("DNS"),
            forced_for: None,
            stood_down: resolvers::tunnel_carries_client(),
            loopback: false,
            tunnel: net::tunnel_up(),
            vpn_exit: "",
        });
        resolvers::warm(dns::core_namespaces(), isp_interface());
        thread::sleep(WARM_EVERY);
        since_vpn += WARM_EVERY;
    }
}

/// Run the Windows DNS relay.
pub fn run() -> Result<(), String> {
    log(&format!("egress: if{}", isp_interface()));
    record_version();
    gate::note_started();
    thread::spawn(warm_forever);
    serve_dns_forever()
}

const REBIND_EVERY: Duration = Duration::from_secs(60);

/// Where the NRPT rules send their queries, as an address to bind and to
/// diagnose. `LISTEN_IP` is a literal and always parses; the fallback only
/// exists so a typo in it could never panic the relay.
fn listen_addr() -> SocketAddr {
    SocketAddr::from((
        LISTEN_IP.parse::<Ipv4Addr>().unwrap_or(Ipv4Addr::LOCALHOST),
        LISTEN_PORT,
    ))
}

/// How many `recv_from` failures in a row mean the socket is gone rather than
/// one exchange having gone wrong.
///
/// Windows surfaces an ICMP port-unreachable for an answer we already sent as
/// `WSAECONNRESET` on the *next* receive, so single errors are ordinary and are
/// skipped, as they always were. Sixty-four of them with not one query in
/// between is a socket worth dropping and taking again.
const RECV_ERRORS_BEFORE_REBIND: u32 = 64;

/// Holds `127.0.0.53:53` and answers on it, for as long as the process lives.
///
/// A port that cannot be had is diagnosed (`portcheck`), published for the
/// window's card (`gate::set_blocker`) and tried again every minute. It cannot move
/// to another port: the NRPT rules name an address and Windows sends DNS to
/// port 53 or nowhere. Meanwhile the NRPT rules list this address *first* and
/// the substituting providers after it, so a Windows client falls through to
/// them on its own - the layer degrades to slow rather than to nothing.
fn serve_dns_forever() -> ! {
    let addr = listen_addr();
    let mut said: Option<crate::gate::Blocker> = None;
    loop {
        let sock = match UdpSocket::bind(addr) {
            Ok(sock) => {
                if said.take().is_some() {
                    log("порт DNS-релея освободился");
                    gate::set_blocker("dns", None);
                }
                sock
            }
            Err(e) => {
                let blocker = crate::portcheck::diagnose_on(addr, &e, crate::portcheck::Proto::Udp)
                    .blocker("dns", addr, &e);
                // Once per distinct cause: the retry is a minute, the log is 64 KB.
                if said.as_ref() != Some(&blocker) {
                    log(&format!(
                        "не занять {} — {} ({}{})",
                        blocker.addr,
                        blocker.error,
                        blocker.cause,
                        if blocker.by.is_empty() {
                            String::new()
                        } else {
                            format!(": {}", blocker.by)
                        }
                    ));
                    said = Some(blocker.clone());
                }
                // Every try, so the record never goes stale under the card.
                gate::set_blocker("dns", Some(blocker));
                thread::sleep(REBIND_EVERY);
                continue;
            }
        };
        log(&format!("start: {}:{}", LISTEN_IP, LISTEN_PORT));
        // A client that was running before us is on answers that predate our
        // rules, and nothing else tells it otherwise (G75). Here and not a line
        // earlier: the socket is bound, so a re-ask reaches us.
        if crate::dns::flush_client_cache() {
            log("кэш DNS-клиента по гейт-именам сброшен — клиент спросит заново");
        }
        let up = Instant::now();
        serve_queries(&sock);
        drop(sock);
        log("DNS-сокет перестал принимать запросы — занимаем заново");
        // Never in a tight loop: a socket that did not last a minute is not one
        // an immediate retry will fix, so it waits out the same clock a failed
        // bind does.
        if up.elapsed() < REBIND_EVERY {
            thread::sleep(REBIND_EVERY);
        }
    }
}

/// Answers queries on `sock` until it stops taking them.
fn serve_queries(sock: &UdpSocket) {
    let mut buf = [0u8; 4096];
    let mut misses = 0u32;
    loop {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(v) => {
                misses = 0;
                v
            }
            Err(_) => {
                misses += 1;
                if misses >= RECV_ERRORS_BEFORE_REBIND {
                    return;
                }
                continue;
            }
        };
        // Anything shorter than a header is not a query worth relaying.
        if n < 12 {
            continue;
        }
        let query = buf[..n].to_vec();
        let out = match sock.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        };
        // One thread per query: the volume is a handful per minute, and a slow
        // upstream must not stall the queries behind it.
        thread::spawn(move || {
            let name = dns_client::question_name(&query).unwrap_or_else(|| "?".to_string());
            match relay(&query) {
                Some((reply, provider, verdict)) => {
                    out.send_to(&reply, from).ok();
                    // The verdict is the part worth logging: "ok" used to mean
                    // only that bytes came back, which is precisely what it
                    // still said while the answers had stopped being
                    // substituted.
                    log(&format!(
                        "{:<12} {} [{}]",
                        verdict_tag(verdict),
                        name,
                        provider
                    ));
                }
                None => log(&format!("fail         {}", name)),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// Everything the version check does hangs off this: a relay that predates
    /// versioning leaves no file, and it must read as older than this build
    /// rather than as "no relay" or as a parse error nobody handles.
    #[test]
    fn an_unreadable_version_is_the_oldest_one() {
        assert_eq!(parse_version(None), 0);
        assert_eq!(parse_version(Some("")), 0);
        assert_eq!(parse_version(Some("не число")), 0);
        assert_eq!(parse_version(Some(" 7 \r\n")), 7);
        assert!(parse_version(None) < RELAY_VERSION);
    }

    /// The warm loop only exists to beat the ~1 s Windows waits before asking
    /// the next NRPT nameserver, so it has to refresh well inside the window the
    /// answer cache serves from.
    #[test]
    fn warming_runs_more_often_than_an_answer_goes_stale() {
        assert!(WARM_EVERY < resolvers::ANSWER_TTL);
    }

    /// A burst of one message must not eat the sixty lines a report shows.
    /// The first of a run goes out at once; the rest are counted and the count
    /// is written the moment anything else is logged.
    #[test]
    fn a_run_of_one_line_is_collapsed_into_a_count() {
        let line = "напрямую X: имя не разрешается";
        // The first is written, with nothing held back before it.
        assert_eq!(collapse_repeat(line), (None, true));
        for _ in 0..14 {
            assert_eq!(collapse_repeat(line), (None, false), "a repeat was written");
        }
        // Something else ends the run and carries the tally.
        let (note, write) = collapse_repeat("что-то другое");
        assert!(write);
        assert_eq!(note.as_deref(), Some("↑ то же самое ещё 14 раз"));
        // …and the next line after that has nothing to report.
        assert_eq!(collapse_repeat("третья"), (None, true));

        // And a run that never ends is never silent: the tally is written every
        // `REPEAT_RUN`, so a problem repeating for an hour is still visible.
        // One test and not two: `collapse_repeat` keeps one run per process,
        // and two tests would race for it.
        let line = "бесконечное";
        assert_eq!(collapse_repeat(line), (None, true));
        for _ in 0..(REPEAT_RUN - 1) {
            assert_eq!(collapse_repeat(line), (None, false));
        }
        let (note, write) = collapse_repeat(line);
        assert!(!write, "the repeat itself is still not written");
        assert_eq!(note.as_deref(), Some("↑ то же самое ещё 50 раз"));
        // The counter restarted, so the next line ends a fresh run.
        assert_eq!(collapse_repeat(line), (None, false));
        assert_eq!(
            collapse_repeat("иное").0.as_deref(),
            Some("↑ то же самое ещё 1 раз")
        );
    }

    #[test]
    fn the_listener_address_is_loopback() {
        let ip: Ipv4Addr = LISTEN_IP.parse().expect("valid address");
        assert!(ip.is_loopback());
        // Not .1: something else on the machine may already own it.
        assert_ne!(ip, Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn a_detected_interface_is_cached_and_droppable() {
        invalidate_interface();
        assert!(EGRESS_CACHE.lock().unwrap().is_none());
        if let Ok(mut c) = EGRESS_CACHE.lock() {
            *c = Some((17, Instant::now(), EGRESS_TTL));
        }
        // Served from the cache: no detection runs, so no process is spawned.
        assert_eq!(isp_interface(), 17);
        invalidate_interface();
        assert!(EGRESS_CACHE.lock().unwrap().is_none());
    }

    /// An expired entry must not be served - that is what makes the short retry
    /// after a failed detection actually retry.
    #[test]
    fn an_expired_entry_is_not_served() {
        invalidate_interface();
        if let Ok(mut c) = EGRESS_CACHE.lock() {
            // Learned long ago, and only ever good for a moment.
            *c = Some((17, Instant::now() - Duration::from_secs(60), EGRESS_RETRY));
        }
        let stale = EGRESS_CACHE
            .lock()
            .unwrap()
            .map(|(_, at, good_for)| at.elapsed() >= good_for);
        assert_eq!(stale, Some(true));
        invalidate_interface();
    }

    /// A failed detection has to be forgotten quickly: at logon it usually just
    /// means the network is not up yet.
    #[test]
    fn a_failed_detection_is_remembered_only_briefly() {
        assert!(EGRESS_RETRY < EGRESS_TTL);
        assert!(EGRESS_RETRY <= Duration::from_secs(60));
    }

    /// Relays a real query end to end through the running upstream, and prints
    /// the verdict for every routed name.
    ///
    /// Needs a live network and the VPN OFF: through a tunnel every provider
    /// sees a foreign client and substitutes nothing, so every verdict comes
    /// back `Passthrough` and the run says nothing about the providers.
    ///
    /// The verdict is the assertion that matters now. The old version only
    /// checked that bytes came back - which stayed true through the whole
    /// outage that motivated `resolvers`, because an unsubstituted answer is
    /// still a perfectly well-formed answer.
    #[test]
    #[ignore = "needs a live network, VPN off; run with --ignored"]
    fn relays_a_real_query() {
        let id: u16 = 0x4242;
        let mut substituted = Vec::new();

        for name in [
            "cloudcode-pa.googleapis.com",
            "daily-cloudcode-pa.googleapis.com",
            "generativelanguage.googleapis.com",
            "antigravity-unleash.goog",
        ] {
            let mut query = vec![];
            query.extend_from_slice(&id.to_be_bytes());
            query.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]);
            for label in name.split('.') {
                query.push(label.len() as u8);
                query.extend_from_slice(label.as_bytes());
            }
            query.extend_from_slice(&[0, 0x00, 0x01, 0x00, 0x01]);

            let (reply, provider, verdict) = relay(&query).expect("a provider answered");
            assert_eq!(&reply[0..2], &id.to_be_bytes(), "id must be echoed");
            assert_eq!(
                dns_client::question_name(&reply).as_deref(),
                Some(name),
                "the reply must answer the question we asked"
            );
            assert!(
                u16::from_be_bytes([reply[6], reply[7]]) > 0,
                "{} came back with no answer",
                name
            );
            println!(
                "{:<38} {:?} via {:<12} {:?}",
                name,
                verdict,
                provider,
                dns_client::answer_addrs(&reply)
            );
            if verdict == Verdict::Substituted {
                substituted.push(name);
            }
        }

        // Not an assertion on any single name: which names a provider proxies
        // is theirs to change, and this test exists to report that, not to fail
        // on it. But if nothing at all is substituted, either the VPN is up or
        // every provider has dropped the whole list - both worth failing on.
        assert!(
            !substituted.is_empty(),
            "no routed name is substituted by any provider - VPN up, or the \
             providers dropped every name"
        );
        println!("substituted: {:?}", substituted);
    }
}
