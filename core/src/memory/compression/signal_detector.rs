//! Signal Detector for Smart Compression Triggers
//!
//! This module provides keyword-based detection for compression signals in user messages.
//! It identifies three types of signals:
//!
//! - **Learning signals**: User preferences, rules, and habits worth remembering
//! - **Correction signals**: User corrections to AI's understanding (highest priority)
//! - **Milestone signals**: Task completion markers
//!
//! ## Priority Levels
//!
//! - `Immediate`: Corrections require immediate compression
//! - `Deferred`: Learning signals are compressed soon
//! - `Batch`: Milestones and default - batched with regular compression

/// Compression signal types detected from user messages
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CompressionSignal {
    /// User is teaching a preference, rule, or habit
    Learning {
        trigger_phrase: String,
        confidence: f32,
    },
    /// User is correcting the AI's understanding
    Correction {
        original_understanding: String,
        corrected_to: String,
        confidence: f32,
    },
    /// User indicates task completion
    Milestone {
        task_description: String,
        completion_indicator: String,
    },
    /// User is switching context/topic
    ContextSwitch { from_topic: String, to_topic: String },
}

/// Priority level for compression
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum CompressionPriority {
    /// Compress immediately (corrections)
    Immediate,
    /// Compress soon (learning signals)
    Deferred,
    /// Batch with regular compression cycle
    #[default]
    Batch,
}

/// Result of signal detection
#[derive(Debug, Clone)]
pub struct DetectionResult {
    /// Signals detected in the message
    pub signals: Vec<CompressionSignal>,
    /// Whether compression should be triggered
    pub should_compress: bool,
    /// Priority level for compression
    pub priority: CompressionPriority,
}

impl Default for DetectionResult {
    fn default() -> Self {
        Self {
            signals: Vec::new(),
            should_compress: false,
            priority: CompressionPriority::Batch,
        }
    }
}

/// Keywords for signal detection
#[derive(Debug, Clone)]
pub struct SignalKeywords {
    /// Keywords indicating learning/preference signals
    pub learning: Vec<String>,
    /// Keywords indicating correction signals
    pub correction: Vec<String>,
    /// Keywords indicating milestone/completion signals
    pub milestone: Vec<String>,
}

impl Default for SignalKeywords {
    fn default() -> Self {
        Self {
            learning: vec![
                // Chinese
                "记住".to_string(),
                "以后".to_string(),
                "偏好".to_string(),
                "喜欢用".to_string(),
                "习惯".to_string(),
                "总是".to_string(),
                "一直".to_string(),
                "我喜欢".to_string(),
                "我讨厌".to_string(),
                "我倾向".to_string(),
                "默认用".to_string(),
                "优先用".to_string(),
                // English
                "remember".to_string(),
                "always".to_string(),
                "prefer".to_string(),
                "i like".to_string(),
                "i hate".to_string(),
                "from now on".to_string(),
                "by default".to_string(),
                "going forward".to_string(),
            ],
            correction: vec![
                // Chinese
                "不对".to_string(),
                "搞错".to_string(),
                "错了".to_string(),
                "我说的是".to_string(),
                "不是这个意思".to_string(),
                "你理解错了".to_string(),
                "应该是".to_string(),
                "纠正一下".to_string(),
                // English
                "wrong".to_string(),
                "incorrect".to_string(),
                "no,".to_string(),
                "not what i meant".to_string(),
                "i meant".to_string(),
                "actually".to_string(),
                "let me clarify".to_string(),
            ],
            milestone: vec![
                // Chinese
                "完成".to_string(),
                "搞定".to_string(),
                "结束".to_string(),
                "做完了".to_string(),
                "好了".to_string(),
                "成功".to_string(),
                "告一段落".to_string(),
                "收工".to_string(),
                // English
                "done".to_string(),
                "finished".to_string(),
                "completed".to_string(),
                "that's it".to_string(),
                "wrap up".to_string(),
                "all set".to_string(),
            ],
        }
    }
}

/// Signal detector for identifying compression triggers in messages
#[derive(Debug, Clone)]
pub struct SignalDetector {
    keywords: SignalKeywords,
}

impl SignalDetector {
    /// Create a new signal detector with default keywords
    pub fn new() -> Self {
        Self {
            keywords: SignalKeywords::default(),
        }
    }

    /// Create a signal detector with custom keywords
    pub fn with_keywords(keywords: SignalKeywords) -> Self {
        Self { keywords }
    }

