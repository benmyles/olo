//! Source code coloured by syntax, in process: tree-sitter's grammars for
//! every major language (each a parser compiled in) and their highlight
//! queries, the captures they make mapped to the token classes the
//! palette colours ([`Class`], `olo_theme::Palette::syntax`).
//!
//! A [`Highlighter`] keeps one text's syntax tree. Parsing is the slow
//! part (a second for 100,000 lines of Rust, 16 ms to take in one
//! keystroke there), so it never happens on the caller's thread: the
//! highlighter hands out a [`Job`] (the text as it is and the tree as it
//! was) for a worker to [`run`](Job::run), and takes the new tree back
//! with [`Highlighter::done`]. Edits made meanwhile are applied to both
//! trees, so the colours stay on the characters they belong to until the
//! new tree comes. Colouring is quick and done where it's asked for:
//! [`Highlighter::classes`] runs the query over just the bytes asked
//! for (a line of what shows: a screenful in a third of a millisecond).
//!
//! Which language a file is in: [`detect`] (its name, its extension, a
//! `#!` line's interpreter); what a Markdown code block's fence names:
//! [`by_name`].

use std::{ops::Range, path::Path, sync::OnceLock};

pub use olo_theme::Syntax as Class;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, Tree};

/// A language Olo colours.
pub struct Language {
    /// Short and stable: what's saved, what a code block's fence says.
    pub id: &'static str,
    /// As a menu or the status line says it.
    pub name: &'static str,
    /// Other names a fence gives it (`rs`, `py`, `sh`).
    pub aliases: &'static [&'static str],
    /// Its files' extensions, lower case, without the dot.
    pub extensions: &'static [&'static str],
    /// Its files' whole names (`Makefile`, `Dockerfile`).
    pub file_names: &'static [&'static str],
    /// What a `#!` line runs it with.
    pub interpreters: &'static [&'static str],
    grammar: fn() -> tree_sitter::Language,
    query: fn() -> String,
    /// Its grammar and compiled query, made the first time a job needs
    /// them (a query takes up to half a second to compile: on a worker).
    ready: OnceLock<Option<Ready>>,
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id)
    }
}

impl PartialEq for Language {
    fn eq(&self, other: &Language) -> bool {
        self.id == other.id
    }
}

/// A language made ready to parse and colour.
struct Ready {
    grammar: tree_sitter::Language,
    query: Query,
    /// Each capture's class (None: not a colour, `@spell` say).
    classes: Vec<Option<Class>>,
}

impl Language {
    fn ready(&self) -> Option<&Ready> {
        self.ready
            .get_or_init(|| {
                let grammar = (self.grammar)();
                let query = Query::new(&grammar, &(self.query)()).ok()?;
                let classes = query.capture_names().iter().map(|n| class_of(n)).collect();
                Some(Ready { grammar, query, classes })
            })
            .as_ref()
    }

    /// Whether it's ready to colour (its first job has run).
    pub fn is_ready(&self) -> bool {
        self.ready.get().is_some_and(Option::is_some)
    }
}

/// The class a capture's name colours as: the first of its dotted parts
/// that says (`function.method.call` is a function, `string.special.uri`
/// a link). None: it isn't a colour at all; `Plain`: it takes the colour
/// of what it's in away (a variable in a string's interpolation).
pub fn class_of(capture: &str) -> Option<Class> {
    let full = capture.trim_start_matches('_');
    let class = match full {
        "string.special.uri" | "text.uri" | "markup.link" | "markup.link.url" | "string.special.url" => Class::Link,
        "string.special.symbol" | "string.special.key" | "string.escape" | "escape" | "character.special" | "constant.character.escape" => Class::Constant,
        "variable.builtin" | "variable.language" | "constant.builtin" | "boolean" | "constant.macro" => Class::Constant,
        "keyword.operator" | "keyword.conditional.ternary" => Class::Keyword,
        "punctuation.special" | "embedded" => Class::Plain,
        "tag.attribute" | "attribute" | "attribute.builtin" | "decorator" | "annotation" => Class::Attribute,
        "constructor" => Class::Type,
        _ => {
            let first = full.split('.').next().unwrap_or(full);
            match first {
                "comment" => Class::Comment,
                "string" | "character" | "regex" => Class::String,
                "number" | "float" | "integer" => Class::Number,
                "constant" => Class::Constant,
                "keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass" | "preproc" | "define" | "import" | "directive" | "label" => Class::Keyword,
                "function" | "method" | "macro" => Class::Function,
                "type" | "class" | "interface" | "struct" | "enum" => Class::Type,
                "tag" => Class::Tag,
                "markup" if full.starts_with("markup.heading") || full.starts_with("markup.title") => Class::Heading,
                "text" if full.starts_with("text.title") => Class::Heading,
                "variable" | "parameter" | "property" | "field" | "punctuation" | "operator" | "delimiter" | "none" | "module" | "namespace" | "identifier" | "local" => Class::Plain,
                _ => return None,
            }
        }
    };
    Some(class)
}

