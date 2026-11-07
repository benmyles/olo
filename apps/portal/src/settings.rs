//! The Settings portal (`org.freedesktop.impl.portal.Settings`): what
//! sandboxed programs, and GTK 4, libadwaita, Chrome and Firefox where
//! they ask the portal, read for the desktop's look. Olo's own, in place
//! of GTK's and GNOME's backends (which read GNOME's settings): the
//! colour scheme, accent and contrast (`org.freedesktop.appearance`),
//! and what GTK reads under GNOME's names (`org.gnome.desktop.interface`,
//! `org.gnome.desktop.wm.preferences`), the same list app_server writes
//! for unsandboxed programs (`olo_theme::toolkit::desktop_settings`).
//! Followed live: each change app_server tells of is signalled
//! (`SettingChanged`), key by key, only where a value changed.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use olo_sync::Mutex;
use olo_theme::{ThemeMode, toolkit};
use zbus::{
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
};

/// Namespace, then key, then value.
pub type Values = BTreeMap<String, BTreeMap<String, OwnedValue>>;

const APPEARANCE: &str = "org.freedesktop.appearance";

/// `org.freedesktop.impl.portal.Settings`.
#[derive(Clone, Default)]
pub struct Settings {
    pub values: Arc<Mutex<Values>>,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotFound(String),
}

fn owned(v: Value<'_>) -> OwnedValue {
    // (Only plain values are made here: they always convert.)
    OwnedValue::try_from(v).expect("a plain value")
}

/// What Olo says for its look now (from app_server's settings).
pub fn values(a: &olo_protocol::Appearance) -> Values {
    let mode = a.effective_theme;
    let mut out = Values::new();
    let appearance = out.entry(APPEARANCE.to_owned()).or_default();
    // 0 no preference (light, as GNOME's "default"), 1 dark.
    appearance.insert("color-scheme".into(), owned(Value::U32(if mode == ThemeMode::Dark { 1 } else { 0 })));
    appearance.insert("accent-color".into(), owned(Value::from(toolkit::ACCENT_BLUE)));
    appearance.insert("contrast".into(), owned(Value::U32(0)));
    for s in toolkit::desktop_settings(&a.icon_theme, &a.colors, mode, olo_xdg::CURSOR_THEME, olo_xdg::CURSOR_SIZE) {
        let value = match s.value {
            toolkit::DesktopValue::Str(v) => owned(Value::from(v)),
            toolkit::DesktopValue::Int(v) => owned(Value::I32(v)),
        };
        out.entry(s.schema.to_owned()).or_default().insert(s.key.to_owned(), value);
    }
    out
}

/// Whether `namespace` is asked for by `patterns` (none: all; a trailing
/// `*` matches what begins so, as the portal's globbing has it).
fn wanted(patterns: &[String], namespace: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|p| p.strip_suffix('*').map_or(p == namespace, |prefix| namespace.starts_with(prefix)))
}

/// The values of `values` in the namespaces `patterns` ask for.
pub fn read_all(values: &Values, patterns: &[String]) -> HashMap<String, HashMap<String, OwnedValue>> {
    values.iter().filter(|(ns, _)| wanted(patterns, ns)).map(|(ns, keys)| (ns.clone(), keys.iter().filter_map(|(k, v)| Some((k.clone(), v.try_clone().ok()?))).collect())).collect()
}

