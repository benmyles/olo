//! Language & Region (Ben, 2026-11-18: "we should add a language setting
//! and also let's translate all olo apps and prefs and ux to support all
//! the major languages (defaulting to us english)").
//!
//! The language Olo's own words are in (`Settings::language`; the kit
//! puts every running app's windows in it at once, `olo_kit::words`),
//! each language named in itself, with how much of Olo is in it so far.
//! Other programs are given it as they start (`LANGUAGE`, kept in the
//! person's own `~/.config/locale.conf`: `olo_xdg::locale`), in the
//! languages they have. Dates, times and numbers follow the system's
//! region (`LANG`) for now.

use olo_kit::{SharedString, words};

use crate::form::Ui;

/// The language chosen: its place in `words::LANGUAGES` (English first).
fn chosen(code: &str) -> usize {
    words::language(code).and_then(|l| words::LANGUAGES.iter().position(|m| m.code == l.code)).unwrap_or(0)
}

/// What other programs are told (`LANGUAGE`, gettext's): the language's
/// locale, then the language alone ("de_DE:de").
pub fn for_others(code: &str) -> String {
    let l = &words::LANGUAGES[chosen(code)];
    let first = l.locale.split('_').next().unwrap_or(l.locale);
    format!("{}:{first}", l.locale)
}

/// The language chosen given to the programs Olo starts from now on (the
/// person's locale.conf). Off the looper: a small file.
pub fn tell_others(code: &str) {
    // (The latest asked for is what's kept: two tellings close together,
    // each on a thread of its own, wrote in whichever order they ran.)
    static WANTED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    *WANTED.lock().unwrap() = for_others(code);
    std::thread::spawn(move || {
        let wanted = WANTED.lock().unwrap();
        let value = wanted.clone();
        if let Err(e) = olo_xdg::locale::set_language(Some(&value)) {
            tracing::warn!("language: locale.conf: {e}");
        }
    });
}

pub fn ui(ui: &mut Ui) {
    let now = chosen(&ui.s.language);
    ui.bbox("Language", |ui| {
        // (Each in itself, as it would want to be found; never translated.)
        let names: Vec<SharedString> = words::LANGUAGES.iter().map(|l| SharedString::from(if l.name == l.english { l.name.to_owned() } else { format!("{} ({})", l.name, l.english) })).collect();
        ui.menu(
            "language",
            names,
            now,
            |f| f.label("Olo’s language:"),
            |s, i| {
                let code = if i == 0 { String::new() } else { words::LANGUAGES[i].code.to_owned() };
                tell_others(&code);
                s.language = code;
            },
        );
        let l = &words::LANGUAGES[now];
        let (have, all) = words::covered(l, words::english().iter().copied());
        if now != 0 {
            ui.note(if have >= all {
                olo_kit::tr!("All of Olo’s {} words are in {}.", all, l.english)
            } else {
                olo_kit::tr!("{} of Olo’s {} words are in {} so far; the rest stay in English.", have, all, l.english)
            });
        }
        ui.note("Olo’s windows change at once. Other programs take it as they next start, in the languages they have.");
    });
    ui.bbox("Formats", |ui| {
        ui.note("Dates, times and numbers follow this computer’s region for now (its LANG). Date & Time sets the clock’s 12 or 24 hours.");
    });
}
