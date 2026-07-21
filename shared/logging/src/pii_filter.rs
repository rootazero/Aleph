/// PII scrubbing format layer for tracing
///
/// Custom `FormatEvent` implementation that scrubs PII from all log messages
/// before writing to console or file output.
use std::sync::atomic::{AtomicBool, Ordering};

use crate::pii::scrub_pii;
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

static PASSTHROUGH_WARNED: AtomicBool = AtomicBool::new(false);

/// Tracing layer that scrubs PII from all log messages — **PASSTHROUGH
/// ONLY.** This type is registered by callers that expected "a layer that
/// scrubs", but tracing's `Layer` trait cannot rewrite event fields — PII
/// scrubbing for log lines must run in a `FormatEvent`, which is what
/// [`create_pii_scrubbing_layer`] installs. This struct is kept in the public
/// API for backward compatibility only; new code MUST use
/// [`create_pii_scrubbing_layer`] (or compose it via
/// `SubscriberExt::with(...)`). The first time this no-op observes an event
/// it emits a `warn!` so misconfiguration shows up in operator logs.
pub struct PiiScrubbingLayer;

impl<S: Subscriber> Layer<S> for PiiScrubbingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let _ = event;
        if !PASSTHROUGH_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "PiiScrubbingLayer is a passthrough — install PII scrubbing via \
                 create_pii_scrubbing_layer() (a FormatEvent) instead. See the \
                 note on PiiScrubbingLayer in aleph_logging::pii_filter."
            );
        }
    }
}

/// Create a PII scrubbing format layer
#[must_use]
pub fn create_pii_scrubbing_layer<S>() -> Box<dyn Layer<S> + Send + Sync + 'static>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
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
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let metadata = event.metadata();
        let level = metadata.level();
        let target = metadata.target();

        write!(writer, "{timestamp} {level:5} {target}: ")?;

        let mut visitor = StringVisitor::default();
        event.record(&mut visitor);

        let scrubbed_message = scrub_pii(&visitor.message);
        write!(writer, "{scrubbed_message}")?;

        if let Some(scope) = ctx.event_scope() {
            let mut seen = false;
            for span in scope.from_root() {
                write!(writer, "{}", if seen { ":" } else { " in " })?;
                seen = true;

                let fields = {
                    let ext = span.extensions();
                    ext.get::<tracing_subscriber::fmt::FormattedFields<N>>()
                        .map(|fields| scrub_pii(fields.as_str()))
                        .unwrap_or_default()
                };

                write!(writer, "{}{{", scrub_pii(span.name()))?;
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
        use std::fmt::Write;
        if !self.message.is_empty() {
            self.message.push_str(", ");
        }
        if field.name() == "message" {
            let formatted = format!("{value:?}");
            let trimmed = formatted
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(&formatted);
            self.message.push_str(trimmed);
        } else {
            let _ = write!(&mut self.message, "{}={:?}", field.name(), value);
        }
    }
}
