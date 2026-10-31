//! Signing in (Preferences ▸ Integrations ▸ Telegram): with a phone
//! number and the code Telegram sends, or by scanning a QR code with a
//! phone already signed in; then the two-step password if the account
//! has one. On a thread and runtime of its own; what happens comes back
//! as [`Step`]s. Once signed in, the session goes to the keyring and
//! Telegram (`olo-telegram`) can use it.

use std::{sync::Arc, time::Duration};

use grammers_client::{Client, client::SignInError};
use grammers_session::Session;
use grammers_tl_types as tl;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::{
    account::{self, Api, Store},
    model::Me,
    net::{connect, explain},
};

/// Where signing in is.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Connecting to Telegram.
    Starting,
    /// Waiting for a phone number (or `qr`).
    Phone,
    /// Waiting for the code sent to `phone`.
    Code { phone: String },
    /// Waiting for the two-step password.
    Password { hint: Option<String> },
    /// Show this as a QR code to scan with Telegram on a phone
    /// (Settings ▸ Devices ▸ Link Desktop Device); it changes every 30 s.
    Qr { url: String },
    /// Asking Telegram (a step was taken).
    Working,
    Done(Me),
    /// It went wrong and can't go on (start again).
    Failed(String),
}

/// What happened: the step it's at, and a problem with the last answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub step: Step,
    pub problem: Option<String>,
}

enum Ask {
    Phone(String),
    Code(String),
    Password(String),
    Qr,
}

/// A sign-in under way; dropping it gives up.
pub struct SignIn {
    to: UnboundedSender<Ask>,
}

impl SignIn {
    /// Start signing in with `api`; `on` hears each step.
    pub fn start(api: Api, on: impl Fn(Update) + Send + Sync + 'static) -> SignIn {
        let (to, from) = tokio::sync::mpsc::unbounded_channel();
        let on: Arc<dyn Fn(Update) + Send + Sync> = Arc::new(on);
        let _ = std::thread::Builder::new().name("telegram-signin".into()).spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("a runtime");
            rt.block_on(run(api, from, on));
        });
        SignIn { to }
    }

    pub fn phone(&self, number: &str) {
        let digits: String = number.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
        let _ = self.to.send(Ask::Phone(digits));
    }

    pub fn code(&self, code: &str) {
        let _ = self.to.send(Ask::Code(code.chars().filter(char::is_ascii_digit).collect()));
    }

    pub fn password(&self, password: &str) {
        let _ = self.to.send(Ask::Password(password.to_owned()));
    }

    /// Sign in with a QR code instead.
    pub fn qr(&self) {
        let _ = self.to.send(Ask::Qr);
    }
}

fn me_of(u: &tl::enums::User) -> Me {
    match u {
        tl::enums::User::User(u) => Me {
            id: u.id,
            name: [u.first_name.as_deref(), u.last_name.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" "),
            phone: u.phone.as_ref().map(|p| format!("+{p}")),
            username: u.username.clone(),
            photo: match &u.photo {
                Some(tl::enums::UserProfilePhoto::Photo(p)) => Some(p.photo_id),
                _ => None,
            },
        },
        tl::enums::User::Empty(e) => Me { id: e.id, ..Me::default() },
    }
}

