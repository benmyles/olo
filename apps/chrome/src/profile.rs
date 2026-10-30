//! Which Chrome profile the extension is in, and that profile's account:
//! the extension keeps a random id of its own in its storage, which is
//! the profile's (`chrome.storage.local`, a LevelDB under the profile's
//! `Local Extension Settings/<extension id>`); the profile whose storage
//! holds it is this one, and Chrome's `Local State` says who is signed in
//! to it. So Passwords can fill each of six Google accounts in its own
//! profile, with no permission the extension would have to ask for.
//!
//! The profiles looked in are those whose storage for the extension
//! Chrome (our parent) has open, else every one in its user data folder:
//! Chrome rewrites its command line (one string, spaces between), so a
//! `--user-data-dir` there can't be relied on.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A profile's account, as `Local State` has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    /// Its folder's name ("Default", "Profile 3").
    pub dir: String,
    /// The account signed in to Chrome there (an email), if any.
    pub email: String,
    /// The name it shows in Chrome's profile menu.
    pub name: String,
}

/// Larger files aren't searched (an extension's storage is small).
const MAX_FILE: u64 = 8 << 20;

/// The profile of `user_data` whose storage for extension `extension`
/// holds `token`.
pub fn find(user_data: &Path, extension: &str, token: &str) -> Option<Profile> {
    let dirs: Vec<PathBuf> = std::fs::read_dir(user_data).ok()?.flatten().map(|e| e.path()).collect();
    find_in(&dirs, extension, token)
}

/// Which of the profile folders `profiles` (each in a user data folder,
/// beside its `Local State`) has storage for extension `extension` that
/// holds `token`.
pub fn find_in(profiles: &[PathBuf], extension: &str, token: &str) -> Option<Profile> {
    if token.len() < 16 || extension.contains('/') || extension.contains("..") {
        return None;
    }
    let p = profiles.iter().find(|p| holds(&p.join("Local Extension Settings").join(extension), token.as_bytes()))?;
    let dir = p.file_name()?.to_string_lossy().into_owned();
    let state: Option<Value> = p.parent().and_then(|u| std::fs::read(u.join("Local State")).ok()).and_then(|b| serde_json::from_slice(&b).ok());
    let info = state.as_ref().and_then(|s| s.pointer(&format!("/profile/info_cache/{}", pointer_escape(&dir))));
    let text = |k: &str| info.and_then(|i| i.get(k)).and_then(Value::as_str).unwrap_or_default().to_owned();
    Some(Profile { email: text("user_name"), name: text("name"), dir })
}

/// The profile folders whose storage for `extension` process `pid` has
/// open (Chrome keeps each's LevelDB `LOCK` open while the extension runs
/// there).
pub fn open_profiles(pid: u32, extension: &str) -> Vec<PathBuf> {
    let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return Vec::new() };
    let tail = Path::new("Local Extension Settings").join(extension).join("LOCK");
    let mut out: Vec<PathBuf> = Vec::new();
    for fd in fds.flatten() {
        if let Ok(target) = std::fs::read_link(fd.path())
            && target.ends_with(&tail)
            && let Some(profile) = target.ancestors().nth(3)
            && !out.iter().any(|o| o == profile)
        {
            out.push(profile.to_owned());
        }
    }
    out
}

/// The profile of the Chrome that started us whose storage for our
/// extension holds `token`.
pub fn this_one(token: &str) -> Option<Profile> {
    let ext = extension_id()?;
    let open = open_profiles(std::os::unix::process::parent_id(), &ext);
    find_in(&open, &ext, token).or_else(|| find(&user_data_dir()?, &ext, token))
}

/// Whether a file in `dir` (LevelDB's log and tables: a value this small
/// is kept as it is, compressed or not) holds `needle`.
fn holds(dir: &Path, needle: &[u8]) -> bool {
    let Ok(files) = std::fs::read_dir(dir) else { return false };
    files.flatten().any(|f| {
        let path = f.path();
        let small = f.metadata().is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE);
        small && std::fs::read(&path).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle))
    })
}

fn pointer_escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Chrome's user data directory, from the browser that started us (its
/// command line, which Chrome may have made one string, spaces between),
/// else the usual one.
pub fn user_data_dir() -> Option<PathBuf> {
    let parent = std::os::unix::process::parent_id();
    let line = std::fs::read(format!("/proc/{parent}/cmdline")).unwrap_or_default();
    let mut args: Vec<String> = line.split(|&c| c == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    if let Some(joined) = args.first().filter(|a| a.contains(" --")).cloned() {
        args.extend(joined.split(" --").skip(1).map(|a| format!("--{a}")));
    }
    crate::prefs::user_data_dir(&args)
}

/// The extension that started us (`chrome-extension://ID/`, our first
/// argument).
pub fn extension_id() -> Option<String> {
    let origin = std::env::args().nth(1)?;
    let id = origin.strip_prefix("chrome-extension://")?.trim_end_matches('/');
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase())).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_profile_holding_the_token() {
        let root = std::env::temp_dir().join(format!("olo-chrome-profile-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ext = "abcdefghijklmnopabcdefghijklmnop";
        for (dir, token) in [("Default", "11112222333344445555666677778888"), ("Profile 2", "aaaabbbbccccddddeeeeffff00001111")] {
            let store = root.join(dir).join("Local Extension Settings").join(ext);
            std::fs::create_dir_all(&store).unwrap();
            let mut log = b"\x00\x01junk\x07profile\"".to_vec();
            log.extend_from_slice(token.as_bytes());
            log.extend_from_slice(b"\"more");
            std::fs::write(store.join("000003.log"), log).unwrap();
        }
        let state = serde_json::json!({ "profile": { "info_cache": {
            "Default": { "user_name": "first@example.com", "name": "Work" },
            "Profile 2": { "user_name": "second@example.com", "name": "Home" },
        } } });
        std::fs::write(root.join("Local State"), state.to_string()).unwrap();
        let found = find(&root, ext, "aaaabbbbccccddddeeeeffff00001111").unwrap();
        assert_eq!(found, Profile { dir: "Profile 2".into(), email: "second@example.com".into(), name: "Home".into() });
        assert_eq!(find(&root, ext, "11112222333344445555666677778888").unwrap().email, "first@example.com");
        assert!(find(&root, ext, "99998888777766665555444433332222").is_none());
        // Too short to be ours, or a path for an id: nothing.
        assert!(find(&root, ext, "1111").is_none());
        assert!(find(&root, "../x", "aaaabbbbccccddddeeeeffff00001111").is_none());
        // Among the profiles given (those Chrome has the storage open in).
        assert_eq!(find_in(&[root.join("Default")], ext, "aaaabbbbccccddddeeeeffff00001111"), None);
        assert_eq!(find_in(&[root.join("Default"), root.join("Profile 2")], ext, "aaaabbbbccccddddeeeeffff00001111").unwrap().dir, "Profile 2");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_the_profiles_with_the_storage_open() {
        let root = std::env::temp_dir().join(format!("olo-chrome-open-test-{}", std::process::id()));
        let ext = "abcdefghijklmnopabcdefghijklmnop";
        let store = root.join("Profile 4").join("Local Extension Settings").join(ext);
        std::fs::create_dir_all(&store).unwrap();
        let _lock = std::fs::File::create(store.join("LOCK")).unwrap();
        let open = open_profiles(std::process::id(), ext);
        assert!(open.contains(&root.join("Profile 4")), "{open:?}");
        assert!(open_profiles(std::process::id(), "someotherextensionidabcdefghijkl").iter().all(|p| !p.starts_with(&root)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
