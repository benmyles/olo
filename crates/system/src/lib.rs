//! Talking to the system natively, in process: PAM (passwords), fprintd
//! (Touch ID), logind (power, inhibitors, backlight), the battery,
//! NetworkManager (Wi-Fi), BlueZ (Bluetooth), timedated (clock and time zone), UDisks2 (disks and partitions), polkit (the authentication agent), the keyring's password prompts, the D-Bus activation environment, dconf (the
//! settings GTK apps read) and the sound server (PulseAudio protocol, which
//! PipeWire speaks too). Everything here blocks, so call it from a worker
//! thread, never the UI thread.

pub mod audio;
pub mod auth;
pub mod backlight;
pub mod battery;
pub mod bluetooth;
pub mod dconf;
pub mod disks;
pub mod env;
pub mod fingerprint;
pub mod github;
pub mod edid;
pub mod hid_monitor;
pub mod keyring_prompt;
pub mod network;
pub mod polkit;
pub mod power;
pub mod priority;
pub mod secrets;
pub mod systemd;
pub mod time;

mod bus;

pub use zbus::Error;
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod live {
    /// Prints what this machine reports (read-only). Run with
    /// `cargo test -p olo-system -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn status() {
        println!("wifi: {:?}", crate::network::wifi());
        println!("network: {:#?}", crate::network::status());
        println!("bluetooth: {:#?}", crate::bluetooth::status());
        let me = std::env::var("FPRINT_USER").unwrap_or_else(|_| crate::auth::current_user().unwrap_or_default());
        println!("fingers of {me}: {:?}", crate::fingerprint::fingers(&me));
        println!("volume: {:?}", crate::audio::volume());
        println!("time: {:?}", crate::time::status());
        println!("zones: {:?}", crate::time::timezones().map(|z| z.len()));
        if let Ok(s) = crate::disks::status() {
            for d in &s.disks {
                println!("disk {} {} {} {:?}: {:?}", d.device, d.kind(), d.size, d.table, d.volumes.iter().map(|v| (v.device.as_str(), v.content.as_str(), v.mounted())).collect::<Vec<_>>());
            }
            println!("filesystems: {:?}", s.filesystems);
        }
    }

    /// Writes the given dconf string key back (set DCONF_KEY and
    /// DCONF_VALUE to its current value to leave it unchanged).
    #[test]
    #[ignore]
    fn dconf_write() {
        let (Ok(k), Ok(v)) = (std::env::var("DCONF_KEY"), std::env::var("DCONF_VALUE")) else { return };
        crate::dconf::set_strings(&[(&k, &v)]).unwrap();
    }
}
