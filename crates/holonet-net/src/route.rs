//! Does this PC have a route to an address?
//!
//! For `decisions.md`: "Skip the wait when the primary address is private and
//! this PC has no route to it." Away from home, a private address such as
//! 192.168.1.51 is only reachable through the default route (to the
//! internet), which cannot deliver it, so trying it would just time out.
//! `GetBestRoute2` says which route Windows would use: a more specific route
//! (the local network, or a VPN or Tailscale subnet route) means the address
//! may answer; only the default route, or none, means it cannot.

use std::net::IpAddr;

/// What Windows would do with a packet to an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteCheck {
    /// A route more specific than the default, with its prefix length.
    Specific { prefix_len: u8 },
    /// Only the default route (prefix length 0).
    DefaultOnly,
    /// No route at all (no network).
    NoRoute,
    /// The check could not be made (not Windows, or an unexpected error).
    Unknown,
}

/// Private, link-local or unique-local: an address that only works on its
/// own network. Tailscale's 100.64.0.0/10 counts only when `cgnat_private`
/// is set, because with Tailscale running it is routed.
pub fn is_private(ip: IpAddr, cgnat_private: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private() || v4.is_link_local() || (cgnat_private && o[0] == 100 && (o[1] & 0xC0) == 64)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            (first & 0xFE00) == 0xFC00 || (first & 0xFFC0) == 0xFE80
        }
    }
}

/// Whether a primary address is private and has no route from here, so the
/// fallback wait can be skipped. Hostnames are never skipped: resolving them
/// could itself take time, and many resolve to public addresses.
pub fn primary_unreachable(address: &str, tailscale_running: bool) -> bool {
    let address = address.trim().trim_start_matches('[').trim_end_matches(']');
    let Ok(ip) = address.parse::<IpAddr>() else { return false };
    is_private(ip, !tailscale_running) && matches!(route_to(ip), RouteCheck::DefaultOnly | RouteCheck::NoRoute)
}

#[cfg(windows)]
pub fn route_to(ip: IpAddr) -> RouteCheck {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_INET};

    // SAFETY: plain C structs, valid when zeroed; the union variant written
    // matches the family set, and GetBestRoute2 only writes the out params.
    unsafe {
        let mut dest: SOCKADDR_INET = std::mem::zeroed();
        match ip {
            IpAddr::V4(v4) => {
                dest.Ipv4.sin_family = AF_INET;
                dest.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(v4.octets());
            }
            IpAddr::V6(v6) => {
                dest.Ipv6.sin6_family = AF_INET6;
                dest.Ipv6.sin6_addr.u.Byte = v6.octets();
            }
        }
        let mut row: MIB_IPFORWARD_ROW2 = std::mem::zeroed();
        let mut source: SOCKADDR_INET = std::mem::zeroed();
        let status = GetBestRoute2(std::ptr::null(), 0, std::ptr::null(), &dest, 0, &mut row, &mut source);
        const ERROR_NETWORK_UNREACHABLE: u32 = 1231;
        const ERROR_HOST_UNREACHABLE: u32 = 1232;
        const ERROR_NOT_FOUND: u32 = 1168;
        match status {
            0 if row.DestinationPrefix.PrefixLength == 0 => RouteCheck::DefaultOnly,
            0 => RouteCheck::Specific { prefix_len: row.DestinationPrefix.PrefixLength },
            ERROR_NETWORK_UNREACHABLE | ERROR_HOST_UNREACHABLE | ERROR_NOT_FOUND => RouteCheck::NoRoute,
            _ => RouteCheck::Unknown,
        }
    }
}

#[cfg(not(windows))]
pub fn route_to(_ip: IpAddr) -> RouteCheck {
    RouteCheck::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges() {
        for p in ["10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.51", "169.254.1.1", "fd00::1", "fe80::1"] {
            assert!(is_private(p.parse().unwrap(), false), "{p}");
        }
        for public in ["8.8.8.8", "172.32.0.1", "203.0.113.20", "2001:db8::1", "100.64.0.21"] {
            assert!(!is_private(public.parse().unwrap(), false), "{public}");
        }
        // Tailscale's range is private only when Tailscale is not routing it.
        assert!(is_private("100.64.0.21".parse().unwrap(), true));
        assert!(is_private("100.127.255.1".parse().unwrap(), true));
        assert!(!is_private("100.128.0.1".parse().unwrap(), true));
    }

    #[test]
    fn loopback_has_a_specific_route_and_hostnames_are_never_skipped() {
        if cfg!(windows) {
            assert!(matches!(route_to("127.0.0.1".parse().unwrap()), RouteCheck::Specific { .. }));
        }
        assert!(!primary_unreachable("nas.local", false));
        assert!(!primary_unreachable("8.8.8.8", false), "public addresses always get their wait");
    }
}