macro_rules! lang {
    ($id:literal, $name:literal, $grammar:expr, $query:expr, aliases: [$($a:literal),*], ext: [$($e:literal),*], files: [$($f:literal),*], run: [$($r:literal),*]) => {
        Language {
            id: $id,
            name: $name,
            aliases: &[$($a),*],
            extensions: &[$($e),*],
            file_names: &[$($f),*],
            interpreters: &[$($r),*],
            grammar: || $grammar.into(),
            query: || $query.into(),
            ready: OnceLock::new(),
        }
    };
}

fn joined(parts: &[&str]) -> String {
    parts.join("\n")
}

/// Every language Olo colours, by name.
pub static LANGUAGES: [Language; 43] = [
    lang!("bash", "Shell", tree_sitter_bash::LANGUAGE, tree_sitter_bash::HIGHLIGHT_QUERY,
        aliases: ["sh", "shell", "shellscript", "zsh", "ksh", "console"],
        ext: ["sh", "bash", "zsh", "ksh", "ebuild", "eclass", "bats"],
        files: [".bashrc", ".bash_profile", ".bash_logout", ".bash_aliases", ".zshrc", ".zprofile", ".zshenv", ".profile", ".xprofile", ".xinitrc", "PKGBUILD", "APKBUILD"],
        run: ["sh", "bash", "zsh", "ksh", "dash", "ash"]),
    lang!("c", "C", tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY,
        aliases: [], ext: ["c", "h"], files: [], run: []),
    lang!("cmake", "CMake", tree_sitter_cmake::LANGUAGE, tree_sitter_cmake::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["cmake"], files: ["CMakeLists.txt"], run: []),
    lang!("cpp", "C++", tree_sitter_cpp::LANGUAGE, joined(&[tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY]),
        aliases: ["c++", "cxx", "cc"], ext: ["cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++", "ipp", "tpp", "inl", "ixx", "cppm"], files: [], run: []),
    lang!("csharp", "C#", tree_sitter_c_sharp::LANGUAGE, tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        aliases: ["c#", "cs"], ext: ["cs", "csx"], files: [], run: []),
    lang!("css", "CSS", tree_sitter_css::LANGUAGE, tree_sitter_css::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["css"], files: [], run: []),
    lang!("dart", "Dart", tree_sitter_dart::LANGUAGE, tree_sitter_dart::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["dart"], files: [], run: []),
    lang!("diff", "Diff", tree_sitter_diff::LANGUAGE, tree_sitter_diff::HIGHLIGHTS_QUERY,
        aliases: ["patch"], ext: ["diff", "patch", "rej"], files: [], run: []),
    lang!("dockerfile", "Dockerfile", tree_sitter_dockerfile_updated::language(), include_str!("../queries/dockerfile.scm"),
        aliases: ["docker", "containerfile"], ext: ["dockerfile", "containerfile"], files: ["Dockerfile", "Containerfile"], run: []),
    lang!("elixir", "Elixir", tree_sitter_elixir::LANGUAGE, tree_sitter_elixir::HIGHLIGHTS_QUERY,
        aliases: ["ex"], ext: ["ex", "exs"], files: ["mix.lock"], run: ["elixir"]),
    lang!("erlang", "Erlang", tree_sitter_erlang::LANGUAGE, tree_sitter_erlang::HIGHLIGHTS_QUERY,
        aliases: ["erl"], ext: ["erl", "hrl", "escript"], files: ["rebar.config"], run: ["escript"]),
    lang!("fish", "Fish", tree_sitter_fish::language(), tree_sitter_fish::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["fish"], files: [], run: ["fish"]),
    lang!("go", "Go", tree_sitter_go::LANGUAGE, tree_sitter_go::HIGHLIGHTS_QUERY,
        aliases: ["golang"], ext: ["go"], files: [], run: []),
    lang!("haskell", "Haskell", tree_sitter_haskell::LANGUAGE, tree_sitter_haskell::HIGHLIGHTS_QUERY,
        aliases: ["hs"], ext: ["hs", "hs-boot", "hsc"], files: [], run: ["runhaskell"]),
    lang!("html", "HTML", tree_sitter_html::LANGUAGE, tree_sitter_html::HIGHLIGHTS_QUERY,
        aliases: ["xhtml"], ext: ["html", "htm", "xhtml", "shtml"], files: [], run: []),
    lang!("ini", "INI", tree_sitter_ini::LANGUAGE, tree_sitter_ini::HIGHLIGHTS_QUERY,
        aliases: ["dosini", "systemd", "desktop"],
        ext: ["ini", "cfg", "desktop", "service", "socket", "timer", "mount", "automount", "target", "slice", "path", "network", "netdev", "link", "conf"],
        files: [".gitconfig", ".gitmodules", ".editorconfig", ".npmrc", "pylintrc", ".pylintrc", "setup.cfg", "tox.ini"], run: []),
    lang!("java", "Java", tree_sitter_java::LANGUAGE, tree_sitter_java::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["java"], files: [], run: []),
    lang!("javascript", "JavaScript", tree_sitter_javascript::LANGUAGE, joined(&[tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY]),
        aliases: ["js", "jsx", "node", "ecmascript"], ext: ["js", "mjs", "cjs", "jsx"], files: [], run: ["node", "nodejs", "deno", "bun", "qjs"]),
    lang!("json", "JSON", tree_sitter_json::LANGUAGE, tree_sitter_json::HIGHLIGHTS_QUERY,
        aliases: ["jsonc", "json5"], ext: ["json", "jsonc", "json5", "geojson", "webmanifest", "jsonl", "ndjson", "har", "ipynb"], files: [".babelrc", ".eslintrc", "flake.lock", ".prettierrc"], run: []),
    lang!("julia", "Julia", tree_sitter_julia::LANGUAGE, include_str!("../queries/julia.scm"),
        aliases: ["jl"], ext: ["jl"], files: [], run: ["julia"]),
    lang!("kotlin", "Kotlin", tree_sitter_kotlin_ng::LANGUAGE, include_str!("../queries/kotlin.scm"),
        aliases: ["kt", "kts"], ext: ["kt", "kts"], files: [], run: []),
    lang!("lua", "Lua", tree_sitter_lua::LANGUAGE, tree_sitter_lua::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["lua", "rockspec"], files: [".luacheckrc"], run: ["lua", "luajit"]),
    lang!("make", "Makefile", tree_sitter_make::LANGUAGE, tree_sitter_make::HIGHLIGHTS_QUERY,
        aliases: ["makefile", "mk"], ext: ["mk", "mak", "make"], files: ["Makefile", "makefile", "GNUmakefile", "Kbuild"], run: ["make"]),
    lang!("nix", "Nix", tree_sitter_nix::LANGUAGE, tree_sitter_nix::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["nix"], files: [], run: []),
    lang!("objc", "Objective-C", tree_sitter_objc::LANGUAGE, tree_sitter_objc::HIGHLIGHTS_QUERY,
        aliases: ["objective-c", "objectivec"], ext: ["m"], files: [], run: []),
    lang!("ocaml", "OCaml", tree_sitter_ocaml::LANGUAGE_OCAML, tree_sitter_ocaml::HIGHLIGHTS_QUERY,
        aliases: ["ml"], ext: ["ml", "mll", "mly"], files: [], run: ["ocaml"]),
    lang!("ocaml-interface", "OCaml Interface", tree_sitter_ocaml::LANGUAGE_OCAML_INTERFACE, tree_sitter_ocaml::HIGHLIGHTS_QUERY,
        aliases: ["mli"], ext: ["mli"], files: [], run: []),
    lang!("php", "PHP", tree_sitter_php::LANGUAGE_PHP, tree_sitter_php::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["php", "phtml", "php3", "php4", "php5", "php7", "phps", "phpt"], files: [], run: ["php"]),
    lang!("powershell", "PowerShell", tree_sitter_powershell::LANGUAGE, tree_sitter_powershell::HIGHLIGHTS_QUERY,
        aliases: ["ps1", "pwsh", "posh"], ext: ["ps1", "psm1", "psd1"], files: [], run: ["pwsh", "powershell"]),
    lang!("proto", "Protocol Buffers", tree_sitter_proto::LANGUAGE, include_str!("../queries/proto.scm"),
        aliases: ["protobuf"], ext: ["proto"], files: [], run: []),
    lang!("python", "Python", tree_sitter_python::LANGUAGE, tree_sitter_python::HIGHLIGHTS_QUERY,
        aliases: ["py", "python3", "py3", "gyp"], ext: ["py", "pyi", "pyw", "pyx", "gyp", "gypi", "bzl", "star"], files: ["SConstruct", "SConscript", "BUILD", "BUILD.bazel", "WORKSPACE"], run: ["python", "python3", "python2", "pypy", "pypy3"]),
    lang!("r", "R", tree_sitter_r::LANGUAGE, tree_sitter_r::HIGHLIGHTS_QUERY,
        aliases: ["rscript"], ext: ["r"], files: [".Rprofile"], run: ["Rscript"]),
    lang!("ruby", "Ruby", tree_sitter_ruby::LANGUAGE, tree_sitter_ruby::HIGHLIGHTS_QUERY,
        aliases: ["rb"], ext: ["rb", "rake", "gemspec", "ru", "rbw", "podspec", "thor"], files: ["Gemfile", "Rakefile", "Podfile", "Vagrantfile", "Brewfile", "Guardfile", "Fastfile"], run: ["ruby", "jruby", "rake"]),
    lang!("rust", "Rust", tree_sitter_rust::LANGUAGE, tree_sitter_rust::HIGHLIGHTS_QUERY,
        aliases: ["rs"], ext: ["rs"], files: [], run: ["rust-script"]),
    lang!("scala", "Scala", tree_sitter_scala::LANGUAGE, tree_sitter_scala::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["scala", "sc", "sbt"], files: [], run: ["scala"]),
    lang!("sql", "SQL", tree_sitter_sequel::LANGUAGE, tree_sitter_sequel::HIGHLIGHTS_QUERY,
        aliases: ["postgresql", "postgres", "mysql", "sqlite", "plsql"], ext: ["sql", "psql", "ddl", "dml"], files: [], run: []),
    lang!("swift", "Swift", tree_sitter_swift::LANGUAGE, tree_sitter_swift::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["swift"], files: [], run: ["swift"]),
    lang!("toml", "TOML", tree_sitter_toml_ng::LANGUAGE, tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["toml"], files: ["Cargo.lock", "Pipfile", "poetry.lock", "uv.lock", "Gopkg.lock"], run: []),
    lang!("tsx", "TSX", tree_sitter_typescript::LANGUAGE_TSX, joined(&[tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY]),
        aliases: [], ext: ["tsx"], files: [], run: []),
    lang!("typescript", "TypeScript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT, joined(&[tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY]),
        aliases: ["ts"], ext: ["ts", "mts", "cts"], files: [], run: ["ts-node", "tsx"]),
    lang!("xml", "XML", tree_sitter_xml::LANGUAGE_XML, tree_sitter_xml::XML_HIGHLIGHT_QUERY,
        aliases: ["svg", "plist", "xsl"],
        ext: ["xml", "xsd", "xsl", "xslt", "svg", "plist", "rss", "atom", "csproj", "vcxproj", "fsproj", "props", "targets", "ui", "glade", "xaml", "xib", "storyboard", "metainfo", "appdata", "kml", "gpx", "opml", "xliff", "xlf", "wsdl", "pom"],
        files: [], run: []),
    lang!("yaml", "YAML", tree_sitter_yaml::LANGUAGE, tree_sitter_yaml::HIGHLIGHTS_QUERY,
        aliases: ["yml"], ext: ["yaml", "yml"], files: [".clang-format", ".clang-tidy", ".gemrc", "CITATION.cff"], run: []),
    lang!("zig", "Zig", tree_sitter_zig::LANGUAGE, tree_sitter_zig::HIGHLIGHTS_QUERY,
        aliases: [], ext: ["zig", "zon"], files: [], run: []),
];

