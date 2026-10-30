//! The Screenshot portal (`org.freedesktop.impl.portal.Screenshot`): an
//! app asking for a picture of the screen (GIMP's and Inkscape's Take
//! Screenshot, Flameshot-style tools, sandboxed apps). Olo's Screenshot
//! app takes it, so it lands where ⌘⇧3's do, named and in the format the
//! person chose: the screen under the pointer, or, when the app wants the
//! person to choose (`interactive`), the region they drag
//! (`olo-screenshot --region`; Escape cancels). Its path goes back as a
//! `file://` URI. Picking a colour isn't offered yet.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const OK: u32 = 0;
const CANCELLED: u32 = 1;
const FAILED: u32 = 2;

pub struct Screenshot;

/// Take one with Olo's Screenshot app; where it went (None: cancelled).
fn take(interactive: bool) -> Result<Option<String>, String> {
    let program = olo_kit::find_program("olo-screenshot").ok_or("Screenshot isn't installed")?;
    let mode = if interactive { "--region" } else { "--silent" };
    let out = std::process::Command::new(program).args([mode, "--for-app"]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    let path = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().trim().to_owned();
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok((!path.is_empty()).then_some(path))
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl Screenshot {
    async fn screenshot(&self, _handle: OwnedObjectPath, _app_id: String, _parent_window: String, options: HashMap<String, OwnedValue>) -> (u32, HashMap<String, OwnedValue>) {
        let interactive = options.get("interactive").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
        match blocking::unblock(move || take(interactive)).await {
            Ok(Some(path)) => {
                let uri = olo_kit::dnd::file_uri(std::path::Path::new(&path));
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
        (FAILED, HashMap::new())
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }
}
