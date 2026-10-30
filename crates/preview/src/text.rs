//! Text and source code: shown at once, then coloured by syntax (Sublime
//! grammars via syntect) with token classes the preview maps onto the
//! theme, so code reads the same in light and dark mode.

use std::{
    io::Read,
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};
pub use olo_kit::style::Syntax as Tok;

use super::load::{Body, Send, Update};

/// Read at most this much of a text file.
const MAX_BYTES: usize = 4 << 20;
const MAX_LINES: usize = 100_000;
/// Colour at most this many lines (the rest stay plain).
const MAX_HIGHLIGHT: usize = 20_000;

#[derive(Clone, Default)]
pub struct Text {
    pub lines: Vec<String>,
    /// Per line: byte ranges and their token class (empty until coloured).
    pub spans: Vec<Vec<(u32, u32, Tok)>>,
    pub note: String,
}

/// Grammars of our own, for what the bundled ones lack.
const OWN: &[&str] = &[include_str!("syntaxes/toml.sublime-syntax"), include_str!("syntaxes/ini.sublime-syntax")];

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(|| {
        let mut set = SyntaxSet::load_defaults_newlines().into_builder();
        for grammar in OWN {
            match syntect::parsing::SyntaxDefinition::load_from_str(grammar, true, None) {
                Ok(def) => set.add(def),
                Err(e) => tracing::warn!("a grammar of Quick Look's own didn't load: {e}"),
            }
        }
        set.build()
    })
}

pub fn load(path: &Path, cancel: &Arc<AtomicBool>, send: Send) {
    let mut data = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(MAX_BYTES as u64 + 1).read_to_end(&mut data));
    if let Err(e) = read {
        send(Update::Failed(e.to_string()));
        return;
    }
    let mut truncated = data.len() > MAX_BYTES;
    data.truncate(MAX_BYTES);
    let s = String::from_utf8_lossy(&data);
    let mut lines: Vec<String> = s.lines().map(|l| l.replace('\t', "    ")).collect();
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        truncated = true;
    }
    let ss = syntaxes();
    let syntax = find_syntax(ss, path, lines.first().map(String::as_str).unwrap_or(""));
    let kind = match syntax {
        Some(s) if s.name != "Plain Text" => format!("{} source", s.name),
        _ => "Plain text".into(),
    };
    let n = lines.len();
    let note = format!("{kind}, {n} line{}{}", if n == 1 { "" } else { "s" }, if truncated { " (start shown)" } else { "" });
    let mut text = Text { lines, spans: Vec::new(), note };
    send(Update::Body(Body::Text(text.clone())));

    let Some(syntax) = syntax.filter(|s| s.name != "Plain Text") else { return };
    let Some(spans) = colour(syntax, &text.lines, cancel) else { return };
    text.spans = spans;
    send(Update::Highlighted(text));
}

/// Token classes for `lines` of code in `lang` (a Markdown fence's word:
/// "rust", "py", "sh", …), or none if no grammar knows it.
pub fn highlight(lang: &str, lines: &[String]) -> Vec<Vec<(u32, u32, Tok)>> {
    let ss = syntaxes();
    let lang = lang.trim().to_lowercase();
    let alias = match lang.as_str() {
        "toml" | "ini" | "conf" => "ini",
        "ts" | "tsx" | "typescript" | "jsx" | "mjs" => "js",
        "zsh" | "fish" | "shell" | "console" | "bash" => "sh",
        "kotlin" | "kt" => "java",
        "jsonc" | "json5" => "json",
        "yml" => "yaml",
        other => other,
    };
    let syntax = ss.find_syntax_by_token(&lang).or_else(|| ss.find_syntax_by_token(alias)).filter(|s| s.name != "Plain Text");
    syntax.and_then(|s| colour(s, lines, &Arc::new(AtomicBool::new(false)))).unwrap_or_default()
}

/// Each line's token classes, in `syntax` (None if cancelled).
fn colour(syntax: &SyntaxReference, lines: &[String], cancel: &AtomicBool) -> Option<Vec<Vec<(u32, u32, Tok)>>> {
    let ss = syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut spans = Vec::with_capacity(lines.len().min(MAX_HIGHLIGHT));
    for line in lines.iter().take(MAX_HIGHLIGHT) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        // Grammars expect the newline.
        let with_nl = format!("{line}\n");
        let Ok(ops) = state.parse_line(&with_nl, ss) else { break };
        let mut out = Vec::new();
        let mut at = 0;
        for (pos, op) in ops {
            let pos = pos.min(line.len());
            if pos > at {
                push(&mut out, at, pos, classify(&stack));
            }
            at = at.max(pos);
            let _ = stack.apply(&op);
        }
        if at < line.len() {
            push(&mut out, at, line.len(), classify(&stack));
        }
        spans.push(out);
    }
    Some(spans)
}