/// The language with this id, alias or extension (any case): what a
/// Markdown code block's fence names (` ```rust `, ` ```py `).
pub fn by_name(name: &str) -> Option<&'static Language> {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() {
        return None;
    }
    LANGUAGES
        .iter()
        .find(|l| l.id == n || l.name.eq_ignore_ascii_case(&n))
        .or_else(|| LANGUAGES.iter().find(|l| l.aliases.contains(&n.as_str())))
        .or_else(|| LANGUAGES.iter().find(|l| l.extensions.contains(&n.as_str())))
}

/// The language of the file at `path` whose first line is `first`: by
/// its name (`Makefile`), its extension, or the interpreter its `#!`
/// line runs (`#!/usr/bin/env python3`). None: not code Olo colours.
pub fn detect(path: &Path, first: &str) -> Option<&'static Language> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(l) = LANGUAGES.iter().find(|l| l.file_names.contains(&name)) {
        return Some(l);
    }
    // Dockerfile.dev, Makefile.am and the like.
    if let Some((stem, _)) = name.split_once('.')
        && let Some(l) = LANGUAGES.iter().find(|l| l.file_names.contains(&stem) && matches!(l.id, "dockerfile" | "make"))
    {
        return Some(l);
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if let Some(ext) = &ext
        && let Some(l) = LANGUAGES.iter().find(|l| l.extensions.contains(&ext.as_str()))
    {
        return Some(l);
    }
    shebang(first)
}

