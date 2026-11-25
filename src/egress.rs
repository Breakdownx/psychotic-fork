use std::time::Duration;

use crate::utils::{powershell, powershell_within};

// Finding the ISP-facing interface, and why that is the whole job here.
//
// With a full-tunnel VPN up, the NRPT rules resolve to nothing useful: xbox-dns
// substitutes addresses only for clients it geolocates to a blocked region, and
// through the tunnel it sees a foreign address and forwards the genuine Google
// one. The region gate then answers 400 as if the unlocker were not installed.
//
// Routing the resolver addresses back onto the ISP link does not work either,
// and this is worth remembering rather than rediscovering: AmneziaVPN already
// holds its own /32 for exactly those addresses. Equal prefix length hands the
// decision to RouteMetric + InterfaceMetric, where the tunnel's 5 beats
// Ethernet's 26; no legal RouteMetric closes that gap, the only other lever -
// the interface metric - would drag every other route off the VPN with it, and
// the WireGuard tunnel service restores its route as fast as it is deleted.
//
// What does work is not touching the routing table at all: `dns_client` names
// the outgoing interface on the socket via IP_UNICAST_IF, which skips the route
// lookup entirely. Verified against a live tunnel - the same resolver answers
// 172.217.119.4 (genuine Google) through the tunnel and 87.228.47.204 (proxy)
// through the ISP link, and a query to an address nothing holds a route for
// still leaves through the named interface. So this module only has to say
// which interface that is; `hosts_pin` does the rest.

// The resolver addresses used to live here as a single xbox-dns.ru pair. They
// now live in `resolvers`, which holds several providers and picks between them
// per query - a hardcoded pair cannot notice that a provider stopped
// substituting a name, which is exactly how the tool broke. This module is back
// to its one job: naming the interface those queries must leave through.

/// Prefixes an earlier build pinned to the physical adapter. They are harmless
/// but pointless, and the persistent ones outlive the tool, so cleanup drops
/// them. Remove this once no installed build can still have written them.
const LEGACY_PINNED_PREFIXES: &[&str] = &[
    "111.88.96.50/32",
    "111.88.96.51/32",
    "2a00:ab00:1233:26::50/128",
    "2a00:ab00:1233:26::51/128",
];

#[derive(Debug, Clone)]
pub struct Egress {
    pub if_index: u32,
    /// True when some non-physical adapter also holds a default route - i.e. a
    /// tunnel is up and the NRPT path alone cannot work.
    pub vpn_active: bool,
}

/// `ifIndex|gateway|vpn`. The gateway itself is not kept - nothing routes any
/// more - but an interface without one is not an internet-facing link, so its
/// presence still decides whether the line describes a usable egress.
fn parse_egress(line: &str) -> Option<Egress> {
    let mut parts = line.trim().split('|');
    let if_index = parts.next()?.trim().parse::<u32>().ok()?;
    let gateway = parts.next()?.trim();
    let vpn = parts.next()?.trim();

    if gateway.is_empty() || gateway == "-" {
        return None;
    }
    Some(Egress {
        if_index,
        vpn_active: vpn.eq_ignore_ascii_case("true"),
    })
}

/// Linux/macOS: the ISP link is the non-tunnel default route, and a tunnel is
/// up when the routing table sends a public address through one (`netif`).
#[cfg(not(target_os = "windows"))]
pub fn detect() -> Option<Egress> {
    let (if_index, vpn_active) = crate::netif::egress()?;
    Some(Egress {
        if_index,
        vpn_active,
    })
}

/// The default route that belongs to real hardware. `Get-NetAdapter -Physical`
/// drops WireGuard/TAP tunnels and the Hyper-V/VMware virtual switches in one
/// go, and requiring a real next hop drops host-only adapters, which carry a
/// gateway address but no default route.
#[cfg(target_os = "windows")]
pub fn detect() -> Option<Egress> {
    const SCRIPT: &str = "\
$phys=@(Get-NetAdapter -Physical -ErrorAction SilentlyContinue | \
  Where-Object {$_.Status -eq 'Up'} | ForEach-Object {$_.ifIndex}); \
$def=@(Get-NetRoute -DestinationPrefix '0.0.0.0/0' -PolicyStore ActiveStore -ErrorAction SilentlyContinue); \
$mine=@($def | Where-Object {$phys -contains $_.ifIndex -and $_.NextHop -ne '0.0.0.0'}); \
if ($mine.Count -eq 0) { 'none' } else { \
  $best=$mine | Sort-Object {[int]$_.RouteMetric + \
    [int](Get-NetIPInterface -InterfaceIndex $_.ifIndex -AddressFamily IPv4 \
      -ErrorAction SilentlyContinue).InterfaceMetric} | Select-Object -First 1; \
  $vpn=@($def | Where-Object {$phys -notcontains $_.ifIndex}).Count -gt 0; \
  '{0}|{1}|{2}' -f $best.ifIndex,$best.NextHop,$vpn }";

    let out = powershell(SCRIPT)?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(parse_egress)
}

