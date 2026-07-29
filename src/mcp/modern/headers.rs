//! Streamable HTTP request metadata headers (`2026-07-28`).
//!
//! The transport mirrors selected body fields into HTTP headers so gateways and
//! load balancers can route and authorize without parsing the body. Servers
//! **must** reject any request whose headers disagree with its body
//! (`HeaderMismatch`, `-32020`), so every value here is derived *from the body
//! that is about to be sent* — the two cannot drift because there is only one
//! source.
//!
//! Three families:
//!
//! - `Mcp-Method` — the JSON-RPC `method`. Required on all requests.
//! - `Mcp-Name` — `params.name` or `params.uri`. Required on `tools/call`,
//!   `resources/read`, and `prompts/get`.
//! - `Mcp-Param-{Name}` — tool arguments a server asked to have mirrored via an
//!   `x-mcp-header` annotation in its `inputSchema`. Optional for servers to
//!   use; clients **must** support it.
//!
//! Values that cannot be carried literally in an HTTP header are wrapped in the
//! `=?base64?…?=` sentinel, which servers decode before comparing against the
//! body.

use std::collections::HashSet;

use base64::Engine;
use serde_json::Value;

/// Header carrying the protocol revision. Must equal the `_meta` copy in the body.
pub const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";
/// Header carrying the JSON-RPC method name.
pub const HEADER_METHOD: &str = "mcp-method";
/// Header carrying the target tool name, resource URI, or prompt name.
pub const HEADER_NAME: &str = "mcp-name";
/// Prefix for headers mirrored from annotated tool parameters.
pub const PARAM_HEADER_PREFIX: &str = "mcp-param-";
/// The tool-schema annotation that requests parameter mirroring.
pub const X_MCP_HEADER: &str = "x-mcp-header";

/// Opening marker of the base64 sentinel encoding. Case-sensitive.
const SENTINEL_PREFIX: &str = "=?base64?";
/// Closing marker of the base64 sentinel encoding. Case-sensitive.
const SENTINEL_SUFFIX: &str = "?=";

/// Largest magnitude an integer may carry and still round-trip through a
/// JavaScript number (2^53 - 1).
const JS_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Whether a value can be carried literally in an HTTP header field value.
///
/// Per RFC 9110 a field value is visible ASCII, space, and horizontal tab; the
/// MCP spec additionally excludes leading/trailing whitespace (intermediaries
/// strip it, which would break the server's header-vs-body comparison) and any
/// value that would be mistaken for the sentinel encoding.
fn is_header_safe(value: &str) -> bool {
    if value.starts_with(SENTINEL_PREFIX) && value.ends_with(SENTINEL_SUFFIX) {
        return false;
    }
    if value.starts_with([' ', '\t']) || value.ends_with([' ', '\t']) {
        return false;
    }
    value
        .chars()
        .all(|c| c == '\t' || ('\u{20}'..='\u{7e}').contains(&c))
}

/// Encode a value for transmission in an HTTP header.
///
/// Returns the value unchanged when it is already header-safe, and the
/// `=?base64?…?=` sentinel form otherwise.
#[must_use]
pub fn encode_header_value(value: &str) -> String {
    if is_header_safe(value) {
        return value.to_string();
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
    format!("{SENTINEL_PREFIX}{encoded}{SENTINEL_SUFFIX}")
}

/// The `Mcp-Name` value for a request, if its method carries one.
///
/// Reads `params.name` for `tools/call` and `prompts/get` and `params.uri` for
/// `resources/read`, matching the spec's source-field table exactly. Every other
/// method contributes no `Mcp-Name` header.
#[must_use]
pub fn name_header_value(method: &str, params: Option<&Value>) -> Option<String> {
    let field = match method {
        "tools/call" | "prompts/get" => "name",
        "resources/read" => "uri",
        _ => return None,
    };
    params?
        .get(field)
        .and_then(Value::as_str)
        .map(encode_header_value)
}

/// A tool parameter the server asked to have mirrored into a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamHeader {
    /// The `{Name}` portion of `Mcp-Param-{Name}`.
    pub suffix: String,
    /// The chain of `properties` keys locating the value in the call arguments.
    pub path: Vec<String>,
}

