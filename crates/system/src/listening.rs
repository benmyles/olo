//! Which processes hold the socket listening on a TCP port, read from
//! /proc: the socket's inode from `/proc/net/tcp` (and `tcp6`), then the
//! processes whose `/proc/PID/fd` links say `socket:[inode]`. Token Pool's
//! keeper follows the copy serving the pool's port by it, and Olo Doctor
//! says who has the port.

use std::path::Path;

/// The inodes of the sockets listening on `port` (on any address), as
/// `root`'s /proc says (`/` for this computer; a test's folder).
pub fn inodes(root: &Path, port: u16) -> Vec<u64> {
    let port = format!(":{port:04X}");
    let mut found = Vec::new();
    for table in ["proc/net/tcp", "proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(root.join(table)) else { continue };
        for line in text.lines().skip(1) {
            // sl local_address rem_address st tx:rx tr:when retrnsmt uid timeout inode
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() > 9
                && f[1].ends_with(&port)
                && f[3] == "0A"
                && let Ok(inode) = f[9].parse::<u64>()
                && inode != 0
                && !found.contains(&inode)
            {
                found.push(inode);
            }
        }
    }
    found
}

/// The processes holding a socket listening on `port`, of those `among`
/// says to look into (by pid), lowest first. Each one looked into costs
/// reading its descriptors' links, so a caller that knows what it's
/// after (one program's processes) says so.
pub fn holders(root: &Path, port: u16, among: impl Fn(i32) -> bool) -> Vec<i32> {
    let inodes = inodes(root, port);
    if inodes.is_empty() {
        return Vec::new();
    }
    let links: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
    let mut found: Vec<i32> = std::fs::read_dir(root.join("proc"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok().map(|pid| (pid, e.path())))
        .filter(|(pid, _)| among(*pid))
        .filter(|(_, dir)| std::fs::read_dir(dir.join("fd")).into_iter().flatten().flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|to| links.iter().any(|l| to.as_os_str() == l.as_str()))))
        .map(|(pid, _)| pid)
        .collect();
    found.sort_unstable();
    found
}

/// The name of the program process `pid` runs (its executable's file
/// name, " (deleted)" once replaced taken off), as `root`'s /proc says.
pub fn program(root: &Path, pid: i32) -> Option<String> {
    let exe = std::fs::read_link(root.join("proc").join(pid.to_string()).join("exe")).ok()?;
    Some(exe.file_name()?.to_string_lossy().trim_end_matches(" (deleted)").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The listening socket on 17891 (and not the connection to it, nor
    /// another port's), and the processes holding it.
    #[test]
    fn who_listens_on_a_port() {
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-listening-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, c: &str| {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        w(
            "proc/net/tcp",
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   \
             0: 0100007F:45E3 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 555 1 0 100 0 0 10 0\n   \
             1: 0100007F:9C40 0100007F:45E3 01 00000000:00000000 00:00000000 00000000  1000        0 556 1 0 20 4 30 10 -1\n   \
             2: 0100007F:45E4 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 557 1 0 100 0 0 10 0\n",
        );
        for (pid, fds) in [(10, &["socket:[555]", "/dev/null"][..]), (11, &["socket:[557]"]), (12, &["socket:[556]"]), (13, &["socket:[555]"])] {
            std::fs::create_dir_all(root.join(format!("proc/{pid}/fd"))).unwrap();
            for (i, to) in fds.iter().enumerate() {
                std::os::unix::fs::symlink(to, root.join(format!("proc/{pid}/fd/{i}"))).unwrap();
            }
        }
        std::os::unix::fs::symlink("/home/me/.local/bin/olo-pool (deleted)", root.join("proc/10/exe")).unwrap();
        assert_eq!(inodes(&root, 17891), [555]);
        assert_eq!(holders(&root, 17891, |_| true), [10, 13]);
        assert_eq!(holders(&root, 17891, |p| p != 13), [10]);
        assert!(holders(&root, 17893, |_| true).is_empty());
        assert_eq!(program(&root, 10).as_deref(), Some("olo-pool"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
