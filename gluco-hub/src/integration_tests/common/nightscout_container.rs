// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nightscout + MongoDB testcontainer harness (Phase B).
//!
//! Uses testcontainers' `docker-compose` feature to bring up the
//! two-container stack defined in `gluco-hub/tests/fixtures/
//! nightscout-compose.yml`. Each test gets its own stack (no sharing)
//! so independent NS state — `entries` collection, `auth` cache —
//! cannot leak between tests.
//!
//! Boot time is dominated by NS itself (~30 s including the readiness
//! poll); MongoDB is up in a few seconds. Tests that don't need NS
//! (Phase A MQTT) stay fast — only Phase B/C pay this cost.

use std::time::Duration;

use testcontainers::compose::DockerCompose;

/// The compose-file's NS env value. Kept here so the test code that
/// computes the sha1 `api-secret` header doesn't pluck it from the
/// YAML at runtime (which would couple test code to compose parsing).
pub const API_SECRET: &str = "itest-secret-please-rotate";

/// Running stack — owns the compose handle so on-drop teardown brings
/// everything down. Test code uses `ns_url()` to build the
/// `NightscoutClient` against the host-mapped port.
pub struct NightscoutStack {
    compose: DockerCompose,
    ns_port: u16,
}

impl NightscoutStack {
    /// Plain-HTTP base URL ready to drop into `NightscoutClient::new`.
    /// The test stack runs with `INSECURE_USE_HTTP=true` so HTTPS is
    /// not required.
    pub fn ns_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.ns_port)
    }

    pub fn ns_port(&self) -> u16 {
        self.ns_port
    }

    /// Tear the stack down explicitly. Optional — Drop does the same
    /// thing.
    pub async fn shutdown(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let compose = self.compose;
        compose.down().await?;
        Ok(())
    }
}

/// Bring up the NS + Mongo stack via the bundled compose file and
/// return the running handle once NS is responding on its REST port.
///
/// Boot is split into two waits:
///   1. testcontainers' compose layer brings up Mongo (healthcheck
///      from the YAML gates this) and starts NS.
///   2. We then poll NS's `/api/v1/status` until it returns 200 — the
///      NS process listens on the port before it's fully up, so a
///      probe is needed to avoid racing the first real request.
pub async fn start_nightscout_stack()
-> Result<NightscoutStack, Box<dyn std::error::Error + Send + Sync>> {
    // Compose file lives at `<manifest-dir>/tests/fixtures/...`.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let compose_path = format!("{manifest_dir}/tests/fixtures/nightscout-compose.yml");

    let mut compose = DockerCompose::with_local_client(&[compose_path.as_str()]);
    compose.up().await?;

    // `compose.service(name).get_host_port_ipv4(port)` is the API form
    // that's stable across testcontainers 0.27. The flat
    // `compose.get_host_port_ipv4(name, port)` shorthand exists in
    // some docs but isn't on `DockerCompose` directly.
    let ns_port = {
        let ns_service = compose
            .service("nightscout")
            .ok_or("compose: `nightscout` service not running")?;
        ns_service.get_host_port_ipv4(1337).await?
    };
    let stack = NightscoutStack { compose, ns_port };

    wait_for_ns_ready(&stack, Duration::from_secs(120)).await?;

    Ok(stack)
}

/// Poll `/api/v1/status` until it returns a 2xx. NS opens its listener
/// well before the Express app is ready to serve requests; a 30–60 s
/// wait window covers a typical cold start.
async fn wait_for_ns_ready(
    stack: &NightscoutStack,
    deadline: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let url = format!("{}/api/v1/status", stack.ns_url());
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > deadline {
            return Err(
                format!("Nightscout did not become ready at {url} within {deadline:?}",).into(),
            );
        }
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => return Ok(()),
            _ => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }
}

/// Convenience helper for tests: read entries back from the running
/// stack via the v3 API. Returns the parsed JSON `result` array (or
/// the raw body on parse failure for diagnostic output).
pub async fn fetch_entries_v3(
    stack: &NightscoutStack,
    api_secret_sha1_hex: &str,
    count: u32,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    // NS v3 expects `sort$desc=date` literally — the `$` is part of
    // the API's bracket-style sort syntax, not URL-encoded. Build the
    // query string by hand so `RequestBuilder::query()`'s Serialize
    // path doesn't escape it.
    let url = format!(
        "{}/api/v3/entries?count={}&sort$desc=date",
        stack.ns_url(),
        count,
    );
    let resp = client
        .get(url)
        .header("api-secret", api_secret_sha1_hex)
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    Ok(body)
}

/// sha1(API_SECRET) hex — same hash NS expects in the `api-secret`
/// header. Re-implemented locally instead of pulling the sink-internal
/// helper to keep this module standalone-testable.
pub fn api_secret_sha1() -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(API_SECRET.as_bytes());
    let digest = h.finalize();
    hex(&digest)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
