//! kura-egress — the outbound-request policy every egress point shares.
//!
//! # What this defends against, and what it does not
//!
//! Server-side request forgery: making the daemon fetch a URL it was tricked
//! into fetching. The classic target is the cloud instance-metadata service at
//! `169.254.169.254`, which hands out credentials to anything on the host.
//!
//! [`EgressPolicy::check_url`] is **syntactic**: it inspects the URL's host.
//! That catches IP literals and blocked names, and it is the right check for
//! configuration-time validation. It does **not** stop a hostname that
//! *resolves* to a blocked address — `evil.test` with an A record pointing at
//! `169.254.169.254` passes it.
//!
//! [`EgressPolicy::check_url_resolved`] closes that by resolving the host and
//! checking every address. Call-time egress must use it. It still does not
//! stop DNS rebinding (resolve to a safe address, then return a blocked one
//! for the connection's own lookup); defeating that requires pinning the
//! checked address through to the socket, which belongs in the HTTP client
//! layer and is recorded as an open gap rather than implied here.
//!
//! The split is deliberate: a single `check()` that sometimes resolves and
//! sometimes does not is how a validation call gets mistaken for a security
//! boundary.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use serde::{Deserialize, Serialize};

/// Why an outbound request was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EgressDenial {
    #[error("the URL could not be parsed: {0}")]
    Unparseable(String),
    #[error("scheme {0:?} is not allowed for outbound requests")]
    SchemeNotAllowed(String),
    #[error("the URL has no host")]
    HostMissing,
    #[error("host {host} is on the blocklist")]
    HostBlocked { host: String },
    #[error("host {host} resolves to {addr}, which is not a permitted destination ({reason})")]
    AddressBlocked {
        host: String,
        addr: String,
        reason: &'static str,
    },
    #[error("host {host} could not be resolved: {detail}")]
    Unresolvable { host: String, detail: String },
}

/// A URL that passed the policy. Carrying the parsed form keeps callers from
/// re-parsing — and re-parsing differently, which is its own class of bypass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedUrl {
    pub url: String,
    pub host: String,
    pub scheme: String,
}

/// Outbound policy. The defaults deny the addresses an SSRF is aiming for;
/// operators widen it explicitly, never by omission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressPolicy {
    /// Hostnames refused outright. Matching is case-insensitive and covers
    /// subdomains: `example.com` also blocks `api.example.com`.
    #[serde(default)]
    pub blocked_hostnames: Vec<String>,
    /// Permit RFC1918 / unique-local destinations. Off by default: a
    /// self-hosted daemon usually has a whole private network reachable that
    /// no agent should be able to probe.
    #[serde(default)]
    pub allow_private_networks: bool,
    /// Permit loopback. Off by default. Turning it on is how a local model
    /// server or a sidecar is reached, and it is worth an explicit choice
    /// because it also exposes every other local service.
    #[serde(default)]
    pub allow_loopback: bool,
    /// Permitted URL schemes.
    #[serde(default = "default_schemes")]
    pub allowed_schemes: Vec<String>,
}

fn default_schemes() -> Vec<String> {
    vec!["https".to_string(), "http".to_string()]
}

impl Default for EgressPolicy {
    fn default() -> Self {
        EgressPolicy {
            blocked_hostnames: Vec::new(),
            allow_private_networks: false,
            allow_loopback: false,
            allowed_schemes: default_schemes(),
        }
    }
}

impl EgressPolicy {
    /// Syntactic check: scheme, blocklist, and IP literals. Use at
    /// configuration time — when an operator saves a base URL — where a DNS
    /// lookup would be wrong (the name may not resolve yet) and where the
    /// answer must be stable.
    pub fn check_url(&self, raw: &str) -> Result<CheckedUrl, EgressDenial> {
        let (parsed, host) = self.parse_and_screen(raw)?;
        if let Some(ip) = parse_ip_literal(&host) {
            self.check_addr(&host, ip)?;
        }
        Ok(parsed)
    }

    /// Resolving check: everything [`Self::check_url`] does, plus every
    /// address the host resolves to. Use at call time. A host that resolves to
    /// several addresses passes only if **all** of them are permitted —
    /// checking just the first is how a dual-homed name slips through.
    pub fn check_url_resolved(&self, raw: &str) -> Result<CheckedUrl, EgressDenial> {
        let (parsed, host) = self.parse_and_screen(raw)?;
        if let Some(ip) = parse_ip_literal(&host) {
            self.check_addr(&host, ip)?;
            return Ok(parsed);
        }
        // Port is irrelevant to the address check but required by the resolver.
        let addrs =
            (host.as_str(), 0u16)
                .to_socket_addrs()
                .map_err(|err| EgressDenial::Unresolvable {
                    host: host.clone(),
                    detail: err.to_string(),
                })?;
        let mut any = false;
        for addr in addrs {
            any = true;
            self.check_addr(&host, addr.ip())?;
        }
        if !any {
            return Err(EgressDenial::Unresolvable {
                host: host.clone(),
                detail: "no addresses".to_string(),
            });
        }
        Ok(parsed)
    }

