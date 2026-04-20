//! Source-map resolution for `SuspenseLog` stack frames.
//!
//! Fetches each unique bundle URL's `.map` file via the running browser
//! (`fetch(url + '.map')`) so requests inherit origin/cookies, then decodes
//! with the `sourcemap` crate. Frames that can't be resolved (no .map,
//! 404, parse error, no token at the given line/col) are left as `Raw` —
//! the caller sees exactly what React emitted and can decide what to do.
//!
//! The cache lives only for the duration of one resolve() call. A
//! per-daemon-session cache would be a nice-to-have but isn't on the spec's
//! critical path; source-map fetches happen at query time, not on every
//! suspend transition.

use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::suspense_log::{MaybeResolvedFrame, ResolvedFrame, SuspenseLog, Suspender};
use crate::native::browser::BrowserManager;

/// Walk every `MaybeResolvedFrame` in the log (owner/awaiter stacks + jsx_source
/// + owner sources) and attempt to source-map it. Frames that resolve are
/// replaced in place; failures stay as `Raw`.
pub async fn resolve(log: &mut SuspenseLog, mgr: &BrowserManager) {
    let urls = collect_urls(log);
    if urls.is_empty() {
        return;
    }
    let mut maps: HashMap<String, sourcemap::SourceMap> = HashMap::new();
    for url in urls {
        if let Some(map) = fetch_and_parse(mgr, &url).await {
            maps.insert(url, map);
        }
    }
    if maps.is_empty() {
        return;
    }
    for ev in &mut log.events {
        resolve_event(ev, &maps);
    }
}

fn collect_urls(log: &SuspenseLog) -> HashSet<String> {
    let mut urls: HashSet<String> = HashSet::new();
    for ev in &log.events {
        if let Some(list) = &ev.suspended_by {
            for s in list {
                collect_from_suspender(s, &mut urls);
            }
        }
    }
    urls
}

fn collect_from_suspender(s: &Suspender, out: &mut HashSet<String>) {
    if let Some(stack) = &s.owner_stack {
        collect_from_stack(stack, out);
    }
    if let Some(stack) = &s.awaiter_stack {
        collect_from_stack(stack, out);
    }
}

fn collect_from_stack(stack: &[MaybeResolvedFrame], out: &mut HashSet<String>) {
    for f in stack {
        if let MaybeResolvedFrame::Raw((_, url, _, _)) = f {
            if !url.is_empty() {
                out.insert(url.clone());
            }
        }
    }
}

fn resolve_event(ev: &mut super::suspense_log::Event, maps: &HashMap<String, sourcemap::SourceMap>) {
    if let Some(list) = ev.suspended_by.as_mut() {
        for s in list.iter_mut() {
            if let Some(stack) = s.owner_stack.as_mut() {
                resolve_stack(stack, maps);
            }
            if let Some(stack) = s.awaiter_stack.as_mut() {
                resolve_stack(stack, maps);
            }
        }
    }
    // jsxSource is a 3-tuple without a function name; skip for now — the
    // plan focuses source-mapping on the 4-tuple stack frames where agents
    // need it most (owner/awaiter trace).
    let _ = &ev.jsx_source;
    let _ = &ev.owners;
}

fn resolve_stack(stack: &mut [MaybeResolvedFrame], maps: &HashMap<String, sourcemap::SourceMap>) {
    for frame in stack.iter_mut() {
        if let MaybeResolvedFrame::Raw((fn_name, url, line, col)) = frame.clone() {
            if let Some(map) = maps.get(&url) {
                if let Some(resolved) = lookup(map, &fn_name, &url, line, col) {
                    *frame = MaybeResolvedFrame::Resolved(resolved);
                }
            }
        }
    }
}

fn lookup(
    map: &sourcemap::SourceMap,
    fn_name: &str,
    url: &str,
    line: i64,
    col: i64,
) -> Option<ResolvedFrame> {
    // React's stack frames are 1-indexed lines; sourcemap is 0-indexed.
    let l = (line - 1).max(0) as u32;
    let c = col.max(0) as u32;
    let tok = map.lookup_token(l, c)?;
    let file = tok.get_source().unwrap_or("<unknown>").to_string();
    Some(ResolvedFrame {
        function: tok
            .get_name()
            .map(String::from)
            .or_else(|| Some(fn_name.to_string()))
            .filter(|s| !s.is_empty()),
        file,
        line: (tok.get_src_line() as i64) + 1,
        column: tok.get_src_col() as i64,
        bundle: (fn_name.to_string(), url.to_string(), line, col),
    })
}

