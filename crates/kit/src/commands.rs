//! The agent and palette socket (see `olo_kit_base::commands`), with
//! what needs the kit: menus flattened and egui's AccessKit trees read.

pub use olo_kit_base::commands::*;

use crate::menu::{Menu, MenuItem};

/// Flatten a menu bar, skipping separators.
pub fn flatten(app_id: &str, bar: &[Menu]) -> Menus {
    fn walk(menu: &Menu, path: &mut Vec<String>, out: &mut Vec<Entry>) {
        path.push(menu.title.clone());
        for item in &menu.items {
            match item {
                MenuItem::Action(a) => out.push(Entry {
                    path: path.clone(),
                    label: a.label.clone(),
                    command: a.command.clone(),
                    keys: a.key_label(),
                    enabled: a.enabled,
                    checked: a.checked,
                }),
                MenuItem::Submenu(m) => walk(m, path, out),
                MenuItem::Separator => {}
            }
        }
        path.pop();
    }
    let mut entries = Vec::new();
    for m in bar {
        walk(m, &mut Vec::new(), &mut entries);
    }
    Menus { app_id: app_id.to_owned(), entries }
}

/// A window's tree from one frame's AccessKit update (its bounds in
/// logical pixels).
pub fn tree_from_update(title: &str, size: (u32, u32), active: bool, update: &egui::accesskit::TreeUpdate) -> WindowTree {
    use std::collections::HashMap;
    let nodes: HashMap<u64, &egui::accesskit::Node> = update.nodes.iter().map(|(id, n)| (id.0, n)).collect();
    let mut parents: HashMap<u64, u64> = HashMap::new();
    for (id, n) in &update.nodes {
        for c in n.children() {
            parents.insert(c.0, id.0);
        }
    }
    let root = update.tree.as_ref().map(|t| t.root.0).or_else(|| update.nodes.iter().map(|(id, _)| id.0).find(|id| !parents.contains_key(id)));
    let mut out = Vec::new();
    let mut stack: Vec<u64> = root.into_iter().collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(n) = nodes.get(&id) else { continue };
        // The window itself has no bounds: it is all of the content.
        let r = n.bounds().filter(|_| Some(id) != root).unwrap_or(egui::accesskit::Rect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 });
        let r = if Some(id) == root { egui::accesskit::Rect { x1: size.0 as f64, y1: size.1 as f64, ..r } } else { r };
        let text = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
        let clickable = n.supports_action(egui::accesskit::Action::Click);
        let picks = matches!(n.role(), egui::accesskit::Role::Row | egui::accesskit::Role::Tab | egui::accesskit::Role::ColumnHeader | egui::accesskit::Role::ListItem);
        let role = match n.role() {
            egui::accesskit::Role::Unknown if clickable => "Clickable".to_owned(),
            role => format!("{role:?}"),
        };
        out.push(UiNode {
            id,
            parent: parents.get(&id).copied(),
            role,
            name: text(n.label()),
            value: text(n.value()).or_else(|| n.numeric_value().map(|v| v.to_string())),
            x: r.x0 as f32,
            y: r.y0 as f32,
            width: (r.x1 - r.x0) as f32,
            height: (r.y1 - r.y0) as f32,
            enabled: !n.is_disabled(),
            clickable,
            focused: update.focus.0 == id && Some(id) != root,
            // egui says a row's, tab's or column header's selection
            // as "toggled": for those it is selected, not checked.
            checked: n.toggled().filter(|_| !picks).map(|t| t == egui::accesskit::Toggled::True),
            selected: n.is_selected().or_else(|| n.toggled().filter(|_| picks).map(|t| t == egui::accesskit::Toggled::True)),
            row: n.row_index(),
            rows: n.row_count(),
            minimized: false,
        });
        stack.extend(n.children().iter().rev().map(|c| c.0));
    }
    // Text is drawn beside the area that takes the click, not in it:
    // each text (a label, a list cell) names the smallest unnamed one
    // its middle is inside (a key, not the whole keypad behind it; a
    // row, by its cells).
    let inside = |n: &UiNode, x: f32, y: f32| (n.x..n.x + n.width).contains(&x) && (n.y..n.y + n.height).contains(&y);
    let mut names: Vec<Vec<String>> = vec![Vec::new(); out.len()];
    // (Few take clicks unnamed, and a window can have thousands of
    // texts: each text looks through those few, not everything.)
    let unnamed: Vec<usize> = (0..out.len()).filter(|&i| out[i].clickable && out[i].name.is_none()).collect();
    for t in out.iter().filter(|t| t.role == "Label" || t.role == "Cell") {
        let Some(text) = &t.name else { continue };
        let (x, y) = (t.x + t.width / 2.0, t.y + t.height / 2.0);
        let area = |i: &usize| out[*i].width * out[*i].height;
        let smallest = unnamed.iter().filter(|&&i| inside(&out[i], x, y)).min_by(|a, b| area(a).total_cmp(&area(b)));
        if let Some(&i) = smallest {
            names[i].push(text.clone());
        }
    }
    for (n, words) in out.iter_mut().zip(names) {
        if !words.is_empty() {
            n.name = Some(words.join(" "));
        }
    }
    WindowTree { title: title.to_owned(), width: size.0, height: size.1, active, layer: None, surface: None, path: None, nodes: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Menu;

    #[test]
    fn flattens_submenus() {
        let bar = vec![
            Menu::new("File").item("New", "new", Some(egui::Key::N)).separator().submenu(Menu::new("Recent").item("a.txt", "open:a", None)),
            Menu::new("View").item("Zoom", "zoom", None).checked(true).enabled(false),
        ];
        let m = flatten("olo.test", &bar);
        assert_eq!(m.entries.len(), 3);
        assert_eq!(m.entries[0].keys.as_deref(), Some("⌘N"));
        assert_eq!(m.entries[1].path, ["File", "Recent"]);
        assert!(m.entries[2].checked && !m.entries[2].enabled);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Menus>(&json).unwrap(), m);
        assert_eq!(serde_json::to_string(&Ask::Menus(true)).unwrap(), r#"{"menus":true}"#);
    }

    #[test]
    fn names_what_takes_clicks() {
        use egui::accesskit::{Action, Node, NodeId, Rect, Role, TreeId, TreeInfo, TreeUpdate};
        let rect = |x0, y0, x1, y1| Rect { x0, y0, x1, y1 };
        let mut root = Node::new(Role::Window);
        root.set_children(vec![NodeId(5), NodeId(2), NodeId(3), NodeId(4)]);
        // Behind it all, a clickable background: the text names the smaller area.
        let mut back = Node::new(Role::Unknown);
        back.add_action(Action::Click);
        back.set_bounds(rect(0.0, 0.0, 300.0, 200.0));
        let mut area = Node::new(Role::Unknown);
        area.add_action(Action::Click);
        area.set_bounds(rect(10.0, 10.0, 110.0, 74.0));
        let mut label = Node::new(Role::Label);
        label.set_label("Appearance");
        label.set_bounds(rect(20.0, 50.0, 90.0, 64.0));
        let mut field = Node::new(Role::TextInput);
        field.set_value("hi");
        field.add_action(Action::Click);
        field.set_bounds(rect(0.0, 80.0, 200.0, 100.0));
        let update = TreeUpdate {
            nodes: vec![(NodeId(1), root), (NodeId(5), back), (NodeId(2), area), (NodeId(3), label), (NodeId(4), field)],
            tree: Some(TreeInfo::new(NodeId(1))),
            tree_id: TreeId::ROOT,
            focus: NodeId(4),
        };
        let t = tree_from_update("Preferences", (300, 200), true, &update);
        let roles: Vec<&str> = t.nodes.iter().map(|n| n.role.as_str()).collect();
        assert_eq!(roles, ["Window", "Clickable", "Clickable", "Label", "TextInput"]);
        assert_eq!((t.nodes[0].width, t.nodes[0].height), (300.0, 200.0));
        assert_eq!(t.nodes[1].name, None);
        assert_eq!(t.nodes[2].name.as_deref(), Some("Appearance"));
        assert_eq!(t.nodes[2].parent, Some(1));
        assert!(t.nodes[4].focused && t.nodes[4].clickable && t.nodes[4].name.is_none());
        assert_eq!(t.nodes[4].value.as_deref(), Some("hi"));
        assert!(!t.nodes[0].focused);
    }
}
