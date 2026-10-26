//! Custom sandbox networks: a gateway container owns the network namespace the sandbox joins
//! (`--network container:<gateway>`). The gateway drops all outbound traffic except what the
//! rules allow (iptables), and answers DNS itself (dnsmasq): only the allowed domains resolve,
//! and their addresses are added to ipsets that the rules match. The sandbox has no
//! capabilities, so it can't change any of this.

use std::fmt::Write as _;
use std::net::Ipv4Addr;

use sha2::{Digest, Sha256};

use crate::config::{NetProtocol, SandboxNetRule};

/// The gateway image, built by the app from this text.
pub const GATEWAY_DOCKERFILE: &str = "FROM ubuntu:24.04\n\
RUN apt-get update \\\n \
&& DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends iptables ipset dnsmasq-base \\\n \
&& rm -rf /var/lib/apt/lists/* \\\n \
&& useradd -r -u 64999 -M -s /usr/sbin/nologin sandboxdns\n";

/// The user dnsmasq runs as: the only one allowed to reach the upstream DNS servers.
const DNS_UID: u32 = 64999;
/// What the gateway prints once its rules are in place and dnsmasq is running.
pub const READY: &str = "cleanrstai-network-ready";
/// Most ports and ranges one rule takes.
const MAX_PORTS: usize = 64;

/// The tag of the gateway image: a hash of [`GATEWAY_DOCKERFILE`].
pub fn gateway_image() -> String {
    let digest = Sha256::digest(GATEWAY_DOCKERFILE.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("cleanrstai-sandbox-net:{}", &hex[..12])
}

/// The name of the gateway container of the sandbox container `container`.
pub fn gateway_name(container: &str) -> String {
    format!("{container}-net")
}

/// What a rule's host names.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Any,
    /// An IPv4 network: address and prefix length.
    Net(Ipv4Addr, u8),
    /// A domain and its subdomains.
    Domain(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Rule {
    target: Target,
    protocol: NetProtocol,
    /// Inclusive port ranges; empty allows every port.
    ports: Vec<(u16, u16)>,
}

/// Checks one rule, for Settings → Docker. The error is a sentence about the rule.
pub fn check_rule(rule: &SandboxNetRule) -> Result<(), String> {
    parse(rule).map(|_| ())
}

fn parse(rule: &SandboxNetRule) -> Result<Rule, String> {
    Ok(Rule {
        target: parse_host(rule.host.trim())?,
        protocol: rule.protocol,
        ports: parse_ports(rule.ports.trim())?,
    })
}

fn parse_host(host: &str) -> Result<Target, String> {
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return Err("Enter a domain, an IPv4 address or range, or * for any address.".into());
    }
    if host == "*" {
        return Ok(Target::Any);
    }
    if host.contains(':') {
        return Err(format!(
            "{host}: IPv6 addresses aren't supported (the sandbox network is IPv4 only)."
        ));
    }
    if host
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '/')
    {
        let (addr, prefix) = match host.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (host.as_str(), None),
        };
        let addr: Ipv4Addr = addr
            .parse()
            .map_err(|_| format!("{host} isn't a valid IPv4 address."))?;
        let prefix = match prefix {
            None => 32,
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= 32)
                .ok_or_else(|| format!("{host}: the range after / must be 0 to 32."))?,
        };
        return Ok(Target::Net(addr, prefix));
    }
    let domain = host.strip_prefix("*.").unwrap_or(&host);
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if domain.len() > 253 || !domain.split('.').all(label_ok) {
        return Err(format!("{host} isn't a valid domain."));
    }
    Ok(Target::Domain(domain.to_owned()))
}

fn parse_ports(ports: &str) -> Result<Vec<(u16, u16)>, String> {
    let port = |p: &str| {
        p.trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| format!("{} isn't a port (1 to 65535).", p.trim()))
    };
    let mut out = Vec::new();
    for item in ports.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let range = match item.split_once('-') {
            Some((lo, hi)) => {
                let (lo, hi) = (port(lo)?, port(hi)?);
                if lo > hi {
                    return Err(format!("{item}: the first port must be the lower one."));
                }
                (lo, hi)
            }
            None => {
                let p = port(item)?;
                (p, p)
            }
        };
        out.push(range);
    }
    if out.len() > MAX_PORTS {
        return Err(format!(
            "Use at most {MAX_PORTS} ports or ranges in one rule."
        ));
    }
    Ok(out)
}

