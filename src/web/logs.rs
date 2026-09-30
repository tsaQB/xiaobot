//! Keeps the latest log lines in memory for the WebUI log page.
//!
//! [`RingLayer`] is a tracing layer installed next to the stderr formatter,
//! so the console shows the same lines journald receives, filtered by the
//! same `RUST_LOG`, without reading the journal.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{LazyLock, Mutex};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Lines kept in memory.
pub(crate) const LOG_CAPACITY: usize = 500;
/// Longest message kept per line.
const MAX_LINE_CHARS: usize = 4_000;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LogLine {
    pub seq: u64,
    pub ts: String,
    pub level: &'static str,
    pub target: String,
    pub msg: String,
}

#[derive(Default)]
struct Ring {
    next_seq: u64,
    lines: VecDeque<LogLine>,
}

static RING: LazyLock<Mutex<Ring>> = LazyLock::new(|| Mutex::new(Ring::default()));

/// Lines with a sequence number above `after`, oldest first, and the highest
/// sequence number handed out so far.
pub(crate) fn lines_after(after: u64) -> (Vec<LogLine>, u64) {
    let Ok(ring) = RING.lock() else {
        return (Vec::new(), after);
    };
    let lines = ring
        .lines
        .iter()
        .filter(|line| line.seq > after)
        .cloned()
        .collect();
    (lines, ring.next_seq)
}

fn push(level: &'static str, target: &str, msg: String) {
    let Ok(mut ring) = RING.lock() else {
        return;
    };
    ring.next_seq = ring.next_seq.saturating_add(1);
    let seq = ring.next_seq;
    if ring.lines.len() >= LOG_CAPACITY {
        ring.lines.pop_front();
    }
    ring.lines.push_back(LogLine {
        seq,
        ts: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        level,
        target: target.to_string(),
        msg,
    });
}

fn level_name(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARN",
        Level::INFO => "INFO",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

/// Collects the `message` field and appends the other fields as `key=value`.
#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: String,
}

impl Visit for LineVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

/// Tracing layer that copies every enabled event into the ring buffer.
pub(crate) struct RingLayer;

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = LineVisitor::default();
        event.record(&mut visitor);
        let mut msg = visitor.message;
        msg.push_str(&visitor.fields);
        let msg = crate::util::truncate_chars(msg.trim(), MAX_LINE_CHARS);
        let metadata = event.metadata();
        push(level_name(metadata.level()), metadata.target(), msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_newest_lines_in_order() {
        let (_, start) = lines_after(u64::MAX);
        for index in 0..(LOG_CAPACITY + 20) {
            push("INFO", "xiao::test", format!("line {index}"));
        }
        let (lines, last) = lines_after(start);
        assert!(last >= start + (LOG_CAPACITY as u64) + 20);
        assert!(lines.len() <= LOG_CAPACITY);
        assert!(lines.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        let (none, _) = lines_after(last);
        assert!(none.is_empty(), "nothing is newer than the last sequence");
    }
}
