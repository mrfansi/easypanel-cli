//! Cloudflare — a bounded context OUTSIDE EasyPanel: manage one or more Cloudflare
//! accounts' zones and DNS records. Nothing here touches the EasyPanel domain; the two
//! share only the TUI event loop and the config directory (separate files).

use serde::{Deserialize, Serialize};

/// A stored Cloudflare account: a user-labelled scoped API token, kept in cloudflare.json
/// independent of any EasyPanel server (an operator may hold several CF accounts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareAccount {
    pub name: String,
    pub api_token: String,
    /// Needed only to CREATE a zone; not needed to list zones or manage records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default)]
    pub default: bool,
}