    /// Detect signals in a message
    ///
    /// Detection priority:
    /// 1. Correction keywords (set priority to Immediate)
    /// 2. Learning keywords (set priority to Deferred if not already Immediate)
    /// 3. Milestone keywords (keep existing priority)
    pub fn detect(&self, message: &str) -> DetectionResult {
        let mut result = DetectionResult::default();
        let message_lower = message.to_lowercase();

        // Check correction keywords first (highest priority)
        for keyword in &self.keywords.correction {
            if message_lower.contains(&keyword.to_lowercase()) {
                // Note: original_understanding is initially empty, to be filled by LLM
                // during fact extraction based on conversation context
                result.signals.push(CompressionSignal::Correction {
                    original_understanding: String::new(),
                    corrected_to: message.to_string(),
                    confidence: 0.8,
                });
                result.should_compress = true;
                result.priority = CompressionPriority::Immediate;
                break; // Only add one correction signal per message
            }
        }

        // Check learning keywords
        for keyword in &self.keywords.learning {
            if message_lower.contains(&keyword.to_lowercase()) {
                result.signals.push(CompressionSignal::Learning {
                    trigger_phrase: keyword.clone(),
                    confidence: 0.7,
                });
                result.should_compress = true;
                // Only upgrade to Deferred if not already Immediate
                if result.priority == CompressionPriority::Batch {
                    result.priority = CompressionPriority::Deferred;
                }
                break; // Only add one learning signal per message
            }
        }

        // Check milestone keywords
        for keyword in &self.keywords.milestone {
            if message_lower.contains(&keyword.to_lowercase()) {
                result.signals.push(CompressionSignal::Milestone {
                    task_description: String::new(),
                    completion_indicator: keyword.clone(),
                });
                result.should_compress = true;
                // Keep existing priority (don't override Immediate or Deferred)
                break; // Only add one milestone signal per message
            }
        }

        result
    }

    /// Detect context switch based on embedding distance
    ///
    /// Returns Some(ContextSwitch) if the cosine distance exceeds the threshold.
    /// The threshold is the minimum distance to consider as a context switch
    /// (default 0.5 means topics should be at least 50% dissimilar).
    ///
    /// Note: `from_topic` and `to_topic` fields are initially empty and will be
    /// filled by LLM during fact extraction based on conversation context.
    pub fn detect_context_switch(
        &self,
        prev_embedding: &[f32],
        current_embedding: &[f32],
        threshold: f32,
    ) -> Option<CompressionSignal> {
        if prev_embedding.len() != current_embedding.len() || prev_embedding.is_empty() {
            return None;
        }

        let distance = Self::cosine_distance(prev_embedding, current_embedding);

        if distance > threshold {
            Some(CompressionSignal::ContextSwitch {
                from_topic: String::new(), // To be summarized by LLM
                to_topic: String::new(),
            })
        } else {
            None
        }
    }

    /// Calculate cosine distance between two vectors
    ///
    /// Returns 1.0 - cosine_similarity, so:
    /// - 0.0 = identical vectors
    /// - 1.0 = orthogonal vectors
    /// - 2.0 = opposite vectors
    fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 1.0; // Max distance for zero vectors
        }

        let similarity = dot / (norm_a * norm_b);
        1.0 - similarity // Convert similarity to distance
    }

    /// Combined detection with context switch
    ///
    /// Detects keyword-based signals AND context switch based on embedding distance.
    /// Use this when you have access to both the message text and embeddings.
    pub fn detect_with_context(
        &self,
        message: &str,
        prev_embedding: Option<&[f32]>,
        current_embedding: Option<&[f32]>,
        context_switch_threshold: f32,
    ) -> DetectionResult {
        let mut result = self.detect(message);

        // Check for context switch if embeddings provided
        if let (Some(prev), Some(curr)) = (prev_embedding, current_embedding) {
            if let Some(switch_signal) =
                self.detect_context_switch(prev, curr, context_switch_threshold)
            {
                result.signals.push(switch_signal);
                if !result.should_compress {
                    result.should_compress = true;
                    result.priority = CompressionPriority::Batch;
                }
            }
        }

        result
    }
}

