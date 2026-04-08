use super::error::ChannelResolutionError;

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedInput {
    Id(String),
    GuildPrefix { guild: String, channel: String },
    ChannelName(String),
}

pub fn parse(input: &str) -> Result<ParsedInput, ChannelResolutionError> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Err(ChannelResolutionError::NotFound("Empty input".to_string()));
    }

    if let Some(slash_pos) = trimmed.rfind('/') {
        let guild_part = &trimmed[..slash_pos];
        let channel_part = &trimmed[slash_pos + 1..];

        if !guild_part.is_empty() && !channel_part.is_empty() {
            return Ok(ParsedInput::GuildPrefix {
                guild: guild_part.to_string(),
                channel: channel_part.to_string(),
            });
        }
    }

    if let Some(colon_pos) = trimmed.find(':') {
        let prefix = &trimmed[..colon_pos];
        let value = &trimmed[colon_pos + 1..];

        if prefix.eq_ignore_ascii_case("channel") && !value.is_empty() {
            return Ok(ParsedInput::Id(value.to_string()));
        }
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Ok(ParsedInput::Id(trimmed.to_string()));
    }

    Ok(ParsedInput::ChannelName(trimmed.to_string()))
}

pub fn normalize_for_comparison(s: &str) -> String {
    s.trim().to_lowercase()
}

pub fn to_slug(name: &str) -> String {
    let normalized = name.trim().to_lowercase();
    let mut slug = String::with_capacity(normalized.len());

    for c in normalized.chars() {
        if c.is_alphanumeric() || c == '-' {
            slug.push(c);
        } else if c == ' ' || c == '_' || c == '#' && !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }

    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bare_id() {
        assert_eq!(parse("123"), Ok(ParsedInput::Id("123".to_string())));
    }

    #[test]
    fn test_parse_explicit_id() {
        assert_eq!(parse("channel:123"), Ok(ParsedInput::Id("123".to_string())));
        assert_eq!(parse("CHANNEL:456"), Ok(ParsedInput::Id("456".to_string())));
    }

    #[test]
    fn test_parse_guild_prefix() {
        assert_eq!(
            parse("my-guild/general"),
            Ok(ParsedInput::GuildPrefix {
                guild: "my-guild".to_string(),
                channel: "general".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_channel_name() {
        assert_eq!(
            parse("general"),
            Ok(ParsedInput::ChannelName("general".to_string()))
        );
        assert_eq!(
            parse("general "),
            Ok(ParsedInput::ChannelName("general".to_string()))
        );
        assert_eq!(
            parse(" my-channel "),
            Ok(ParsedInput::ChannelName("my-channel".to_string()))
        );
    }

    #[test]
    fn test_parse_empty() {
        assert!(matches!(
            parse(""),
            Err(ChannelResolutionError::NotFound(_))
        ));
        assert!(matches!(
            parse("   "),
            Err(ChannelResolutionError::NotFound(_))
        ));
    }

    #[test]
    fn test_normalize_for_comparison() {
        assert_eq!(normalize_for_comparison("  General  "), "general");
        assert_eq!(normalize_for_comparison("UPPERCASE"), "uppercase");
    }

    #[test]
    fn test_to_slug() {
        assert_eq!(to_slug("General Channel"), "general-channel");
        assert_eq!(to_slug("My_Server #general"), "my-server-general");
        assert_eq!(to_slug("already-slug"), "already-slug");
    }
}
