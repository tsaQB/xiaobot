//! Where the WebUI listens and who may reach it.
//!
//! `XIAO_WEB_BIND` picks the address (`127.0.0.1:8787` by default, `off`
//! disables the console). When it listens on more than loopback,
//! `XIAO_WEB_ALLOWED_NETWORKS` limits which client addresses get an answer at
//! all; loopback is always allowed because tunnels arrive as local
//! connections.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

pub(crate) const DEFAULT_BIND: &str = "127.0.0.1:8787";
pub(crate) const DEFAULT_ALLOWED_NETWORKS: &str = "192.168.0.0/16, 10.0.0.0/8, 172.16.0.0/12";

/// An IPv4 or IPv6 network in CIDR notation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Parses `10.0.0.0/8`, `fd00::/8` or a bare address (a single host).
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (addr, prefix) = match raw.split_once('/') {
            Some((addr, prefix)) => (addr.trim(), Some(prefix.trim())),
            None => (raw, None),
        };
        let network: IpAddr = addr
            .parse()
            .map_err(|_| format!("'{raw}' is not an IP network"))?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= max)
                .ok_or_else(|| format!("'{raw}' has an invalid prefix length"))?,
            None => max,
        };
        Ok(Self {
            network: mask(network, prefix),
            prefix,
        })
    }

    pub(crate) fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        ip.is_ipv4() == self.network.is_ipv4() && mask(ip, self.prefix) == self.network
    }
}

fn mask(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let masked = if prefix == 0 {
                0
            } else {
                bits & (u32::MAX << (32 - u32::from(prefix)))
            };
            IpAddr::V4(Ipv4Addr::from(masked))
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let masked = if prefix == 0 {
                0
            } else {
                bits & (u128::MAX << (128 - u32::from(prefix)))
            };
            IpAddr::V6(std::net::Ipv6Addr::from(masked))
        }
    }
}

/// Parses a comma separated list of networks. Empty entries are skipped.
pub(crate) fn parse_networks(raw: &str) -> Result<Vec<Cidr>, String> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(Cidr::parse)
        .collect()
}

/// Whether a client address may use the console.
pub(crate) fn is_allowed(peer: IpAddr, networks: &[Cidr]) -> bool {
    let peer = peer.to_canonical();
    peer.is_loopback() || networks.iter().any(|network| network.contains(peer))
}

/// Parses `XIAO_WEB_BIND`: `off`, `host:port`, `[v6]:port` or a bare port
/// (which listens on loopback). Ports below 1024 are refused.
pub(crate) fn parse_bind(raw: &str) -> Result<Option<SocketAddr>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return parse_bind(DEFAULT_BIND);
    }
    if raw.eq_ignore_ascii_case("off") || raw.eq_ignore_ascii_case("false") {
        return Ok(None);
    }
    let addr = if let Ok(port) = raw.parse::<u16>() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    } else {
        raw.parse::<SocketAddr>()
            .map_err(|_| format!("'{raw}' is not an address such as 127.0.0.1:8787"))?
    };
    if addr.port() < 1024 {
        return Err("the port must be between 1024 and 65535".to_string());
    }
    Ok(Some(addr))
}

/// The address other devices on the LAN would use to reach this machine.
/// Connecting a UDP socket sends nothing; it only asks the OS which local
/// address carries the default route.
pub(crate) fn primary_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().expect("test address parses")
    }

    #[test]
    fn cidr_membership_follows_the_prefix() {
        let net = Cidr::parse("192.168.0.0/16").expect("valid network");
        assert!(net.contains(ip("192.168.1.23")));
        assert!(!net.contains(ip("192.169.0.1")));
        let host = Cidr::parse("10.1.2.3").expect("bare address");
        assert!(host.contains(ip("10.1.2.3")));
        assert!(!host.contains(ip("10.1.2.4")));
        let all = Cidr::parse("0.0.0.0/0").expect("everything");
        assert!(all.contains(ip("8.8.8.8")));
        assert!(!all.contains(ip("::1")), "IPv4 networks never match IPv6");
    }

    #[test]
    fn host_bits_are_ignored_and_ipv6_works() {
        let net = Cidr::parse("172.16.5.9/12").expect("valid network");
        assert!(net.contains(ip("172.31.255.255")));
        assert!(!net.contains(ip("172.32.0.1")));
        let ula = Cidr::parse("fd00::/8").expect("valid v6 network");
        assert!(ula.contains(ip("fd12:3456::1")));
        assert!(!ula.contains(ip("fe80::1")));
    }

    #[test]
    fn invalid_networks_are_rejected() {
        for raw in [
            "192.168.0.0/33",
            "300.1.1.1/8",
            "lan",
            "10.0.0.0/x",
            "::/129",
        ] {
            assert!(Cidr::parse(raw).is_err(), "{raw} must be rejected");
        }
        assert!(parse_networks("10.0.0.0/8, nope").is_err());
        assert_eq!(
            parse_networks(DEFAULT_ALLOWED_NETWORKS)
                .expect("defaults parse")
                .len(),
            3
        );
    }

    #[test]
    fn loopback_and_mapped_addresses_are_handled() {
        let nets = parse_networks("192.168.0.0/16").expect("valid list");
        assert!(is_allowed(ip("127.0.0.1"), &nets));
        assert!(is_allowed(ip("::1"), &nets));
        assert!(is_allowed(ip("::ffff:192.168.1.5"), &nets));
        assert!(!is_allowed(ip("203.0.113.9"), &nets));
        assert!(!is_allowed(ip("10.0.0.1"), &[]));
    }

    #[test]
    fn bind_values_parse() {
        assert_eq!(
            parse_bind("").expect("default"),
            Some("127.0.0.1:8787".parse().expect("addr"))
        );
        assert_eq!(parse_bind("off").expect("off"), None);
        assert_eq!(
            parse_bind("9000").expect("bare port"),
            Some("127.0.0.1:9000".parse().expect("addr"))
        );
        assert_eq!(
            parse_bind("0.0.0.0:8787").expect("lan"),
            Some("0.0.0.0:8787".parse().expect("addr"))
        );
        assert!(parse_bind("[::]:8787").is_ok());
        assert!(parse_bind("0.0.0.0:80").is_err(), "privileged port");
        assert!(parse_bind("example.com:8787").is_err());
    }
}
