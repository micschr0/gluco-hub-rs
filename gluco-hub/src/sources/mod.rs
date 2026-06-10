// SPDX-License-Identifier: AGPL-3.0-or-later

//! Concrete `Source` implementations. Each lives behind its own Cargo
//! feature so the binary stays small when only some sources are enabled.

#[cfg(feature = "source-llu")]
pub mod llu;

#[cfg(feature = "source-ns-socket")]
pub mod ns_socket;
