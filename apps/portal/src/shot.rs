//! The Screenshot portal (`org.freedesktop.impl.portal.Screenshot`): an
//! app asking for a picture of the screen (GIMP's and Inkscape's Take
//! Screenshot, Flameshot-style tools, sandboxed apps). Olo's Screenshot
//! app takes it, so it lands where ⌘⇧3's do, named and in the format the
//! person chose: the screen under the pointer, or, when the app wants the
//! person to choose (`interactive`), the region they drag
//! (`olo-screenshot --region`; Escape cancels). Its path goes back as a
//! `file://` URI. PickColor freezes the screens with a loupe by the
//! pointer (`olo-screenshot --pick-color`); the colour clicked goes back.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const OK: u32 = 0;
const CANCELLED: u32 = 1;
const FAILED: u32 = 2;

pub struct Screenshot;

/// Take one with Olo's Screenshot app; where it went (None: cancelled).
fn take(interactive: bool) -> Result<Option<String>, String> {
    let program = olo_xdg::find_program("olo-screenshot").ok_or("Screenshot isn't installed")?;
    let mode = if interactive { "--region" } else { "--silent" };
    let out = std::process::Command::new(program).args([mode, "--for-app"]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    let path = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().trim().to_owned();
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok((!path.is_empty()).then_some(path))
}

/// Pick a colour with Olo's Screenshot app: (r, g, b) from 0 to 1, None
/// if cancelled.
fn pick() -> Result<Option<(f64, f64, f64)>, String> {
    let program = olo_xdg::find_program("olo-screenshot").ok_or("Screenshot isn't installed")?;
    let out = std::process::Command::new(program).arg("--pick-color").stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(parse_colour(&String::from_utf8_lossy(&out.stdout)))
}

/// "R G B" (0–255) as fractions.
fn parse_colour(s: &str) -> Option<(f64, f64, f64)> {
    let v: Vec<f64> = s.split_whitespace().filter_map(|w| w.parse::<u8>().ok()).map(|n| n as f64 / 255.0).collect();
    match v.as_slice() {
        [r, g, b] => Some((*r, *g, *b)),
        _ => None,
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl Screenshot {
    async fn screenshot(&self, _handle: OwnedObjectPath, _app_id: String, _parent_window: String, options: HashMap<String, OwnedValue>) -> (u32, HashMap<String, OwnedValue>) {
        let interactive = options.get("interactive").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        match blocking::unblock(move || take(interactive)).await {
            Ok(Some(path)) => {
                let uri = olo_ui::files::file_uri(std::path::Path::new(&path));
                (OK, HashMap::from([("uri".to_owned(), OwnedValue::try_from(Value::from(uri)).expect("a string"))]))
            }
            Ok(None) => (CANCELLED, HashMap::new()),
            Err(e) => {
                tracing::warn!("portal: no screenshot: {e}");
                (FAILED, HashMap::new())
            }
        }
    }

    async fn pick_color(&self, _handle: OwnedObjectPath, _app_id: String, _parent_window: String, _options: HashMap<String, OwnedValue>) -> (u32, HashMap<String, OwnedValue>) {
        match blocking::unblock(pick).await {
            Ok(Some((r, g, b))) => (OK, HashMap::from([("color".to_owned(), OwnedValue::try_from(Value::from((r, g, b))).expect("numbers"))])),
            Ok(None) => (CANCELLED, HashMap::new()),
            Err(e) => {
                tracing::warn!("portal: no colour picked: {e}");
                (FAILED, HashMap::new())
            }
        }
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn colours() {
        assert_eq!(super::parse_colour("255 0 51\n"), Some((1.0, 0.0, 0.2)));
        assert_eq!(super::parse_colour(""), None);
        assert_eq!(super::parse_colour("1 2"), None);
    }
}
