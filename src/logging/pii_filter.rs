/// PII scrubbing layer for tracing
///
/// This layer intercepts tracing events and scrubs PII before they are
/// written to log files or console output.
use std::sync::atomic::{AtomicBool, Ordering};

use aleph_logging::scrub_pii;
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

static PASSTHROUGH_WARNED: AtomicBool = AtomicBool::new(false);

/// Tracing layer that scrubs PII from all log messages — **PASSTHROUGH
/// ONLY.** The `Layer` trait cannot rewrite event fields, so this struct
/// cannot do real scrubbing; the actual implementation lives in the
/// `FormatEvent` returned by [`create_pii_scrubbing_layer`]. New code MUST
/// use [`create_pii_scrubbing_layer`]; this struct is kept in the public
/// API only for backward compatibility and the first time its `on_event`
/// fires it emits a `warn!` so misconfiguration shows up in operator logs.
pub struct PiiScrubbingLayer;

impl<S: Subscriber> Layer<S> for PiiScrubbingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let _ = event;
        if !PASSTHROUGH_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "PiiScrubbingLayer is a passthrough — install PII scrubbing via \
                 create_pii_scrubbing_layer() (a FormatEvent) instead. See the \
                 note on PiiScrubbingLayer in src/logging/pii_filter."
            );
        }
    }
}

/// Format layer with PII scrubbing
///
/// This is the actual implementation that scrubs PII during event formatting.
/// It wraps the standard tracing-subscriber format layer and scrubs all
/// messages before they are written to the output.
///
/// # Example
///
/// ```rust,ignore
/// use aleph::logging::create_pii_scrubbing_layer;
/// use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
///
/// tracing_subscriber::registry()
///     .with(create_pii_scrubbing_layer())
///     .init();
/// ```
#[must_use]
pub fn create_pii_scrubbing_layer<S>() -> Box<dyn Layer<S> + Send + Sync + 'static>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    // Create a custom formatting layer that scrubs PII
    let format_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .event_format(PiiScrubbingFormat);

    Box::new(format_layer)
}

/// Custom event formatter that scrubs PII
pub struct PiiScrubbingFormat;

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for PiiScrubbingFormat
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        // Get timestamp
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");

        // Get level
        let metadata = event.metadata();
        let level = metadata.level();

        // Get target (module path)
        let target = metadata.target();

        // Write prefix: timestamp level target
        write!(writer, "{timestamp} {level:5} {target}: ")?;

        // Collect all fields into a string
        let mut visitor = StringVisitor::default();
        event.record(&mut visitor);

        // Scrub PII from the collected message
        let scrubbed_message = scrub_pii(&visitor.message);

        // Write scrubbed message
        write!(writer, "{scrubbed_message}")?;

        // Add span context if available
        if let Some(scope) = ctx.event_scope() {
            let mut seen = false;
            for span in scope.from_root() {
                write!(writer, "{}", if seen { ":" } else { " in " })?;
                seen = true;

                let ext = span.extensions();
                let fields = ext
                    .get::<tracing_subscriber::fmt::FormattedFields<N>>()
                    .map(|fields| scrub_pii(fields.as_str()))
                    .unwrap_or_default();

                write!(writer, "{}{{", span.name())?;
                if !fields.is_empty() {
                    write!(writer, "{fields}")?;
                }
                write!(writer, "}}")?;
            }
        }

        writeln!(writer)
    }
}

/// Visitor that collects all event fields into a single string
#[derive(Default)]
struct StringVisitor {
    message: String,
}

impl Visit for StringVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if !self.message.is_empty() {
            self.message.push_str(", ");
        }

        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.message.push_str(field.name());
            self.message.push('=');
            self.message.push_str(value);
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if !self.message.is_empty() {
            self.message.push_str(", ");
        }

        if field.name() == "message" {
            // Strip surrounding quotes from debug-formatted strings to ensure
            // PII scrubbing works correctly (scrub_pii regex relies on word boundaries)
            let formatted = format!("{value:?}");
            let trimmed = formatted
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(&formatted);
            self.message.push_str(trimmed);
        } else {
            use std::fmt::Write;
            let _ = write!(&mut self.message, "{}={:?}", field.name(), value);
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.record_debug(field, &value);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.record_debug(field, &value);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.record_debug(field, &value);
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.record_debug(field, &value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::info;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn test_pii_scrubbing_layer_basic() {
        // Note: This test is primarily for compilation
        // Actual scrubbing behavior is tested in utils::pii tests
        let _layer = PiiScrubbingLayer;
    }

    #[test]
    fn test_create_pii_scrubbing_layer() {
        // Test that we can create the layer
        let _layer = create_pii_scrubbing_layer::<tracing_subscriber::Registry>();
    }

    #[test]
    fn test_string_visitor() {
        let mut visitor = StringVisitor::default();

        // Create a mock field
        let metadata = tracing::Metadata::new(
            "test",
            "test_target",
            tracing::Level::INFO,
            None,
            None,
            None,
            tracing::field::FieldSet::new(
                &["message"],
                tracing::callsite::Identifier(&TEST_CALLSITE),
            ),
            tracing::metadata::Kind::EVENT,
        );

        static TEST_CALLSITE: TestCallsite = TestCallsite;
        static TEST_METADATA: std::sync::OnceLock<tracing::Metadata<'static>> =
            std::sync::OnceLock::new();

        struct TestCallsite;
        impl tracing::callsite::Callsite for TestCallsite {
            fn set_interest(&self, _: tracing::subscriber::Interest) {}
            fn metadata(&self) -> &tracing::Metadata<'_> {
                TEST_METADATA.get_or_init(|| {
                    tracing::Metadata::new(
                        "test",
                        "test_target",
                        tracing::Level::INFO,
                        None,
                        None,
                        None,
                        tracing::field::FieldSet::new(
                            &["message"],
                            tracing::callsite::Identifier(&TEST_CALLSITE),
                        ),
                        tracing::metadata::Kind::EVENT,
                    )
                })
            }
        }

        // Simulate visiting a field
        let field = metadata.fields().field("message").unwrap();
        visitor.record_debug(&field, &"test message");

        assert_eq!(visitor.message, "test message");
    }

    /// Integration test: verify PII is actually scrubbed in logged output
    #[test]
    fn test_pii_scrubbing_integration() {
        // This test captures log output and verifies PII scrubbing works end-to-end
        use crate::sync_primitives::{Arc, Mutex};

        // Create a custom writer to capture output
        #[derive(Clone)]
        struct CaptureWriter {
            captured: Arc<Mutex<String>>,
        }

        impl std::io::Write for CaptureWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let s = String::from_utf8_lossy(buf);
                self.captured
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_str(&s);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let captured = Arc::new(Mutex::new(String::new()));
        let writer = CaptureWriter {
            captured: captured.clone(),
        };

        // Set up subscriber with PII scrubbing
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(move || writer.clone())
                .event_format(PiiScrubbingFormat),
        );

        // Use this subscriber for the test
        tracing::subscriber::with_default(subscriber, || {
            info!("User email: john@example.com");
        });

        // Verify output has PII scrubbed
        let output = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert!(output.contains("[EMAIL]"), "Output: {}", &*output);
        assert!(!output.contains("john@example.com"), "Output: {}", &*output);
    }
}