/// The shell script the gateway runs: firewall rules, the DNS config, dnsmasq, then
/// [`READY`] and sleep. Every rule is checked first; the error names the rule's number.
pub fn gateway_script(rules: &[SandboxNetRule]) -> Result<String, String> {
    let parsed = rules
        .iter()
        .enumerate()
        .map(|(i, r)| parse(r).map_err(|e| format!("Network rule {}: {e}", i + 1)))
        .collect::<Result<Vec<_>, _>>()?;

    let mut s = String::new();
    s.push_str(
        r#"set -eu
ups=$(awk '$1=="nameserver" && $2 ~ /^[0-9.]+$/ {print $2}' /etc/resolv.conf)
[ -n "$ups" ] || { echo "no IPv4 DNS server for the sandbox network" >&2; exit 1; }
iptables -P INPUT DROP
iptables -P FORWARD DROP
iptables -P OUTPUT DROP
if ! { ip6tables -P INPUT DROP && ip6tables -P FORWARD DROP && ip6tables -P OUTPUT DROP; } 2>/dev/null; then
  if grep -qv ' lo$' /proc/net/if_inet6 2>/dev/null; then
    echo "can't block IPv6 for the sandbox network" >&2; exit 1
  fi
fi
iptables -A INPUT -i lo -j ACCEPT
iptables -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -o lo -j ACCEPT
iptables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
"#,
    );
    let _ = writeln!(
        s,
        "for ns in $ups; do\n  \
         iptables -A OUTPUT -d \"$ns\" -p udp --dport 53 -m owner --uid-owner {DNS_UID} -j ACCEPT\n  \
         iptables -A OUTPUT -d \"$ns\" -p tcp --dport 53 -m owner --uid-owner {DNS_UID} -j ACCEPT\n\
         done"
    );

    // Domains and the sets their addresses go to, in first-seen order.
    let mut domains: Vec<(&str, Vec<String>)> = Vec::new();
    let mut any_host = false;
    for (i, rule) in parsed.iter().enumerate() {
        let target = match &rule.target {
            Target::Any => {
                any_host = true;
                String::new()
            }
            Target::Net(addr, prefix) => format!(" -d {addr}/{prefix}"),
            Target::Domain(domain) => {
                let set = format!("r{}", i + 1);
                let _ = writeln!(s, "ipset create {set} hash:ip");
                match domains.iter_mut().find(|(d, _)| d == domain) {
                    Some((_, sets)) => sets.push(set.clone()),
                    None => domains.push((domain, vec![set.clone()])),
                }
                format!(" -m set --match-set {set} dst")
            }
        };
        let protocols: &[&str] = match rule.protocol {
            NetProtocol::Tcp => &["tcp"],
            NetProtocol::Udp => &["udp"],
            NetProtocol::Any if rule.ports.is_empty() => &[""],
            NetProtocol::Any => &["tcp", "udp"],
        };
        for proto in protocols {
            let proto = if proto.is_empty() {
                String::new()
            } else {
                format!(" -p {proto}")
            };
            if rule.ports.is_empty() {
                let _ = writeln!(s, "iptables -A OUTPUT{target}{proto} -j ACCEPT");
            }
            for (lo, hi) in &rule.ports {
                let port = if lo == hi {
                    lo.to_string()
                } else {
                    format!("{lo}:{hi}")
                };
                let _ = writeln!(
                    s,
                    "iptables -A OUTPUT{target}{proto} --dport {port} -j ACCEPT"
                );
            }
        }
    }
    // Refused rather than dropped, so blocked connections fail at once instead of hanging.
    s.push_str(
        "iptables -A OUTPUT -p tcp -j REJECT --reject-with tcp-reset\n\
         iptables -A OUTPUT -j REJECT\n",
    );

    s.push_str("{\n");
    for line in [
        "no-resolv",
        "no-hosts",
        "listen-address=127.0.0.1",
        "bind-interfaces",
        "user=sandboxdns",
        "pid-file=",
        "filter-AAAA",
    ] {
        let _ = writeln!(s, "echo '{line}'");
    }
    if any_host {
        // Any address is allowed, so any name may resolve.
        s.push_str("for ns in $ups; do echo \"server=$ns\"; done\n");
    } else {
        s.push_str("echo 'address=/#/'\n");
    }
    for (domain, sets) in &domains {
        let _ = writeln!(
            s,
            "for ns in $ups; do echo \"server=/{domain}/$ns\"; done\necho 'ipset=/{domain}/{}'",
            sets.join(",")
        );
    }
    s.push_str("} > /tmp/dnsmasq.conf\n");
    s.push_str("echo 'nameserver 127.0.0.1' > /etc/resolv.conf\n");
    s.push_str("dnsmasq -C /tmp/dnsmasq.conf\n");
    let _ = writeln!(s, "echo {READY}");
    s.push_str("exec sleep infinity\n");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(host: &str, protocol: NetProtocol, ports: &str) -> SandboxNetRule {
        SandboxNetRule {
            host: host.into(),
            protocol,
            ports: ports.into(),
        }
    }

    #[test]
    fn hosts_parse_as_any_networks_or_domains() {
        assert_eq!(parse_host("*"), Ok(Target::Any));
        assert_eq!(
            parse_host("10.0.0.0/8"),
            Ok(Target::Net(Ipv4Addr::new(10, 0, 0, 0), 8))
        );
        assert_eq!(
            parse_host("1.2.3.4"),
            Ok(Target::Net(Ipv4Addr::new(1, 2, 3, 4), 32))
        );
        assert_eq!(
            parse_host("*.PyPI.org."),
            Ok(Target::Domain("pypi.org".into()))
        );
        assert_eq!(
            parse_host("files.pythonhosted.org"),
            Ok(Target::Domain("files.pythonhosted.org".into()))
        );
        for bad in [
            "",
            "1.2.3",
            "1.2.3.4/33",
            "300.1.1.1",
            "::1",
            "fe80::/10",
            "-x.com",
            "a..b",
            "a b.com",
            "a;rm -rf.com",
            "$(id).com",
        ] {
            assert!(parse_host(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ports_parse_as_ranges() {
        assert_eq!(parse_ports(""), Ok(vec![]));
        assert_eq!(
            parse_ports("443, 8000-8100,"),
            Ok(vec![(443, 443), (8000, 8100)])
        );
        for bad in ["0", "65536", "http", "90-80", "1-", "4 43"] {
            assert!(parse_ports(bad).is_err(), "{bad:?}");
        }
        let many = (1..=65)
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_ports(&many).is_err());
    }

    #[test]
    fn script_allows_only_the_rules() {
        let script = gateway_script(&[
            rule("pypi.org", NetProtocol::Tcp, "443"),
            rule("10.0.0.0/8", NetProtocol::Any, "53, 8000-8100"),
            rule("*.pypi.org", NetProtocol::Udp, ""),
            rule("1.1.1.1", NetProtocol::Any, ""),
        ])
        .unwrap();
        for line in [
            "ipset create r1 hash:ip",
            "iptables -A OUTPUT -m set --match-set r1 dst -p tcp --dport 443 -j ACCEPT",
            "iptables -A OUTPUT -d 10.0.0.0/8 -p tcp --dport 53 -j ACCEPT",
            "iptables -A OUTPUT -d 10.0.0.0/8 -p udp --dport 8000:8100 -j ACCEPT",
            "iptables -A OUTPUT -m set --match-set r3 dst -p udp -j ACCEPT",
            "iptables -A OUTPUT -d 1.1.1.1/32 -j ACCEPT",
            "echo 'address=/#/'",
            "for ns in $ups; do echo \"server=/pypi.org/$ns\"; done",
            "echo 'ipset=/pypi.org/r1,r3'",
            "iptables -P OUTPUT DROP",
            "dnsmasq -C /tmp/dnsmasq.conf",
        ] {
            assert!(script.lines().any(|l| l == line), "missing {line:?}");
        }
        // The rules come before the final refusals.
        let last_accept = script.rfind("-j ACCEPT").unwrap();
        assert!(script.find("-j REJECT").unwrap() > last_accept);
        assert!(script.find(READY).unwrap() > script.find("dnsmasq -C").unwrap());
    }

    #[test]
    fn any_host_lets_every_name_resolve() {
        let script = gateway_script(&[rule("*", NetProtocol::Tcp, "443")]).unwrap();
        assert!(script.contains("iptables -A OUTPUT -p tcp --dport 443 -j ACCEPT"));
        assert!(script.contains("echo \"server=$ns\""));
        assert!(!script.contains("address=/#/"));
    }

    #[test]
    fn no_rules_blocks_everything() {
        let script = gateway_script(&[]).unwrap();
        assert_eq!(
            script.matches(" -j ACCEPT").count(),
            // lo and established for INPUT and OUTPUT, and DNS over UDP and TCP for dnsmasq.
            6
        );
        assert!(script.contains("address=/#/"));
    }

    #[test]
    fn a_bad_rule_is_named_by_number() {
        let err = gateway_script(&[
            rule("pypi.org", NetProtocol::Tcp, ""),
            rule("pypi.org", NetProtocol::Tcp, "https"),
        ])
        .unwrap_err();
        assert!(err.starts_with("Network rule 2: "), "{err}");
    }

    #[test]
    fn gateway_image_is_tagged_by_its_dockerfile() {
        let tag = gateway_image();
        assert!(tag.starts_with("cleanrstai-sandbox-net:"));
        assert_eq!(tag.len(), "cleanrstai-sandbox-net:".len() + 12);
        assert_eq!(gateway_name("cleanrstai-abc"), "cleanrstai-abc-net");
    }
}
