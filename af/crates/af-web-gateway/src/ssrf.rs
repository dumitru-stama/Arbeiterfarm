use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Check if an IP address is in a private, reserved, loopback, link-local,
/// or otherwise non-routable range. Returns true if the IP should be blocked.
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 0.0.0.0/8 — current network
    if octets[0] == 0 {
        return true;
    }
    // 10.0.0.0/8 — private
    if octets[0] == 10 {
        return true;
    }
    // 100.64.0.0/10 — CGNAT
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return true;
    }
    // 127.0.0.0/8 — loopback
    if octets[0] == 127 {
        return true;
    }
    // 169.254.0.0/16 — link-local
    if octets[0] == 169 && octets[1] == 254 {
        return true;
    }
    // 172.16.0.0/12 — private
    if octets[0] == 172 && (octets[1] & 0xF0) == 16 {
        return true;
    }
    // 192.0.0.0/24 — IETF protocol assignments
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return true;
    }
    // 192.0.2.0/24 — documentation (TEST-NET-1)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }
    // 192.168.0.0/16 — private
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }
    // 198.18.0.0/15 — benchmark testing
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return true;
    }
    // 198.51.100.0/24 — documentation (TEST-NET-2)
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }
    // 203.0.113.0/24 — documentation (TEST-NET-3)
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }
    // 224.0.0.0/4 — multicast
    if (octets[0] & 0xF0) == 224 {
        return true;
    }
    // 240.0.0.0/4 — reserved
    if (octets[0] & 0xF0) == 240 {
        return true;
    }
    // 255.255.255.255 — broadcast
    if octets == [255, 255, 255, 255] {
        return true;
    }
    false
}

fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    // ::1 — loopback
    if ip == &Ipv6Addr::LOCALHOST {
        return true;
    }
    // :: — unspecified
    if ip == &Ipv6Addr::UNSPECIFIED {
        return true;
    }
    let segments = ip.segments();
    // fe80::/10 — link-local
    if (segments[0] & 0xFFC0) == 0xFE80 {
        return true;
    }
    // fc00::/7 — unique local
    if (segments[0] & 0xFE00) == 0xFC00 {
        return true;
    }
    // ::ffff:0:0/96 — IPv4-mapped — re-check inner IPv4
    if segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0xFFFF {
        let v4 = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            (segments[6] & 0xFF) as u8,
            (segments[7] >> 8) as u8,
            (segments[7] & 0xFF) as u8,
        );
        return is_private_ipv4(&v4);
    }
    // ::x.x.x.x — IPv4-compatible (deprecated but still processed by some stacks)
    if segments[0..5] == [0, 0, 0, 0, 0] && segments[5] == 0 && *ip != Ipv6Addr::UNSPECIFIED && *ip != Ipv6Addr::LOCALHOST {
        let v4 = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            (segments[6] & 0xFF) as u8,
            (segments[7] >> 8) as u8,
            (segments[7] & 0xFF) as u8,
        );
        return is_private_ipv4(&v4);
    }
    // 2002::/16 — 6to4 (encodes IPv4 address in bits 16-47)
    if segments[0] == 0x2002 {
        let v4 = Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            (segments[1] & 0xFF) as u8,
            (segments[2] >> 8) as u8,
            (segments[2] & 0xFF) as u8,
        );
        return is_private_ipv4(&v4);
    }
    // 2001:0000::/32 — Teredo (IPv4 inverted in last 32 bits)
    if segments[0] == 0x2001 && segments[1] == 0x0000 {
        let inverted = Ipv4Addr::new(
            !(segments[6] >> 8) as u8,
            !(segments[6] & 0xFF) as u8,
            !(segments[7] >> 8) as u8,
            !(segments[7] & 0xFF) as u8,
        );
        return is_private_ipv4(&inverted);
    }
    // 64:ff9b::/96 — NAT64 well-known prefix (IPv4 in last 32 bits)
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2..6] == [0, 0, 0, 0] {
        let v4 = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            (segments[6] & 0xFF) as u8,
            (segments[7] >> 8) as u8,
            (segments[7] & 0xFF) as u8,
        );
        return is_private_ipv4(&v4);
    }
    // 2001:db8::/32 — documentation range
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return true;
    }
    // 100::/64 — discard-only (RFC 6666)
    if segments[0] == 0x0100 && segments[1..4] == [0, 0, 0] {
        return true;
    }
    // ff00::/8 — multicast
    if (segments[0] & 0xFF00) == 0xFF00 {
        return true;
    }
    false
}

/// Human-readable reason for SSRF rejection.
pub fn ssrf_reason(ip: &IpAddr) -> String {
    format!("IP address {} is in a private/reserved range (SSRF protection)", ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_private_ipv4() {
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_private_ip(&"169.254.0.1".parse().unwrap()));
        assert!(is_private_ip(&"0.0.0.0".parse().unwrap()));
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv6() {
        assert!(is_private_ip(&"::1".parse().unwrap()));
        assert!(is_private_ip(&"fe80::1".parse().unwrap()));
        assert!(is_private_ip(&"fc00::1".parse().unwrap()));
        assert!(is_private_ip(&"2001:db8::1".parse().unwrap())); // documentation range
    }

    #[test]
    fn test_ipv6_embedding_formats() {
        // 6to4 encoding 127.0.0.1
        assert!(is_private_ip(&"2002:7f00:0001::".parse().unwrap()));
        // 6to4 encoding 10.0.0.1
        assert!(is_private_ip(&"2002:0a00:0001::".parse().unwrap()));
        // 6to4 encoding 8.8.8.8 (public)
        assert!(!is_private_ip(&"2002:0808:0808::".parse().unwrap()));
        // documentation
        assert!(is_private_ip(&"2001:db8::1".parse().unwrap()));
        // NAT64 encoding 127.0.0.1
        assert!(is_private_ip(&"64:ff9b::7f00:1".parse().unwrap()));
        // discard-only
        assert!(is_private_ip(&"100::".parse().unwrap()));
    }
}
