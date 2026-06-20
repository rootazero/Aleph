//! Built-in source display labels for provenance badges (`via {label}`). Falls
//! back to the raw source id for unknown sources (e.g. future config hubs).

#[must_use]
pub fn source_label(source_id: &str) -> &str {
    match source_id {
        "aleph-hub" => "Aleph Hub",
        "clawhub" => "ClawHub",
        "hermes-atlas" => "Hermes Atlas",
        "mcp-official" => "MCP Registry",
        "docker-mcp" => "Docker",
        "cc-marketplace" => "Plugin Marketplace",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown_labels() {
        assert_eq!(source_label("aleph-hub"), "Aleph Hub");
        assert_eq!(source_label("clawhub"), "ClawHub");
        assert_eq!(source_label("some-future-hub"), "some-future-hub");
    }
}