/// `tg://login?token=…`, the token in unpadded base64url.
fn qr_url(token: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut s = String::from("tg://login?token=");
    for chunk in token.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..chunk.len() + 1 {
            s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    s
}

async fn run(api: Api, mut from: UnboundedReceiver<Ask>, on: Arc<dyn Fn(Update) + Send + Sync>) {
    let say = |step: Step, problem: Option<String>| on(Update { step, problem });
    say(Step::Starting, None);
    let store = Store::fresh();
    let (client, _updates) = connect(store.clone(), &api);
    say(Step::Phone, None);
    let mut login = None;
    let mut password = None;
    let mut qr = false;
    loop {
        // While a QR code shows, ask again every few seconds whether it was scanned.
        let ask = if qr && password.is_none() {
            match tokio::time::timeout(Duration::from_secs(3), from.recv()).await {
                Ok(a) => a,
                Err(_) => {
                    match qr_step(&client, &api, &store).await {
                        Ok(QrNow::Show(url)) => say(Step::Qr { url }, None),
                        Ok(QrNow::Done(me)) => return finish(&store, me, &say).await,
                        Ok(QrNow::Password(token)) => {
                            say(Step::Password { hint: token.hint().map(str::to_owned) }, None);
                            password = Some(token);
                        }
                        Err(e) => say(Step::Qr { url: String::new() }, Some(e)),
                    }
                    continue;
                }
            }
        } else {
            from.recv().await
        };
        let Some(ask) = ask else { return };
        match ask {
            Ask::Phone(phone) => {
                qr = false;
                say(Step::Working, None);
                match client.request_login_code(&phone, &api.hash).await {
                    Ok(t) => {
                        login = Some(t);
                        say(Step::Code { phone }, None);
                    }
                    Err(e) => say(Step::Phone, Some(explain(&e))),
                }
            }
            Ask::Code(code) => {
                let Some(t) = &login else { continue };
                say(Step::Working, None);
                match client.sign_in(t, &code).await {
                    Ok(user) => return finish(&store, me_of(&user.raw), &say).await,
                    Err(SignInError::PasswordRequired(token)) => {
                        say(Step::Password { hint: token.hint().map(str::to_owned) }, None);
                        password = Some(token);
                    }
                    Err(SignInError::InvalidCode) => say(Step::Code { phone: String::new() }, Some("That code isn’t right.".into())),
                    Err(SignInError::SignUpRequired) => say(Step::Failed("This number has no Telegram account yet. Sign up with Telegram on a phone first.".into()), None),
                    Err(SignInError::Other(e)) => say(Step::Code { phone: String::new() }, Some(explain(&e))),
                    Err(e) => say(Step::Code { phone: String::new() }, Some(e.to_string())),
                }
            }
            Ask::Password(pw) => {
                let Some(token) = password.take() else { continue };
                say(Step::Working, None);
                match client.check_password(token, pw.as_bytes()).await {
                    Ok(user) => return finish(&store, me_of(&user.raw), &say).await,
                    Err(SignInError::InvalidPassword(token)) => {
                        say(Step::Password { hint: token.hint().map(str::to_owned) }, Some("That password isn’t right.".into()));
                        password = Some(token);
                    }
                    Err(e) => {
                        // The token is spent: get a new one.
                        let why = match &e {
                            SignInError::Other(e) => explain(e),
                            e => e.to_string(),
                        };
                        match fresh_password(&client).await {
                            Ok(token) => {
                                say(Step::Password { hint: token.hint().map(str::to_owned) }, Some(why));
                                password = Some(token);
                            }
                            Err(e) => say(Step::Failed(e), None),
                        }
                    }
                }
            }
            Ask::Qr => {
                qr = true;
                login = None;
                match qr_step(&client, &api, &store).await {
                    Ok(QrNow::Show(url)) => say(Step::Qr { url }, None),
                    Ok(QrNow::Done(me)) => return finish(&store, me, &say).await,
                    Ok(QrNow::Password(token)) => {
                        say(Step::Password { hint: token.hint().map(str::to_owned) }, None);
                        password = Some(token);
                    }
                    Err(e) => say(Step::Phone, Some(e)),
                }
            }
        }
    }
}

enum QrNow {
    Show(String),
    Done(Me),
    Password(grammers_client::client::PasswordToken),
}

async fn fresh_password(client: &Client) -> Result<grammers_client::client::PasswordToken, String> {
    let tl::enums::account::Password::Password(p) = client.invoke(&tl::functions::account::GetPassword {}).await.map_err(|e| explain(&e))?;
    Ok(grammers_client::client::PasswordToken::new(p))
}

/// Ask for a login token: a new one to show, or the account it was
/// scanned into.
async fn qr_step(client: &Client, api: &Api, store: &Arc<Store>) -> Result<QrNow, String> {
    use tl::enums::auth::LoginToken as T;
    let r = client.invoke(&tl::functions::auth::ExportLoginToken { api_id: api.id, api_hash: api.hash.clone(), except_ids: Vec::new() }).await;
    let r = match r {
        Ok(r) => r,
        Err(e) if matches!(&e, grammers_client::InvocationError::Rpc(r) if r.name == "SESSION_PASSWORD_NEEDED") => return Ok(QrNow::Password(fresh_password(client).await?)),
        Err(e) => return Err(explain(&e)),
    };
    let r = match r {
        T::MigrateTo(m) => {
            store.set_home_dc_id(m.dc_id).await.map_err(|e| e.to_string())?;
            match client.invoke_in_dc(m.dc_id, &tl::functions::auth::ImportLoginToken { token: m.token }).await {
                Ok(r) => r,
                Err(e) if matches!(&e, grammers_client::InvocationError::Rpc(r) if r.name == "SESSION_PASSWORD_NEEDED") => return Ok(QrNow::Password(fresh_password(client).await?)),
                Err(e) => return Err(explain(&e)),
            }
        }
        r => r,
    };
    match r {
        T::Token(t) => Ok(QrNow::Show(qr_url(&t.token))),
        T::Success(s) => match s.authorization {
            tl::enums::auth::Authorization::Authorization(a) => Ok(QrNow::Done(me_of(&a.user))),
            tl::enums::auth::Authorization::SignUpRequired(_) => Err("This account needs signing up with Telegram on a phone first.".into()),
        },
        T::MigrateTo(_) => Err("Telegram moved the sign-in twice; try again.".into()),
    }
}

async fn finish(store: &Arc<Store>, me: Me, say: &impl Fn(Step, Option<String>)) {
    store.set_me(me.clone());
    let s = store.clone();
    match tokio::task::spawn_blocking(move || s.save(true)).await {
        Ok(Ok(())) => say(Step::Done(me), None),
        Ok(Err(e)) => say(Step::Failed(format!("Signed in, but the session couldn’t be kept in the keyring ({e}).")), None),
        Err(e) => say(Step::Failed(e.to_string()), None),
    }
}

/// Forget the session, telling Telegram first (blocking; a worker's).
pub fn sign_out() -> Result<(), String> {
    let (Some(api), Some(store)) = (account::api()?, Store::load()?) else { return account::forget_session() };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let (client, _u) = connect(store, &api);
        let _ = tokio::time::timeout(Duration::from_secs(10), client.sign_out()).await;
    });
    account::forget_session()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_urls() {
        assert_eq!(qr_url(b"\xfb\xff\x00a"), "tg://login?token=-_8AYQ");
        assert_eq!(qr_url(b"Man"), "tg://login?token=TWFu");
    }
}
