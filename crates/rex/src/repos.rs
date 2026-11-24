//! The person's repositories on a host's account (Ben, 2026-11-22: "do
//! it in a way where we can extend this over time too, e.g. choosing
//! what repos to clone and how to set them up"): a list the person
//! keeps (the desktop's settings), each a repository's address, where
//! it goes under the home there, and a command that sets it up once it
//! is cloned. The host is asked how each stands ([`look`],
//! [`There::read`]); one that is missing is cloned and set up
//! ([`Repo::clone_script`]), its lines shown as they come, safe to run
//! again: a repository that is there is never cloned over, a folder
//! that holds something else is left alone and said, and a set-up that
//! ran isn't run twice (it is noted in the clone's own `.git`).
//!
//! Cloned as the account, with its own git. Over SSH the keys are
//! those Rex forwards from Olo Passwords (`~/.ssh/olo-rex.conf` on the
//! host), with nothing to answer: a host whose key is new to the
//! account is trusted as it is first met, one whose key changed is
//! refused, and nothing asks for a password.

use crate::provision::shell_word;

/// One repository of the person's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repo {
    /// Where it is cloned from: `git@host:owner/name.git`, `https://…`,
    /// `ssh://…`.
    pub url: String,
    /// Where it goes under the account's home; empty: `src/NAME`.
    pub folder: String,
    /// A command run in it once it is cloned (`sh -c`); empty: none.
    pub setup: String,
}

impl Repo {
    /// Its name: the last part of its address, without `.git`.
    pub fn name(&self) -> String {
        let url = self.url.trim().trim_end_matches('/');
        let last = url.rsplit(['/', ':']).next().unwrap_or(url);
        last.strip_suffix(".git").unwrap_or(last).to_owned()
    }

    /// Where it goes under the home, as said or by its name.
    pub fn place(&self) -> String {
        let folder = self.folder.trim().trim_matches('/');
        let folder = folder.strip_prefix("~/").unwrap_or(folder);
        if folder.is_empty() { format!("src/{}", self.name()) } else { folder.to_owned() }
    }

    /// What is wrong with it as written, if anything: said in the
    /// window where it is typed, and nothing is run for one that is.
    pub fn wrong(&self) -> Option<&'static str> {
        let url = self.url.trim();
        let known = ["https://", "http://", "ssh://", "git://", "file://"].iter().any(|scheme| url.starts_with(scheme));
        // (`git@host:owner/name`: a host, a colon, a path.)
        let scp = url.split_once(':').is_some_and(|(host, path)| !host.is_empty() && !host.contains('/') && !path.is_empty() && !path.starts_with('/'));
        if url.is_empty() {
            Some("Type the repository’s address.")
        } else if url.starts_with('-') || url.contains(char::is_whitespace) || !(known || scp) {
            Some("This isn’t a repository’s address (git@host:owner/name.git, or https://…).")
        } else if self.name().is_empty() {
            Some("The address has no repository’s name in it.")
        } else if self.place().starts_with('-') || self.place().split('/').any(|part| part == ".." || part.is_empty()) {
            Some("The folder is a place under your home there (src/name).")
        } else {
            None
        }
    }

    /// A word that changes when the set-up command does: what is noted
    /// in the clone when it has run.
    fn setup_mark(&self) -> String {
        use sha2::Digest as _;
        sha2::Sha256::digest(self.setup.trim().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    /// The script that clones it and sets it up, for `sh` as the
    /// account. Each of the two only if it hasn't been done.
    pub fn clone_script(&self) -> String {
        let (place, url, mark) = (shell_word(&self.place()), shell_word(self.url.trim()), self.setup_mark());
        let setup = match self.setup.trim() {
            "" => String::new(),
            command => format!(
                r#"if [ "$(cat "$d/.git/olo-set-up" 2> /dev/null)" != {mark} ]; then
  echo "Setting up: {said}"
  ( cd "$d" && sh -c {command} ) < /dev/null
  echo {mark} > "$d/.git/olo-set-up"
fi
"#,
                said = command.lines().next().unwrap_or("").replace(['"', '$', '`', '\\'], ""),
                command = shell_word(command),
            ),
        };
        format!(
            r#"set -eu
command -v git > /dev/null 2>&1 || {{ echo "git isn't installed on this host." >&2; exit 1; }}
cd "$HOME"
export PATH="$HOME/.local/bin:$PATH"
d={place}
if [ ! -e "$d/.git" ]; then
  if [ -e "$d" ] && [ -n "$(ls -A "$d" 2> /dev/null)" ]; then
    echo "~/$d is there already, and isn't this repository." >&2
    exit 1
  fi
  mkdir -p "$(dirname "$d")"
  GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new" git clone --progress -- {url} "$d" < /dev/null 2>&1
fi
{setup}"#
        )
    }
}

/// How a repository stands on a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum There {
    /// Not there.
    Absent,
    /// Cloned; and set up (true where there is no set-up to run).
    Cloned { set_up: bool },
    /// Its folder holds another repository: where that one is from.
    Another(String),
    /// Its folder holds something that is no repository.
    Taken,
}