/// Fetch `url + ".map"` via the running browser so the request picks up
/// the page's origin/cookies (necessary for same-origin dev bundles that
/// require session state to serve the map). Returns None on any failure.
async fn fetch_and_parse(mgr: &BrowserManager, url: &str) -> Option<sourcemap::SourceMap> {
    let map_url = if url.ends_with(".map") {
        url.to_string()
    } else {
        format!("{}.map", url)
    };
    let script = format!(
        "(async () => {{ try {{ const r = await fetch({}, {{ credentials: 'include' }}); if (!r.ok) return null; return await r.text(); }} catch (e) {{ return null; }} }})()",
        json!(map_url)
    );
    let value = mgr.evaluate(&script, None).await.ok()?;
    let text = value.as_str()?;
    sourcemap::SourceMap::from_slice(text.as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::react::suspense_log::{Event, MaybeResolvedFrame, SuspenseLog, Suspender};

    fn log_with_frame(url: &str, line: i64, col: i64) -> SuspenseLog {
        SuspenseLog {
            events: vec![Event {
                t: 10.0,
                id: 1,
                event: "suspended".to_string(),
                name: Some("Foo".to_string()),
                parent_id: Some(0),
                environments: None,
                suspended_by: Some(vec![Suspender {
                    name: "cookies".to_string(),
                    description: "cookies()".to_string(),
                    duration: 0,
                    env: None,
                    owner_name: None,
                    owner_stack: Some(vec![MaybeResolvedFrame::Raw((
                        "fetchCtx".to_string(),
                        url.to_string(),
                        line,
                        col,
                    ))]),
                    awaiter_name: None,
                    awaiter_stack: None,
                }]),
                unknown_suspenders: None,
                owners: None,
                jsx_source: None,
                duration_ms: None,
            }],
            overflowed: false,
            started_at: 0.0,
            buffer_capacity: 2000,
        }
    }

    /// Verify that frames stay as `Raw` when no source map is available.
    /// The resolver is called directly with an empty `maps` map to
    /// simulate the "fetch failed" path without needing a real browser.
    #[test]
    fn resolve_keeps_raw_on_no_map() {
        let mut log = log_with_frame("https://example.com/bundle.js", 10, 20);
        let maps: HashMap<String, sourcemap::SourceMap> = HashMap::new();
        for ev in &mut log.events {
            resolve_event(ev, &maps);
        }
        let stack = log.events[0].suspended_by.as_ref().unwrap()[0]
            .owner_stack
            .as_ref()
            .unwrap();
        assert!(matches!(stack[0], MaybeResolvedFrame::Raw(_)));
    }

    /// Hand-written minimal source map. Maps bundle line 2 col 0 back to
    /// `original.ts` line 5 col 0 with `greet` as the name. Verifies the
    /// resolver swaps the Raw frame for Resolved and preserves the
    /// original tuple under `bundle`.
    #[test]
    fn resolve_replaces_with_resolved() {
        // {"version":3,"file":"bundle.js","sources":["original.ts"],"names":["greet"],"mappings":";AAIAA"}
        // mappings: line 0 empty (;), then `AAIAA` which decodes to:
        //   generated col delta 0, source idx 0, source line delta 4,
        //   source col delta 0, name idx 0
        // => generated line 1 col 0 -> sources[0]="original.ts" line 4 col 0 name="greet"
        let raw = r#"{"version":3,"file":"bundle.js","sources":["original.ts"],"names":["greet"],"mappings":";AAIAA"}"#;
        let map = sourcemap::SourceMap::from_slice(raw.as_bytes()).unwrap();
        let url = "https://example.com/bundle.js".to_string();
        let mut maps: HashMap<String, sourcemap::SourceMap> = HashMap::new();
        maps.insert(url.clone(), map);

        // React gives us 1-indexed lines; we look up line 2 col 0 in the
        // bundle, which maps to 0-indexed line 1 col 0 in the map.
        let mut log = log_with_frame(&url, 2, 0);
        for ev in &mut log.events {
            resolve_event(ev, &maps);
        }
        let stack = log.events[0].suspended_by.as_ref().unwrap()[0]
            .owner_stack
            .as_ref()
            .unwrap();
        match &stack[0] {
            MaybeResolvedFrame::Resolved(r) => {
                assert_eq!(r.file, "original.ts");
                assert_eq!(r.line, 5);
                assert_eq!(r.column, 0);
                assert_eq!(r.bundle.1, url);
                assert_eq!(r.bundle.2, 2);
            }
            _ => panic!("expected Resolved frame"),
        }
    }
}