/// The language a `#!` line runs.
pub fn shebang(first: &str) -> Option<&'static Language> {
    let line = first.strip_prefix("#!")?.trim();
    let mut words = line.split_whitespace();
    let mut prog = words.next()?.rsplit('/').next()?;
    if prog == "env" {
        // (`env -S python3 -u`: the first word that isn't a flag.)
        prog = words.find(|w| !w.starts_with('-') && !w.contains('='))?;
    }
    // python3.12, ruby2.7: the name before the version.
    let base = prog.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    LANGUAGES.iter().find(|l| l.interpreters.contains(&prog) || (!base.is_empty() && l.interpreters.contains(&base)))
}

/// A change to the text, for a [`Highlighter`]: bytes `removed` became
/// `inserted`, with where they were in lines and bytes along them (as
/// tree-sitter keeps its tree's places).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit(InputEdit);

impl Edit {
    /// Bytes `removed` replaced by `inserted`; `start` and `end` are
    /// where `removed`'s ends were, as (line, byte in that line).
    pub fn new(removed: Range<usize>, inserted: &str, start: (usize, usize), end: (usize, usize)) -> Edit {
        let lines = inserted.bytes().filter(|&b| b == b'\n').count();
        let new_end = match inserted.rfind('\n') {
            Some(i) => (start.0 + lines, inserted.len() - i - 1),
            None => (start.0, start.1 + inserted.len()),
        };
        let pt = |(row, column): (usize, usize)| Point { row, column };
        Edit(InputEdit {
            start_byte: removed.start,
            old_end_byte: removed.end,
            new_end_byte: removed.start + inserted.len(),
            start_position: pt(start),
            old_end_position: pt(end),
            new_end_position: pt(new_end),
        })
    }

