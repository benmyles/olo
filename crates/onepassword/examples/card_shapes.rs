//! What 1Password's credit cards hold, as shapes only: for each card in
//! each account signed in to the 1Password app, every field's id, label,
//! type and section, and its value as a shape (digits as N, letters as A,
//! anything else as it is: "NN/NNNN"), never a value, a title or a name.
//! The 1Password app asks the person to allow it first.
//!
//!   cargo run -p olo-onepassword --example card_shapes

use onepassword::{Client, ItemCategory};
use serde_json::Value;

fn shape(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                'N'
            } else if c.is_alphabetic() {
                'A'
            } else {
                c
            }
        })
        .collect()
}

/// `v` with every string made its shape, keys kept.
fn shaped(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(shape(s)),
        Value::Array(a) => Value::Array(a.iter().map(shaped).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), shaped(v))).collect()),
        other => other.clone(),
    }
}

fn main() {
    // `VAULT ITEM`: that item alone (its shapes), in whichever account has it.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [vault, item] = &args[..] {
        for account in onepassword::accounts::local() {
            let Ok(client) = Client::builder().desktop_app_integration(&account.uuid).integration_info("Olo card probe", env!("CARGO_PKG_VERSION")).build() else { continue };
            let Ok(found) = client.items().get(vault, item) else { continue };
            println!("found in the account at {}: category {:?}", account.address, found.category);
            for f in &found.fields {
                println!("  field id={:?} label={:?} type={:?} section={:?} value={:?}", f.id, f.title, f.field_type, f.section_id.as_deref().map(shape), shape(&f.value));
            }
            // Each field 1Password won't give an app as a value, asked for
            // by its secret reference instead.
            for f in found.fields.iter().filter(|f| f.field_type == onepassword::ItemFieldType::Unsupported) {
                let reference = format!("op://{vault}/{item}/{}", f.id);
                match client.secrets().resolve(&reference) {
                    Ok(v) => println!("  resolved {:?}: {:?}", f.id, shape(&v)),
                    Err(e) => println!("  resolving {:?} failed: {e}", f.id),
                }
            }
            let refs: Vec<String> = found.fields.iter().filter(|f| f.field_type == onepassword::ItemFieldType::Unsupported).map(|f| format!("op://{vault}/{item}/{}", f.id)).collect();
            if !refs.is_empty() {
                match client.secrets().resolve_all(&refs) {
                    Ok(r) => {
                        for (k, v) in &r.individual_responses {
                            println!("  resolved at once {:?}: {:?}", k.rsplit('/').next().unwrap_or_default(), v.content.as_ref().map(|c| shape(&c.secret)));
                        }
                    }
                    Err(e) => println!("  resolving at once failed: {e}"),
                }
            }
            if let Ok(raw) = client.items().get_json(vault, item) {
                let expiry = raw.get("fields").and_then(Value::as_array).and_then(|fs| fs.iter().find(|f| f.get("id").and_then(Value::as_str) == Some("expiry")).cloned());
                println!("  raw expiry: {}", expiry.as_ref().map(shaped).unwrap_or(Value::Null));
            }
            return;
        }
        println!("no account here has that item");
        return;
    }
    let accounts = onepassword::accounts::local();
    if accounts.is_empty() {
        eprintln!("no 1Password account signed in here");
        return;
    }
    for (n, account) in accounts.iter().enumerate() {
        println!("account {}", n + 1);
        let client = match Client::builder().desktop_app_integration(&account.uuid).integration_info("Olo card probe", env!("CARGO_PKG_VERSION")).build() {
            Ok(c) => c,
            Err(e) => {
                println!("  couldn't connect: {e}");
                continue;
            }
        };
        let vaults = match client.vaults().list(None) {
            Ok(v) => v,
            Err(e) => {
                println!("  couldn't list vaults: {e}");
                continue;
            }
        };
        let mut card = 0;
        for v in vaults {
            let Ok(items) = client.items().list(&v.id, &[]) else { continue };
            for o in items.iter().filter(|o| o.category == ItemCategory::CreditCard) {
                card += 1;
                println!("  card {card}");
                match client.items().get(&v.id, &o.id) {
                    Ok(item) => {
                        for f in &item.fields {
                            println!("    field id={:?} label={:?} type={:?} section={:?} value={:?}", f.id, f.title, f.field_type, f.section_id.as_deref().map(shape), shape(&f.value));
                        }
                        let sections: Vec<String> = item.sections.iter().map(|s| format!("{:?}:{:?}", shape(&s.id), s.title)).collect();
                        println!("    sections {sections:?}");
                    }
                    Err(e) => println!("    couldn't read it: {e}"),
                }
                if let Ok(raw) = client.items().get_json(&v.id, &o.id)
                    && let Some(fields) = raw.get("fields").and_then(Value::as_array)
                {
                    for f in fields {
                        let keys: Vec<&str> = f.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
                        println!("    raw field keys {keys:?}: {}", shaped(f));
                    }
                }
            }
        }
        if card == 0 {
            println!("  no credit cards");
        }
        // The same cards fetched as Passwords' sync fetches them: many at
        // once (ItemsGetAll). Each card's expiry shape, as it came.
        let Ok(vaults) = client.vaults().list(None) else { continue };
        let mut n = 0;
        for v in vaults {
            let Ok(items) = client.items().list(&v.id, &[]) else { continue };
            let ids: Vec<String> = items.iter().filter(|o| o.category == ItemCategory::CreditCard).map(|o| o.id.clone()).collect();
            if ids.is_empty() {
                continue;
            }
            match client.items().get_all(&v.id, &ids) {
                Ok(resp) => {
                    for r in &resp.individual_responses {
                        n += 1;
                        match &r.content {
                            Some(item) => {
                                let e = item.fields.iter().find(|f| f.id == "expiry");
                                println!("  get_all card {n}: expiry {:?}", e.map(|f| (f.field_type, shape(&f.value))));
                            }
                            None => println!("  get_all card {n}: no content ({:?})", r.error.as_ref().map(|_| "an error")),
                        }
                    }
                }
                Err(e) => println!("  get_all failed: {e}"),
            }
        }
    }
}
