//! Command palette ranking and keyboard-shortcut resolution.
//!
//! `score` moved here unchanged from the Leptos command palette so both
//! Panels rank commands the same way. `resolve_bindings` decides which key
//! each command ends up with; it does not know keystroke syntax — the caller
//! passes the host's normaliser (NSH `Keystroke::parse_with` on the TSX side).

use std::collections::BTreeMap;

use super::Binding;

/// Substring score: 0 = no match, >0 = better. Word-start matches outrank
/// mid-word; label matches outrank keyword matches. Empty query returns
/// `u32::MAX` so everything passes through.
#[must_use]
pub fn score(query: &str, label: &str, keywords: &[&str]) -> u32 {
    if query.is_empty() {
        return u32::MAX;
    }
    let q = query.to_lowercase();
    let l = label.to_lowercase();
    let mut best: u32 = 0;
    if l.starts_with(&q) {
        best = best.max(100);
    } else if l.contains(&q) {
        if l.split(|c: char| !c.is_alphanumeric())
            .any(|t| t.starts_with(&q))
        {
            best = best.max(70);
        } else {
            best = best.max(40);
        }
    }
    for k in keywords {
        let kl = k.to_lowercase();
        if kl.starts_with(&q) {
            best = best.max(35);
        } else if kl.contains(&q) {
            best = best.max(20);
        }
    }
    best
}

/// One palette entry to rank.
#[derive(Debug, Clone)]
pub struct RankItem<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub keywords: Vec<&'a str>,
}

/// Ids of the items matching `query`, best first. Ties keep input order, and
/// an empty query returns every item in input order.
#[must_use]
pub fn rank<'a>(query: &str, items: &[RankItem<'a>]) -> Vec<&'a str> {
    let mut scored: Vec<(u32, usize, &'a str)> = items
        .iter()
        .enumerate()
        .map(|(position, item)| (score(query, item.label, &item.keywords), position, item.id))
        .filter(|(points, _, _)| *points > 0)
        .collect();
    // Higher score first; equal scores keep their input position.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, id)| id).collect()
}

/// A command and the key it ships with.
#[derive(Debug, Clone, Copy)]
pub struct CommandKey<'a> {
    pub id: &'a str,
    pub default_key: Option<&'a str>,
}

/// Where a command's key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Default,
    User,
    Cleared,
}

/// Why a key is not in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyProblem {
    /// The text is not a keystroke.
    Invalid,
    /// An earlier command already owns the same keystroke (its id).
    Conflict(String),
}

/// The outcome for one command. `key` is the normalised keystroke; it is
/// filled for conflicts too so settings can show it, and only takes effect
/// when `problem` is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKey {
    pub id: String,
    pub key: Option<String>,
    pub source: KeySource,
    pub problem: Option<KeyProblem>,
}

