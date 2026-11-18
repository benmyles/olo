//! A stand-in CalDAV server by itself, with the demo's calendars, for
//! trying Calendar and Preferences ▸ Integrations ▸ Calendars with no
//! account of anyone's (HACKING.md, "Calendar"):
//!
//!     cargo run --release -p olo-calendar --example standin [YYYY-MM-DD]
//!
//! It prints where it is and how to sign in, and runs until it's ended.
//! It signs in by name and password (as Fastmail does, or any CalDAV
//! server) and as Google does (a consent page that agrees at once, a
//! refresh token, access tokens).

use olo_calendar::standin::Standin;

fn main() {
    let today = std::env::args().nth(1).and_then(|d| d.parse().ok()).unwrap_or_else(|| jiff::Zoned::now().date());
    let there = Standin::start("ada@example.org", "stand-in");
    there.fill_demo(today);
    there.knows_refresh_token("stand-in-refresh");
    println!("{}", there.url());
    println!("name ada@example.org, password stand-in; as Google: OLO_CALENDAR_GOOGLE={} OLO_CALENDAR_CONSENT=self, any client", there.url().trim_end_matches('/'));
    loop {
        std::thread::park();
    }
}
