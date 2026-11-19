//! An upgrade carries Calendar's own Fastmail accounts over to
//! Integrations ▸ Fastmail by itself: read once, its Fastmail account is
//! moved, app password and all, another CalDAV server's left, and the
//! account signs in with the password moved. A test of its own: it names
//! every list by the environment, which is the process's.

use olo_calendar::account::{self, Account, Provider, Secret};

#[test]
fn a_fastmail_account_moves_with_its_password() {
    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-calendar-fastmail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: the one test in this binary.
    unsafe {
        std::env::set_var("OLO_CALENDAR_ACCOUNTS", dir.join("calendar.json"));
        std::env::set_var("OLO_CALENDAR_SECRETS", dir.join("calendar-secrets"));
        std::env::set_var("OLO_FASTMAIL_ACCOUNTS", dir.join("fastmail.json"));
        std::env::set_var("OLO_FASTMAIL_SECRETS", dir.join("fastmail-secrets"));
    }
    let fastmail = Account { provider: Provider::Fastmail, address: "ben@example.org".into(), server: String::new() };
    let other = Account { provider: Provider::Other, address: "ben".into(), server: "https://cloud.example.net/remote.php/dav/".into() };
    account::save_accounts(&[fastmail.clone(), other.clone()]).unwrap();
    account::keep_secret(&fastmail, &Secret::Password("fastmail-app-password".into())).unwrap();
    account::keep_secret(&other, &Secret::Password("nextcloud-password".into())).unwrap();

    assert_eq!(account::accounts(), [fastmail.clone(), other.clone()]);
    assert_eq!(olo_fastmail::accounts(), [olo_fastmail::Account { address: "ben@example.org".into(), services: vec![olo_fastmail::Service::Calendar] }]);
    assert_eq!(olo_fastmail::password("ben@example.org").as_deref(), Some("fastmail-app-password"));
    assert_eq!(account::own_accounts(), [other.clone()]);
    assert_eq!(account::secret(&fastmail), Some(Secret::Password("fastmail-app-password".into())));
    assert_eq!(account::secret(&other), Some(Secret::Password("nextcloud-password".into())));
    assert_eq!(account::accounts(), [fastmail, other]);
    let _ = std::fs::remove_dir_all(&dir);
}
