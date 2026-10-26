//! Lists the 1Password app's accounts (`--count` prints only how many).
fn main() {
    let accounts = onepassword::accounts::local();
    if std::env::args().any(|a| a == "--count") {
        println!("{} accounts, all named: {}", accounts.len(), accounts.iter().all(|a| !a.name.is_empty() && a.name != a.uuid));
        return;
    }
    for a in accounts {
        println!("{}\t{}\t{}\t{}", a.name, a.email, a.address, a.uuid);
    }
}