fn push(out: &mut Vec<(u32, u32, Tok)>, a: usize, b: usize, t: Tok) {
    if let Some(last) = out.last_mut()
        && last.2 == t
        && last.1 as usize == a
    {
        last.1 = b as u32;
        return;
    }
    out.push((a as u32, b as u32, t));
}

fn find_syntax<'a>(ss: &'a SyntaxSet, path: &Path, first: &str) -> Option<&'a SyntaxReference> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    // Names the bundled grammars don't claim.
    let alias = match ext.as_str() {
        "lock" => Some("toml"),
        "ts" | "tsx" | "mjs" | "cjs" | "jsx" => Some("js"),
        "zsh" | "fish" | "bashrc" => Some("sh"),
        "kt" | "kts" => Some("java"),
        "swift" => Some("go"),
        "jsonc" | "json5" => Some("json"),
        _ => None,
    };
    let by_name = match name.as_str() {
        "Makefile" | "makefile" | "GNUmakefile" => ss.find_syntax_by_extension("make"),
        "Dockerfile" | "Containerfile" => ss.find_syntax_by_extension("sh"),
        "CMakeLists.txt" => ss.find_syntax_by_extension("cmake"),
        "Cargo.lock" | "poetry.lock" | "uv.lock" | "Pipfile" => ss.find_syntax_by_extension("toml"),
        ".gitconfig" | ".gitmodules" | ".editorconfig" => ss.find_syntax_by_extension("ini"),
        _ => None,
    };
    by_name
        .or_else(|| ss.find_syntax_by_extension(&ext))
        .or_else(|| alias.and_then(|a| ss.find_syntax_by_extension(a)))
        .or_else(|| ss.find_syntax_by_first_line(first))
}

