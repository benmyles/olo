//! Other programs' widget trees, through AT-SPI (the accessibility bus
//! GTK 3 and 4, Qt and Chrome's own interface speak): for a window whose
//! program isn't built on Olo's kit, its frame's accessibles become a
//! `WindowTree` like a kit app's, so `tree`, `click --name`, `wait` and
//! the rest work on it too. Roles are mapped to AccessKit's names
//! ("push button" is a `Button`), states to the node's flags, and each
//! node's place is its extents relative to the window (`WINDOW`
//! coordinates: on Wayland a program doesn't know where on screen it is).
//! Only what is showing is walked, and at most `MAX_NODES` of it.

use olo_kit::commands::{UiNode, WindowTree};
use zbus::{blocking::Connection, zvariant::OwnedObjectPath};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const TEXT: &str = "org.a11y.atspi.Text";
const VALUE: &str = "org.a11y.atspi.Value";
const MAX_NODES: usize = 3000;
/// How long one question to a program may take.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
const MAX_DEPTH: usize = 60;

/// AT-SPI's states (`AtspiStateType`) that matter here, by bit.
mod state {
    pub const CHECKED: u32 = 4;
    pub const ENABLED: u32 = 8;
    pub const FOCUSED: u32 = 12;
    pub const SELECTABLE: u32 = 22;
    pub const SELECTED: u32 = 23;
    pub const SENSITIVE: u32 = 24;
    pub const SHOWING: u32 = 25;
    pub const VISIBLE: u32 = 30;
    pub const CHECKABLE: u32 = 41;
}

type Ref = (String, OwnedObjectPath);

/// The accessibility bus (`AT_SPI_BUS_ADDRESS`, else its address from
/// `org.a11y.Bus` on the session bus), if there is one.
fn a11y_bus() -> Option<Connection> {
    let address = match std::env::var("AT_SPI_BUS_ADDRESS") {
        Ok(a) if !a.is_empty() => a,
        _ => {
            let session = Connection::session().ok()?;
            let reply = session.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &()).ok()?;
            reply.body().deserialize().ok()?
        }
    };
    // A program that doesn't answer (busy, or hung) mustn't hold up the
    // whole tree.
    zbus::blocking::connection::Builder::address(address.as_str()).ok()?.method_timeout(CALL_TIMEOUT).build().ok()
}

fn call<T: serde::de::DeserializeOwned + zbus::zvariant::Type>(c: &Connection, r: &Ref, iface: &str, method: &str, body: &(impl serde::Serialize + zbus::zvariant::DynamicType)) -> Option<T> {
    let reply = c.call_method(Some(r.0.as_str()), r.1.as_str(), Some(iface), method, body).ok()?;
    reply.body().deserialize().ok()
}

fn property<T: TryFrom<zbus::zvariant::OwnedValue>>(c: &Connection, r: &Ref, iface: &str, name: &str) -> Option<T> {
    let v: zbus::zvariant::OwnedValue = call(c, r, "org.freedesktop.DBus.Properties", "Get", &(iface, name))?;
    T::try_from(v).ok()
}

fn children(c: &Connection, r: &Ref) -> Vec<Ref> {
    call::<Vec<Ref>>(c, r, ACCESSIBLE, "GetChildren", &()).unwrap_or_default()
}

fn has(states: &[u32], bit: u32) -> bool {
    states.get((bit / 32) as usize).is_some_and(|w| w & (1 << (bit % 32)) != 0)
}

