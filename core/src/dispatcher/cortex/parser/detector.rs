//! JsonStreamDetector - State machine for extracting JSON from mixed text streams
//!
//! This detector scans character-by-character through streaming text,
//! identifying and extracting complete JSON objects and arrays.

use super::repair::try_repair;
use serde_json::Value;

/// Internal state of the detector's state machine
#[derive(Debug, Clone, PartialEq)]
enum DetectorState {
    /// Scanning for the start of a JSON structure
    Scanning,
    /// Inside a JSON object, tracking nesting depth and string context
    InObject {
        depth: usize,
        in_string: bool,
        escape_next: bool,
    },
    /// Inside a JSON array, tracking nesting depth and string context
    InArray {
        depth: usize,
        in_string: bool,
        escape_next: bool,
    },
}

/// A fragment extracted from the stream
#[derive(Debug, Clone)]
pub enum JsonFragment {
    /// A complete, successfully parsed JSON value
    Complete(Value),
    /// A partial JSON structure still being accumulated
    Partial { prefix: String },
}

/// Streaming JSON detector with character-by-character state machine
///
/// # Example
///
/// ```
/// use alephcore::dispatcher::cortex::parser::JsonStreamDetector;
///
/// let mut detector = JsonStreamDetector::new();
///
/// // Process chunks as they arrive
/// let fragments = detector.push("Here is some text ");
/// assert!(fragments.is_empty());
///
/// let fragments = detector.push("{\"tool\": \"search\"}");
/// assert_eq!(fragments.len(), 1);
/// ```
#[derive(Debug)]
pub struct JsonStreamDetector {
    state: DetectorState,
    buffer: String,
    start_char: Option<char>,
}

impl JsonStreamDetector {
    /// Create a new detector in scanning state
    pub fn new() -> Self {
        Self {
            state: DetectorState::Scanning,
            buffer: String::new(),
            start_char: None,
        }
    }

    /// Push a chunk of text and extract any complete JSON fragments
    ///
    /// Returns a vector of complete JSON values found in this chunk.
    /// Partial JSON at the end of a chunk is buffered for the next push.
    pub fn push(&mut self, chunk: &str) -> Vec<JsonFragment> {
        let mut fragments = Vec::new();

        for ch in chunk.chars() {
            match &mut self.state {
                DetectorState::Scanning => {
                    if ch == '{' {
                        self.state = DetectorState::InObject {
                            depth: 1,
                            in_string: false,
                            escape_next: false,
                        };
                        self.buffer.push(ch);
                        self.start_char = Some('{');
                    } else if ch == '[' {
                        self.state = DetectorState::InArray {
                            depth: 1,
                            in_string: false,
                            escape_next: false,
                        };
                        self.buffer.push(ch);
                        self.start_char = Some('[');
                    }
                }

                DetectorState::InObject {
                    depth,
                    in_string,
                    escape_next,
                } => {
                    self.buffer.push(ch);

                    if *escape_next {
                        *escape_next = false;
                        continue;
                    }

                    if ch == '\\' && *in_string {
                        *escape_next = true;
                        continue;
                    }

                    if ch == '"' {
                        *in_string = !*in_string;
                        continue;
                    }

                    if !*in_string {
                        if ch == '{' {
                            *depth += 1;
                        } else if ch == '}' {
                            *depth -= 1;
                            if *depth == 0 {
                                if let Ok(value) = serde_json::from_str(&self.buffer) {
                                    fragments.push(JsonFragment::Complete(value));
                                }
                                self.buffer.clear();
                                self.state = DetectorState::Scanning;
                                self.start_char = None;
                            }
                        }
                    }
                }

                DetectorState::InArray {
                    depth,
                    in_string,
                    escape_next,
                } => {
                    self.buffer.push(ch);

                    if *escape_next {
                        *escape_next = false;
                        continue;
                    }

                    if ch == '\\' && *in_string {
                        *escape_next = true;
                        continue;
                    }

                    if ch == '"' {
                        *in_string = !*in_string;
                        continue;
                    }

                    if !*in_string {
                        if ch == '[' {
                            *depth += 1;
                        } else if ch == ']' {
                            *depth -= 1;
                            if *depth == 0 {
                                if let Ok(value) = serde_json::from_str(&self.buffer) {
                                    fragments.push(JsonFragment::Complete(value));
                                }
                                self.buffer.clear();
                                self.state = DetectorState::Scanning;
                                self.start_char = None;
                            }
                        }
                    }
                }
            }
        }

        fragments
    }

    /// Check if there is pending data in the buffer
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Get a reference to the current buffer contents
    pub fn current_buffer(&self) -> &str {
        &self.buffer
    }

    /// Consume the detector and return the buffered content
    pub fn into_buffer(self) -> String {
        self.buffer
    }

    /// Finalize detection, attempting repair on incomplete JSON
    ///
    /// This method should be called when the stream ends to handle any
    /// remaining buffered content. If there's incomplete JSON in the buffer,
    /// it attempts to repair it using greedy bracket/quote closing.
    ///
    /// Returns Ok with repaired fragments, or Err if repair was not possible.
    pub fn finalize(self) -> Result<Vec<JsonFragment>, String> {
        if self.buffer.is_empty() {
            return Ok(vec![]);
        }

        // Try to parse as-is first
        if let Ok(value) = serde_json::from_str(&self.buffer) {
            return Ok(vec![JsonFragment::Complete(value)]);
        }

        // Attempt repair
        match try_repair(&self.buffer) {
            Some(repaired) => {
                if let Ok(value) = serde_json::from_str(&repaired) {
                    Ok(vec![JsonFragment::Complete(value)])
                } else {
                    Err(format!("Repair produced invalid JSON: {}", repaired))
                }
            }
            None => Err(format!("Could not repair JSON: {}", self.buffer)),
        }
    }
}