impl ParamHeader {
    /// The full lowercase header name.
    #[must_use]
    pub fn header_name(&self) -> String {
        format!("{PARAM_HEADER_PREFIX}{}", self.suffix.to_ascii_lowercase())
    }
}

/// Why a tool definition's `x-mcp-header` annotations are unusable.
///
/// The spec requires the client to exclude such a tool from `tools/list`
/// entirely: a malformed annotation means the client cannot construct headers
/// the server will accept, so every call to that tool would fail with a header
/// mismatch. Rejecting one tool must not take the server's other tools down
/// with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamHeaderError {
    /// The annotation value was not a non-empty HTTP token.
    InvalidName(String),
    /// Two annotations differ only by case.
    DuplicateName(String),
    /// The annotated property is not an integer, string, or boolean.
    /// `number` is explicitly excluded by the spec.
    UnsupportedType {
        /// Header suffix carried by the offending annotation.
        suffix: String,
        /// The declared type, or `"<unspecified>"`.
        declared: String,
    },
    /// The annotation sits somewhere a client cannot statically reach — inside
    /// an array, a composition or conditional keyword, or a `$ref`.
    Unreachable(String),
}

impl std::fmt::Display for ParamHeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(name) => {
                write!(f, "'{name}' is not a valid HTTP header token")
            }
            Self::DuplicateName(name) => write!(
                f,
                "'{name}' is used by more than one parameter (names are case-insensitive)"
            ),
            Self::UnsupportedType { suffix, declared } => write!(
                f,
                "'{suffix}' annotates a parameter of type {declared}; \
                 only integer, string, and boolean may be mirrored"
            ),
            Self::Unreachable(name) => write!(
                f,
                "'{name}' is not statically reachable through `properties` alone"
            ),
        }
    }
}

/// Whether every character is an RFC 9110 `tchar`.
fn is_http_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

/// Collect the parameter-mirroring annotations from a tool's `inputSchema`.
///
/// Returns `Err` when the tool definition must be rejected outright. Schemas
/// with no annotations return an empty list — by far the common case, and it
/// costs one walk of a small object.
pub fn collect_param_headers(input_schema: &Value) -> Result<Vec<ParamHeader>, ParamHeaderError> {
    let mut found = Vec::new();
    walk_schema(input_schema, &mut Vec::new(), true, &mut found)?;

    let mut seen: HashSet<String> = HashSet::new();
    for header in &found {
        if !seen.insert(header.suffix.to_ascii_lowercase()) {
            return Err(ParamHeaderError::DuplicateName(header.suffix.clone()));
        }
    }
    Ok(found)
}

/// Schema members whose subschemas are *not* statically reachable: an
/// annotation below any of these cannot be located from the call arguments
/// without evaluating the schema, so the spec forbids it there.
const UNREACHABLE_KEYS: &[&str] = &[
    "items",
    "prefixItems",
    "additionalProperties",
    "additionalItems",
    "contains",
    "propertyNames",
    "patternProperties",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "$defs",
    "definitions",
];

/// Walk a JSON Schema, collecting annotations and rejecting misplaced ones.
///
/// `reachable` tracks whether the current node sits on a chain of `properties`
/// keys from the root. Descending through anything in [`UNREACHABLE_KEYS`]
/// clears it permanently for that subtree.
fn walk_schema(
    node: &Value,
    path: &mut Vec<String>,
    reachable: bool,
    found: &mut Vec<ParamHeader>,
) -> Result<(), ParamHeaderError> {
    let Some(object) = node.as_object() else {
        return Ok(());
    };

    // An annotation is a *string* under the `x-mcp-header` key. A property
    // legitimately named "x-mcp-header" holds a subschema (an object), so the
    // value's type disambiguates the two.
    if let Some(suffix) = object.get(X_MCP_HEADER).and_then(Value::as_str) {
        if !reachable || path.is_empty() {
            return Err(ParamHeaderError::Unreachable(suffix.to_string()));
        }
        if !is_http_token(suffix) {
            return Err(ParamHeaderError::InvalidName(suffix.to_string()));
        }
        match object.get("type").and_then(Value::as_str) {
            Some("integer" | "string" | "boolean") => {}
            other => {
                return Err(ParamHeaderError::UnsupportedType {
                    suffix: suffix.to_string(),
                    declared: other.unwrap_or("<unspecified>").to_string(),
                })
            }
        }
        found.push(ParamHeader {
            suffix: suffix.to_string(),
            path: path.clone(),
        });
    }

    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for (key, child) in properties {
            path.push(key.clone());
            let result = walk_schema(child, path, reachable, found);
            path.pop();
            result?;
        }
    }

    for key in UNREACHABLE_KEYS {
        let Some(child) = object.get(*key) else {
            continue;
        };
        match child {
            Value::Array(items) => {
                for item in items {
                    walk_schema(item, path, false, found)?;
                }
            }
            Value::Object(map) if matches!(*key, "$defs" | "definitions" | "patternProperties") => {
                for sub in map.values() {
                    walk_schema(sub, path, false, found)?;
                }
            }
            other => walk_schema(other, path, false, found)?,
        }
    }

    Ok(())
}

