use crate::config::types::acp::OutputFormatSerde;

/// How to parse stdout from a oneshot harness.
#[derive(Debug, Clone)]
pub enum OutputFormat {
    /// Return stdout as-is (trimmed).
    PlainText,
    /// Parse stdout as JSON and extract a specific field.
    /// Falls back to full JSON string if field missing, then raw text if not JSON.
    JsonField { field: String },
}

impl OutputFormat {
    /// Parse stdout according to the format specification.
    pub fn parse(&self, stdout: &str) -> String {
        let trimmed = stdout.trim();
        match self {
            OutputFormat::PlainText => trimmed.to_string(),
            OutputFormat::JsonField { field } => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    if let Some(value) = json.get(field).and_then(|v| v.as_str()) {
                        return value.to_string();
                    }
                    // Field not found or not a string — return full JSON
                    return json.to_string();
                }
                // Not valid JSON — return raw text
                trimmed.to_string()
            }
        }
    }
}

impl From<&OutputFormatSerde> for OutputFormat {
    fn from(fmt: &OutputFormatSerde) -> Self {
        match fmt {
            OutputFormatSerde::PlainText => OutputFormat::PlainText,
            OutputFormatSerde::Json { field } => OutputFormat::JsonField {
                field: field.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_plain_text() {
        let fmt = OutputFormat::PlainText;
        assert_eq!(fmt.parse("  hello world  "), "hello world");
        assert_eq!(fmt.parse("no trim needed"), "no trim needed");
    }

    #[test]
    fn test_output_format_json_field() {
        let fmt = OutputFormat::JsonField {
            field: "result".to_string(),
        };
        let json = r#"{"result": "success", "other": 123}"#;
        assert_eq!(fmt.parse(json), "success");
    }

    #[test]
    fn test_output_format_json_field_missing() {
        let fmt = OutputFormat::JsonField {
            field: "missing".to_string(),
        };
        let json = r#"{"result": "success"}"#;
        assert_eq!(fmt.parse(json), r#"{"result":"success"}"#);
    }

    #[test]
    fn test_output_format_invalid_json() {
        let fmt = OutputFormat::JsonField {
            field: "result".to_string(),
        };
        let text = "not json at all";
        assert_eq!(fmt.parse(text), "not json at all");
    }
}
