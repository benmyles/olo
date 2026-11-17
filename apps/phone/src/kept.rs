//! What the service keeps of a phone between its runs: its
//! conversations, which messages it has seen, and its contacts, so
//! they're there the moment the service starts again (an upgrade, a
//! restart, the next login), before the phone has been asked anything,
//! and while it's out of reach. An iPhone gives only its inbox's latest,
//! and keeps nothing of what was sent through it: what was sent from
//! here would otherwise be gone with the service.
//!
//! One file, the person's alone (`~/.local/state/olo/phone/kept.json`,
//! readable by them only, in a folder of the same), for the one phone in
//! use: another phone's replaces it, and a phone unpaired has it
//! removed. What a message says is in it, as in the phone's own store.

use std::path::PathBuf;

use olo_protocol::phone::{Conversation, Person};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// Whose: the phone's Bluetooth address.
    pub phone: String,
    pub threads: Vec<Conversation>,
    /// The handles of the messages seen (one not among them is new).
    pub seen: Vec<String>,
    pub contacts: Vec<Person>,
}

/// Where it's kept (`OLO_PHONE_KEPT`: elsewhere, for a test).
pub fn path() -> PathBuf {
    std::env::var_os("OLO_PHONE_KEPT").map(PathBuf::from).unwrap_or_else(|| olo_xdg::state_dir().join("phone").join("kept.json"))
}

/// What was kept of `phone` (its address): none if nothing was, or it
/// was another phone's.
pub fn load(phone: &str) -> Option<Kept> {
    let kept: Kept = serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()?;
    (!phone.is_empty() && kept.phone == phone).then_some(kept)
}

/// Keep it: written whole beside the file and moved over it, so a
/// reader never finds half of it.
pub fn save(kept: &Kept) -> std::io::Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    };
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let beside = path.with_extension("json.new");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&beside)?;
    file.write_all(&serde_json::to_vec(kept).map_err(std::io::Error::other)?)?;
    drop(file);
    std::fs::rename(&beside, &path)
}

/// The phone was unpaired: nothing of it stays.
pub fn forget() {
    let _ = std::fs::remove_file(path());
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use olo_protocol::phone::Message;

    use super::*;

    /// Kept for its phone alone, the person's alone, and gone when
    /// forgotten. (One test: the place is the process's environment.)
    #[test]
    fn kept_for_its_phone_and_the_person_alone() {
        let dir = PathBuf::from(format!("/var/tmp/olo-phone-kept-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: the only test that reads or sets it.
        unsafe { std::env::set_var("OLO_PHONE_KEPT", dir.join("phone/kept.json")) };
        assert_eq!(load("02:00:00:00:F0:01"), None);
        let kept = Kept {
            phone: "02:00:00:00:F0:01".into(),
            threads: vec![Conversation {
                key: "+14155550134".into(),
                name: "Mira Kovač".into(),
                address: "+1 (415) 555-0134".into(),
                messages: vec![
                    Message { handle: "20".into(), text: "Order me a flat white?".into(), at: 1_794_700_000, outgoing: false, read: false },
                    Message { text: "On it".into(), at: 1_794_700_060, outgoing: true, read: true, ..Default::default() },
                ],
            }],
            seen: vec!["20".into()],
            contacts: vec![Person { name: "Mira Kovač".into(), phones: vec!["+1 (415) 555-0134".into()], emails: Vec::new() }],
        };
        save(&kept).unwrap();
        assert_eq!(load("02:00:00:00:F0:01"), Some(kept.clone()));
        assert_eq!(load("02:00:00:00:F0:02"), None, "another phone's");
        assert_eq!(load(""), None);
        assert_eq!(std::fs::metadata(path()).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir.join("phone")).unwrap().permissions().mode() & 0o777, 0o700);
        // Kept again: the file whole, nothing left beside it.
        save(&Kept { seen: Vec::new(), ..kept }).unwrap();
        assert_eq!(std::fs::read_dir(dir.join("phone")).unwrap().count(), 1);
        forget();
        assert_eq!(load("02:00:00:00:F0:01"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