// Which way the *client* leaves, which is not the question `vpn_active` answers.
//
// `vpn_active` reads the routing table, and the routing table only knows that
// some tunnel holds `0.0.0.0/0`. Windows VPN clients route per application - an
// exclusion list, or an "only these apps" list - and that decision is invisible
// there: the tunnel holds the same default route either way, because the
// filtering happens in WFP, keyed on the executable image. So a machine can show
// a full tunnel while `language_server.exe` talks to Google straight off the ISP
// link.
//
// That combination is the worst state this tool can produce, and it produced it:
// the client sat in the blocked region *and* the DNS layer had stood down for a
// tunnel the client was not using, so nothing lifted the gate and every request
// came back `User location is not supported for the API use` (G29). The routing
// table cannot tell them apart. The client's own sockets can, and that is all
// this does: read where its established connections are sourced from.

/// Where the client's own traffic leaves the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientEgress {
    /// Measured: at least one of its connections is sourced from a physical
    /// adapter. A tunnel may well be up - this client is not inside it.
    Physical,
    /// Measured: its connections are sourced from a tunnel adapter.
    Tunnel,
    /// Measured: some of each, which is a state of its own and not a rounding
    /// error.
    ///
    /// Seen live the moment the owner took our service out of his VPN's
    /// exclusions: six sockets still on the physical link, two new ones already
    /// sourced from `10.8.1.2`. For the DNS layer this counts as "outside" — the
    /// rules are needed by the half that is out, exactly as for `Physical`. For
    /// anything that asks "will a provider serving only Russian addresses accept
    /// us", it counts as "inside": the connections that leave through the tunnel
    /// will be refused whatever the others do.
    Mixed,
    /// Nothing to read. The client is not running, or has not opened an outbound
    /// connection yet.
    Unknown,
}

/// Read-only observation of the language server's outbound connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub client: ClientEgress,
}

/// The language server under both names it ships as: `language_server.exe` in the
/// Desktop app and `language_server_windows_x64.exe` in the IDE. The Electron
/// shell is deliberately not matched - it never carries a gated call, so its
/// sockets would only add noise to the count.
const CLIENT_PROCESS_GLOB: &str = "language_server*";

/// Bound for the probe below. Shorter than `PS_LIMIT`: this is three read-only
/// cmdlets on a path the user is watching, and if the machine cannot answer that
/// in fifteen seconds the honest result is `Unknown`, not a frozen menu (I24).
const CLIENT_PROBE_LIMIT: Duration = Duration::from_secs(15);

