//! Who can log in: the people with accounts (`/etc/passwd`'s users with
//! an ordinary id, as `/etc/login.defs` says, and a shell to log in
//! with), each with their full name and picture (AccountsService's, as
//! a desktop's settings set it).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    /// The account's name ("ben").
    pub name: String,
    /// As people know them ("Sam Example"), else the account's name.
    pub full_name: String,
    /// Their picture, if they have one.
    pub picture: Option<PathBuf>,
}

/// The people who can log in, by full name. `root` is the file system's
/// root (`/`; a test's tree).
pub fn all(root: &Path) -> Vec<User> {
    let defs = std::fs::read_to_string(root.join("etc/login.defs")).unwrap_or_default();
    let value = |key: &str| defs.lines().find_map(|l| l.trim().strip_prefix(key).filter(|r| r.starts_with(char::is_whitespace)).and_then(|r| r.trim().parse::<u32>().ok()));
    let (min, max) = (value("UID_MIN").unwrap_or(1000), value("UID_MAX").unwrap_or(60000));
    let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap_or_default();
    let mut users: Vec<User> = passwd
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(':').collect();
            let [name, _, uid, _, gecos, _, shell] = f[..] else { return None };
            let uid: u32 = uid.parse().ok()?;
            let shell = shell.rsplit('/').next().unwrap_or("");
            if uid < min || uid > max || matches!(shell, "nologin" | "false" | "sync" | "shutdown" | "halt") {
                return None;
            }
            let full = gecos.split(',').next().unwrap_or("").trim();
            let icon = root.join("var/lib/AccountsService/icons").join(name);
            Some(User { name: name.to_owned(), full_name: if full.is_empty() { name.to_owned() } else { full.to_owned() }, picture: icon.is_file().then_some(icon) })
        })
        .collect();
    users.sort_by_key(|u| u.full_name.to_lowercase());
    users
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_with_accounts() {
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-greet-users-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::create_dir_all(root.join("var/lib/AccountsService/icons")).unwrap();
        std::fs::write(
            root.join("etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nnobody:x:65534:65534:Kernel Overflow User:/:/usr/sbin/nologin\nsam:x:1000:1000:Sam Example,,,:/home/sam:/usr/bin/brush\nsvc:x:1001:1001::/var/svc:/usr/sbin/nologin\nalice:x:1002:1002::/home/alice:/bin/bash\n",
        )
        .unwrap();
        std::fs::write(root.join("etc/login.defs"), "# ids\nUID_MIN                  1000\nUID_MAX                 60000\n").unwrap();
        std::fs::write(root.join("var/lib/AccountsService/icons/sam"), b"png").unwrap();
        let users = all(&root);
        let names: Vec<(&str, &str, bool)> = users.iter().map(|u| (u.name.as_str(), u.full_name.as_str(), u.picture.is_some())).collect();
        assert_eq!(names, [("alice", "alice", false), ("sam", "Sam Example", true)]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
