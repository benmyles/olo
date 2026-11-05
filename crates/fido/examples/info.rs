//! What each FIDO key plugged in says of itself, asked without a touch:
//! its channel, authenticatorGetInfo, a key agreement, and whether it
//! holds a credential of Olo's (one made up: none).
//!
//! `cargo run -p olo-fido --example info`

use std::{sync::atomic::AtomicBool, time::Duration};

use olo_fido::{cred, ctap::Wait, pin};

fn main() {
    let devices = olo_fido::devices();
    if devices.is_empty() {
        println!("No FIDO key is plugged in.");
    }
    for d in devices {
        println!("{} ({}, {:04x}:{:04x})", d.name, d.path.display(), d.vendor, d.product);
        let mut ch = match olo_fido::hid::Hidraw::open(&d).map_err(olo_fido::Error::from).and_then(olo_fido::ctap::Channel::open) {
            Ok(ch) => ch,
            Err(e) => {
                println!("  {e}");
                continue;
            }
        };
        println!("  CTAPHID: firmware {}.{}.{}, capabilities {:#04x} (CBOR: {})", ch.version.0, ch.version.1, ch.version.2, ch.capabilities, ch.cbor_capable());
        match ch.get_info() {
            Ok(info) => {
                println!("  versions: {}", info.versions.join(", "));
                println!("  extensions: {}", info.extensions.join(", "));
                println!("  options: {}", info.options.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", "));
                println!("  PIN protocols: {:?}; firmware {}", info.pin_protocols, info.firmware_text().unwrap_or_default());
                println!("  hmac-secret: {}; making a credential needs the PIN: {}", info.hmac_secret(), info.make_credential_needs_pin());
                let never = AtomicBool::new(false);
                let wait = Wait { cancel: &never, touch: &|| println!("  (wants a touch: not asked for here)"), limit: Duration::from_secs(3) };
                match pin::agree(&mut ch, info.pin_protocol(), &wait) {
                    Ok(s) => println!("  key agreement in protocol {}: ok", s.protocol),
                    Err(e) => println!("  key agreement: {e}"),
                }
                match cred::holds(&mut ch, &[0x42; 32], &wait) {
                    Ok(h) => println!("  holds a made-up credential: {h}"),
                    Err(e) => println!("  asking for a credential: {e}"),
                }
            }
            Err(e) => println!("  GetInfo: {e}"),
        }
    }
}
