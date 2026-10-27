//! Smithay keeps each surface's state behind a lock that isn't re-entrant:
//! `with_pending_state`, `with_states` and friends hold it while their
//! closure runs, and calling anything inside that takes it again hangs
//! app_server, and with it the whole desktop (unzooming a window did,
//! once: `inner_size` inside `with_pending_state`).
//!
//! The compiler can't see this, so this test reads the source: no closure
//! given to a surface-locking call may call something that locks the
//! surface itself. Work such values out before the call.

use std::path::Path;

/// Calls that hold a surface's lock while their closure runs.
const LOCKING: &[&str] = &[
    "with_pending_state",
    "with_states",
    "with_committed_state",
    "with_cached_state",
    "with_surface_tree_upward",
    "with_surface_tree_downward",
];

/// Calls that take a surface's lock (WindowElement's helpers check the
/// toplevel's state; sending a configure reads it; Space asks elements for
/// their geometry).
const TAKES_LOCK: &[&str] = &[
    "with_pending_state",
    "with_states",
    "with_committed_state",
    "with_cached_state",
    "decorated",
    "inner_size",
    "outer_size",
    "content_offset",
    "client_render_offset",
    "content_size",
    "title",
    "app_id",
    "geometry",
    "bbox",
    "is_zoomed",
    "zoom_area",
    "send_configure",
    "send_pending_configure",
    "map_element",
    "element_geometry",
    "element_bbox",
];

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The text between the parenthesis at `open` and its match.
fn argument(s: &str, open: usize) -> &str {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &s[open + 1..open + i];
                }
            }
            _ => {}
        }
    }
    &s[open + 1..]
}

/// Lock-taking calls in `src`, as "line: outer → inner".
fn nested(src: &str) -> Vec<String> {
    let mut found = Vec::new();
    for outer in LOCKING {
        let needle = format!("{outer}(");
        for (at, _) in src.match_indices(&needle) {
            // A call, not a definition.
            if src[..at].ends_with("fn ") {
                continue;
            }
            let open = at + outer.len();
            let body = argument(src, open);
            for inner in TAKES_LOCK {
                for (i, _) in body.match_indices(&format!(".{inner}(")).chain(body.match_indices(&format!("::{inner}("))) {
                    let line = src[..open + 1 + i].lines().count();
                    found.push(format!("{line}: {outer} → {inner}"));
                }
            }
        }
    }
    found
}

#[test]
fn nothing_relocks_a_surface() {
    let mut paths = Vec::new();
    files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut paths);
    let mut bad = Vec::new();
    for p in paths {
        let src = std::fs::read_to_string(&p).unwrap();
        for f in nested(&src) {
            bad.push(format!("{}:{f}", p.display()));
        }
    }
    assert!(bad.is_empty(), "surface lock taken again inside its own closure (deadlock):\n{}", bad.join("\n"));
}

#[test]
fn catches_the_unzoom_hang() {
    let src = "t.with_pending_state(|state| {\n    state.size = pre.map(|r| window.inner_size(r.size));\n});";
    assert_eq!(nested(src), ["2: with_pending_state → inner_size"]);
}
