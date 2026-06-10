// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nightscout Socket.IO source (Roadmap **V6**).
//!
//! Uses a Nightscout site as the *upstream* CGM data source via its
//! real-time Socket.IO feed, instead of polling LibreLink Up. gluco-hub
//! subscribes to the feed, normalises each `sgv` entry into a
//! [`gluco_hub_core::Reading`], and fans out to the existing sinks
//! (Nightscout / MQTT / HA) and the HTTP API. This is a **standalone**
//! alternative to LLU — running LLU and NS-Socket simultaneously stays
//! deferred (see `CLAUDE.md` Roadmap → Deferred → multi-source routing).
//!
//! # Status
//!
//! **V6 scaffold only.** The module structure, config, `Source` impl, and
//! error codes are in place; the actual Socket.IO connect/subscribe loop is
//! a stub returning `[NSS001]` (see [`client::NsSocketClient::connect`]).
//!
//! # Verified Nightscout Socket.IO contract
//!
//! Verified 2026-06-10 against the official `cgm-remote-monitor` source
//! (`lib/server/websocket.js`, `lib/data/calcdelta.js`) and the Socket.IO v4
//! client docs. Record kept here and in `docs/EXTENDING.md` so the eventual
//! implementation does not have to re-derive it.
//!
//! - **Transport / namespace**: Socket.IO **v4** over an Engine.IO
//!   websocket. Nightscout uses the **default namespace** (`/`) — no custom
//!   namespace. The Engine.IO path is the default `/socket.io/`. Connect with
//!   **wss** for `https` Nightscout origins.
//! - **Auth handshake**: the client emits an **`authorize`** event with a
//!   payload object:
//!   - `client`: a string identifying the client type (Nightscout's own
//!     clients send `"web"` / `"phone"` / `"pump"`); we send a stable
//!     `"gluco-hub"`-style marker.
//!   - `secret`: the **SHA-1 hash of the API secret** (api-secret mode), or
//!   - `token`: an **access token** such as `myreader-0123456789abcdef`
//!     (token mode, preferred on modern deployments).
//!   - `history`: number of **hours** of history to replay (server default
//!     48).
//!
//!   On success the server emits a **`connected`** event and acks with an
//!   authorization object `{ read, write, write_treatment }` (booleans).
//! - **Data push event**: the server broadcasts **`dataUpdate`** to
//!   authorized clients. The payload is a *delta* object initialised with
//!   `delta: true` and a `lastUpdated` millisecond timestamp. When sensor
//!   glucose changed it carries an **`sgvs`** array; on the first push (or
//!   when no prior state exists) the server sends the full dataset instead.
//! - **Entry/`sgv` fields** (Nightscout entries model): each element carries
//!   `mills` (a.k.a. `date`, **Unix epoch milliseconds**), `sgv` / `mgdl`
//!   (glucose in mg/dL), and `direction` (trend string, e.g. `Flat`,
//!   `SingleUp`, `FortyFiveDown`, `NOT COMPUTABLE`, `RATE OUT OF RANGE`).
//!   These map onto [`gluco_hub_core::Trend`] (note the two space-separated
//!   strings normalise to `NotComputable` / `RateOutOfRange`).
//!
//! Sources:
//! - <https://github.com/nightscout/cgm-remote-monitor> (`lib/server/websocket.js`, `lib/data/calcdelta.js`)
//! - <https://socket.io/docs/v4/client-api/>
//! - <https://nightscout.github.io/nightscout/setup_variables/>

pub mod client;
pub mod error;
pub mod source;
