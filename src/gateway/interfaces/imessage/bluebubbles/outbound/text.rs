//! Outbound text: split into iMessage bubbles, resolve chat GUID, POST.

/// Split text into bubbles: paragraphs (blank-line separated) first, then any
/// over-length paragraph hard-wrapped at `max`. Mirrors hermes' splitter minus
/// the "(1/3)" pagination suffix.
#[must_use]
pub fn split_into_bubbles(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let paras: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let paras = if paras.is_empty() { vec![text] } else { paras };
    for p in paras {
        if p.len() <= max {
            out.push(p.to_string());
        } else {
            let mut rest = p;
            while !rest.is_empty() {
                // nth(max) gives the byte offset of the char at position `max` (the first
                // char AFTER our desired chunk). If fewer than `max` chars remain, None →
                // take the whole rest. This is UTF-8-safe: we always split at a char boundary.
                let end = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
                let (head, tail) = rest.split_at(end);
                out.push(head.to_string());
                rest = tail;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::split_into_bubbles;

    #[test]
    fn splits_on_blank_lines_and_caps_length() {
        let out = split_into_bubbles("a\n\nb", 4000);
        assert_eq!(out, vec!["a".to_string(), "b".to_string()]);
        let long = "x".repeat(5000);
        let out = split_into_bubbles(&long, 4000);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.len() <= 4000));
    }

    #[test]
    fn guid_cache_is_lru_bounded() {
        let mut c = super::super::super::api::LruGuidCache::new(2);
        c.put("a", "ga");
        c.put("b", "gb");
        c.put("c", "gc"); // evicts "a"
        assert_eq!(c.get("a"), None);
        assert_eq!(c.get("c").as_deref(), Some("gc"));
    }
}