/// What changed from `old` to `new`: namespace, key, new value.
pub fn changes(old: &Values, new: &Values) -> Vec<(String, String, OwnedValue)> {
    let mut out = Vec::new();
    for (ns, keys) in new {
        for (k, v) in keys {
            if old.get(ns).and_then(|o| o.get(k)) != Some(v)
                && let Ok(v) = v.try_clone()
            {
                out.push((ns.clone(), k.clone(), v));
            }
        }
    }
    out
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Settings")]
impl Settings {
    async fn read_all(&self, namespaces: Vec<String>) -> HashMap<String, HashMap<String, OwnedValue>> {
        read_all(&self.values.lock().unwrap(), &namespaces)
    }

    async fn read(&self, namespace: String, key: String) -> Result<OwnedValue, Error> {
        let values = self.values.lock().unwrap();
        values.get(&namespace).and_then(|keys| keys.get(&key)).and_then(|v| v.try_clone().ok()).ok_or_else(|| Error::NotFound(format!("{namespace} has no {key}")))
    }

    #[zbus(signal)]
    async fn setting_changed(emitter: &SignalEmitter<'_>, namespace: &str, key: &str, value: Value<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

/// Follow app_server's settings for as long as the portal serves on
/// `conn`: the values kept, and what changed signalled. Started again
/// whenever app_server goes (a restart, an upgrade).
pub fn follow(conn: zbus::blocking::Connection, settings: Settings) {
    let apply = move |a: &olo_protocol::Appearance| {
        let new = values(a);
        let changed = {
            let mut values = settings.values.lock().unwrap();
            let changed = changes(&values, &new);
            *values = new;
            changed
        };
        if changed.is_empty() {
            return;
        }
        let Ok(iface) = conn.object_server().interface::<_, Settings>(crate::portal::PATH) else { return };
        for (ns, key, value) in changed {
            let _ = zbus::block_on(Settings::setting_changed(iface.signal_emitter(), &ns, &key, Value::from(value)));
        }
    };
    std::thread::Builder::new()
        .name("settings".into())
        .spawn(move || {
            loop {
                if let Ok(mut c) = olo_protocol::Client::connect()
                    && let Ok(olo_protocol::Reply::Settings { settings }) = c.request(&olo_protocol::Request::GetSettings)
                {
                    apply(&settings.appearance);
                }
                if let Ok(client) = olo_protocol::Client::connect()
                    && let Ok(events) = client.into_events_of(&[olo_protocol::EventKind::Settings])
                {
                    for event in events {
                        match event {
                            Ok(olo_protocol::Event::SettingsChanged { settings }) => apply(&settings.appearance),
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                }
                // (app_server restarting: asked again in a moment.)
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appearance(mode: ThemeMode) -> olo_protocol::Appearance {
        olo_protocol::Appearance { effective_theme: mode, ..Default::default() }
    }

    #[test]
    fn says_the_look_as_the_toolkits_read_it() {
        let light = values(&appearance(ThemeMode::Light));
        assert_eq!(light[APPEARANCE]["color-scheme"], owned(Value::U32(0)));
        let interface = &light["org.gnome.desktop.interface"];
        assert_eq!(interface["color-scheme"], owned(Value::from("default")));
        assert!(String::try_from(interface["gtk-theme"].try_clone().unwrap()).unwrap().starts_with(toolkit::GTK_THEME_PREFIX));
        assert_eq!(interface["cursor-size"], owned(Value::I32(olo_xdg::CURSOR_SIZE as i32)));
        assert_eq!(light["org.gnome.desktop.wm.preferences"]["button-layout"], owned(Value::from("close:maximize")));
        // GTK asks for these patterns.
        let asked = read_all(&light, &["org.gnome.*".into(), APPEARANCE.into()]);
        assert_eq!(asked.len(), 3, "{:?}", asked.keys());
        assert_eq!(read_all(&light, &["org.gnome.desktop.interface".into()]).len(), 1);
        assert!(read_all(&light, &["org.kde.*".into()]).is_empty());
        assert_eq!(read_all(&light, &[]).len(), 3);
        // Dark: the scheme and the theme change, nothing else.
        let dark = values(&appearance(ThemeMode::Dark));
        let changed: Vec<(String, String)> = changes(&light, &dark).into_iter().map(|(ns, k, _)| (ns, k)).collect();
        assert_eq!(
            changed,
            [
                (APPEARANCE.to_owned(), "color-scheme".to_owned()),
                ("org.gnome.desktop.interface".to_owned(), "color-scheme".to_owned()),
                ("org.gnome.desktop.interface".to_owned(), "gtk-theme".to_owned())
            ]
        );
        assert!(changes(&dark, &dark).is_empty());
    }
}