    /// The same, finding the lines in `old` (the text before): for a
    /// text that doesn't keep where its lines start.
    pub fn in_text(old: &str, removed: Range<usize>, inserted: &str) -> Edit {
        let place = |at: usize| {
            let before = &old.as_bytes()[..at];
            let line = before.iter().filter(|&&b| b == b'\n').count();
            let col = at - before.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            (line, col)
        };
        Edit::new(removed.clone(), inserted, place(removed.start), place(removed.end))
    }

    /// Where the change starts and the end of what it put in.
    pub fn inserted(&self) -> Range<usize> {
        self.0.start_byte..self.0.new_end_byte
    }

    /// The bytes it took away (of the text before it).
    pub fn removed(&self) -> Range<usize> {
        self.0.start_byte..self.0.old_end_byte
    }
}

/// What a parse changed ([`Highlighter::done`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Changed {
    /// Nothing that shows.
    Nothing,
    /// The classes of these bytes may be different.
    Ranges(Vec<Range<usize>>),
    /// Any of it may be.
    All,
}

/// One text's syntax, kept up to date as it's edited.
pub struct Highlighter {
    lang: &'static Language,
    tree: Option<Tree>,
    /// None while a job has it.
    parser: Option<Parser>,
    /// The edits since the text the job out was given (for its tree, when
    /// it's back).
    since: Vec<InputEdit>,
    /// The text changed since the last parse began (or none has).
    stale: bool,
    /// Counts resets: a job of a text replaced since is thrown away.
    epoch: u64,
    cursor: QueryCursor,
    /// Captures found for a stretch, and each byte's class, while
    /// colouring (kept to save allocating).
    found: Vec<(usize, usize, usize, u32, Class)>,
    marks: Vec<u8>,
}

/// A parse to run on a worker ([`Highlighter::job`]).
pub struct Job {
    lang: &'static Language,
    parser: Parser,
    old: Option<Tree>,
    text: String,
    epoch: u64,
}

/// A parse done ([`Job::run`]), for [`Highlighter::done`].
pub struct Parsed {
    parser: Parser,
    tree: Option<Tree>,
    /// Where it differs from the tree before (None: there was none).
    changed: Option<Vec<Range<usize>>>,
    epoch: u64,
}

impl Job {
    /// Parses the text (the slow part, a second for 100,000 lines: run it
    /// on a worker). The first job for a language makes it ready too.
    pub fn run(mut self) -> Parsed {
        let Some(ready) = self.lang.ready() else { return Parsed { parser: self.parser, tree: None, changed: None, epoch: self.epoch } };
        if self.parser.language().is_none() && self.parser.set_language(&ready.grammar).is_err() {
            return Parsed { parser: self.parser, tree: None, changed: None, epoch: self.epoch };
        }
        let tree = self.parser.parse(&self.text, self.old.as_ref());
        let changed = match (&self.old, &tree) {
            (Some(old), Some(new)) => Some(old.changed_ranges(new).map(|r| r.start_byte..r.end_byte).collect()),
            _ => None,
        };
        Parsed { parser: self.parser, tree, changed, epoch: self.epoch }
    }

