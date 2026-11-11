//! /ask in the palette (Ben, 2026-11-07: "if i do `/ask how do i find what
//! ports are open` it should use AI (with a config to choose the model
//! under prefs-> ai defaulting to astra on low) to show me an answer, and
//! it should be a structured json response so you can let it provide a
//! text/prose answer as well as up to, say, 4 optional code snippets"):
//! the question goes to Preferences ▸ AI's Ask model (`AiSettings::ask`)
//! on a worker, its answer in JSON (`schema`: the prose and up to four
//! snippets), the prose told as it comes (`Asked::SoFar`, read from the
//! JSON still being written), then the whole (`Asked::Done`). A follow-up
//! carries the conversation before it (`olo_ai::Turn`). Stopped by its
//! `cancel` (Escape while it comes): the reply is let go of where it is.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_protocol::{Client, Reply, Request};
use serde_json::{Value, json};

/// The most snippets an answer has.
pub const SNIPPETS: usize = 4;

/// Code to copy, from an answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub title: String,
    pub language: String,
    pub code: String,
    /// Each line's coloured stretches (from the line's start), by its
    /// language (`colour`; none for one Olo doesn't colour).
    pub classes: Vec<Vec<(std::ops::Range<usize>, olo_kit::code::Class)>>,
}

impl Snippet {
    /// Its lines (as `classes` has them).
    pub fn lines(&self) -> std::str::Split<'_, char> {
        self.code.split('\n')
    }
}

/// `code`'s classes line by line, in `language` (a fence's word: "bash",
/// "py"), as the editor colours it (olo-syntax). On the worker: a
/// language's first parse makes its query, up to half a second.
pub fn colour(language: &str, code: &str) -> Vec<Vec<(std::ops::Range<usize>, olo_kit::code::Class)>> {
    let Some(lang) = olo_kit::code::by_name(language) else { return Vec::new() };
    let mut h = olo_kit::code::syntax::Highlighter::new(lang);
    if let Some(job) = h.job(code) {
        h.done(job.run());
    }
    let mut at = 0;
    code.split('\n')
        .map(|line| {
            let mut out = Vec::new();
            h.classes(code, at..at + line.len(), &mut out);
            at += line.len() + 1;
            out.retain(|(_, c)| *c != olo_kit::code::Class::Plain);
            out
        })
        .collect()
}

/// What the worker tells the palette, by the ask's number.
pub enum Asked {
    /// The model's name, as it starts.
    Model(u64, String),
    /// The prose as far as it's come.
    SoFar(u64, String),
    /// The whole: the prose, the snippets and the JSON (for a follow-up's
    /// history), or why not.
    Done(u64, Result<(String, Vec<Snippet>, String), String>),
}

const SYSTEM: &str = "You answer questions asked in the command palette of Olo, a desktop for Linux. \
Answer briefly and directly, in plain sentences: no Markdown headings, lists or emphasis (`code` in backticks is fine), \
at most two short paragraphs. When commands or code would help, give up to four snippets, the most useful first: \
each a short title (what it does, a few words), its language (bash, python, rust, …) and the code alone, ready to paste \
(no prompt, no output, comments only where they help). Give none when none would help. \
Prefer what a fresh Fedora has; say when something needs installing (with its dnf command as a snippet).";

/// The shape of the answer: the prose, and up to four snippets.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "answer": {"type": "string"},
            "snippets": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "language": {"type": "string"},
                        "code": {"type": "string"}
                    },
                    "required": ["title", "language", "code"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["answer", "snippets"],
        "additionalProperties": false
    })
}

/// The answer's prose and snippets (at most `SNIPPETS`, empty ones left
/// out, tabs as four spaces, no trailing blank lines).
pub fn read(answer: &Value) -> (String, Vec<Snippet>) {
    let prose = answer["answer"].as_str().unwrap_or("").trim().to_owned();
    let snippets = answer["snippets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let code = s["code"].as_str()?.replace('\t', "    ").replace('\r', "");
            let code = code.trim_end().trim_start_matches('\n').to_owned();
            let language = s["language"].as_str().unwrap_or("").trim().to_lowercase();
            (!code.trim().is_empty()).then(|| Snippet { title: s["title"].as_str().unwrap_or("").trim().to_owned(), classes: colour(&language, &code), language, code })
        })
        .take(SNIPPETS)
        .collect();
    (prose, snippets)
}