/// The token class of the innermost scope that has one.
fn classify(stack: &ScopeStack) -> Tok {
    static SCOPES: OnceLock<Vec<(Scope, Tok)>> = OnceLock::new();
    let table = SCOPES.get_or_init(|| {
        [
            ("comment", Tok::Comment),
            ("string", Tok::String),
            ("constant.numeric", Tok::Number),
            ("constant.character", Tok::String),
            ("constant.language", Tok::Constant),
            ("constant", Tok::Constant),
            ("keyword.operator", Tok::Plain),
            ("keyword", Tok::Keyword),
            ("storage.type", Tok::Keyword),
            ("storage.modifier", Tok::Keyword),
            ("storage", Tok::Keyword),
            ("entity.name.function", Tok::Function),
            ("support.function", Tok::Function),
            ("variable.function", Tok::Function),
            ("entity.name.type", Tok::Type),
            ("entity.name.class", Tok::Type),
            ("entity.name.struct", Tok::Type),
            ("entity.name.enum", Tok::Type),
            ("entity.name.trait", Tok::Type),
            ("support.type", Tok::Type),
            ("support.class", Tok::Type),
            ("entity.name.tag", Tok::Tag),
            ("entity.other.attribute-name", Tok::Attribute),
            ("markup.heading", Tok::Heading),
            ("entity.name.section", Tok::Heading),
            ("variable.language", Tok::Constant),
        ]
        .into_iter()
        .filter_map(|(s, t)| Scope::new(s).ok().map(|s| (s, t)))
        .collect()
    });
    for scope in stack.as_slice().iter().rev() {
        for (s, t) in table {
            if s.is_prefix_of(*scope) {
                return *t;
            }
        }
    }
    Tok::Plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_rust() {
        let dir = std::env::temp_dir().join(format!("ql-text-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.rs");
        std::fs::write(&p, "// hi\nfn main() { let x = \"s\"; 42 }\n").unwrap();
        let mut got = Vec::new();
        load(&p, &Arc::new(AtomicBool::new(false)), &mut |u| got.push(u));
        let Some(Update::Highlighted(t)) = got.pop() else { panic!("not coloured") };
        let toks = |i: usize| t.spans[i].iter().map(|s| s.2).collect::<Vec<_>>();
        assert_eq!(toks(0), vec![Tok::Comment]);
        assert!(toks(1).contains(&Tok::Keyword) && toks(1).contains(&Tok::String) && toks(1).contains(&Tok::Number));
        assert!(t.note.starts_with("Rust source"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Each line's text in each token class, as (class, text).
    fn classes(lang: &str, src: &str) -> Vec<Vec<(Tok, String)>> {
        let lines: Vec<String> = src.lines().map(String::from).collect();
        let spans = highlight(lang, &lines);
        assert_eq!(spans.len(), lines.len(), "{lang} not coloured");
        spans
            .iter()
            .zip(&lines)
            .map(|(s, l)| s.iter().filter(|x| x.2 != Tok::Plain).map(|x| (x.2, l[x.0 as usize..x.1 as usize].trim().to_owned())).collect())
            .collect()
    }

    #[test]
    fn colours_toml() {
        let src = r#"# a comment
[package]
name = "olo" # the name
version = 1_000
pi = -3.14e0
big = 0x1F
on = true
when = 1979-05-27T07:32:00Z
day = 1979-05-27
deps = ["a\n", 'b', { x = 1, y.z = "q" }]
[[bin]]
s = """
multi "quoted"
"""
lit = '''raw'''
a.b."c d" = nan
[ "quoted table" . sub ]"#;
        let c = classes("toml", src);
        use Tok::*;
        let one = |t: Tok, s: &str| (t, s.to_owned());
        assert_eq!(c[0], [one(Comment, "# a comment")]);
        assert_eq!(c[1], [one(Heading, "package")]);
        assert_eq!(c[2], [one(Attribute, "name"), one(String, "\"olo\""), one(Comment, "# the name")]);
        assert_eq!(c[3], [one(Attribute, "version"), one(Number, "1_000")]);
        assert_eq!(c[4], [one(Attribute, "pi"), one(Number, "-3.14e0")]);
        assert_eq!(c[5], [one(Attribute, "big"), one(Number, "0x1F")]);
        assert_eq!(c[6], [one(Attribute, "on"), one(Constant, "true")]);
        assert_eq!(c[7], [one(Attribute, "when"), one(Constant, "1979-05-27T07:32:00Z")]);
        assert_eq!(c[8], [one(Attribute, "day"), one(Constant, "1979-05-27")]);
        assert!(c[9].contains(&one(String, "\"a\\n\"")) && c[9].contains(&one(String, "'b'")), "{:?}", c[9]);
        assert!(c[9].contains(&one(Attribute, "x")) && c[9].contains(&one(Number, "1")) && c[9].contains(&one(Attribute, "y")), "{:?}", c[9]);
        assert_eq!(c[10], [one(Heading, "bin")]);
        // A multi-line string, quotes in it, to its end.
        assert_eq!(c[12], [one(String, "multi \"quoted\"")]);
        assert_eq!(c[13], [one(String, "\"\"\"")]);
        assert_eq!(c[14], [one(Attribute, "lit"), one(String, "'''raw'''")]);
        assert_eq!(c[15][0], one(Attribute, "a"));
        assert!(c[15].contains(&one(Attribute, "\"c d\"")) && c[15].contains(&one(Number, "nan")), "{:?}", c[15]);
        assert!(c[16].contains(&one(Heading, "\"quoted table\"")) && c[16].contains(&one(Heading, "sub")), "{:?}", c[16]);
    }

    #[test]
    fn colours_ini_and_units() {
        let c = classes("ini", "; note\n[Unit]\nDescription=Olo\nRestart = yes\nRestartSec=5\nName[de]=\"x\"");
        use Tok::*;
        let one = |t: Tok, s: &str| (t, s.to_owned());
        assert_eq!(c[0], [one(Comment, "; note")]);
        assert_eq!(c[1], [one(Heading, "Unit")]);
        assert_eq!(c[2], [one(Attribute, "Description")]);
        assert_eq!(c[3], [one(Attribute, "Restart"), one(Constant, "yes")]);
        assert_eq!(c[4], [one(Attribute, "RestartSec"), one(Number, "5")]);
        assert_eq!(c[5], [one(Attribute, "Name[de]"), one(String, "\"x\"")]);
    }

    #[test]
    fn toml_and_units_are_found_by_name() {
        let ss = syntaxes();
        for (name, want) in [("Cargo.toml", "TOML"), ("Cargo.lock", "TOML"), ("uv.lock", "TOML"), ("olo.service", "INI"), ("app.desktop", "INI"), (".gitconfig", "INI")] {
            assert_eq!(find_syntax(ss, Path::new(name), "").map(|s| s.name.as_str()), Some(want), "{name}");
        }
    }
}
