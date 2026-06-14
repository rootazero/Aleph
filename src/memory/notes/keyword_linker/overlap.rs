//! Deterministic note pairing by keyword-set overlap. No LLM, no embedding.

use std::collections::BTreeSet;

/// A note's path plus its extracted keyword set.
#[derive(Debug, Clone)]
pub struct NoteKeywords {
    pub path: String,
    pub keywords: Vec<String>,
}

/// An undirected link candidate with the connecting keyword as `relation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTriple {
    pub from: String,
    pub to: String,
    pub relation: String,
}

/// A keyword is "specific" when it names an entity/multi-token concept —
/// heuristically, it contains a `-` or whitespace (e.g. `us-iran-conflict`).
fn is_specific(keyword: &str) -> bool {
    keyword.contains('-') || keyword.contains(char::is_whitespace)
}

/// Pair every note against every other; emit a link when their keyword sets
/// share ≥1 specific entity OR ≥2 generic keywords. The connecting keyword
/// (most specific shared one, else lexicographically-first) is the relation.
/// Pairs are undirected and unique (i<j), no self-links.
pub fn pair_by_overlap(notes: &[NoteKeywords]) -> Vec<LinkTriple> {
    let sets: Vec<BTreeSet<&str>> = notes
        .iter()
        .map(|n| n.keywords.iter().map(String::as_str).collect())
        .collect();
    let mut out = Vec::new();
    for i in 0..notes.len() {
        for j in (i + 1)..notes.len() {
            let shared: Vec<&str> = sets[i].intersection(&sets[j]).copied().collect();
            if shared.is_empty() {
                continue;
            }
            let specific: Vec<&str> = shared.iter().copied().filter(|s| is_specific(s)).collect();
            let connects = if !specific.is_empty() {
                specific.iter().copied().max_by_key(|s| s.len())
            } else if shared.len() >= 2 {
                shared.iter().copied().min()
            } else {
                None
            };
            if let Some(relation) = connects {
                out.push(LinkTriple {
                    from: notes[i].path.clone(),
                    to: notes[j].path.clone(),
                    relation: relation.to_string(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(path: &str, words: &[&str]) -> NoteKeywords {
        NoteKeywords {
            path: path.to_string(),
            keywords: words.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn links_on_one_shared_specific_entity() {
        let notes = vec![
            kw(
                "entity/us-iran-conflict-2026",
                &["us-iran-conflict", "ceasefire"],
            ),
            kw("personal/news-monitoring", &["us-iran-conflict", "cron"]),
        ];
        let links = pair_by_overlap(&notes);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from, "entity/us-iran-conflict-2026");
        assert_eq!(links[0].to, "personal/news-monitoring");
        assert_eq!(links[0].relation, "us-iran-conflict");
    }

    #[test]
    fn no_link_on_single_generic_keyword() {
        let notes = vec![kw("a/x", &["news", "alpha"]), kw("a/y", &["news", "beta"])];
        assert!(pair_by_overlap(&notes).is_empty());
    }

    #[test]
    fn links_on_two_shared_generic_keywords() {
        let notes = vec![
            kw("a/x", &["news", "finance", "alpha"]),
            kw("a/y", &["news", "finance", "beta"]),
        ];
        let links = pair_by_overlap(&notes);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].relation, "finance");
    }

    #[test]
    fn no_self_link_and_pairs_are_unordered_unique() {
        let notes = vec![
            kw("a/x", &["topic-one"]),
            kw("a/y", &["topic-one"]),
            kw("a/z", &["topic-one"]),
        ];
        let links = pair_by_overlap(&notes);
        assert_eq!(links.len(), 3);
        assert!(links.iter().all(|l| l.from != l.to));
    }
}
