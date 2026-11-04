//! Airplane mode: every radio (Wi-Fi, Bluetooth, a modem) turned off or
//! back on at once, through the kernel's rfkill (`/dev/rfkill`, which
//! logind gives the person at the computer), as a laptop's airplane key
//! asks (a ThinkPad's F8 sends `KEY_RFKILL`). NetworkManager and BlueZ
//! follow it. Read from sysfs.

use std::io::Write;

/// The radios, as sysfs lists them: (name, type, soft-blocked,
/// hard-blocked).
pub fn radios() -> Vec<(String, String, bool, bool)> {
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default().trim().to_owned();
    std::fs::read_dir("/sys/class/rfkill")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let d = e.path();
            (read(d.join("name")), read(d.join("type")), read(d.join("soft")) == "1", read(d.join("hard")) == "1")
        })
        .collect()
}

/// Whether airplane mode is on: there are radios, and every one is off
/// (None without radios).
pub fn airplane() -> Option<bool> {
    let r = radios();
    (!r.is_empty()).then(|| r.iter().all(|(_, _, soft, hard)| *soft || *hard))
}

/// Turn every radio off (`on`), or back on.
pub fn set_airplane(on: bool) -> std::io::Result<()> {
    // struct rfkill_event: idx (u32), type (u8: 0 all), op (u8: 3 change
    // all), soft, hard.
    const TYPE_ALL: u8 = 0;
    const OP_CHANGE_ALL: u8 = 3;
    let mut event = [0u8; 8];
    event[4] = TYPE_ALL;
    event[5] = OP_CHANGE_ALL;
    event[6] = u8::from(on);
    std::fs::OpenOptions::new().write(true).open("/dev/rfkill")?.write_all(&event)
}

/// Airplane mode on if it was off, off if it was on: whether it's on now.
pub fn toggle_airplane() -> std::io::Result<bool> {
    let on = !airplane().unwrap_or(false);
    set_airplane(on)?;
    Ok(on)
}

#[cfg(test)]
mod tests {
    /// This computer's radios (`cargo test -p olo-system rfkill -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn live() {
        eprintln!("{:?} airplane {:?}", super::radios(), super::airplane());
    }
}