impl There {
    /// Nothing is left to do for it.
    pub fn done(&self) -> bool {
        matches!(self, There::Cloned { set_up: true })
    }

    /// What a host answered [`look`] of `repos`: how each stands, in
    /// their order. None: it didn't answer of all of them.
    pub fn read(output: &str, repos: &[Repo]) -> Option<Vec<There>> {
        repos
            .iter()
            .enumerate()
            .map(|(i, repo)| {
                let said = output.lines().find_map(|line| line.strip_prefix(&format!("repo {i} ")))?;
                let (what, rest) = said.split_once(' ').unwrap_or((said, ""));
                Some(match what {
                    "absent" => There::Absent,
                    "taken" => There::Taken,
                    "cloned" => {
                        let (mark, origin) = rest.split_once(' ').unwrap_or((rest, ""));
                        if same_repository(origin, &repo.url) { There::Cloned { set_up: repo.setup.trim().is_empty() || mark == repo.setup_mark() } } else { There::Another(origin.trim().to_owned()) }
                    }
                    _ => return None,
                })
            })
            .collect()
    }
}

/// Whether two addresses are one repository's: the same host and path,
/// however each is written (`git@github.com:o/n.git`,
/// `https://github.com/o/n`, `ssh://git@github.com/o/n`).
fn same_repository(a: &str, b: &str) -> bool {
    fn bare(url: &str) -> String {
        let url = url.trim().trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
        let rest = rest.split_once('@').map_or(rest, |(_, rest)| rest);
        rest.replacen(':', "/", 1).to_ascii_lowercase()
    }
    bare(a) == bare(b)
}

