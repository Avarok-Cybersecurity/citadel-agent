//! The tracing layer: reads an event's level, its session and peer from the enclosing
//! spans, and its formatted fields, and hands them to the collector.
//!
//! It never filters: `enabled` is not overridden, so composing it cannot disable an
//! event for another layer. Below WARN it returns after one comparison, and while no
//! session has opted in it returns after one atomic load, before formatting anything.

use super::collector::TelemetryCollector;
use super::entry::{CapturedEvent, TelemetryLevel};
use super::span::{PEER_FIELD, SESSION_FIELD};
use citadel_sdk::logging::tracing::field::{Field, Visit};
use citadel_sdk::logging::tracing::span::{Attributes, Id, Record};
use citadel_sdk::logging::tracing::{Event, Level, Subscriber};
use std::fmt::Write;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

pub struct TelemetryLayer {
    collector: Arc<TelemetryCollector>,
}

impl TelemetryLayer {
    pub fn new(collector: Arc<TelemetryCollector>) -> Self {
        Self { collector }
    }
}

/// The session and peer a span carries, stored in its extensions.
#[derive(Debug, Default, Clone, Copy)]
struct SpanTag {
    cid: Option<u64>,
    peer_cid: Option<u64>,
}

impl Visit for SpanTag {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            SESSION_FIELD => self.cid = Some(value),
            PEER_FIELD => self.peer_cid = Some(value),
            _ => {}
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

/// The message, then every other field as ` name=value`; the `log.*` fields the log
/// bridge adds describe the call site, not the event, and are skipped.
#[derive(Default)]
struct MessageText(String);

impl Visit for MessageText {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.field(field, format_args!("{value}"));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.field(field, format_args!("{value:?}"));
    }
}

impl MessageText {
    fn field(&mut self, field: &Field, value: std::fmt::Arguments<'_>) {
        let name = field.name();
        if name.starts_with("log.") {
            return;
        }
        let separator = if self.0.is_empty() { "" } else { " " };
        let _ = if name == "message" {
            write!(self.0, "{separator}{value}")
        } else {
            write!(self.0, "{separator}{name}={value}")
        };
    }
}

impl<S> Layer<S> for TelemetryLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut tag = SpanTag::default();
        attrs.record(&mut tag);
        if tag.cid.is_some() || tag.peer_cid.is_some() {
            if let Some(span) = ctx.span(id) {
                span.extensions_mut().insert(tag);
            }
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        let mut tag = extensions.get_mut::<SpanTag>().copied().unwrap_or_default();
        values.record(&mut tag);
        extensions.replace(tag);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let level = match *event.metadata().level() {
            Level::ERROR => TelemetryLevel::Error,
            Level::WARN => TelemetryLevel::Warn,
            _ => return,
        };
        if !self.collector.is_capturing() {
            return;
        }
        let mut tag = SpanTag::default();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                if let Some(found) = span.extensions().get::<SpanTag>() {
                    tag.cid = tag.cid.or(found.cid);
                    tag.peer_cid = tag.peer_cid.or(found.peer_cid);
                }
            }
        }
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
        let mut text = MessageText::default();
        event.record(&mut text);
        self.collector.capture(&CapturedEvent {
            time_ms: now_ms(),
            level,
            target: metadata.target(),
            message: &text.0,
            session_cid: tag.cid,
            peer_cid: tag.peer_cid,
        });
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
