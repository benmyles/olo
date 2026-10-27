//! The SDK against the stand-in app, over the real socket protocol.

use std::sync::atomic::{AtomicU32, Ordering};

use onepassword::{
    Client, Error, ItemCategory, ItemCreateParams, ItemField, ItemFieldType, ItemListFilter, ItemListFilterByState,
    ItemState, PasswordRecipe, Secrets,
    fake::{Fake, sample},
};

const ACCOUNT: &str = "Test Account";

fn fake() -> (Fake, Client) {
    static N: AtomicU32 = AtomicU32::new(0);
    let socket = format!("olo-onepassword-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
    let fake = Fake::start(&socket, sample(ACCOUNT)).unwrap();
    let client = Client::builder()
        .desktop_app_integration(ACCOUNT)
        .integration_info("SDK tests", "1")
        .desktop_socket(&socket)
        .build()
        .unwrap();
    (fake, client)
}

#[test]
fn vaults_and_items() {
    let (_fake, client) = fake();
    let vaults = client.vaults().list(None).unwrap();
    assert_eq!(vaults.iter().map(|v| v.title.as_str()).collect::<Vec<_>>(), ["Personal", "Household"]);
    let personal = &vaults[0].id;
    let items = client.items().list(personal, &[]).unwrap();
    assert!(items.iter().any(|i| i.title == "GitHub" && i.category == ItemCategory::Login));
    let github = items.iter().find(|i| i.title == "GitHub").unwrap();
    let item = client.items().get(personal, &github.id).unwrap();
    assert_eq!(item.username(), Some("jane.doe"));
    assert_eq!(item.password(), Some("tusk-Ample-dimly-9-Rover"));
    let otp = item.fields.iter().find(|f| f.field_type == ItemFieldType::Totp).unwrap();
    assert_eq!(otp.otp().and_then(|o| o.code.as_deref()).map(str::len), Some(6));
    assert_eq!(item.websites[0].url, "https://github.com");
}

#[test]
fn create_edit_archive_delete() {
    let (_fake, client) = fake();
    let vault = client.vaults().list(None).unwrap()[0].id.clone();
    let mut params = ItemCreateParams::new(ItemCategory::Login, &vault, "New Login");
    params.fields = vec![
        ItemField::new("username", "username", ItemFieldType::Text, "me"),
        ItemField::new("password", "password", ItemFieldType::Concealed, "s3cret"),
    ];
    let mut item = client.items().create(&params).unwrap();
    assert_eq!(item.version, 1);
    item.field_mut("password").unwrap().value = "better-s3cret".into();
    let saved = client.items().put(&item).unwrap();
    assert_eq!(saved.version, 2);
    assert_eq!(client.items().get(&vault, &item.id).unwrap().password(), Some("better-s3cret"));
    // A stale version is refused.
    assert!(matches!(client.items().put(&item), Err(Error::Core(m)) if m.contains("version")));

    client.items().archive(&vault, &item.id).unwrap();
    assert!(!client.items().list(&vault, &[]).unwrap().iter().any(|i| i.id == item.id));
    let archived = client
        .items()
        .list(&vault, &[ItemListFilter::ByState(ItemListFilterByState { active: false, archived: true })])
        .unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].state, ItemState::Archived);
    client.items().delete(&vault, &item.id).unwrap();
    assert!(client.items().get(&vault, &item.id).is_err());
}

#[test]
fn secrets() {
    let (_fake, client) = fake();
    assert_eq!(client.secrets().resolve("op://Personal/GitHub/password").unwrap(), "tusk-Ample-dimly-9-Rover");
    assert_eq!(client.secrets().resolve("op://Personal/GitHub/Security/one-time password?attribute=otp").unwrap().len(), 6);
    assert!(matches!(client.secrets().resolve("op://Personal/GitHub"), Err(Error::InvalidReference(_))));
    let all = client
        .secrets()
        .resolve_all(&["op://Household/Build box/password".into(), "op://Personal/Nope/password".into()])
        .unwrap();
    let ok = all.individual_responses["op://Household/Build box/password"].clone().into_result().unwrap().unwrap();
    assert_eq!(ok.secret, "Oxbow-Lantern-42");
    assert!(all.individual_responses["op://Personal/Nope/password"].clone().into_result().is_err());
    // No account needed for these.
    Secrets::validate_secret_reference("op://a/b/c").unwrap();
    let p = Secrets::generate_password(PasswordRecipe::Pin { length: 8 }).unwrap();
    assert_eq!(p.password.len(), 8);
}

#[test]
fn files_travel_in_chunks() {
    let (fake, client) = fake();
    let vault = client.vaults().list(None).unwrap()[0].id.clone();
    // Bigger than a chunk both ways (base64 makes it bigger still).
    let content: Vec<u8> = (0..1_300_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let mut params = ItemCreateParams::new(ItemCategory::Document, &vault, "Scan");
    params.document = Some(onepassword::DocumentCreateParams { name: "scan.bin".into(), content: content.clone() });
    let item = client.items().create(&params).unwrap();
    let attributes = item.document.clone().unwrap();
    assert_eq!(attributes.size as usize, content.len());
    assert_eq!(client.items().files().read(&vault, &item.id, &attributes).unwrap(), content);
    assert!(fake.requests.load(Ordering::Relaxed) >= 4);
}

#[test]
fn expired_sessions_are_renewed() {
    let (fake, client) = fake();
    fake.expire_next.store(true, Ordering::Release);
    // One retry on a fresh client, invisibly.
    assert_eq!(client.vaults().list(None).unwrap().len(), 2);
}

#[test]
fn errors() {
    let (_fake, client) = fake();
    assert!(matches!(client.items().get("nope", "nope"), Err(Error::Core(m)) if m.contains("vault not found")));
    // Nothing listening.
    let missing = Client::builder().desktop_app_integration(ACCOUNT).desktop_socket("olo-onepassword-nobody").build();
    assert!(matches!(missing, Err(Error::DesktopAppUnavailable)));
    // A different account.
    let socket = _fake_socket(&client);
    let wrong = Client::builder().desktop_app_integration("Someone Else").desktop_socket(socket).build();
    assert!(matches!(wrong, Err(Error::Core(m)) if m.contains("Someone Else")));
    assert!(matches!(Client::builder().build(), Err(Error::InvalidArgument(_))));
}

fn _fake_socket(_client: &Client) -> String {
    // The fixture's sockets are numbered; start one more for this check.
    static N: AtomicU32 = AtomicU32::new(0);
    let socket = format!("olo-onepassword-test-acct-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
    std::mem::forget(Fake::start(&socket, sample(ACCOUNT)).unwrap());
    socket
}