/// Asks a host, as the account, how each of `repos` stands: what
/// [`There::read`] reads.
pub fn look(repos: &[Repo]) -> String {
    let mut script = String::from("cd \"$HOME\" || exit 1\n");
    for (i, repo) in repos.iter().enumerate() {
        let place = shell_word(&repo.place());
        script.push_str(&format!(
            "d={place}\nif [ -e \"$d/.git\" ]; then echo \"repo {i} cloned $(cat \"$d/.git/olo-set-up\" 2> /dev/null || echo -) $(git -C \"$d\" config --get remote.origin.url 2> /dev/null)\"; elif [ -e \"$d\" ] && [ -n \"$(ls -A \"$d\" 2> /dev/null)\" ]; then echo \"repo {i} taken\"; else echo \"repo {i} absent\"; fi\n"
        ));
    }
    script.push_str("true\n");
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(url: &str, folder: &str, setup: &str) -> Repo {
        Repo { url: url.into(), folder: folder.into(), setup: setup.into() }
    }

    #[test]
    fn a_repository_is_named_and_placed_by_its_address() {
        assert_eq!(repo("git@github.com:example/olo.git", "", "").place(), "src/olo");
        assert_eq!(repo("https://github.com/example/olo/", "", "").name(), "olo");
        assert_eq!(repo("ssh://git@host.example:2222/team/thing.git", "work/thing", "").place(), "work/thing");
        assert_eq!(repo("git@github.com:example/olo.git", "~/code/olo/", "").place(), "code/olo");
        // What isn't run: no address, something that isn't one, an option, a place outside the home.
        assert!(repo("", "", "").wrong().is_some());
        assert!(repo("--upload-pack=x", "", "").wrong().is_some());
        assert!(repo("just words", "", "").wrong().is_some());
        assert!(repo("olo", "", "").wrong().is_some());
        assert!(repo("git@github.com:example/olo.git", "../elsewhere", "").wrong().is_some());
        assert!(repo("git@github.com:example/olo.git", "-x", "").wrong().is_some());
        for good in ["git@github.com:example/olo.git", "https://github.com/example/olo", "ssh://git@host.example/team/thing.git", "file:///var/tmp/x.git"] {
            assert_eq!(repo(good, "", "").wrong(), None, "{good}");
        }
        // One repository, however its address is written.
        assert!(same_repository("git@github.com:Example/olo.git", "https://github.com/example/olo"));
        assert!(same_repository("ssh://git@github.com/example/olo", "git@github.com:example/olo.git"));
        assert!(!same_repository("git@github.com:example/olo.git", "git@github.com:example/other.git"));
    }

    #[test]
    fn what_a_host_says_of_the_repositories_is_read() {
        let repos = [
            repo("git@github.com:example/olo.git", "", "cargo fetch"),
            repo("https://github.com/example/site", "", ""),
            repo("git@github.com:example/new.git", "", ""),
            repo("git@github.com:example/x.git", "", ""),
        ];
        let mark = repos[0].setup_mark();
        let said = format!("repo 0 cloned {mark} git@github.com:example/olo.git\nrepo 1 cloned - git@github.com:other/site.git\nrepo 2 absent\nrepo 3 taken\n");
        assert_eq!(There::read(&said, &repos), Some(vec![There::Cloned { set_up: true }, There::Another("git@github.com:other/site.git".into()), There::Absent, There::Taken]));
        // (Cloned, its set-up not run yet, or run for another command.)
        assert_eq!(There::read("repo 0 cloned - https://github.com/example/olo\n", &repos[..1]), Some(vec![There::Cloned { set_up: false }]));
        assert_eq!(There::read("repo 0 cloned 0000 git@github.com:example/olo.git\n", &repos[..1]), Some(vec![There::Cloned { set_up: false }]));
        // (A host that didn't answer of all of them.)
        assert_eq!(There::read("repo 0 absent\n", &repos), None);
        assert_eq!(There::read("", &[]), Some(Vec::new()));
    }

    /// The scripts against a real home on this machine, with a
    /// repository made here for the address: cloned and set up; run
    /// again, neither is done twice; the set-up changed, it alone runs;
    /// a folder that holds something else is left alone and said.
    #[test]
    fn a_repository_is_cloned_and_set_up_once() {
        // (On disk: /tmp is memory here.)
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (home, origin) = (dir.join("home"), dir.join("origin"));
        std::fs::create_dir_all(&home).unwrap();
        let run_in = |cwd: &std::path::Path, script: &str| -> (bool, String) {
            let out =
                std::process::Command::new("sh").arg("-c").arg(script).current_dir(cwd).env("HOME", &home).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
            (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
        };
        std::fs::create_dir_all(&origin).unwrap();
        let (made, said) = run_in(&origin, "git init -q . && echo hello > README && git add README && git -c user.name=T -c user.email=t@example.org commit -q -m first");
        assert!(made, "{said}");
        let run = |script: &str| run_in(&home, script);
        let mut it = repo(&format!("file://{}", origin.display()), "", "echo ran >> ../../set-up-ran");
        let stands = |it: &Repo| There::read(&run(&look(std::slice::from_ref(it))).1, std::slice::from_ref(it)).unwrap().remove(0);

        assert_eq!(stands(&it), There::Absent);
        let (ok, said) = run(&it.clone_script());
        assert!(ok, "{said}");
        assert!(home.join("src/origin/README").exists() && said.contains("Setting up: echo ran"), "{said}");
        assert_eq!(stands(&it), There::Cloned { set_up: true });
        // Again: nothing is cloned over, and the set-up isn't run twice.
        std::fs::write(home.join("src/origin/mine.txt"), "kept").unwrap();
        let (ok, said) = run(&it.clone_script());
        assert!(ok && !said.contains("Setting up") && home.join("src/origin/mine.txt").exists(), "{said}");
        assert_eq!(std::fs::read_to_string(home.join("set-up-ran")).unwrap(), "ran\n");
        // The set-up changed: it alone is run.
        it.setup = "echo again >> ../../set-up-ran".into();
        assert_eq!(stands(&it), There::Cloned { set_up: false });
        assert!(run(&it.clone_script()).0);
        assert_eq!(std::fs::read_to_string(home.join("set-up-ran")).unwrap(), "ran\nagain\n");
        // A set-up that fails: said, and still to do.
        it.setup = "echo no >&2; exit 3".into();
        let (ok, said) = run(&it.clone_script());
        assert!(!ok && said.contains("no"), "{said}");
        assert_eq!(stands(&it), There::Cloned { set_up: false });
        // A folder that holds something else: left alone, and said.
        let taken = repo(&format!("file://{}", origin.display()), "taken", "");
        std::fs::create_dir_all(home.join("taken")).unwrap();
        std::fs::write(home.join("taken/notes.txt"), "mine").unwrap();
        assert_eq!(stands(&taken), There::Taken);
        let (ok, said) = run(&taken.clone_script());
        assert!(!ok && said.contains("isn't this repository"), "{said}");
        assert_eq!(std::fs::read_to_string(home.join("taken/notes.txt")).unwrap(), "mine");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
