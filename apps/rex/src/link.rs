//! Olo's helper on a Rex host (`olo-rex-link`: HACKING, Rex, "A helper
//! on the host"), as the app carries it: a build for each kind of host
//! (`build.rs`), and the name of this build, by which the app finds its
//! own on a host (`olo_rex_client::helper`).

use olo_rex_client::helper::Build;

/// The builds carried. One that couldn't be built is empty, and a host
/// of its kind goes without.
pub const BUILDS: [Build; 2] = [
    Build { system: "Linux", machines: &["x86_64", "amd64"], bytes: include_bytes!(env!("OLO_REX_LINK_X86_64")) },
    Build { system: "Linux", machines: &["aarch64", "arm64"], bytes: include_bytes!(env!("OLO_REX_LINK_AARCH64")) },
];

/// This build's name: the same for the same helper, whatever the app.
pub const ID: &str = env!("OLO_REX_LINK_ID");

/// Whether a helper is carried at all.
pub fn carried() -> bool {
    BUILDS.iter().any(|b| !b.bytes.is_empty())
}

/// Whether the helpers carried keep files in step (`olo-rex-link sync`:
/// built where zig compiles the engine's C, `build.rs`).
pub fn syncs() -> bool {
    env!("OLO_REX_LINK_SYNC") == "1"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is carried is what `build.rs` built: a static program for
    /// each machine (an ELF file that names no interpreter).
    #[test]
    fn a_static_helper_is_carried_for_each_kind_of_host() {
        assert_eq!(ID.len(), 16);
        for (build, machine) in BUILDS.iter().zip([62u16, 183]) {
            assert!(build.bytes.len() > 100_000, "the helper for {:?} was built", build.machines);
            assert_eq!(&build.bytes[..4], b"\x7fELF");
            assert_eq!(u16::from_le_bytes([build.bytes[18], build.bytes[19]]), machine, "its machine");
            // (A static program asks for no interpreter: no PT_INTERP.)
            let phoff = u64::from_le_bytes(build.bytes[32..40].try_into().unwrap()) as usize;
            let (phentsize, phnum) = (u16::from_le_bytes([build.bytes[54], build.bytes[55]]) as usize, u16::from_le_bytes([build.bytes[56], build.bytes[57]]) as usize);
            let interp = (0..phnum).any(|i| u32::from_le_bytes(build.bytes[phoff + i * phentsize..phoff + i * phentsize + 4].try_into().unwrap()) == 3);
            assert!(!interp, "the helper for {:?} is static", build.machines);
        }
        assert!(carried());
        assert!(ID.chars().all(|c| c.is_ascii_hexdigit()), "a name the host's shell takes as it is");
    }

    /// What goes to a host says nothing of the computer it was built
    /// on: no folder of its builder's is in it (`build.rs` names them
    /// as anybody's).
    #[test]
    fn no_path_of_this_computer_is_in_what_goes_to_a_host() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().display().to_string();
        let mut mine = vec![root];
        mine.extend(["HOME", "CARGO_HOME", "RUSTUP_HOME"].iter().filter_map(|var| std::env::var(var).ok()).filter(|dir| dir.len() > 3));
        for build in &BUILDS {
            for dir in &mine {
                assert!(!build.bytes.windows(dir.len()).any(|w| w == dir.as_bytes()), "the helper for {:?} holds a path of this computer", build.machines);
            }
        }
    }
}
