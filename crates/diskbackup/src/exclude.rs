//! What isn't backed up: groups of what can be made again (builds,
//! dependencies downloaded, caches), chosen in DiskBackup's settings, the
//! person's own patterns, and any folder marked a cache (a `CACHEDIR.TAG`,
//! as Cargo's `target/` has).

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

/// A group of patterns: `~/` is the home folder; `**/name` anywhere.
pub struct Group {
    pub id: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub patterns: &'static [&'static str],
    /// Chosen unless the person says otherwise.
    pub default: bool,
}

pub const GROUPS: &[Group] = &[
    Group {
        id: "caches",
        name: "Caches",
        about: "What apps keep to go faster and fetch again: ~/.cache, browsers' caches, thumbnails",
        patterns: &[
            "~/.cache",
            "~/.config/google-chrome/*/Cache",
            "~/.config/google-chrome/*/Code Cache",
            "~/.config/google-chrome/*/GPUCache",
            "~/.config/google-chrome/*/Service Worker/CacheStorage",
            "~/.config/chromium/*/Cache",
            "~/.mozilla/firefox/*/cache2",
            "~/.var/app/*/cache",
            "~/.local/share/Steam/steamapps/shadercache",
        ],
        default: true,
    },
    Group { id: "trash", name: "Trash", about: "What's in the Trash", patterns: &["~/.local/share/Trash"], default: true },
    Group {
        id: "rust",
        name: "Rust",
        about: "Build output (target/) and Cargo's downloads (~/.cargo/registry, git)",
        patterns: &["**/target/debug", "**/target/release", "**/target/*/debug", "**/target/*/release", "~/.cargo/registry", "~/.cargo/git", "~/.rustup/toolchains", "~/.rustup/downloads"],
        default: true,
    },
    Group {
        id: "node",
        name: "JavaScript",
        about: "node_modules and the bundlers' and frameworks' builds and caches",
        patterns: &["**/node_modules", "**/.next", "**/.nuxt", "**/.turbo", "**/.parcel-cache", "**/.svelte-kit", "~/.npm", "~/.bun/install/cache", "~/.local/share/pnpm/store", "~/.yarn/cache"],
        default: true,
    },
    Group {
        id: "python",
        name: "Python",
        about: "Bytecode, virtual environments and tools' caches",
        patterns: &["**/__pycache__", "**/*.pyc", "**/.venv", "**/.mypy_cache", "**/.pytest_cache", "**/.ruff_cache", "**/.tox", "~/.local/share/virtualenvs"],
        default: true,
    },
    Group { id: "go", name: "Go", about: "Modules downloaded and the build cache", patterns: &["~/go/pkg/mod", "~/.cache/go-build"], default: true },
    Group {
        id: "jvm",
        name: "Java and Kotlin",
        about: "Gradle's and Maven's downloads and builds",
        patterns: &["~/.gradle/caches", "~/.gradle/wrapper", "~/.m2/repository", "**/.gradle"],
        default: true,
    },
    Group {
        id: "native",
        name: "C, C++, Zig, Haskell, Elixir, Dart",
        about: "Their build folders and caches",
        patterns: &["**/CMakeFiles", "**/.zig-cache", "**/zig-cache", "**/dist-newstyle", "**/.stack-work", "**/_build", "**/.dart_tool", "~/.ccache"],
        default: true,
    },
    Group {
        id: "containers",
        name: "Containers and Flatpaks",
        about: "Images and runtimes, all downloaded again (your apps' own data stays)",
        patterns: &["~/.local/share/containers", "~/.local/share/flatpak", "~/.local/share/docker"],
        default: true,
    },
    Group { id: "temporary", name: "Temporary files", about: "Editors' swap and backup files", patterns: &["**/*.swp", "**/*~", "**/.#*"], default: true },
    Group {
        id: "large-media",
        name: "Virtual machines' disks",
        about: "Disk images (large, and changed all the time while a machine runs)",
        patterns: &["**/*.qcow2", "**/*.vdi", "**/*.vmdk", "~/.local/share/gnome-boxes/images", "~/.local/share/libvirt/images"],
        default: false,
    },
];

/// The groups chosen by default.
pub fn default_groups() -> Vec<String> {
    GROUPS.iter().filter(|g| g.default).map(|g| g.id.to_owned()).collect()
}

pub struct Excludes {
    set: GlobSet,
    /// Folders marked as caches skipped.
    cache_tags: bool,
}

impl Excludes {
    /// The groups `groups` (by id) and the person's own `patterns`, `~/`
    /// as `home`.
    pub fn new(groups: &[String], patterns: &[String], home: &Path) -> Excludes {
        let mut b = GlobSetBuilder::new();
        let home = home.to_string_lossy().into_owned();
        let mut add = |p: &str| {
            let p = match p.strip_prefix("~/") {
                Some(rest) => format!("{home}/{rest}"),
                None => p.to_owned(),
            };
            // A folder's pattern takes in what's in it too.
            for q in [p.clone(), format!("{}/**", p.trim_end_matches('/'))] {
                if let Ok(g) = Glob::new(&q) {
                    b.add(g);
                }
            }
        };
        for g in GROUPS.iter().filter(|g| groups.iter().any(|id| id == g.id)) {
            for p in g.patterns {
                add(p);
            }
        }
        for p in patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
            add(p);
        }
        Excludes { set: b.build().unwrap_or_else(|_| GlobSet::empty()), cache_tags: true }
    }

    /// Nothing left out (the tests' and a restore's).
    pub fn none() -> Excludes {
        Excludes { set: GlobSet::empty(), cache_tags: false }
    }

    /// Whether `path` (a folder if `dir`) is left out.
    pub fn excluded(&self, path: &Path, dir: bool) -> bool {
        if self.set.is_match(path) {
            return true;
        }
        dir && self.cache_tags && is_cache(path)
    }
}

/// A folder marked as a cache (the Cache Directory Tagging Specification).
fn is_cache(dir: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(dir.join("CACHEDIR.TAG")) else { return false };
    let mut head = [0u8; 43];
    f.read_exact(&mut head).is_ok() && &head == b"Signature: 8a477f597d28d172789f06886806bc55"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_patterns() {
        let home = Path::new("/home/a");
        let e = Excludes::new(&default_groups(), &["~/Videos/raw".into(), "**/*.iso".into()], home);
        for p in [
            "/home/a/.cache/x",
            "/home/a/src/app/node_modules",
            "/home/a/src/app/node_modules/x/y.js",
            "/home/a/src/p/target/debug",
            "/home/a/Videos/raw/a.mov",
            "/home/a/dl/f.iso",
            "/home/a/.local/share/Trash/files/x",
        ] {
            assert!(e.excluded(Path::new(p), false), "{p}");
        }
        for p in ["/home/a/Documents/x.txt", "/home/a/src/p/target", "/home/a/src/p/src/main.rs", "/home/a/Videos/raw.mov"] {
            assert!(!e.excluded(Path::new(p), false), "{p}");
        }
        assert!(GROUPS.iter().all(|g| !g.patterns.is_empty() && !g.about.is_empty()));
    }
}
