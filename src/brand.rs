//! Brand assets and MCP icon metadata ([SEP-973](https://modelcontextprotocol.io/specification/2025-11-25/basic#icons)).
//!
//! Icons are shipped as `data:` URIs so stdio MCP clients can render them without
//! an HTTP origin. PNG is required for clients that support icons; SVG is optional.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rmcp::model::Icon;

const ICON_PNG_48: &[u8] = include_bytes!("../assets/icon-48.png");
const ICON_SVG: &[u8] = include_bytes!("../assets/icon.svg");

/// Project homepage (also exposed as MCP `serverInfo.websiteUrl`).
pub const WEBSITE_URL: &str = "https://github.com/hocestnonsatis/Compendium";

/// Icons for MCP `serverInfo` and `tools/list` (`compendium` tool).
pub fn mcp_icons() -> Vec<Icon> {
    vec![
        Icon::new(format!("data:image/png;base64,{}", B64.encode(ICON_PNG_48)))
            .with_mime_type("image/png")
            .with_sizes(vec!["48x48".into()]),
        Icon::new(format!("data:image/svg+xml;base64,{}", B64.encode(ICON_SVG)))
            .with_mime_type("image/svg+xml")
            .with_sizes(vec!["any".into()]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_data_uris() {
        let icons = mcp_icons();
        assert_eq!(icons.len(), 2);
        assert!(icons[0].src.starts_with("data:image/png;base64,"));
        assert_eq!(icons[0].mime_type.as_deref(), Some("image/png"));
        assert!(icons[1].src.starts_with("data:image/svg+xml;base64,"));
        assert_eq!(icons[1].mime_type.as_deref(), Some("image/svg+xml"));
    }
}
