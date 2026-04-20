//! Always-on React Suspense transition log.
//!
//! Data types for the ring-buffered event log captured by `SUSPENSE_LOG_INIT`
//! (see `scripts.rs`) and a markdown formatter for the default non-JSON
//! output of `agent-browser react suspense-log`.
//!
//! Raw stack frames captured by React come through as
//! `(function, file, line, column)` tuples. After source-map resolution
//! (see `sourcemap.rs`) individual frames may be replaced by a
//! `ResolvedFrame` — `MaybeResolvedFrame` is an untagged enum that
//! round-trips both shapes through serde so JSON-in/JSON-out is lossless.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SuspenseLog {
    pub events: Vec<Event>,
    #[serde(default)]
    pub overflowed: bool,
    #[serde(rename = "startedAt", default)]
    pub started_at: f64,
    #[serde(rename = "bufferCapacity", default)]
    pub buffer_capacity: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Event {
    pub t: f64,
    pub id: i64,
    /// "suspended" or "resolved".
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "parentID", default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<String>>,
    #[serde(
        rename = "suspendedBy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub suspended_by: Option<Vec<Suspender>>,
    #[serde(
        rename = "unknownSuspenders",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub unknown_suspenders: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owners: Option<Vec<Owner>>,
    #[serde(
        rename = "jsxSource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub jsx_source: Option<JsxSource>,
    #[serde(
        rename = "durationMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub duration_ms: Option<f64>,
}

/// The walker emits `jsxSource` as a 3-tuple `[file, line, column]`. After
/// source-map resolution it may be rewritten as a ResolvedFrame-shaped object.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum JsxSource {
    Resolved(ResolvedFrame),
    Raw((String, i64, i64)),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Owner {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<JsxSource>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Suspender {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub duration: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(
        rename = "ownerName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub owner_name: Option<String>,
    #[serde(
        rename = "ownerStack",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub owner_stack: Option<Vec<MaybeResolvedFrame>>,
    #[serde(
        rename = "awaiterName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub awaiter_name: Option<String>,
    #[serde(
        rename = "awaiterStack",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub awaiter_stack: Option<Vec<MaybeResolvedFrame>>,
}

/// A stack frame that may or may not have been source-map-resolved.
///
/// Untagged: serde tries `Resolved` (object shape) first, falls back to
/// `Raw` (4-tuple). That means both hand-written JSON forms deserialize
/// correctly, and a resolver can swap in place via the ownership of the
/// enclosing `Vec`.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum MaybeResolvedFrame {
    Resolved(ResolvedFrame),
    Raw((String, String, i64, i64)),
}

impl MaybeResolvedFrame {
    /// The original bundle tuple. For `Resolved` frames this is the
    /// preserved `bundle` field; for `Raw` it's the tuple itself.
    pub fn bundle(&self) -> (&str, &str, i64, i64) {
        match self {
            Self::Raw((f, u, l, c)) => (f.as_str(), u.as_str(), *l, *c),
            Self::Resolved(r) => {
                let (f, u, l, c) = &r.bundle;
                (f.as_str(), u.as_str(), *l, *c)
            }
        }
    }
}

/// A source-map-resolved stack frame. `bundle` preserves the original
/// transpiled `(function, file, line, column)` tuple so callers can
/// correlate back to unresolved frames.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ResolvedFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    pub file: String,
    pub line: i64,
    pub column: i64,
    pub bundle: (String, String, i64, i64),
}