/// AT-SPI's role name as AccessKit names it (what agents look for), and
/// whether such a thing takes clicks.
pub fn role(name: &str) -> (String, bool) {
    let (role, clicks) = match name {
        "push button" | "button" => ("Button", true),
        "toggle button" => ("ToggleButton", true),
        "check box" | "check menu item" => ("CheckBox", true),
        "radio button" | "radio menu item" => ("RadioButton", true),
        "entry" | "text" | "text box" => ("TextInput", true),
        "password text" => ("PasswordInput", true),
        "spin button" => ("SpinButton", true),
        "combo box" => ("ComboBox", true),
        "menu item" | "tearoff menu item" => ("MenuItem", true),
        "menu" => ("Menu", true),
        "menu bar" => ("MenuBar", false),
        "page tab" => ("Tab", true),
        "page tab list" => ("TabList", false),
        "link" => ("Link", true),
        "list item" => ("ListItem", true),
        "list" | "list box" => ("List", false),
        "table row" | "tree item" => ("Row", true),
        "table cell" => ("Cell", true),
        "table" | "tree table" | "tree" => ("Table", false),
        "slider" => ("Slider", true),
        "scroll bar" => ("ScrollBar", false),
        "label" | "static" => ("Label", false),
        "heading" => ("Heading", false),
        "image" | "icon" => ("Image", false),
        "document web" | "document frame" | "document text" => ("Document", false),
        "dialog" | "alert" => ("Dialog", false),
        "frame" | "window" => ("Window", false),
        "tool bar" => ("Toolbar", false),
        "progress bar" => ("ProgressIndicator", false),
        "separator" => ("Splitter", false),
        other => return (title_case(other), false),
    };
    (role.to_owned(), clicks)
}

/// "scroll pane" → "ScrollPane".
fn title_case(name: &str) -> String {
    name.split([' ', '-', '_']).filter(|w| !w.is_empty()).map(|w| w[..1].to_uppercase() + &w[1..]).collect()
}

/// The program `pid`'s windows (its AT-SPI frames, dialogs and windows),
/// each a tree; None if it isn't on the accessibility bus.
pub fn trees(pid: u32) -> Option<Vec<WindowTree>> {
    let c = a11y_bus()?;
    // The program's own connections to the bus (each AT-SPI program
    // serves its root at the same path), rather than the registry's list
    // of them: the same programs, without asking one more process.
    let bus = |method: &str, arg: &str| c.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), method, &(arg,)).ok();
    let names: Vec<String> = c
        .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ListNames", &())
        .ok()?
        .body()
        .deserialize()
        .ok()?;
    let root = OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root").ok()?;
    let apps: Vec<Ref> = names
        .into_iter()
        .filter(|n| n.starts_with(':'))
        .filter(|n| bus("GetConnectionUnixProcessID", n).and_then(|m| m.body().deserialize::<u32>().ok()) == Some(pid))
        .map(|n| (n, root.clone()))
        .collect();
    let mut out = Vec::new();
    for frame in apps.iter().flat_map(|app| children(&c, app)) {
        let states: Vec<u32> = call(&c, &frame, ACCESSIBLE, "GetState", &()).unwrap_or_default();
        if !has(&states, state::SHOWING) && !has(&states, state::VISIBLE) {
            continue;
        }
        let title: String = property(&c, &frame, ACCESSIBLE, "Name").unwrap_or_default();
        // (Places are from the window's geometry, its content to Olo.)
        let (_, _, w, h): (i32, i32, i32, i32) = call(&c, &frame, COMPONENT, "GetExtents", &(1u32,)).unwrap_or_default();
        let mut tree = WindowTree { title, width: w.max(0) as u32, height: h.max(0) as u32, active: has(&states, 1), ..Default::default() };
        walk(&c, &frame, None, None, 0, &mut tree.nodes);
        out.push(tree);
    }
    Some(out)
}

