# Extending gluco-hub

Adding a new **source** (where readings come from) or **sink** (where readings go) takes one new file plus a Cargo feature flag — never a refactor.

## How it works

Sources and sinks are small async traits defined in `gluco-hub-core`:

```rust
// gluco-hub-core/src/source.rs
#[async_trait]
pub trait Source: Send + Sync + 'static {
    fn id(&self) -> &SourceId;
    async fn fetch_latest(&self) -> Result<Vec<Reading>, CoreError>;
}

// gluco-hub-core/src/sink.rs
#[async_trait]
pub trait Sink: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    async fn push(&self, readings: &[Reading]) -> Result<(), CoreError>;
}
```

The poller fetches from one `Source`, caches the latest reading, and fans it out to every configured `Sink` in parallel. Each sink fails independently; the others keep running.

## Adding a new sink

Building a complete sink takes five steps. The example below adds a sink that POSTs readings to a custom webhook.

### 1. Create the module

```text
gluco-hub/src/sinks/webhook/
  ├── mod.rs
  └── sink.rs
```

### 2. Implement the trait

```rust
// gluco-hub/src/sinks/webhook/sink.rs
use async_trait::async_trait;
use gluco_hub_core::{CoreError, Reading, Sink};

pub struct WebhookSink {
    url: String,
    client: reqwest::Client,
}

#[async_trait]
impl Sink for WebhookSink {
    fn name(&self) -> &'static str { "webhook" }

    async fn push(&self, readings: &[Reading]) -> Result<(), CoreError> {
        self.client
            .post(&self.url)
            .json(readings)
            .send()
            .await
            .map_err(|e| CoreError::Sink(e.to_string()))?
            .error_for_status()
            .map_err(|e| CoreError::Sink(e.to_string()))?;
        Ok(())
    }
}
```

### 3. Add a Cargo feature

```toml
# gluco-hub/Cargo.toml
[features]
sink-webhook = ["dep:reqwest"]
```

### 4. Register the module

```rust
// gluco-hub/src/sinks/mod.rs
#[cfg(feature = "sink-webhook")]
pub mod webhook;
```

### 5. Wire it into the binary

```rust
// gluco-hub/src/main.rs — inside build_sinks()
#[cfg(feature = "sink-webhook")]
if let Some(cfg) = cfg.sink.webhook.as_ref() {
    sinks.push(Arc::new(WebhookSink::new(cfg.url.clone())));
}
```

Add the matching `[sink.webhook]` section to `Config` in `gluco-hub/src/config.rs`. Build with `--features sink-webhook` and configure it via TOML or `GLUCO_HUB__SINK__WEBHOOK__URL`.

## Adding a new source

Sources follow the same pattern. The example below adds a source that reads from a local CGM file.

### 1. Implement the trait

```rust
// gluco-hub/src/sources/file/source.rs
use async_trait::async_trait;
use gluco_hub_core::{CoreError, Reading, Source, SourceId};

pub struct FileSource {
    id: SourceId,
    path: PathBuf,
}

#[async_trait]
impl Source for FileSource {
    fn id(&self) -> &SourceId { &self.id }

    async fn fetch_latest(&self) -> Result<Vec<Reading>, CoreError> {
        let bytes = tokio::fs::read(&self.path).await
            .map_err(|e| CoreError::Source(e.to_string()))?;
        let readings: Vec<Reading> = serde_json::from_slice(&bytes)
            .map_err(|e| CoreError::Source(e.to_string()))?;
        Ok(readings)
    }
}
```

### 2. Cargo feature + module registration

```toml
# gluco-hub/Cargo.toml
[features]
source-file = []
```

```rust
// gluco-hub/src/sources/mod.rs
#[cfg(feature = "source-file")]
pub mod file;
```

### 3. Wire it into the binary

Add a branch for your variant to the source-selection code in `main.rs`, which picks one source per run based on `[source.*]` config.

## Testing

For sources, use the in-memory `MockSource` in `gluco-hub-core::mock` to drive tests without external services. For sinks, use `wiremock` to mock the HTTP target. See `gluco-hub/src/e2e_tests.rs` for end-to-end examples.

```rust
#[tokio::test]
async fn webhook_sink_posts_readings() {
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let sink = WebhookSink::new(server.uri());
    sink.push(&[Reading::test_fixture()]).await.unwrap();
}
```

## Conventions

- **Errors**: use `CoreError::Sink` / `CoreError::Source` with a stable error code prefix (`SNK*` / `SRC*`).
- **Logs**: emit `tracing` events with structured fields — never log secrets.
- **Idempotency**: the poller may retry `push()`; deduplicate on the receiving side or via local state.
- **Validation**: validate config at startup via `validator`, not at push time.
- **No new dependencies** without `cargo deny check` passing.

See [ARCHITECTURE.md](./ARCHITECTURE.md) for the full data flow and module map.