/// Build the `Mcp-Param-*` headers for one tool call.
///
/// A parameter that is absent or `null` in `arguments` contributes no header —
/// the server is required not to expect one. A value whose runtime type does
/// not match its declared primitive type is skipped with a warning rather than
/// serialized guessily; sending a header the server cannot match against the
/// body would fail the whole call.
#[must_use]
pub fn extract_param_headers(
    annotations: &[ParamHeader],
    arguments: &Value,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();

    for annotation in annotations {
        let mut cursor = arguments;
        for key in &annotation.path {
            let Some(next) = cursor.get(key) else {
                cursor = &Value::Null;
                break;
            };
            cursor = next;
        }

        let rendered = match cursor {
            Value::Null => continue,
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => match n.as_i64() {
                Some(i) if i.abs() <= JS_SAFE_INTEGER => i.to_string(),
                _ => {
                    tracing::warn!(
                        header = %annotation.suffix,
                        "MCP tool argument annotated with x-mcp-header is not a safe integer; \
                         header omitted"
                    );
                    continue;
                }
            },
            _ => {
                tracing::warn!(
                    header = %annotation.suffix,
                    "MCP tool argument annotated with x-mcp-header is not a primitive; \
                     header omitted"
                );
                continue;
            }
        };

        headers.push((annotation.header_name(), encode_header_value(&rendered)));
    }

    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_ascii_values_are_sent_literally() {
        assert_eq!(encode_header_value("us-west1"), "us-west1");
        assert_eq!(encode_header_value("get_weather"), "get_weather");
        assert_eq!(
            encode_header_value("file:///projects/myapp/config.json"),
            "file:///projects/myapp/config.json"
        );
    }

    #[test]
    fn encodes_the_spec_examples() {
        assert_eq!(
            encode_header_value("Hello, 世界"),
            "=?base64?SGVsbG8sIOS4lueVjA==?="
        );
        assert_eq!(encode_header_value(" padded "), "=?base64?IHBhZGRlZCA=?=");
        assert_eq!(
            encode_header_value("line1\nline2"),
            "=?base64?bGluZTEKbGluZTI=?="
        );
        assert_eq!(
            encode_header_value("=?base64?literal?="),
            "=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?="
        );
    }

    #[test]
    fn control_characters_never_reach_the_wire() {
        // A raw CR/LF in a header value is response splitting; the sentinel is
        // what keeps a hostile tool name from injecting one.
        for raw in ["a\rb", "a\nb", "a\0b", "tab\u{7f}del"] {
            let encoded = encode_header_value(raw);
            assert!(
                encoded.starts_with(SENTINEL_PREFIX),
                "{raw:?} was not encoded"
            );
            assert!(!encoded.contains('\r'));
            assert!(!encoded.contains('\n'));
        }
    }

    #[test]
    fn name_header_follows_the_source_field_table() {
        assert_eq!(
            name_header_value("tools/call", Some(&json!({"name": "get_weather"}))).as_deref(),
            Some("get_weather")
        );
        assert_eq!(
            name_header_value("prompts/get", Some(&json!({"name": "summarize"}))).as_deref(),
            Some("summarize")
        );
        assert_eq!(
            name_header_value("resources/read", Some(&json!({"uri": "file:///a.txt"}))).as_deref(),
            Some("file:///a.txt")
        );
    }

    #[test]
    fn methods_without_a_name_source_contribute_no_header() {
        assert!(name_header_value("tools/list", None).is_none());
        assert!(name_header_value("server/discover", Some(&json!({"name": "x"}))).is_none());
        // `tools/call` with no name in the body: nothing to mirror, and the
        // server will reject the malformed body on its own terms.
        assert!(name_header_value("tools/call", Some(&json!({}))).is_none());
    }

    #[test]
    fn non_ascii_tool_names_are_encoded_in_the_name_header() {
        let value = name_header_value("tools/call", Some(&json!({"name": "天气"}))).unwrap();
        assert!(value.starts_with(SENTINEL_PREFIX));
    }

    #[test]
    fn collects_the_spec_example_annotation() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": {
                    "type": "string",
                    "description": "The region to execute the query in",
                    "x-mcp-header": "Region"
                },
                "query": {"type": "string"}
            },
            "required": ["region", "query"]
        });

        let headers = collect_param_headers(&schema).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].suffix, "Region");
        assert_eq!(headers[0].path, vec!["region"]);
        assert_eq!(headers[0].header_name(), "mcp-param-region");
    }

    #[test]
    fn schemas_without_annotations_collect_nothing() {
        let schema = json!({
            "type": "object",
            "properties": {"query": {"type": "string"}}
        });

        assert_eq!(collect_param_headers(&schema).unwrap(), Vec::new());
    }

    #[test]
    fn nested_object_properties_are_reachable() {
        let schema = json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "object",
                    "properties": {
                        "region": {"type": "string", "x-mcp-header": "Region"}
                    }
                }
            }
        });

        let headers = collect_param_headers(&schema).unwrap();
        assert_eq!(headers[0].path, vec!["target", "region"]);
    }

    #[test]
    fn annotations_inside_arrays_reject_the_tool() {
        let schema = json!({
            "type": "object",
            "properties": {
                "regions": {
                    "type": "array",
                    "items": {"type": "string", "x-mcp-header": "Region"}
                }
            }
        });

        assert_eq!(
            collect_param_headers(&schema),
            Err(ParamHeaderError::Unreachable("Region".to_string()))
        );
    }

    #[test]
    fn annotations_inside_composition_keywords_reject_the_tool() {
        for keyword in ["oneOf", "anyOf", "allOf"] {
            let schema = json!({
                "type": "object",
                "properties": {
                    "region": {
                        keyword: [{"type": "string", "x-mcp-header": "Region"}]
                    }
                }
            });
            assert_eq!(
                collect_param_headers(&schema),
                Err(ParamHeaderError::Unreachable("Region".to_string())),
                "{keyword} should be unreachable"
            );
        }
    }

    #[test]
    fn annotations_inside_defs_reject_the_tool() {
        let schema = json!({
            "type": "object",
            "properties": {"region": {"$ref": "#/$defs/region"}},
            "$defs": {
                "region": {"type": "string", "x-mcp-header": "Region"}
            }
        });

        assert_eq!(
            collect_param_headers(&schema),
            Err(ParamHeaderError::Unreachable("Region".to_string()))
        );
    }

    #[test]
    fn number_typed_parameters_are_rejected() {
        // The spec excludes `number` explicitly: a float has no single
        // canonical string form, so header and body could not be compared.
        let schema = json!({
            "type": "object",
            "properties": {
                "ratio": {"type": "number", "x-mcp-header": "Ratio"}
            }
        });

        assert_eq!(
            collect_param_headers(&schema),
            Err(ParamHeaderError::UnsupportedType {
                suffix: "Ratio".to_string(),
                declared: "number".to_string(),
            })
        );
    }

    #[test]
    fn non_primitive_and_untyped_parameters_are_rejected() {
        for declared in [json!("object"), json!("array")] {
            let schema = json!({
                "type": "object",
                "properties": {"p": {"type": declared, "x-mcp-header": "P"}}
            });
            assert!(collect_param_headers(&schema).is_err());
        }

        let untyped = json!({
            "type": "object",
            "properties": {"p": {"x-mcp-header": "P"}}
        });
        assert_eq!(
            collect_param_headers(&untyped),
            Err(ParamHeaderError::UnsupportedType {
                suffix: "P".to_string(),
                declared: "<unspecified>".to_string(),
            })
        );
    }

    #[test]
    fn invalid_header_tokens_are_rejected() {
        for bad in ["", "has space", "colon:name", "new\nline", "quote\"d"] {
            let schema = json!({
                "type": "object",
                "properties": {"p": {"type": "string", "x-mcp-header": bad}}
            });
            assert_eq!(
                collect_param_headers(&schema),
                Err(ParamHeaderError::InvalidName(bad.to_string())),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn case_insensitive_duplicate_names_are_rejected() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "x-mcp-header": "Region"},
                "b": {"type": "string", "x-mcp-header": "region"}
            }
        });

        assert!(matches!(
            collect_param_headers(&schema),
            Err(ParamHeaderError::DuplicateName(_))
        ));
    }

    #[test]
    fn a_property_named_x_mcp_header_is_not_an_annotation() {
        // The key is a property name here, and its value is a subschema rather
        // than a string.
        let schema = json!({
            "type": "object",
            "properties": {
                "x-mcp-header": {"type": "string"}
            }
        });

        assert_eq!(collect_param_headers(&schema).unwrap(), Vec::new());
    }

    #[test]
    fn root_level_annotations_are_rejected() {
        let schema = json!({"type": "string", "x-mcp-header": "Root"});

        assert_eq!(
            collect_param_headers(&schema),
            Err(ParamHeaderError::Unreachable("Root".to_string()))
        );
    }

    #[test]
    fn extracts_the_spec_example_header() {
        let annotations = vec![ParamHeader {
            suffix: "Region".to_string(),
            path: vec!["region".to_string()],
        }];
        let arguments = json!({"region": "us-west1", "query": "SELECT 1"});

        assert_eq!(
            extract_param_headers(&annotations, &arguments),
            vec![("mcp-param-region".to_string(), "us-west1".to_string())]
        );
    }

    #[test]
    fn absent_and_null_arguments_contribute_no_header() {
        let annotations = vec![ParamHeader {
            suffix: "Region".to_string(),
            path: vec!["region".to_string()],
        }];

        assert!(extract_param_headers(&annotations, &json!({})).is_empty());
        assert!(extract_param_headers(&annotations, &json!({"region": null})).is_empty());
    }

    #[test]
    fn renders_primitives_in_their_canonical_form() {
        let annotations = vec![
            ParamHeader {
                suffix: "Count".to_string(),
                path: vec!["count".to_string()],
            },
            ParamHeader {
                suffix: "Flag".to_string(),
                path: vec!["flag".to_string()],
            },
        ];
        let arguments = json!({"count": -7, "flag": true});

        assert_eq!(
            extract_param_headers(&annotations, &arguments),
            vec![
                ("mcp-param-count".to_string(), "-7".to_string()),
                ("mcp-param-flag".to_string(), "true".to_string()),
            ]
        );
    }

    #[test]
    fn nested_paths_are_read_at_their_exact_location() {
        let annotations = vec![ParamHeader {
            suffix: "Region".to_string(),
            path: vec!["target".to_string(), "region".to_string()],
        }];

        assert_eq!(
            extract_param_headers(&annotations, &json!({"target": {"region": "eu-west1"}})),
            vec![("mcp-param-region".to_string(), "eu-west1".to_string())]
        );
        // Same key at the wrong depth must not be picked up.
        assert!(extract_param_headers(&annotations, &json!({"region": "eu-west1"})).is_empty());
    }

    #[test]
    fn non_ascii_argument_values_are_encoded() {
        let annotations = vec![ParamHeader {
            suffix: "City".to_string(),
            path: vec!["city".to_string()],
        }];

        let headers = extract_param_headers(&annotations, &json!({"city": "北京"}));
        assert_eq!(headers[0].1, "=?base64?5YyX5Lqs?=");
    }

    #[test]
    fn unsafe_integers_are_omitted_rather_than_truncated() {
        let annotations = vec![ParamHeader {
            suffix: "Big".to_string(),
            path: vec!["big".to_string()],
        }];

        let arguments = json!({"big": 9_007_199_254_740_993i64});
        assert!(extract_param_headers(&annotations, &arguments).is_empty());
    }
}
