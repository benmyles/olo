//! Talking to the system natively, in process: PAM (passwords), fprintd
//! (Touch ID), logind (power, inhibitors, backlight), the battery,
//! NetworkManager (Wi-Fi), BlueZ (Bluetooth), timedated (clock and time zone), polkit (the authentication agent), the D-Bus activation environment, dconf (the
//! settings GTK apps read) and the sound server (PulseAudio protocol, which
//! PipeWire speaks too). Everything here blocks, so call it from a worker
//! thread, never the UI thread.

pub mod audio;
pub mod auth;
pub mod backlight;
pub mod battery;
pub mod bluetooth;
pub mod dconf;
pub mod env;
pub mod fingerprint;
pub mod network;
pub mod polkit;
pub mod power;
pub mod time;

mod bus;

pub use zbus::Error;
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod live {
    /// Prints what this machine reports (read-only). Run with
    /// `cargo test -p tabula-system -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn status() {
        println!("wifi: {:?}", crate::network::wifi());
        println!("network: {:#?}", crate::network::status());
        println!("bluetooth: {:#?}", crate::bluetooth::status());
        println!("volume: {:?}", crate::audio::volume());
        println!("time: {:?}", crate::time::status());
        println!("zones: {:?}", crate::time::timezones().map(|z| z.len()));
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