    /// How many bytes it parses.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Whether a node of this kind is a number (`integer_literal`).
fn is_number(kind: &str) -> bool {
    ["integer", "float", "number", "decimal", "hex", "octal", "binary"].iter().any(|k| kind.contains(k))
}

/// Not a class: plain (`Highlighter::marks`).
const UNMARKED: u8 = u8::MAX;

fn class_index(c: Class) -> u8 {
    CLASSES.iter().position(|k| *k == c).unwrap_or(0) as u8
}

const CLASSES: [Class; 12] =
    [Class::Plain, Class::Comment, Class::String, Class::Keyword, Class::Number, Class::Type, Class::Function, Class::Constant, Class::Tag, Class::Attribute, Class::Heading, Class::Link];

impl Highlighter {
    /// For a text in `lang` (nothing coloured until its first job is done).
    pub fn new(lang: &'static Language) -> Highlighter {
        Highlighter { lang, tree: None, parser: Some(Parser::new()), since: Vec::new(), stale: true, epoch: 0, cursor: QueryCursor::new(), found: Vec::new(), marks: Vec::new() }
    }

    pub fn language(&self) -> &'static Language {
        self.lang
    }

    /// Whether it has a tree to colour from.
    pub fn has_tree(&self) -> bool {
        self.tree.is_some()
    }

    /// The text was changed by `edit`.
    pub fn edit(&mut self, edit: &Edit) {
        if let Some(t) = &mut self.tree {
            t.edit(&edit.0);
        }
        if self.parser.is_none() {
            self.since.push(edit.0);
        }
        self.stale = true;
    }

    /// The text was replaced, all of it: parsed again from nothing.
    pub fn reset(&mut self) {
        self.tree = None;
        self.since.clear();
        self.stale = true;
        self.epoch += 1;
    }

    /// A parse of `text` to run on a worker, if the text changed since
    /// the last one began and no other is out.
    pub fn job(&mut self, text: &str) -> Option<Job> {
        if !self.stale {
            return None;
        }
        let parser = self.parser.take()?;
        self.stale = false;
        self.since.clear();
        Some(Job { lang: self.lang, parser, old: self.tree.clone(), text: text.to_owned(), epoch: self.epoch })
    }

    /// A job came back: its tree is the one coloured from now. What it
    /// changed; if the text changed meanwhile, another job is wanted
    /// ([`job`](Self::job) gives it).
    pub fn done(&mut self, parsed: Parsed) -> Changed {
        self.parser = Some(parsed.parser);
        let Some(mut tree) = parsed.tree else { return Changed::Nothing };
        // Of a text since replaced: no use.
        if parsed.epoch != self.epoch {
            return Changed::Nothing;
        }
        if !self.since.is_empty() {
            for e in self.since.drain(..) {
                tree.edit(&e);
            }
            self.tree = Some(tree);
            return Changed::All;
        }
        let first = self.tree.is_none();
        self.tree = Some(tree);
        match parsed.changed {
            _ if first => Changed::All,
            Some(r) if r.is_empty() => Changed::Nothing,
            Some(r) => Changed::Ranges(r),
            None => Changed::All,
        }
    }