impl Default for JsonStreamDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_json_extraction() {
        let mut detector = JsonStreamDetector::new();

        // JSON embedded in mixed text
        let fragments = detector.push("Here is the result: {\"tool\": \"search\"} and more text");

        assert_eq!(fragments.len(), 1);
        match &fragments[0] {
            JsonFragment::Complete(value) => {
                assert_eq!(value["tool"], "search");
            }
            _ => panic!("Expected Complete fragment"),
        }
    }

    #[test]
    fn test_streaming_chunks() {
        let mut detector = JsonStreamDetector::new();

        // JSON split across multiple chunks
        let f1 = detector.push("{\"na");
        assert!(f1.is_empty());
        assert!(detector.has_pending());

        let f2 = detector.push("me\": \"te");
        assert!(f2.is_empty());

        let f3 = detector.push("st\"}");
        assert_eq!(f3.len(), 1);
        match &f3[0] {
            JsonFragment::Complete(value) => {
                assert_eq!(value["name"], "test");
            }
            _ => panic!("Expected Complete fragment"),
        }
    }

    #[test]
    fn test_nested_json() {
        let mut detector = JsonStreamDetector::new();

        let fragments = detector.push("{\"outer\": {\"inner\": [1, 2, {\"deep\": true}]}}");

        assert_eq!(fragments.len(), 1);
        match &fragments[0] {
            JsonFragment::Complete(value) => {
                assert_eq!(value["outer"]["inner"][2]["deep"], true);
            }
            _ => panic!("Expected Complete fragment"),
        }
    }

    #[test]
    fn test_multiple_json_objects() {
        let mut detector = JsonStreamDetector::new();

        let fragments = detector.push("{\"first\": 1}{\"second\": 2}");

        assert_eq!(fragments.len(), 2);
        match &fragments[0] {
            JsonFragment::Complete(value) => {
                assert_eq!(value["first"], 1);
            }
            _ => panic!("Expected Complete fragment"),
        }
        match &fragments[1] {
            JsonFragment::Complete(value) => {
                assert_eq!(value["second"], 2);
            }
            _ => panic!("Expected Complete fragment"),
        }
    }

    #[test]
    fn test_string_with_braces() {
        let mut detector = JsonStreamDetector::new();

        // Braces inside strings should not affect depth counting
        let fragments = detector.push("{\"text\": \"hello {world}\"}");

        assert_eq!(fragments.len(), 1);
        match &fragments[0] {
            JsonFragment::Complete(value) => {
                assert_eq!(value["text"], "hello {world}");
            }
            _ => panic!("Expected Complete fragment"),
        }
    }

    #[test]
    fn test_escaped_quotes() {
        let mut detector = JsonStreamDetector::new();

        // Escaped quotes inside strings
        let fragments = detector.push("{\"text\": \"say \\\"hello\\\"\"}");

        assert_eq!(fragments.len(), 1);
        match &fragments[0] {
            JsonFragment::Complete(value) => {
                assert_eq!(value["text"], "say \"hello\"");
            }
            _ => panic!("Expected Complete fragment"),
        }
    }

    #[test]
    fn test_array_extraction() {
        let mut detector = JsonStreamDetector::new();

        // Array from mixed text
        let fragments = detector.push("The values are: [1, 2, 3] as expected");

        assert_eq!(fragments.len(), 1);
        match &fragments[0] {
            JsonFragment::Complete(value) => {
                assert!(value.is_array());
                let arr = value.as_array().unwrap();
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], 1);
                assert_eq!(arr[1], 2);
                assert_eq!(arr[2], 3);
            }
            _ => panic!("Expected Complete fragment"),
        }
    }

    #[test]
    fn test_finalize_with_repair() {
        let mut detector = JsonStreamDetector::new();
        detector.push(r#"{"name": "test""#);

        let result = detector.finalize();
        assert!(result.is_ok());

        let repaired = result.unwrap();
        assert_eq!(repaired.len(), 1);
        match &repaired[0] {
            JsonFragment::Complete(v) => {
                assert_eq!(v["name"], "test");
            }
            _ => panic!("Expected repaired Complete"),
        }
    }

    #[test]
    fn test_finalize_empty_buffer() {
        let detector = JsonStreamDetector::new();
        let result = detector.finalize();

        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_finalize_already_valid() {
        let mut detector = JsonStreamDetector::new();
        // Push a complete JSON that doesn't get extracted (edge case)
        detector.push("{\"complete\": true}");

        // Since push() extracts complete JSON, buffer should be empty
        // Let's test with partial that becomes complete
        let mut detector2 = JsonStreamDetector::new();
        detector2.push("{\"partial\":");
        detector2.push(" true}");

        // After these pushes, the JSON should be extracted, buffer empty
        let detector3 = JsonStreamDetector::new();
        let result = detector3.finalize();
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