    fn parse_and_screen(&self, raw: &str) -> Result<(CheckedUrl, String), EgressDenial> {
        let url = url::Url::parse(raw.trim())
            .map_err(|err| EgressDenial::Unparseable(err.to_string()))?;
        let scheme = url.scheme().to_ascii_lowercase();
        if !self
            .allowed_schemes
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&scheme))
        {
            return Err(EgressDenial::SchemeNotAllowed(scheme));
        }
        let host = url
            .host_str()
            .map(|h| h.trim_matches(['[', ']']).to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .ok_or(EgressDenial::HostMissing)?;
        if self.is_hostname_blocked(&host) {
            return Err(EgressDenial::HostBlocked { host });
        }
        let checked = CheckedUrl {
            url: url.to_string(),
            host: host.clone(),
            scheme,
        };
        Ok((checked, host))
    }

    fn is_hostname_blocked(&self, host: &str) -> bool {
        self.blocked_hostnames.iter().any(|blocked| {
            let blocked = blocked.trim().trim_start_matches('.').to_ascii_lowercase();
            if blocked.is_empty() {
                return false;
            }
            // Subdomains of a blocked name are blocked: blocking
            // `example.com` and leaving `api.example.com` open would make the
            // list a formality.
            host == blocked || host.ends_with(&format!(".{blocked}"))
        })
    }

    fn check_addr(&self, host: &str, addr: IpAddr) -> Result<(), EgressDenial> {
        let deny = |reason: &'static str| {
            Err(EgressDenial::AddressBlocked {
                host: host.to_string(),
                addr: addr.to_string(),
                reason,
            })
        };
        match addr {
            IpAddr::V4(v4) => {
                if is_v4_link_local(v4) {
                    // 169.254.0.0/16 holds the cloud instance-metadata service.
                    // Never permitted, regardless of the private-network flag:
                    // there is no legitimate agent reason to reach it, and it
                    // is the payload of most real SSRF.
                    return deny("link-local / cloud metadata range");
                }
                if v4.is_loopback() && !self.allow_loopback {
                    return deny("loopback");
                }
                if is_v4_private(v4) && !self.allow_private_networks {
                    return deny("private network");
                }
                if v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast() {
                    return deny("unspecified, broadcast or multicast");
                }
            }
            IpAddr::V6(v6) => {
                if is_v6_link_local(v6) {
                    return deny("link-local");
                }
                if v6.is_loopback() && !self.allow_loopback {
                    return deny("loopback");
                }
                if is_v6_unique_local(v6) && !self.allow_private_networks {
                    return deny("unique-local");
                }
                if v6.is_unspecified() || v6.is_multicast() {
                    return deny("unspecified or multicast");
                }
                // An IPv4-mapped address must be judged as the IPv4 it is, or
                // ::ffff:169.254.169.254 walks straight past the v4 rules.
                if let Some(mapped) = v6.to_ipv4_mapped() {
                    return self.check_addr(host, IpAddr::V4(mapped));
                }
            }
        }
        Ok(())
    }
}

fn parse_ip_literal(host: &str) -> Option<IpAddr> {
    host.parse::<IpAddr>().ok()
}

fn is_v4_link_local(addr: Ipv4Addr) -> bool {
    addr.octets()[0] == 169 && addr.octets()[1] == 254
}

fn is_v4_private(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        // 100.64.0.0/10, carrier-grade NAT: routable-looking but internal.
        || (o[0] == 100 && (64..=127).contains(&o[1]))
}

fn is_v6_link_local(addr: Ipv6Addr) -> bool {
    addr.segments()[0] & 0xffc0 == 0xfe80
}