    /// The classes of bytes `range` of `text` (a line, say: what's asked
    /// for is all that's looked at), as stretches from `range`'s start;
    /// what isn't in one is plain. Nothing until the first job is done.
    pub fn classes(&mut self, text: &str, range: Range<usize>, out: &mut Vec<(Range<usize>, Class)>) {
        out.clear();
        let (Some(tree), Some(ready)) = (&self.tree, self.lang.ready.get().and_then(Option::as_ref)) else { return };
        let range = range.start.min(text.len())..range.end.min(text.len());
        if range.is_empty() {
            return;
        }
        self.found.clear();
        self.cursor.set_byte_range(range.clone());
        let mut caps = self.cursor.captures(&ready.query, tree.root_node(), text.as_bytes());
        while let Some((m, i)) = caps.next() {
            let c = m.captures()[*i];
            let Some(mut class) = ready.classes.get(c.index as usize).copied().flatten() else { continue };
            // (Some grammars' queries call numbers constants: Rust's.)
            if class == Class::Constant && is_number(c.node.kind()) {
                class = Class::Number;
            }
            let r = c.node.byte_range();
            let (a, b) = (r.start.max(range.start), r.end.min(range.end));
            if a >= b {
                continue;
            }
            self.found.push((a - range.start, b - range.start, r.len(), m.pattern_index as u32, class));
        }
        if self.found.is_empty() {
            return;
        }
        // Outer captures first, and for one node the later patterns
        // first: what's inside, and a node's first pattern, paint last.
        self.found.sort_unstable_by(|x, y| y.2.cmp(&x.2).then(y.3.cmp(&x.3)));
        self.marks.clear();
        self.marks.resize(range.len(), UNMARKED);
        for &(a, b, _, _, class) in &self.found {
            self.marks[a..b].fill(class_index(class));
        }
        let mut k = 0;
        while k < self.marks.len() {
            let m = self.marks[k];
            let mut j = k + 1;
            while j < self.marks.len() && self.marks[j] == m {
                j += 1;
            }
            if m != UNMARKED && CLASSES[m as usize] != Class::Plain {
                out.push((k..j, CLASSES[m as usize]));
            }
            k = j;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `text` as `lang` at once (as a worker would).
    fn parsed(lang: &'static Language, text: &str) -> Highlighter {
        let mut h = Highlighter::new(lang);
        let job = h.job(text).unwrap();
        assert_eq!(h.done(job.run()), Changed::All);
        h
    }

    fn classes(h: &mut Highlighter, text: &str) -> Vec<(String, Class)> {
        let mut out = Vec::new();
        let mut all = Vec::new();
        let mut at = 0;
        for line in text.split_inclusive('\n') {
            h.classes(text, at..at + line.len(), &mut out);
            all.extend(out.iter().map(|(r, c)| (line[r.clone()].to_owned(), *c)));
            at += line.len();
        }
        all
    }

    #[test]
    fn every_language_parses_and_colours() {
        for l in &LANGUAGES {
            let ready = l.ready().unwrap_or_else(|| panic!("{}: its query doesn't compile", l.id));
            assert!(ready.query.pattern_count() > 0, "{}", l.id);
            assert!(ready.classes.iter().any(|c| c.is_some_and(|c| c != Class::Plain)), "{}: nothing coloured", l.id);
            let mut p = Parser::new();
            p.set_language(&ready.grammar).unwrap_or_else(|e| panic!("{}: {e}", l.id));
        }
        let ids: Vec<_> = LANGUAGES.iter().map(|l| l.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids, sorted, "kept in order, once each");
    }

    #[test]
    fn rust_coloured() {
        let text = "// hi\nfn main() {\n    let x = \"s\"; 42\n}\n";
        let mut h = parsed(by_name("rust").unwrap(), text);
        let got = classes(&mut h, text);
        assert!(got.contains(&("// hi\n".into(), Class::Comment)) || got.contains(&("// hi".into(), Class::Comment)), "{got:?}");
        assert!(got.contains(&("fn".into(), Class::Keyword)), "{got:?}");
        assert!(got.contains(&("main".into(), Class::Function)), "{got:?}");
        assert!(got.contains(&("\"s\"".into(), Class::String)), "{got:?}");
        assert!(got.contains(&("42".into(), Class::Number)), "{got:?}");
    }

    #[test]
    fn samples_colour_in_each_language() {
        let samples: &[(&str, &str, &str, Class)] = &[
            ("python", "def f(x):\n    return 'a'  # c\n", "def", Class::Keyword),
            ("python", "def f(x):\n    return 'a'  # c\n", "'a'", Class::String),
            ("javascript", "const a = 1; // x\nfunction g() {}\n", "const", Class::Keyword),
            ("typescript", "let a: number = 1;\ninterface I {}\n", "interface", Class::Keyword),
            ("c", "#include <stdio.h>\nint main(void) { return 0; }\n", "return", Class::Keyword),
            ("cpp", "class A { public: int x; };\n", "class", Class::Keyword),
            ("go", "package main\nfunc main() {}\n", "func", Class::Keyword),
            ("bash", "echo \"hi\" # c\nif true; then :; fi\n", "if", Class::Keyword),
            ("ruby", "def f\n  puts 'x'\nend\n", "def", Class::Keyword),
            ("java", "class A { void f() { return; } }\n", "class", Class::Keyword),
            ("kotlin", "fun main() { val x = \"s\" }\n", "fun", Class::Keyword),
            ("kotlin", "fun main() { val x = \"s\" }\n", "\"s\"", Class::String),
            ("swift", "func f() -> Int { return 1 }\n", "func", Class::Keyword),
            ("csharp", "class A { void F() { return; } }\n", "class", Class::Keyword),
            ("php", "<?php echo 'x'; function f() {} ?>\n", "function", Class::Keyword),
            ("lua", "local x = 1 -- c\nfunction f() end\n", "local", Class::Keyword),
            ("sql", "SELECT a FROM t WHERE b = 1;\n", "SELECT", Class::Keyword),
            ("html", "<p class=\"a\">x</p>\n", "p", Class::Tag),
            ("css", "a { color: red; }\n", "color", Class::Plain),
            ("json", "{\"a\": 1}\n", "1", Class::Number),
            ("yaml", "a: 'b'\n", "'b'", Class::String),
            ("toml", "a = \"b\"\n", "\"b\"", Class::String),
            ("xml", "<a b=\"c\"/>\n", "a", Class::Tag),
            ("make", "all:\n\techo hi\n", "all", Class::Constant),
            ("dockerfile", "FROM alpine\nRUN echo hi\n", "FROM", Class::Keyword),
        ];
        for (id, text, token, class) in samples {
            let lang = by_name(id).unwrap();
            let mut h = parsed(lang, text);
            let got = classes(&mut h, text);
            if *class == Class::Plain {
                continue;
            }
            assert!(got.iter().any(|(t, c)| t == token && c == class), "{id}: {token:?} as {class:?} in {got:?}");
        }
    }

    #[test]
    fn edits_keep_colours_on_their_characters() {
        let lang = by_name("rust").unwrap();
        let mut text = String::from("fn a() {}\nfn b() {}\n");
        let mut h = parsed(lang, &text);
        // Typed at the top, not parsed yet: "fn b" still a keyword where it moved.
        let e = Edit::in_text(&text, 0..0, "// x\n");
        text.insert_str(0, "// x\n");
        h.edit(&e);
        let got = classes(&mut h, &text);
        assert!(got.iter().filter(|(t, c)| t == "fn" && *c == Class::Keyword).count() == 2, "{got:?}");
        // Parsed: the comment too.
        let job = h.job(&text).unwrap();
        let changed = h.done(job.run());
        assert!(matches!(changed, Changed::Ranges(_) | Changed::Nothing), "{changed:?}");
        let got = classes(&mut h, &text);
        assert!(got.iter().any(|(t, c)| t.starts_with("// x") && *c == Class::Comment), "{got:?}");
        // An edit while a job is out: its tree takes the edit when back.
        let job = {
            let e = Edit::in_text(&text, text.len()..text.len(), "/* open");
            text.push_str("/* open");
            h.edit(&e);
            h.job(&text).unwrap()
        };
        let e = Edit::in_text(&text, text.len()..text.len(), " */");
        text.push_str(" */");
        h.edit(&e);
        assert!(h.job(&text).is_none(), "one at a time");
        assert_eq!(h.done(job.run()), Changed::All);
        let job = h.job(&text).expect("the text changed meanwhile");
        h.done(job.run());
        let got = classes(&mut h, &text);
        assert!(got.iter().any(|(t, c)| t == "/* open */" && *c == Class::Comment), "{got:?}");
    }

    #[test]
    fn files_found_by_name_extension_or_interpreter() {
        let d = |p: &str, first: &str| detect(Path::new(p), first).map(|l| l.id);
        assert_eq!(d("src/main.rs", ""), Some("rust"));
        assert_eq!(d("a/Makefile", ""), Some("make"));
        assert_eq!(d("Dockerfile.dev", ""), Some("dockerfile"));
        assert_eq!(d("x.HPP", ""), Some("cpp"));
        assert_eq!(d("CMakeLists.txt", ""), Some("cmake"));
        assert_eq!(d("Cargo.lock", ""), Some("toml"));
        assert_eq!(d("run", "#!/usr/bin/env python3"), Some("python"));
        assert_eq!(d("run", "#!/bin/bash -e"), Some("bash"));
        assert_eq!(d("run", "#!/usr/bin/env -S node --flag"), Some("javascript"));
        assert_eq!(d("notes.txt", "hello"), None);
        assert_eq!(by_name("py").map(|l| l.id), Some("python"));
        assert_eq!(by_name("C++").map(|l| l.id), Some("cpp"));
        assert_eq!(by_name("Shell").map(|l| l.id), Some("bash"));
        assert_eq!(by_name("rs").map(|l| l.id), Some("rust"));
        assert_eq!(by_name(""), None);
    }

    #[test]
    fn capture_names_map_to_classes() {
        assert_eq!(class_of("function.method.call"), Some(Class::Function));
        assert_eq!(class_of("string.special.uri"), Some(Class::Link));
        assert_eq!(class_of("keyword.return"), Some(Class::Keyword));
        assert_eq!(class_of("variable.builtin"), Some(Class::Constant));
        assert_eq!(class_of("variable"), Some(Class::Plain));
        assert_eq!(class_of("spell"), None);
        assert_eq!(class_of("comment.documentation"), Some(Class::Comment));
    }
}