impl Default for SignalDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_learning_signal_detection() {
        let detector = SignalDetector::new();
        let result = detector.detect("记住，我喜欢用 Rust 写代码");
        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Deferred));
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Learning { .. })));
    }

    #[test]
    fn test_correction_signal_detection() {
        let detector = SignalDetector::new();
        let result = detector.detect("不对，我说的是 Python 不是 JavaScript");
        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Immediate));
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Correction { .. })));
    }

    #[test]
    fn test_milestone_signal_detection() {
        let detector = SignalDetector::new();
        let result = detector.detect("好了，这个功能终于完成了");
        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Batch));
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Milestone { .. })));
    }

    #[test]
    fn test_no_signal_for_normal_conversation() {
        let detector = SignalDetector::new();
        let result = detector.detect("今天天气怎么样？");
        assert!(!result.should_compress);
        assert!(result.signals.is_empty());
    }

    #[test]
    fn test_english_learning_signal() {
        let detector = SignalDetector::new();
        let result = detector.detect("Remember, I always prefer using tabs over spaces");
        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Deferred));
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Learning { .. })));
    }

    #[test]
    fn test_english_correction_signal() {
        let detector = SignalDetector::new();
        let result = detector.detect("No, that's wrong. I meant the other file");
        assert!(result.should_compress);
        assert!(matches!(result.priority, CompressionPriority::Immediate));
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Correction { .. })));
    }

    #[test]
    fn test_english_milestone_signal() {
        let detector = SignalDetector::new();
        let result = detector.detect("Done! The feature is finished");
        assert!(result.should_compress);
        // Should have both signals but priority stays at higher level
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Milestone { .. })));
    }

    #[test]
    fn test_case_insensitive_detection() {
        let detector = SignalDetector::new();
        let result = detector.detect("REMEMBER this for later");
        assert!(result.should_compress);
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Learning { .. })));
    }

    #[test]
    fn test_correction_takes_priority_over_learning() {
        let detector = SignalDetector::new();
        // Message contains both correction ("不对") and learning ("记住") keywords
        let result = detector.detect("不对，记住我说的是 Rust");
        assert!(result.should_compress);
        // Priority should be Immediate due to correction
        assert!(matches!(result.priority, CompressionPriority::Immediate));
        // Should detect both signals
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Correction { .. })));
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Learning { .. })));
    }

    #[test]
    fn test_custom_keywords() {
        let custom_keywords = SignalKeywords {
            learning: vec!["custom_learn".to_string()],
            correction: vec!["custom_fix".to_string()],
            milestone: vec!["custom_done".to_string()],
        };
        let detector = SignalDetector::with_keywords(custom_keywords);

        let result = detector.detect("custom_learn this please");
        assert!(result.should_compress);
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Learning { .. })));
    }

    #[test]
    fn test_default_priority_is_batch() {
        let result = DetectionResult::default();
        assert!(matches!(result.priority, CompressionPriority::Batch));
        assert!(!result.should_compress);
        assert!(result.signals.is_empty());
    }

    #[test]
    fn test_signal_detector_default() {
        let detector = SignalDetector::default();
        // Should work the same as new()
        let result = detector.detect("记住这个");
        assert!(result.should_compress);
    }

    #[test]
    fn test_context_switch_detection() {
        let detector = SignalDetector::new();

        // Simulate previous embedding (about programming)
        let prev_embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];

        // Current message about cooking (very different direction - opposite)
        // Using opposite direction vectors to ensure high cosine distance
        let current_embedding = vec![-0.1, -0.2, -0.3, -0.4, -0.5];

        let result = detector.detect_context_switch(&prev_embedding, &current_embedding, 0.5);

        assert!(result.is_some());
        assert!(matches!(
            result.unwrap(),
            CompressionSignal::ContextSwitch { .. }
        ));
    }

    #[test]
    fn test_no_context_switch_for_similar_topics() {
        let detector = SignalDetector::new();

        // Similar embeddings (same topic)
        let prev_embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let current_embedding = vec![0.15, 0.22, 0.28, 0.42, 0.48];

        let result = detector.detect_context_switch(&prev_embedding, &current_embedding, 0.5);

        assert!(result.is_none());
    }

    #[test]
    fn test_detect_with_context_combines_signals() {
        let detector = SignalDetector::new();

        let prev_embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        // Using opposite direction vectors to ensure high cosine distance
        let current_embedding = vec![-0.1, -0.2, -0.3, -0.4, -0.5];

        // Message with learning signal AND context switch
        let result = detector.detect_with_context(
            "记住，我喜欢用 Python",
            Some(&prev_embedding),
            Some(&current_embedding),
            0.5,
        );

        assert!(result.should_compress);
        // Should have both learning signal and context switch
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::Learning { .. })));
        assert!(result
            .signals
            .iter()
            .any(|s| matches!(s, CompressionSignal::ContextSwitch { .. })));
    }
}