fn is_v6_unique_local(addr: Ipv6Addr) -> bool {
    addr.segments()[0] & 0xfe00 == 0xfc00
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict() -> EgressPolicy {
        EgressPolicy::default()
    }

    /// The payload of most real SSRF. Denied unconditionally — not behind the
    /// private-network flag — because no agent has a legitimate reason to read
    /// instance credentials.
    #[test]
    fn cloud_metadata_is_denied_even_when_private_networks_are_allowed() {
        let permissive = EgressPolicy {
            allow_private_networks: true,
            allow_loopback: true,
            ..EgressPolicy::default()
        };
        for policy in [strict(), permissive] {
            let err = policy
                .check_url("http://169.254.169.254/latest/meta-data/")
                .expect_err("must deny");
            assert!(
                matches!(err, EgressDenial::AddressBlocked { reason, .. } if reason.contains("metadata")),
                "{err:?}"
            );
        }
    }

    /// `::ffff:169.254.169.254` is the same destination wearing a v6 hat. If
    /// the v6 arm does not defer to the v4 rules, this walks straight through.
    #[test]
    fn an_ipv4_mapped_metadata_address_is_denied() {
        let err = strict()
            .check_url("http://[::ffff:169.254.169.254]/latest/meta-data/")
            .expect_err("must deny");
        assert!(
            matches!(err, EgressDenial::AddressBlocked { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn loopback_and_private_ranges_follow_their_flags() {
        assert!(strict().check_url("http://127.0.0.1:8080/").is_err());
        assert!(strict().check_url("http://10.0.0.5/").is_err());
        assert!(strict().check_url("http://192.168.1.10/").is_err());
        assert!(strict().check_url("http://172.16.0.1/").is_err());
        // Carrier-grade NAT looks routable but is not.
        assert!(strict().check_url("http://100.64.0.1/").is_err());
        assert!(strict().check_url("http://[fd00::1]/").is_err());

        let open = EgressPolicy {
            allow_loopback: true,
            allow_private_networks: true,
            ..EgressPolicy::default()
        };
        assert!(open.check_url("http://127.0.0.1:8080/").is_ok());
        assert!(open.check_url("http://10.0.0.5/").is_ok());
    }

    /// Blocking `example.com` while leaving `api.example.com` open would make
    /// the list a formality.
    #[test]
    fn the_hostname_blocklist_covers_subdomains_and_ignores_case() {
        let policy = EgressPolicy {
            blocked_hostnames: vec!["Example.COM".to_string(), ".internal".to_string()],
            ..EgressPolicy::default()
        };
        for blocked in [
            "https://example.com/x",
            "https://API.Example.com/x",
            "https://deep.sub.example.com/",
            "https://anything.internal/",
        ] {
            assert!(
                policy.check_url(blocked).is_err(),
                "{blocked} must be denied"
            );
        }
        // A name that merely ends in the same letters is not a subdomain.
        assert!(policy.check_url("https://notexample.com/").is_ok());
    }

    #[test]
    fn only_permitted_schemes_pass() {
        let policy = strict();
        assert!(policy.check_url("https://example.org/").is_ok());
        for bad in [
            "file:///etc/passwd",
            "gopher://example.org/",
            "ftp://example.org/",
        ] {
            assert!(
                matches!(
                    policy.check_url(bad),
                    Err(EgressDenial::SchemeNotAllowed(_))
                ),
                "{bad} must be denied"
            );
        }
    }

    /// The documented boundary, asserted so it cannot be quietly forgotten:
    /// the syntactic check passes a name it has not resolved. Call-time egress
    /// must use `check_url_resolved`.
    #[test]
    fn the_syntactic_check_does_not_resolve() {
        let policy = strict();
        // `localhost` is a name, not a literal, so the syntactic check has
        // nothing to inspect and lets it through.
        assert!(
            policy.check_url("http://localhost:8080/").is_ok(),
            "check_url is syntactic by design"
        );
        // The resolving check is what actually refuses it.
        let err = policy
            .check_url_resolved("http://localhost:8080/")
            .expect_err("resolution must deny loopback");
        assert!(
            matches!(err, EgressDenial::AddressBlocked { reason, .. } if reason == "loopback"),
            "{err:?}"
        );
    }

    #[test]
    fn a_checked_url_carries_the_parsed_form() {
        let checked = strict()
            .check_url("HTTPS://Example.ORG/path?q=1")
            .expect("ok");
        assert_eq!(checked.scheme, "https");
        assert_eq!(checked.host, "example.org");
        assert!(checked.url.contains("/path?q=1"));
    }

    #[test]
    fn malformed_and_hostless_urls_are_refused() {
        let policy = strict();
        assert!(matches!(
            policy.check_url("not a url"),
            Err(EgressDenial::Unparseable(_))
        ));
        // An empty authority is a parse error.
        assert!(matches!(
            policy.check_url("https://"),
            Err(EgressDenial::Unparseable(_))
        ));
    }

    /// A surprise worth pinning: per the WHATWG URL rules the `url` crate
    /// normalises `https:///internal` to `https://internal/` — the path
    /// segment becomes the **host**. That is not a hole (the name still faces
    /// the blocklist and resolution), but anyone reading a denial for host
    /// `internal` from a URL that looks hostless deserves this test as the
    /// explanation.
    #[test]
    fn a_triple_slash_url_normalises_its_first_segment_into_the_host() {
        let checked = strict().check_url("https:///internal").expect("parses");
        assert_eq!(checked.host, "internal");

        let policy = EgressPolicy {
            blocked_hostnames: vec!["internal".to_string()],
            ..EgressPolicy::default()
        };
        assert!(matches!(
            policy.check_url("https:///internal"),
            Err(EgressDenial::HostBlocked { .. })
        ));
    }
}