/// Reads the client's HTTPS sockets without opening or routing any connection.
/// Loopback traffic does not establish a physical or VPN egress.
pub fn read() -> Reading {
    let script = format!(
        "$ids=@(Get-Process -Name '{glob}' -ErrorAction SilentlyContinue | \
           ForEach-Object {{$_.Id}}); \
         if ($ids.Count -eq 0) {{ 'none' }} else {{ \
           $phys=@(Get-NetAdapter -Physical -ErrorAction SilentlyContinue | \
             Where-Object {{$_.Status -eq 'Up'}} | ForEach-Object {{$_.ifIndex}}); \
           $ix=@{{}}; \
           foreach ($a in @(Get-NetIPAddress -ErrorAction SilentlyContinue)) {{ \
             $ix[($a.IPAddress -split '%')[0]]=$a.InterfaceIndex }}; \
           $p=0; $t=0; \
           foreach ($c in @(Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue)) {{ \
             if ($ids -notcontains $c.OwningProcess -or $c.RemotePort -ne 443) {{ continue }}; \
             $remote=($c.RemoteAddress -split '%')[0]; \
             $local=($c.LocalAddress -split '%')[0]; \
             if ([System.Net.IPAddress]::IsLoopback($remote) -or \
                 [System.Net.IPAddress]::IsLoopback($local)) {{ continue }}; \
             $i=$ix[$local]; \
             if ($null -eq $i) {{ continue }}; \
             if ($phys -contains $i) {{ $p++ }} else {{ $t++ }} }}; \
           '{{0}}|{{1}}' -f $p,$t }}",
        glob = CLIENT_PROCESS_GLOB,
    );
    let Some(out) = powershell_within(&script, CLIENT_PROBE_LIMIT) else {
        return Reading { client: ClientEgress::Unknown };
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map_or(Reading { client: ClientEgress::Unknown }, parse_reading)
}

fn parse_reading(line: &str) -> Reading {
    let mut fields = line.trim().split('|').map(|f| f.trim().parse::<u32>().ok());
    let client = match (fields.next().flatten(), fields.next().flatten(), fields.next()) {
        (Some(p), Some(t), None) if p > 0 && t > 0 => ClientEgress::Mixed,
        (Some(p), Some(_), None) if p > 0 => ClientEgress::Physical,
        (Some(_), Some(t), None) if t > 0 => ClientEgress::Tunnel,
        _ => ClientEgress::Unknown,
    };
    Reading { client }
}

pub fn client_egress() -> ClientEgress {
    read().client
}

/// A VPN requires DNS stand-down only when the client's sockets are measured
/// inside the tunnel. Physical, mixed, or unavailable evidence does not qualify.
/// Returns the observation alongside the verdict without probing twice.
pub fn vpn_verdict(egress: Option<&Egress>) -> (bool, ClientEgress) {
    if !egress.is_some_and(|e| e.vpn_active) {
        return (false, ClientEgress::Unknown);
    }
    let client = client_egress();
    (client == ClientEgress::Tunnel, client)
}

/// Drops the host routes an earlier build pinned. Deleted per interface rather
/// than by prefix alone, so a route stranded on an adapter the machine no longer
/// uses goes too; netsh cleans up whatever the cmdlet could not.
pub fn remove_legacy_routes_checked() -> Result<(), String> {
    let list = LEGACY_PINNED_PREFIXES.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(",");
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         foreach ($store in @('ActiveStore','PersistentStore')) {{ \
           Get-NetRoute -PolicyStore $store | Where-Object {{$_.DestinationPrefix -in @({list})}} | \
             Remove-NetRoute -Confirm:$false \
         }}"
    );
    let out = powershell(&script).ok_or("legacy route removal timed out")?;
    if out.status.success() { Ok(()) } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "windows")]
