use alephcore::gateway::interfaces::email::EmailMessageOps;

#[test]
fn test_email_fixture_parsing() {
    let json_str = include_str!("fixtures/email/text_email.json");
    let data: serde_json::Value = serde_json::from_str(json_str).unwrap();

    assert_eq!(data["subject"], "[coder] Fix this bug");
    assert_eq!(data["from"], "user@example.com");
    assert_eq!(data["body_text"], "Please fix the login bug");
    assert_eq!(data["message_id"], "\u{003c}msg123@example.com\u{003e}");
}

#[test]
fn test_extract_agent_from_subject() {
    assert_eq!(
        EmailMessageOps::extract_agent_from_subject("[coder] Fix this bug"),
        Some("coder".to_string())
    );
    assert_eq!(
        EmailMessageOps::extract_agent_from_subject("[reviewer] Check PR #42"),
        Some("reviewer".to_string())
    );
    assert_eq!(
        EmailMessageOps::extract_agent_from_subject("No brackets here"),
        None
    );
    assert_eq!(
        EmailMessageOps::extract_agent_from_subject("[] Empty brackets"),
        None
    );
}

#[test]
fn test_strip_agent_tag() {
    assert_eq!(
        EmailMessageOps::strip_agent_tag("[coder] Fix this bug"),
        "Fix this bug"
    );
    assert_eq!(
        EmailMessageOps::strip_agent_tag("No brackets here"),
        "No brackets here"
    );
}

#[test]
fn test_markdown_to_html_email() {
    let html = EmailMessageOps::markdown_to_html_email("**bold** and *italic*");
    assert!(html.contains("\u{003c}strong\u{003e}bold\u{003c}/strong\u{003e}"));
    assert!(html.contains("\u{003c}em\u{003e}italic\u{003c}/em\u{003e}"));
}