/// Render the log as a human-readable markdown report.
///
/// Two sections: a per-boundary summary table, then a chronological timeline.
/// Raw data only — no classification, thresholding, or "too slow" / "unused"
/// labels (agent-first output per the project CLAUDE.md).
pub fn format_suspense_log(log: &SuspenseLog) -> String {
    let mut lines: Vec<String> = Vec::new();
    let elapsed = log
        .events
        .last()
        .map(|e| e.t - log.started_at)
        .unwrap_or(0.0);
    let captured = (elapsed.max(0.0) / 1000.0 * 100.0).round() / 100.0;

    // Group suspend→resolve pairs per fiber id.
    #[derive(Default)]
    struct Episode {
        suspended_count: usize,
        total_suspended_ms: f64,
        last_name: Option<String>,
        last_blocker_name: Option<String>,
        last_blocker_env: Option<String>,
        last_source: Option<String>,
    }
    let mut per_id: HashMap<i64, Episode> = HashMap::new();

    for ev in &log.events {
        let entry = per_id.entry(ev.id).or_default();
        if let Some(ref n) = ev.name {
            entry.last_name = Some(n.clone());
        }
        if ev.event == "suspended" {
            entry.suspended_count += 1;
            if let Some(list) = &ev.suspended_by {
                if let Some(first) = list.first() {
                    entry.last_blocker_name = Some(first.name.clone());
                    entry.last_blocker_env = first.env.clone();
                    if let Some(frame) = first
                        .owner_stack
                        .as_ref()
                        .and_then(|s| s.first())
                        .or_else(|| first.awaiter_stack.as_ref().and_then(|s| s.first()))
                    {
                        entry.last_source = Some(format_frame(frame));
                    }
                }
            }
            if entry.last_source.is_none() {
                if let Some(src) = &ev.jsx_source {
                    entry.last_source = Some(format_jsx_source(src));
                }
            }
        } else if ev.event == "resolved" {
            if let Some(d) = ev.duration_ms {
                entry.total_suspended_ms += d;
            }
        }
    }

    let boundary_count = per_id.len();
    lines.push(format!(
        "# Suspense Log — {:.2}s captured, {} events across {} boundar{}",
        captured,
        log.events.len(),
        boundary_count,
        if boundary_count == 1 { "y" } else { "ies" }
    ));
    if log.overflowed {
        lines.push(format!(
            "(ring buffer overflowed — oldest events dropped, capacity {})",
            log.buffer_capacity
        ));
    }
    lines.push(String::new());

    if log.events.is_empty() {
        lines.push("(no suspense activity captured)".to_string());
        return lines.join("\n");
    }

    // Summary table — sort by first-seen t per id.
    let mut first_seen: HashMap<i64, f64> = HashMap::new();
    for ev in &log.events {
        first_seen.entry(ev.id).or_insert(ev.t);
    }
    let mut ids: Vec<i64> = per_id.keys().copied().collect();
    ids.sort_by(|a, b| {
        first_seen
            .get(a)
            .copied()
            .unwrap_or(0.0)
            .partial_cmp(&first_seen.get(b).copied().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    lines.push("| Boundary | Episodes | Suspended (ms) | Blocker | Source |".to_string());
    lines.push("| --- | --- | --- | --- | --- |".to_string());
    for id in &ids {
        let ep = per_id.get(id).unwrap();
        let name = ep
            .last_name
            .clone()
            .unwrap_or_else(|| format!("boundary-{}", id));
        let blocker = match (&ep.last_blocker_name, &ep.last_blocker_env) {
            (Some(n), Some(e)) => format!("{} ({})", n, e),
            (Some(n), None) => n.clone(),
            _ => "-".to_string(),
        };
        let source = ep.last_source.clone().unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            escape_cell(&name),
            ep.suspended_count,
            format_ms(ep.total_suspended_ms),
            escape_cell(&blocker),
            escape_cell(&source),
        ));
    }
    lines.push(String::new());

    lines.push("## Timeline".to_string());
    for ev in &log.events {
        let name = ev
            .name
            .clone()
            .unwrap_or_else(|| format!("boundary-{}", ev.id));
        match ev.event.as_str() {
            "suspended" => {
                let blocker = ev
                    .suspended_by
                    .as_ref()
                    .and_then(|v| v.first())
                    .map(|s| {
                        let src = s
                            .owner_stack
                            .as_ref()
                            .and_then(|s| s.first())
                            .or_else(|| s.awaiter_stack.as_ref().and_then(|s| s.first()))
                            .map(format_frame)
                            .unwrap_or_else(|| "-".to_string());
                        format!(" ({} @ {})", s.name, src)
                    })
                    .unwrap_or_default();
                let unknown = ev
                    .unknown_suspenders
                    .as_deref()
                    .map(|r| format!(" [unknownSuspenders: {}]", r))
                    .unwrap_or_default();
                lines.push(format!(
                    "T={:>7.2}ms  {:30}  suspended{}{}",
                    ev.t - log.started_at,
                    name,
                    blocker,
                    unknown,
                ));
            }
            "resolved" => {
                let dur = ev
                    .duration_ms
                    .map(|d| format!(" ({:.2}ms)", d))
                    .unwrap_or_default();
                lines.push(format!(
                    "T={:>7.2}ms  {:30}  resolved{}",
                    ev.t - log.started_at,
                    name,
                    dur,
                ));
            }
            other => {
                lines.push(format!(
                    "T={:>7.2}ms  {:30}  {}",
                    ev.t - log.started_at,
                    name,
                    other,
                ));
            }
        }
    }
    lines.join("\n")
}

fn format_ms(ms: f64) -> String {
    format!("{:.2}", ms)
}

fn format_frame(f: &MaybeResolvedFrame) -> String {
    match f {
        MaybeResolvedFrame::Raw((_fn_name, file, line, _col)) => format!("{}:{}", file, line),
        MaybeResolvedFrame::Resolved(r) => format!("{}:{}", r.file, r.line),
    }
}

fn format_jsx_source(s: &JsxSource) -> String {
    match s {
        JsxSource::Raw((file, line, _col)) => format!("{}:{}", file, line),
        JsxSource::Resolved(r) => format!("{}:{}", r.file, r.line),
    }
}

fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_event(id: i64, t: f64, event: &str) -> Event {
        Event {
            t,
            id,
            event: event.to_string(),
            name: Some(format!("Boundary{}", id)),
            parent_id: Some(1),
            environments: Some(vec!["Server".to_string()]),
            suspended_by: None,
            unknown_suspenders: None,
            owners: None,
            jsx_source: None,
            duration_ms: None,
        }
    }

    #[test]
    fn parse_event_roundtrip() {
        let ev = Event {
            t: 123.4,
            id: 42,
            event: "suspended".to_string(),
            name: Some("TeamLayout".to_string()),
            parent_id: Some(1),
            environments: Some(vec!["Server".to_string()]),
            suspended_by: Some(vec![Suspender {
                name: "cookies".to_string(),
                description: "cookies()".to_string(),
                duration: 0,
                env: Some("Server".to_string()),
                owner_name: Some("fetchCtx".to_string()),
                owner_stack: Some(vec![MaybeResolvedFrame::Raw((
                    "fetchCtx".to_string(),
                    "webpack-internal:///./app/ctx.tsx".to_string(),
                    47,
                    12,
                ))]),
                awaiter_name: None,
                awaiter_stack: None,
            }]),
            unknown_suspenders: None,
            owners: None,
            jsx_source: Some(JsxSource::Raw((
                "webpack-internal:///./app/layout.tsx".to_string(),
                18,
                9,
            ))),
            duration_ms: None,
        };
        let json = serde_json::to_value(&ev).unwrap();
        let back: Event = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.id, 42);
        assert_eq!(back.name.as_deref(), Some("TeamLayout"));
        let sb = back.suspended_by.as_ref().unwrap();
        assert_eq!(sb[0].name, "cookies");
        assert!(matches!(
            sb[0].owner_stack.as_ref().unwrap()[0],
            MaybeResolvedFrame::Raw(_)
        ));
    }

    #[test]
    fn format_empty_log() {
        let log = SuspenseLog {
            events: vec![],
            overflowed: false,
            started_at: 0.0,
            buffer_capacity: 2000,
        };
        let out = format_suspense_log(&log);
        assert!(out.contains("(no suspense activity captured)"), "{}", out);
        assert!(!out.contains("| Boundary |"), "empty should not print a table: {}", out);
    }

    #[test]
    fn format_with_events_produces_table_and_timeline() {
        let mut suspend = sample_event(42, 10.0, "suspended");
        suspend.suspended_by = Some(vec![Suspender {
            name: "cookies".to_string(),
            description: "cookies()".to_string(),
            duration: 0,
            env: Some("Server".to_string()),
            owner_name: None,
            owner_stack: Some(vec![MaybeResolvedFrame::Raw((
                "fetchCtx".to_string(),
                "app/ctx.tsx".to_string(),
                47,
                12,
            ))]),
            awaiter_name: None,
            awaiter_stack: None,
        }]);
        let mut resolve = sample_event(42, 65.0, "resolved");
        resolve.duration_ms = Some(55.0);

        let log = SuspenseLog {
            events: vec![suspend, resolve],
            overflowed: false,
            started_at: 0.0,
            buffer_capacity: 2000,
        };
        let out = format_suspense_log(&log);
        assert!(out.contains("# Suspense Log"), "{}", out);
        assert!(out.contains("Boundary42"), "{}", out);
        assert!(out.contains("cookies"), "{}", out);
        assert!(out.contains("app/ctx.tsx:47"), "{}", out);
        assert!(out.contains("## Timeline"), "{}", out);
        assert!(out.contains("suspended"), "{}", out);
        assert!(out.contains("resolved"), "{}", out);
        assert!(out.contains("55.00ms"), "{}", out);
    }

    #[test]
    fn maybe_resolved_frame_deserializes_both_shapes() {
        let raw = json!(["fn", "bundle.js", 10, 5]);
        let parsed: MaybeResolvedFrame = serde_json::from_value(raw).unwrap();
        match parsed {
            MaybeResolvedFrame::Raw((f, u, l, c)) => {
                assert_eq!(f, "fn");
                assert_eq!(u, "bundle.js");
                assert_eq!(l, 10);
                assert_eq!(c, 5);
            }
            _ => panic!("expected Raw"),
        }

        let resolved = json!({
            "function": "fetchCtx",
            "file": "/src/ctx.tsx",
            "line": 42,
            "column": 8,
            "bundle": ["fetchCtx", "webpack-internal:///./app/ctx.tsx", 47, 12]
        });
        let parsed: MaybeResolvedFrame = serde_json::from_value(resolved).unwrap();
        match parsed {
            MaybeResolvedFrame::Resolved(r) => {
                assert_eq!(r.function.as_deref(), Some("fetchCtx"));
                assert_eq!(r.file, "/src/ctx.tsx");
                assert_eq!(r.line, 42);
                assert_eq!(r.column, 8);
                assert_eq!(r.bundle.2, 47);
            }
            _ => panic!("expected Resolved"),
        }
    }

    #[test]
    fn overflowed_flag_rendered_in_header() {
        let log = SuspenseLog {
            events: vec![sample_event(1, 10.0, "suspended")],
            overflowed: true,
            started_at: 0.0,
            buffer_capacity: 2000,
        };
        let out = format_suspense_log(&log);
        assert!(out.contains("overflowed"), "{}", out);
    }
}
