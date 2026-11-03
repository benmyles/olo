//! The text work of `op inject` and `op run`: finding `op://` references
//! in templates and env files, putting the values in, and keeping
//! secrets out of a command's output.

/// A template's references, in order of appearance: `{{ op://… }}`, with
/// `$VAR` and `${VAR}` inside taken from the environment.
pub fn references(template: &str, env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let inner = after[..end].trim();
        if inner.starts_with("op://") {
            out.push(expand(inner, env));
        }
        rest = &after[end + 2..];
    }
    out
}

/// The template with each reference replaced by its value (`values` in
/// the order [`references`] found them).
pub fn fill(template: &str, values: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    let mut next = values.iter();
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let inner = after[..end].trim();
        out.push_str(&rest[..start]);
        if inner.starts_with("op://") {
            out.push_str(next.next().map_or("", String::as_str));
        } else {
            out.push_str(&rest[start..start + 2 + end + 2]);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// `$VAR` and `${VAR}` from the environment (unset: left as it was).
pub fn expand(s: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            let (name, len) = if b.get(i + 1) == Some(&b'{') {
                match s[i + 2..].find('}') {
                    Some(e) => (&s[i + 2..i + 2 + e], e + 3),
                    None => ("", 0),
                }
            } else {
                let n = s[i + 1..].bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_').count();
                (&s[i + 1..i + 1 + n], n + 1)
            };
            if !name.is_empty()
                && let Some(v) = env(name)
            {
                out.push_str(&v);
                i += len;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap_or_default();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// An env file's variables, in order: `KEY=value` lines (an optional
/// `export `, `#` comments, single or double quotes).
pub fn env_file(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        let value = if value.len() >= 2 && (value.starts_with('"') && value.ends_with('"') || value.starts_with('\'') && value.ends_with('\'')) {
            value[1..value.len() - 1].to_string()
        } else {
            // An unquoted value ends at a comment.
            value.split(" #").next().unwrap_or(value).trim().to_string()
        };
        out.push((key.trim().to_string(), value));
    }
    out
}

pub const CONCEALED: &str = "<concealed by 1Password>";

/// Replaces secrets in a stream of output, holding back just enough of
/// each chunk's end that a secret split between chunks is still caught.
pub struct Masker {
    secrets: Vec<Vec<u8>>,
    held: Vec<u8>,
}

impl Masker {
    pub fn new(secrets: &[String]) -> Masker {
        let mut secrets: Vec<Vec<u8>> = secrets.iter().filter(|s| s.len() >= 3).map(|s| s.as_bytes().to_vec()).collect();
        // Longest first, so one secret holding another is caught whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Masker { secrets, held: Vec::new() }
    }

    /// The output that can go out now.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.held.extend_from_slice(chunk);
        let keep = self.secrets.first().map_or(0, |s| s.len() - 1);
        let masked = self.mask(&self.held);
        // Hold back the longest tail that a secret could go on from.
        let start = masked.len().saturating_sub(keep);
        let cut = (start..masked.len()).find(|&c| self.secrets.iter().any(|s| s.starts_with(&masked[c..]))).unwrap_or(masked.len());
        self.held = masked[cut..].to_vec();
        masked[..cut].to_vec()
    }

    /// What's left at the end.
    pub fn finish(&mut self) -> Vec<u8> {
        let out = self.mask(&self.held);
        self.held.clear();
        out
    }

    fn mask(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        'outer: while i < data.len() {
            for s in &self.secrets {
                if data[i..].starts_with(s) {
                    out.extend_from_slice(CONCEALED.as_bytes());
                    i += s.len();
                    continue 'outer;
                }
            }
            out.push(data[i]);
            i += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        (name == "STAGE").then(|| "prod".to_string())
    }

    #[test]
    fn templates() {
        let t = "A={{ op://Dev/App/ai/KEY }}\nB={{op://Dev/App-$STAGE/password}}\nC={{ not a ref }}\nD=plain\n";
        let refs = references(t, &env);
        assert_eq!(refs, ["op://Dev/App/ai/KEY", "op://Dev/App-prod/password"]);
        let out = fill(t, &["k".into(), "p".into()]);
        assert_eq!(out, "A=k\nB=p\nC={{ not a ref }}\nD=plain\n");
        assert_eq!(expand("op://${STAGE}/x/$NOPE", &env), "op://prod/x/$NOPE");
    }

    #[test]
    fn env_files() {
        let vars = env_file("# c\nexport A=op://V/I/f\nB=\"quoted # not a comment\"\nC=x # comment\n\nD='s'\n");
        assert_eq!(vars, [("A".into(), "op://V/I/f".into()), ("B".into(), "quoted # not a comment".into()), ("C".into(), "x".into()), ("D".into(), "s".into())]);
    }

    #[test]
    fn masking() {
        let mut m = Masker::new(&["hunter2".into(), "sk-abc".into()]);
        let mut out = Vec::new();
        for chunk in ["pass: hun", "ter2 and sk", "-abc!", " done"] {
            out.extend(m.push(chunk.as_bytes()));
        }
        out.extend(m.finish());
        assert_eq!(String::from_utf8(out).unwrap(), format!("pass: {CONCEALED} and {CONCEALED}! done"));
        // Plain output isn't held back.
        let mut m = Masker::new(&["hunter2".into()]);
        assert_eq!(m.push(b"hello world\n"), b"hello world\n");
    }
}