/// Resolve every command's key in order. User overrides beat defaults; a
/// keystroke `normalize` rejects is `Invalid`; a keystroke an earlier command
/// already owns is a `Conflict` and does not take effect.
pub fn resolve_bindings(
    commands: &[CommandKey<'_>],
    overrides: &BTreeMap<String, Binding>,
    normalize: impl Fn(&str) -> Option<String>,
) -> Vec<ResolvedKey> {
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    let mut resolved = Vec::with_capacity(commands.len());
    for command in commands {
        let (text, source) = match overrides.get(command.id) {
            Some(Binding::Key(text)) => (Some(text.as_str()), KeySource::User),
            Some(Binding::Cleared) => (None, KeySource::Cleared),
            None => (command.default_key, KeySource::Default),
        };
        let mut entry = ResolvedKey {
            id: command.id.to_owned(),
            key: None,
            source,
            problem: None,
        };
        if let Some(text) = text {
            match normalize(text) {
                None => entry.problem = Some(KeyProblem::Invalid),
                Some(key) => {
                    if let Some(owner) = owners.get(&key) {
                        entry.problem = Some(KeyProblem::Conflict(owner.clone()));
                    } else {
                        owners.insert(key.clone(), command.id.to_owned());
                    }
                    entry.key = Some(key);
                }
            }
        }
        resolved.push(entry);
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_passes_all() {
        assert_eq!(score("", "Anything", &["k1"]), u32::MAX);
    }

    #[test]
    fn label_prefix_beats_substring_beats_keyword() {
        let prefix = score("set", "Settings", &["preferences"]);
        let mid = score("ings", "Settings", &["preferences"]);
        let kw_prefix = score("pre", "Settings", &["preferences"]);
        let kw_mid = score("ref", "Settings", &["preferences"]);
        assert!(prefix > mid);
        assert!(prefix > kw_prefix);
        assert!(kw_prefix > kw_mid);
    }

    #[test]
    fn case_insensitive() {
        assert!(score("SET", "settings", &[]) > 0);
        assert!(score("set", "SETTINGS", &[]) > 0);
    }

    #[test]
    fn no_match_returns_zero() {
        assert_eq!(score("zzz", "Settings", &["preferences"]), 0);
    }

    #[test]
    fn word_start_beats_random_substring() {
        // "mem" matches inside "Remember" mid-word vs at start of "Memory"
        let start = score("mem", "Memory", &[]);
        let mid = score("mem", "Remember", &[]);
        assert!(start > mid);
    }

    fn item<'a>(id: &'a str, label: &'a str) -> RankItem<'a> {
        RankItem {
            id,
            label,
            keywords: Vec::new(),
        }
    }

    #[test]
    fn rank_orders_by_score_then_input() {
        let items = [
            item("a", "Open Chat"),
            item("b", "Chat history"),
            item("c", "Settings"),
            item("d", "Chatter"),
        ];
        assert_eq!(rank("chat", &items), vec!["b", "d", "a"]);
        assert_eq!(rank("", &items), vec!["a", "b", "c", "d"]);
        assert!(rank("zzz", &items).is_empty());
    }

    /// Lowercases and treats `mod` as `cmd`; rejects text containing spaces.
    fn normalize(text: &str) -> Option<String> {
        (!text.contains(' ')).then(|| text.to_lowercase().replace("mod", "cmd"))
    }

    fn keys<'a>(pairs: &[(&'a str, Option<&'a str>)]) -> Vec<CommandKey<'a>> {
        pairs
            .iter()
            .map(|(id, default_key)| CommandKey {
                id,
                default_key: *default_key,
            })
            .collect()
    }

    #[test]
    fn user_override_beats_default() {
        let commands = keys(&[("palette", Some("mod+k"))]);
        let overrides = BTreeMap::from([("palette".to_owned(), Binding::Key("mod+j".into()))]);
        let resolved = resolve_bindings(&commands, &overrides, normalize);
        assert_eq!(resolved[0].key.as_deref(), Some("cmd+j"));
        assert_eq!(resolved[0].source, KeySource::User);
        assert_eq!(resolved[0].problem, None);
    }

    #[test]
    fn cleared_has_no_key() {
        let commands = keys(&[("palette", Some("mod+k"))]);
        let overrides = BTreeMap::from([("palette".to_owned(), Binding::Cleared)]);
        let resolved = resolve_bindings(&commands, &overrides, normalize);
        assert_eq!(resolved[0].key, None);
        assert_eq!(resolved[0].source, KeySource::Cleared);
        assert_eq!(resolved[0].problem, None);
    }

    #[test]
    fn invalid_override_is_reported() {
        let commands = keys(&[("palette", Some("mod+k")), ("settings", Some("mod+,"))]);
        let overrides = BTreeMap::from([("palette".to_owned(), Binding::Key("not a key".into()))]);
        let resolved = resolve_bindings(&commands, &overrides, normalize);
        assert_eq!(resolved[0].key, None);
        assert_eq!(resolved[0].problem, Some(KeyProblem::Invalid));
        // One bad entry leaves the others alone.
        assert_eq!(resolved[1].key.as_deref(), Some("cmd+,"));
        assert_eq!(resolved[1].problem, None);
    }

    #[test]
    fn conflict_on_equivalent_keystrokes() {
        let commands = keys(&[("palette", Some("mod+k")), ("settings", None)]);
        let overrides = BTreeMap::from([("settings".to_owned(), Binding::Key("cmd+K".into()))]);
        let resolved = resolve_bindings(&commands, &overrides, normalize);
        assert_eq!(resolved[0].problem, None);
        assert_eq!(resolved[1].key.as_deref(), Some("cmd+k"));
        assert_eq!(
            resolved[1].problem,
            Some(KeyProblem::Conflict("palette".to_owned()))
        );
    }
}
