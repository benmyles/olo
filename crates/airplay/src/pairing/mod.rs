//! Pairing with a receiver, as AirPlay 2 has it: HomeKit's. Once, with
//! the code the receiver shows on its screen ([`Setup`]: pair-setup),
//! which leaves each side with the other's long-term key
//! ([`Credentials`]); then at every connection each proves it holds its
//! key ([`verify`]: pair-verify), and what follows on that connection is
//! enciphered with keys of that proving ([`Cipher`]).
//!
//! What the messages are is HomeKit's Accessory Protocol's, published;
//! how AirPlay carries them (its paths and headers) is as pyatv, which
//! pairs with Apple TVs, has found them. Tests pair with a receiver of
//! their own ([`fake`]); nothing here has yet been tried on a real one,
//! which is the person's to do, at its screen.

mod channel;
#[doc(hidden)]
pub mod fake;
mod hap;
mod http;
pub mod srp;
pub mod tlv8;

pub use channel::{Cipher, NotTheirs};
pub use hap::{Credentials, Error, Paired, Post, Session, Setup, verify};
pub use http::Http;

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::*;

    /// Paired with the code it shows: each side has the other's key and
    /// name, and the receiver was asked to show the code first.
    #[test]
    fn paired_with_the_code_it_shows() {
        let mut tv = fake::Receiver::new("3939");
        let setup = Setup::begin(&mut tv).unwrap();
        assert!(tv.shown);
        let paired = setup.finish(&mut tv, " 3939\n", "Ben's Olo").unwrap();
        assert!(paired.signed);
        let c = &paired.credentials;
        assert_eq!(c.receiver_id, tv.id);
        assert_eq!(tv.paired.len(), 1);
        assert_eq!(tv.paired[0].0, c.our_id);
        assert_eq!(c.our_id.len(), 36);
        // What it lists us as: {"name": "Ben's Olo"}, packed.
        assert_eq!(tv.paired[0].2, [&[0xe1, 0x44][..], b"name", &[0x49], b"Ben's Olo"].concat());
        // Kept in a line, and read back.
        assert_eq!(Credentials::from_line(&c.to_line()).as_ref(), Some(c));
        assert_eq!(Credentials::from_line("not:a:pairing"), None);
        assert!(!format!("{c:?}").contains(&c.to_line()[65..80]), "its secret isn't printed");
    }

    #[test]
    fn the_wrong_code_pairs_nothing() {
        let mut tv = fake::Receiver::new("3939");
        let setup = Setup::begin(&mut tv).unwrap();
        assert_eq!(setup.finish(&mut tv, "0000", "x").unwrap_err(), Error::WrongCode);
        assert!(tv.paired.is_empty());
    }

    /// A receiver that doesn't sign for its key as HomeKit says is still
    /// paired with (the code was proved both ways), and it's said.
    #[test]
    fn one_that_doesnt_sign_is_said() {
        let mut tv = fake::Receiver::new("1234");
        tv.signs = false;
        let paired = Setup::begin(&mut tv).unwrap().finish(&mut tv, "1234", "x").unwrap();
        assert!(!paired.signed);
        assert!(verify(&mut tv, &paired.credentials).is_ok());
    }

    /// Each connection after: both prove themselves and have the same
    /// keys; something else answering for the receiver is refused; and a
    /// receiver that has forgotten us says so.
    #[test]
    fn proved_at_each_connection() {
        let mut tv = fake::Receiver::new("3939");
        let c = Setup::begin(&mut tv).unwrap().finish(&mut tv, "3939", "x").unwrap().credentials;
        let session = verify(&mut tv, &c).unwrap();
        let (mut ours, mut theirs) = (session.control(), tv.control().unwrap());
        assert_eq!(theirs.open(&ours.seal(b"GET /info")).unwrap(), b"GET /info");
        assert_eq!(ours.open(&theirs.seal(b"200 OK")).unwrap(), b"200 OK");
        // Another receiver saying it is that one (its name, not its key).
        let mut other = fake::Receiver::new("3939");
        other.paired = tv.paired.clone();
        assert_eq!(verify(&mut other, &c).err(), Some(Error::Imposter));
        // The receiver reset, or we removed from it: it has the key we
        // know it by no more (an imposter, as far as can be told), or it
        // has it and not us.
        let known = tv.paired.clone();
        tv.paired.clear();
        assert_eq!(verify(&mut tv, &c).err(), Some(Error::Forgotten));
        tv.paired = known;
        assert!(verify(&mut tv, &c).is_ok());
        tv.reset();
        assert_eq!(verify(&mut tv, &c).err(), Some(Error::Imposter));
    }

    /// The same over a connection, as with a real receiver: HTTP for the
    /// pairing and the proving, then enciphered.
    #[test]
    fn over_a_connection() {
        let tv = Arc::new(Mutex::new(fake::Receiver::new("2468")));
        let wait = Duration::from_secs(5);
        let mut http = Http::connect(fake::serve(tv.clone()), wait).unwrap();
        let c = Setup::begin(&mut http).unwrap().finish(&mut http, "2468", "Olo").unwrap().credentials;
        assert_eq!(tv.lock().unwrap().paired.len(), 1);
        // Not proved yet: it answers nothing else.
        assert_eq!(http.request("GET", "/info", "text/plain", &[]).unwrap().0, 470);
        let session = verify(&mut http, &c).unwrap();
        http.encipher(session.control());
        assert_eq!(http.request("GET", "/info", "text/plain", &[]).unwrap(), (200, b"a receiver of the test's own".to_vec()));
        assert_eq!(http.request("GET", "/info", "text/plain", &[]).unwrap().0, 200);
        // A new connection: proved again from what was kept, no code.
        let mut again = Http::connect(fake::serve(tv.clone()), wait).unwrap();
        let session = verify(&mut again, &Credentials::from_line(&c.to_line()).unwrap()).unwrap();
        again.encipher(session.control());
        assert_eq!(again.request("GET", "/info", "text/plain", &[]).unwrap().0, 200);
    }

    /// Nothing listening, or something that isn't a receiver: said, not
    /// waited on for ever.
    #[test]
    fn nothing_there() {
        let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(matches!(Http::connect(gone, Duration::from_millis(500)), Err(Error::Io(_))));
    }
}
