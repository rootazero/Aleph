use super::error::Candidate;
use super::input::{normalize_for_comparison, to_slug};
use crate::gateway::interfaces::discord::api::{ChannelSummary, GuildSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStrategy {
    Exact,
    Name,
    Slug,
    Fuzzy,
}

pub fn match_channel(channel: &ChannelSummary, query: &str, strategy: SearchStrategy) -> bool {
    match strategy {
        SearchStrategy::Exact => channel.channel_id.to_string() == query,
        SearchStrategy::Name => {
            normalize_for_comparison(&channel.name) == normalize_for_comparison(query)
        }
        SearchStrategy::Slug => to_slug(&channel.name) == to_slug(query),
        SearchStrategy::Fuzzy => {
            let channel_name = normalize_for_comparison(&channel.name);
            let query_norm = normalize_for_comparison(query);
            levenshtein_distance(&channel_name, &query_norm) <= 3
        }
    }
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let len_a = a.chars().count();
    let len_b = b.chars().count();

    if len_a == 0 {
        return len_b;
    }
    if len_b == 0 {
        return len_a;
    }

    let mut prev = (0..=len_b).collect::<Vec<_>>();
    let mut curr = vec![0; len_b + 1];

    for i in 1..=len_a {
        curr[0] = i;
        for j in 1..=len_b {
            let cost = if a.chars().nth(i - 1) == b.chars().nth(j - 1) {
                0
            } else {
                1
            };
            curr[j] = std::cmp::min(
                prev[j] + 1,
                std::cmp::min(curr[j - 1] + 1, prev[j - 1] + cost),
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[len_b]
}

pub fn search_in_guild(
    channels: &[ChannelSummary],
    query: &str,
    guild: &GuildSummary,
) -> Vec<Candidate> {
    let mut matches = Vec::new();

    for strategy in &[
        SearchStrategy::Exact,
        SearchStrategy::Name,
        SearchStrategy::Slug,
        SearchStrategy::Fuzzy,
    ] {
        for channel in channels {
            if channel.channel_type != 0 {
                continue;
            }

            if match_channel(channel, query, *strategy) {
                matches.push(Candidate {
                    channel_id: channel.channel_id.to_string(),
                    channel_name: channel.name.clone(),
                    guild_id: guild.guild_id.to_string(),
                    guild_name: guild.name.clone(),
                });
            }
        }

        if !matches.is_empty() {
            return matches;
        }
    }

    matches
}

pub fn search_in_guilds(
    guilds: &[(GuildSummary, Vec<ChannelSummary>)],
    query: &str,
) -> Vec<Candidate> {
    let mut all_matches = Vec::new();

    for (guild, channels) in guilds {
        let guild_matches = search_in_guild(channels, query, guild);
        all_matches.extend(guild_matches);
    }

    all_matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel(id: u64, name: &str, channel_type: u8) -> ChannelSummary {
        ChannelSummary {
            channel_id: id,
            name: name.to_string(),
            channel_type,
            position: 0,
        }
    }

    fn make_guild(id: u64, name: &str) -> GuildSummary {
        GuildSummary {
            guild_id: id,
            name: name.to_string(),
            icon: None,
            member_count: None,
            bot_permissions: 0,
        }
    }

    #[test]
    fn test_exact_id_match() {
        let ch = make_channel(123, "general", 0);
        assert!(match_channel(&ch, "123", SearchStrategy::Exact));
        assert!(!match_channel(&ch, "456", SearchStrategy::Exact));
    }

    #[test]
    fn test_name_match_case_insensitive() {
        let ch = make_channel(1, "General", 0);
        assert!(match_channel(&ch, "general", SearchStrategy::Name));
        assert!(match_channel(&ch, "GENERAL", SearchStrategy::Name));
        assert!(!match_channel(&ch, "random", SearchStrategy::Name));
    }

    #[test]
    fn test_slug_match() {
        let ch = make_channel(1, "General Channel", 0);
        assert!(match_channel(&ch, "general-channel", SearchStrategy::Slug));
        assert!(match_channel(&ch, "General Channel", SearchStrategy::Slug));
        assert!(!match_channel(&ch, "random", SearchStrategy::Slug));
    }

    #[test]
    fn test_fuzzy_match() {
        let ch = make_channel(1, "general", 0);
        assert!(match_channel(&ch, "generl", SearchStrategy::Fuzzy));
        assert!(match_channel(&ch, "jeneral", SearchStrategy::Fuzzy));
        assert!(!match_channel(&ch, "xyz", SearchStrategy::Fuzzy));
    }

    #[test]
    fn test_priority_order() {
        let channels = vec![
            make_channel(1, "general", 0),
            make_channel(2, "General Channel", 0),
        ];

        let guild = make_guild(100, "Test");
        let matches = search_in_guild(&channels, "general", &guild);
        assert_eq!(matches.len(), 1);

        let matches = search_in_guild(&channels, "general-channel", &guild);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].channel_name, "General Channel");
    }

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein_distance("kitten", "kitten"), 0);
        assert_eq!(levenshtein_distance("kitten", "kittan"), 1);
        assert_eq!(levenshtein_distance("kitten", "kittens"), 1);
        assert_eq!(levenshtein_distance("kitten", "mitten"), 1);
        assert_eq!(levenshtein_distance("hello", "world"), 4);
    }
}