/// `r` and what it holds, as nodes; `parent` the node it goes under, and
/// `named` that node's name (a button's own label says it again).
fn walk(c: &Connection, r: &Ref, parent: Option<u64>, named: Option<&str>, depth: usize, nodes: &mut Vec<UiNode>) {
    if nodes.len() >= MAX_NODES || depth > MAX_DEPTH {
        return;
    }
    let states: Vec<u32> = call(c, r, ACCESSIBLE, "GetState", &()).unwrap_or_default();
    // What isn't on screen, nor anything in it (a hidden page of a stack).
    if depth > 0 && !has(&states, state::SHOWING) {
        return;
    }
    let role_name: String = call(c, r, ACCESSIBLE, "GetRoleName", &()).unwrap_or_default();
    let (role, clicks) = role(&role_name);
    let (x, y, w, h): (i32, i32, i32, i32) = call(c, r, COMPONENT, "GetExtents", &(1u32,)).unwrap_or((0, 0, 0, 0));
    let name: Option<String> = property::<String>(c, r, ACCESSIBLE, "Name").filter(|n| !n.is_empty());
    let value = value_of(c, r, &role);
    let id = nodes.len() as u64 + 1;
    let checkable = has(&states, state::CHECKABLE) || matches!(role.as_str(), "CheckBox" | "RadioButton" | "ToggleButton");
    // Containers say nothing to an agent (their children do), nor does
    // what has no size, nor a label saying its button's name again.
    let container = matches!(role.as_str(), "Panel" | "Filler" | "Unknown" | "Section" | "Grouping" | "ScrollPane" | "Viewport" | "Generic");
    let repeat = role == "Label" && name.is_some() && name.as_deref() == named;
    let keep = depth == 0 || ((name.is_some() || value.is_some() || clicks || !container) && w > 0 && h > 0 && !repeat);
    let my_name = if keep { name.clone() } else { None };
    let me = if keep {
        nodes.push(UiNode {
            id,
            parent,
            role,
            name,
            value,
            x: x as f32,
            y: y as f32,
            width: w.max(0) as f32,
            height: h.max(0) as f32,
            enabled: has(&states, state::ENABLED) || has(&states, state::SENSITIVE),
            clickable: clicks,
            focused: has(&states, state::FOCUSED),
            checked: checkable.then(|| has(&states, state::CHECKED)),
            selected: has(&states, state::SELECTABLE).then(|| has(&states, state::SELECTED)),
            ..Default::default()
        });
        Some(id)
    } else {
        parent
    };
    let named = if keep { my_name.as_deref() } else { named };
    for child in children(c, r) {
        walk(c, &child, me, named, depth + 1, nodes);
    }
}

/// A field's text, a slider's or spin button's number.
fn value_of(c: &Connection, r: &Ref, role: &str) -> Option<String> {
    match role {
        "TextInput" | "PasswordInput" | "SpinButton" | "ComboBox" | "Document" => {
            let n: i32 = property(c, r, TEXT, "CharacterCount")?;
            if role == "PasswordInput" {
                return Some("•".repeat(n.max(0) as usize));
            }
            let text: String = call(c, r, TEXT, "GetText", &(0i32, n.min(4096)))?;
            Some(text)
        }
        "Slider" | "ProgressIndicator" => property::<f64>(c, r, VALUE, "CurrentValue").map(|v| format!("{v}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OLO_ATSPI_PID=… cargo test -p olo-screen atspi -- --ignored
    /// --nocapture`: what a running program's tree is.
    #[test]
    #[ignore]
    fn live() {
        let pid: u32 = std::env::var("OLO_ATSPI_PID").expect("OLO_ATSPI_PID").parse().unwrap();
        let t = std::time::Instant::now();
        let trees = trees(pid);
        eprintln!("{:?} in {:?}", trees.as_ref().map(|t| t.iter().map(|w| (w.title.clone(), w.nodes.len())).collect::<Vec<_>>()), t.elapsed());
        for n in trees.iter().flatten().flat_map(|t| t.nodes.iter().take(12)) {
            eprintln!("  {} {:?} at {},{} {}x{}", n.role, n.name, n.x, n.y, n.width, n.height);
        }
    }

    #[test]
    fn roles_as_accesskit_names_them() {
        assert_eq!(role("push button"), ("Button".into(), true));
        assert_eq!(role("check box"), ("CheckBox".into(), true));
        assert_eq!(role("label"), ("Label".into(), false));
        assert_eq!(role("scroll pane"), ("ScrollPane".into(), false));
        assert!(has(&[1 << 25, 0], state::SHOWING) && has(&[0, 1 << 9], state::CHECKABLE) && !has(&[0], state::CHECKABLE));
    }
}
