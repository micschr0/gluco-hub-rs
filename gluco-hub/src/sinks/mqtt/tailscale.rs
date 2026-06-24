// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tailscale Local API resolver for MQTT broker hostname discovery.
//!
//! When `tailscale_hostname` is set in `[sink.mqtt]`, gluco-hub queries
//! the local `tailscaled` daemon's HTTP API at `http://100.100.100.100`
//! to resolve a MagicDNS name or tailnet hostname to a tailnet IP.
//! The resolved IP replaces `broker_host` for the lifetime of the sink.
//!
//! If the daemon is unreachable or the hostname is not found, the sink
//! falls back to the configured `broker_host` and logs a warning — a
//! missing tailscaled daemon is a configuration edge case, not a crash.

use std::net::Ipv4Addr;

use tracing::{info, warn};

/// Resolve a Tailscale MagicDNS hostname to a tailnet IP address
/// using the local `tailscaled` daemon's HTTP API.
///
/// Returns `Some(ip)` if the hostname is found in the tailnet peer
/// list, `None` if the daemon is unreachable or the hostname is
/// not present.
pub async fn resolve_tailscale_hostname(hostname: &str) -> Option<String> {
    let url = "http://100.100.100.100/localapi/v0/status";
    let client = match reqwest::Client::builder()
        // The local API is unauthenticated and only reachable from
        // the same machine — a short timeout is safe and prevents
        // blocking startup when tailscaled is not running.
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("failed to build reqwest client for Tailscale resolution: {e}");
            return None;
        }
    };

    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("tailscaled daemon not reachable at {url}: {e}");
            return None;
        }
    };

    let status: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            warn!("failed to parse tailscaled status response: {e}");
            return None;
        }
    };

    // Tailscale local API returns `Peer` as a JSON object keyed by node-key,
    // not an array — iterate over the map values.
    let peer_map = match status["Peer"].as_object() {
        Some(p) => p,
        None => {
            warn!("tailscaled status response has no Peer object");
            return None;
        }
    };

    for peer in peer_map.values() {
        let dns_name = peer["DNSName"].as_str().unwrap_or("").trim_end_matches('.');
        if (dns_name == hostname || dns_name.starts_with(&format!("{hostname}.")))
            && let Some(ip) = peer["TailscaleIPs"]
                .as_array()
                .and_then(|ips| ips.first())
                .and_then(|ip| ip.as_str())
        {
            if !is_tailscale_cgnat_ip(ip) {
                warn!(
                    hostname = %hostname,
                    ip = %ip,
                    "tailscaled returned IP outside Tailscale CGNAT range (100.64.0.0/10); ignoring"
                );
                return None;
            }
            info!(
                hostname = %hostname,
                resolved_ip = %ip,
                "resolved Tailscale hostname to tailnet IP"
            );
            return Some(ip.to_string());
        }
    }

    warn!(
        "Tailscale hostname {hostname} not found in tailnet peer list \
         — falling back to configured broker_host"
    );
    None
}

/// Returns true if `ip` falls within the Tailscale CGNAT range 100.64.0.0/10.
fn is_tailscale_cgnat_ip(ip: &str) -> bool {
    ip.parse::<Ipv4Addr>()
        .map(|a| {
            let [first, second, ..] = a.octets();
            first == 100 && (second & 0xC0) == 64
        })
        .unwrap_or(false)
}