pub fn remove_legacy_routes() {
    let list = LEGACY_PINNED_PREFIXES
        .iter()
        .map(|p| format!("'{}'", p))
        .collect::<Vec<_>>()
        .join(",");
    let cmd = format!(
        "foreach ($d in @({})) {{ \
           $fam=$(if ($d -like '*:*') {{'ipv6'}} else {{'ipv4'}}); \
           foreach ($s in @('ActiveStore','PersistentStore')) {{ \
             $st=$(if ($s -eq 'ActiveStore') {{'active'}} else {{'persistent'}}); \
             foreach ($r in @(Get-NetRoute -DestinationPrefix $d -PolicyStore $s \
                 -ErrorAction SilentlyContinue)) {{ \
               Remove-NetRoute -DestinationPrefix $d -InterfaceIndex $r.ifIndex \
                 -PolicyStore $s -Confirm:$false -ErrorAction SilentlyContinue; \
               if (@(Get-NetRoute -DestinationPrefix $d -PolicyStore $s -ErrorAction SilentlyContinue | \
                   Where-Object {{$_.ifIndex -eq $r.ifIndex}}).Count -gt 0) {{ \
                 netsh interface $fam delete route prefix=$d interface=$($r.ifIndex) store=$st | Out-Null }} }} }} }}",
        list
    );
    powershell(&cmd);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns_client;
    use std::net::Ipv4Addr;

    #[test]
    fn egress_line_is_parsed() {
        let eg = parse_egress("17|192.168.0.1|True").expect("parses");
        assert_eq!(eg.if_index, 17);
        assert!(eg.vpn_active);

        let plain = parse_egress(" 12|10.0.0.1|False ").expect("parses");
        assert!(!plain.vpn_active);
    }

    #[test]
    fn client_socket_counts_are_read_as_evidence() {
        for (line, expected) in [
            ("2|0", ClientEgress::Physical),
            ("0|3", ClientEgress::Tunnel),
            ("1|4", ClientEgress::Mixed),
            ("0|0", ClientEgress::Unknown),
            ("none", ClientEgress::Unknown),
            ("2|x", ClientEgress::Unknown),
            ("2|0|3", ClientEgress::Unknown),
        ] {
            assert_eq!(parse_reading(line).client, expected, "{line}");
        }
    }

    #[test]
    fn standing_down_needs_the_client_to_be_in_the_tunnel() {
        let no_vpn = Egress {
            if_index: 29,
            vpn_active: false,
        };
        // No tunnel: nothing to stand down for, and the probe never runs.
        assert_eq!(vpn_verdict(Some(&no_vpn)), (false, ClientEgress::Unknown));
        assert_eq!(vpn_verdict(None), (false, ClientEgress::Unknown));
    }

    #[test]
    fn egress_line_rejects_garbage() {
        assert!(parse_egress("none").is_none());
        assert!(parse_egress("").is_none());
        assert!(parse_egress("17|-|False").is_none());
        assert!(parse_egress("17|192.168.0.1").is_none());
    }

    /// The one thing unit tests cannot cover: that the socket really leaves
    /// through the interface we named. Needs a live network, and only says
    /// something with a VPN up - that is when the two answers must differ.
    #[test]
    #[ignore = "needs a live network; run with --ignored"]
    fn resolves_past_the_tunnel() {
        let eg = detect().expect("physical egress");
        // The first provider with a UDP address - a DoH one has none (I49).
        let server: Ipv4Addr = crate::resolvers::PROVIDERS
            .iter()
            .find_map(|p| p.v4.first())
            .expect("a provider answering on UDP")
            .parse()
            .unwrap();
        let host = "cloudcode-pa.googleapis.com";
        let isp = dns_client::resolve_a_via(host, server, eg.if_index);
        let tunnelled = dns_client::resolve_a_via(host, server, 0);
        println!("egress if{} (vpn: {})", eg.if_index, eg.vpn_active);
        println!("  via ISP:     {:?}", isp);
        println!("  via default: {:?}", tunnelled);
        assert!(
            isp.as_ref().map_or(false, |a| !a.is_empty()),
            "no answer over the ISP link"
        );
        if eg.vpn_active {
            assert_ne!(
                isp.unwrap(),
                tunnelled.unwrap(),
                "the tunnel was not bypassed"
            );
        }
    }

    /// What the machine says right now. The only test that can tell a working
    /// probe from one that always answers `Unknown`, because the whole question
    /// is about live sockets.
    ///
    /// Reads, never asserts a particular answer: all three are legitimate
    /// depending on what is running. Run it with Antigravity open, and with the
    /// client in and out of the VPN's exclusion list - the printed verdict must
    /// follow.
    ///
    ///     cargo test reads_where_the_client_actually_leaves -- --ignored --nocapture
    #[test]
    #[ignore = "reads live processes and sockets; run with --ignored"]
    fn reads_where_the_client_actually_leaves() {
        let eg = detect();
        let reading = read();
        let (stand_down, _) = vpn_verdict(eg.as_ref());
        println!(
            "vpn_active: {}\nclient:     {:?}\nstand down: {}  (правила DNS {})",
            eg.as_ref().is_some_and(|e| e.vpn_active),
            reading.client,
            stand_down,
            if stand_down {
                "НЕ ставятся"
            } else {
                "ставятся"
            }
        );
    }

    /// Forcing the interface must work for a destination nothing holds a route
    /// for - that is what makes the host-route pinning unnecessary.
    #[test]
    #[ignore = "needs a live network; run with --ignored"]
    fn unicast_if_needs_no_host_route() {
        let eg = detect().expect("physical egress");
        let unrouted: Ipv4Addr = "1.1.1.1".parse().unwrap();
        let answer = dns_client::resolve_a_via("github.com", unrouted, eg.if_index);
        println!("via if{} to 1.1.1.1: {:?}", eg.if_index, answer);
        assert!(
            answer.as_ref().map_or(false, |a| !a.is_empty()),
            "IP_UNICAST_IF needs a host route after all"
        );
    }
}
