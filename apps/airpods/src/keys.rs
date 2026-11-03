//! The keys each pair's broadcasts are made with (`proto::Keys`), kept
//! so their LE broadcasts can be told apart from other people's and read
//! (levels while they're in another device's use, the case opened
//! nearby). Only for this person: `~/.local/state/olo/airpods-keys.json`,
//! readable by them alone.

use std::{collections::BTreeMap, path::PathBuf};

use olo_system::airpods::proto::Keys;

fn path() -> PathBuf {
    olo_xdg::state_dir().join("airpods-keys.json")
}

fn hex(k: &[u8; 16]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

#[allow(dead_code, reason = "read by the LE monitor, coming next")]
fn unhex(s: &str) -> Option<[u8; 16]> {
    let v: Vec<u8> = (0..s.len()).step_by(2).filter_map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect();
    v.try_into().ok()
}

type Stored = BTreeMap<String, BTreeMap<String, String>>;

/// Every pair's keys.
#[allow(dead_code, reason = "read by the LE monitor, coming next")]
pub fn all() -> BTreeMap<String, Keys> {
    let stored: Stored = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    stored.into_iter().map(|(addr, k)| (addr, Keys { irk: k.get("irk").and_then(|s| unhex(s)), encryption: k.get("encryption").and_then(|s| unhex(s)) })).collect()
}

/// Keep a pair's keys (if they're new).
pub fn remember(addr: &str, keys: &Keys) {
    let mut stored: Stored = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut k = BTreeMap::new();
    if let Some(irk) = &keys.irk {
        k.insert("irk".to_owned(), hex(irk));
    }
    if let Some(enc) = &keys.encryption {
        k.insert("encryption".to_owned(), hex(enc));
    }
    if stored.get(addr) == Some(&k) {
        return;
    }
    stored.insert(addr.to_owned(), k);
    // (The person's alone, as every file the kit makes is.)
    match olo_kit_base::store::write(&path(), &serde_json::to_vec_pretty(&stored).unwrap_or_default()) {
        Ok(()) => tracing::info!("airpods {addr}: kept their broadcast keys"),
        Err(e) => tracing::warn!("airpods: keeping their keys: {e}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hex_round_trips() {
        let k = [0xAB; 16];
        assert_eq!(super::unhex(&super::hex(&k)), Some(k));
        assert_eq!(super::unhex("zz"), None);
    }
}

/// What Olo keeps of each pair besides its keys: the modes press and hold
/// goes through (the AirPods don't say it unasked, and are told it again
/// at each connection, as other hosts may have changed it).
fn kept_path() -> PathBuf {
    olo_xdg::state_dir().join("airpods.json")
}

type Kept = BTreeMap<String, Vec<olo_protocol::airpods::ListeningMode>>;

pub fn press_and_hold(addr: &str) -> Option<Vec<olo_protocol::airpods::ListeningMode>> {
    let kept: Kept = olo_kit_base::store::load(&kept_path()).unwrap_or_default();
    kept.get(addr).cloned()
}

pub fn remember_press_and_hold(addr: &str, modes: &[olo_protocol::airpods::ListeningMode]) {
    let path = kept_path();
    let mut kept: Kept = olo_kit_base::store::load(&path).unwrap_or_default();
    kept.insert(addr.to_owned(), modes.to_vec());
    if let Err(e) = olo_kit_base::store::write(&path, &serde_json::to_vec_pretty(&kept).unwrap_or_default()) {
        tracing::warn!("airpods: keeping press and hold: {e}");
    }
}
