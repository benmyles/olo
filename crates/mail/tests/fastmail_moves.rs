//! An upgrade carries Mail's own Fastmail accounts over to Integrations ▸
//! Fastmail by itself (Ben, 2026-11-18: "make sure it auto migrates the
//! existing storage/creds on upgrade"): read once, the list Mail kept
//! has its Fastmail account moved, app password and all, its Gmail one
//! left, and the account signs in with the password moved. A test of its
//! own: it names every list by the environment, which is the process's.

use olo_mail::{Account, Auth, Provider};

#[test]
fn a_fastmail_account_moves_with_its_password() {
    let dir = std::path::PathBuf::from(format!("/var/tmp/olo-mail-fastmail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("mail-secrets")).unwrap();
    // SAFETY: the one test in this binary.
    unsafe {
        std::env::set_var("OLO_MAIL_ACCOUNTS", dir.join("mail.json"));
        std::env::set_var("OLO_MAIL_SECRETS", dir.join("mail-secrets"));
        std::env::set_var("OLO_GOOGLE_ACCOUNTS", dir.join("google.json"));
        std::env::set_var("OLO_FASTMAIL_ACCOUNTS", dir.join("fastmail.json"));
        std::env::set_var("OLO_FASTMAIL_SECRETS", dir.join("fastmail-secrets"));
    }
    // As Mail kept them before: a Gmail account and a Fastmail one, each
    // with its app password.
    let gmail = Account { provider: Provider::Gmail, address: "ann@example.com".into() };
    let fastmail = Account { provider: Provider::Fastmail, address: "ben@example.org".into() };
    olo_mail::save_accounts(&[gmail.clone(), fastmail.clone()]).unwrap();
    olo_mail::keep_password("ann@example.com", "gmail-app-password").unwrap();
    olo_mail::keep_password("ben@example.org", "fastmail-app-password").unwrap();

    // A Gmail one connected under Google since (Mail unticked there), with
    // its old app password: let go of, not read with that password.
    let under_google = Account { provider: Provider::Gmail, address: "cy@example.com".into() };
    let mut own = olo_mail::own_accounts();
    own.push(under_google.clone());
    olo_mail::save_accounts(&own).unwrap();
    olo_mail::keep_password("cy@example.com", "old-gmail-app-password").unwrap();
    std::fs::write(dir.join("google.json"), r#"[{"address":"cy@example.com","services":["calendar"],"scopes":["https://www.googleapis.com/auth/calendar"]}]"#).unwrap();

    // Read once: the Fastmail account comes from Integrations' list now.
    assert_eq!(olo_mail::accounts(), [fastmail.clone(), gmail.clone()]);
    assert_eq!(olo_mail::password("cy@example.com"), None);
    assert_eq!(olo_fastmail::accounts(), [olo_fastmail::Account { address: "ben@example.org".into(), services: vec![olo_fastmail::Service::Mail] }]);
    assert_eq!(olo_fastmail::password("ben@example.org").as_deref(), Some("fastmail-app-password"));
    // Mail's own list keeps the Gmail one alone, and let go of the
    // Fastmail one's password.
    assert_eq!(olo_mail::own_accounts(), [gmail.clone()]);
    assert_eq!(olo_mail::password("ben@example.org"), None);
    assert_eq!(olo_mail::password("ann@example.com").as_deref(), Some("gmail-app-password"));
    // It signs in with the password moved.
    assert_eq!(olo_mail::auth_of(&fastmail), Ok(Auth::Password("fastmail-app-password".into())));
    // Read again: the same, nothing moved twice.
    assert_eq!(olo_mail::accounts(), [fastmail, gmail]);
    let _ = std::fs::remove_dir_all(&dir);
}
