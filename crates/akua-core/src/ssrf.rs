//! SSRF guard — reject repo URLs whose host is a private / loopback /
//! link-local IP literal. Blocks the most obvious attack vectors (cloud
//! metadata at `169.254.169.254`, `127.0.0.1`, RFC1918 ranges) without
//! a full DNS resolver.
//!
//! Limitation: a DNS name resolving to a private IP is *not* caught
//! here — mitigate that at the network layer (egress firewall /
//! resolver allowlist). Opt out entirely with
//! `AKUA_ALLOW_PRIVATE_HOSTS=1` for local development.

use std::net::IpAddr;

use crate::fetch::FetchError;

/// Allow-private bypass env var. Unset by default: private IP literals
/// in `repo:` URLs are rejected. Set to `1` / `true` for local dev.
const ALLOW_PRIVATE_ENV: &str = "AKUA_ALLOW_PRIVATE_HOSTS";

/// Validate a repo host. Rejects bare-IP hosts in private ranges
/// unless the bypass env var is set. DNS names pass through.
pub fn validate_host(host: &str) -> Result<(), FetchError> {
    if allow_private() {
        return Ok(());
    }
    let host_only = strip_port(host);
    // Only IP literals reach here — DNS names aren't parsable.
    if let Ok(ip) = host_only.parse::<IpAddr>() {
        if is_private(ip) {
            return Err(FetchError::PrivateHost {
                host: host.to_string(),
            });
        }
    }
    Ok(())
}

fn allow_private() -> bool {
    matches!(
        std::env::var(ALLOW_PRIVATE_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Strip an optional `:port` (IPv6 literals get bracket-stripped too).
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return &rest[..end];
        }
    }
    match host.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    }
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()       // 127/8
                || v4.is_private() // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local() // 169.254/16 (includes AWS metadata)
                || v4.is_broadcast()
                || v4.is_unspecified() // 0.0.0.0
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 0x40 // CGNAT 100.64/10
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00  // fc00::/7 ULA
                || (v6.segments()[0] & 0xffc0) == 0xfe80  // fe80::/10 link-local
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_aws_metadata_ip() {
        assert!(validate_host("169.254.169.254").is_err());
    }

    #[test]
    fn rejects_loopback() {
        assert!(validate_host("127.0.0.1").is_err());
        assert!(validate_host("127.0.0.1:8080").is_err());
        assert!(validate_host("[::1]").is_err());
    }

    #[test]
    fn rejects_rfc1918() {
        assert!(validate_host("10.0.0.1").is_err());
        assert!(validate_host("172.16.5.5").is_err());
        assert!(validate_host("192.168.1.1").is_err());
    }

    #[test]
    fn allows_public_ip() {
        assert!(validate_host("8.8.8.8").is_ok());
        assert!(validate_host("1.1.1.1:443").is_ok());
    }

    #[test]
    fn allows_dns_names() {
        // DNS names aren't IP-parseable — we trust them (consumer's
        // network layer handles DNS-rebinding mitigations).
        assert!(validate_host("ghcr.io").is_ok());
        assert!(validate_host("charts.bitnami.com").is_ok());
        assert!(validate_host("registry.example.com:5000").is_ok());
    }

    #[test]
    fn env_var_bypass() {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = super::super::test_util::ScopedEnvVar::set("AKUA_ALLOW_PRIVATE_HOSTS", "1");
        assert!(validate_host("127.0.0.1").is_ok());
    }
}