/// What the question carries of this computer: the system and the shell.
fn context() -> String {
    let os =
        std::fs::read_to_string("/etc/os-release").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned()))).unwrap_or_else(|| "Linux".into());
    let shell = std::env::var("SHELL").ok().and_then(|s| std::path::Path::new(&s).file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "bash".into());
    format!("(Asked on {os}, the shell {shell}, the desktop Olo on Wayland.)")
}

/// Ask `question` (after `history`) on a worker, telling the palette as it
/// comes; stops once `cancel` is set.
pub fn ask(n: u64, question: String, history: Vec<olo_ai::Turn>, cancel: Arc<AtomicBool>, proxy: olo_kit::Proxy) {
    let _ = std::thread::Builder::new().name("palette-ask".into()).spawn(move || {
        let done = |r: Result<(String, Vec<Snippet>, String), String>| proxy.send(Asked::Done(n, r));
        let settings = match Client::connect().map_err(|e| e.to_string()).and_then(|mut c| c.request(&Request::GetSettings).map_err(|e| e.to_string())) {
            Ok(Reply::Settings { settings }) => settings,
            Ok(_) => return done(Err("The window server didn't give its settings.".into())),
            Err(e) => return done(Err(e)),
        };
        let feature = settings.ai.ask.clone();
        if !feature.enabled {
            return done(Err("Asking is off in Preferences ▸ AI.".into()));
        }
        proxy.send(Asked::Model(n, feature.model_name().to_owned()));
        let content = vec![olo_ai::text_block(format!("{question}\n\n{}", context()))];
        let ask = olo_ai::Ask { system: SYSTEM.into(), content, schema: schema(), max_tokens: 8000, history };
        // The prose as it comes, each time there's more of it.
        let mut told = String::new();
        let result = olo_ai::ask_streaming(&settings.ai, &feature, &ask, &cancel, |so_far| {
            if let Some(prose) = olo_ai::partial_string(so_far, "answer")
                && prose.len() > told.len()
            {
                told.clone_from(&prose);
                proxy.send(Asked::SoFar(n, prose));
            }
        });
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        done(match result {
            Ok(v) => {
                let (prose, snippets) = read(&v);
                Ok((prose, snippets, v.to_string()))
            }
            Err(e) => Err(e.to_string()),
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer read: its prose, at most four snippets, empty ones left
    /// out, tabs as spaces, blank lines around the code gone.
    #[test]
    fn reads_an_answer() {
        let v = json!({
            "answer": "  Use ss.  ",
            "snippets": [
                {"title": "Listening ports", "language": "Bash", "code": "\nss -tulpn\n\n"},
                {"title": "empty", "language": "bash", "code": "  \n"},
                {"title": "b", "language": "bash", "code": "\tb"},
                {"title": "c", "language": "bash", "code": "c"},
                {"title": "d", "language": "bash", "code": "d"},
                {"title": "e", "language": "bash", "code": "e"}
            ]
        });
        let (prose, snippets) = read(&v);
        assert_eq!(prose, "Use ss.");
        assert_eq!(snippets.len(), 4);
        assert_eq!((snippets[0].title.as_str(), snippets[0].language.as_str(), snippets[0].code.as_str()), ("Listening ports", "bash", "ss -tulpn"));
        assert_eq!(snippets[1].code, "    b");
        assert_eq!(snippets[3].title, "d");
    }

    /// A snippet is coloured as the editor colours its language, line by
    /// line; one in a language Olo doesn't know is plain.
    #[test]
    fn snippets_are_coloured() {
        use olo_kit::code::Class;
        let lines = colour("bash", "# list them\nss -tulpn | grep \"LISTEN\"");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].iter().any(|(r, c)| *c == Class::Comment && r.start == 0), "{lines:?}");
        assert!(lines[1].iter().any(|(_, c)| *c == Class::String), "{lines:?}");
        assert!(colour("klingon", "qapla'").is_empty());
    }

    /// The schema asks for the prose and the snippets, nothing else (as
    /// strict structured output wants).
    #[test]
    fn a_strict_schema() {
        let s = schema();
        assert_eq!(s["required"], json!(["answer", "snippets"]));
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(s["properties"]["snippets"]["items"]["additionalProperties"], false);
    }
}
