//! How well a thing matches what was typed: every word typed must be
//! found, best at the start of the title, then at the start of one of its
//! words, then anywhere in it, then among its keywords, then as letters in
//! order ("nwin" for New Window).

/// The words typed, lowercased.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// A score for `title` (with `keywords`, lowercased, space separated)
/// against `words`, higher is better; None if a word isn't there.
pub fn score(words: &[String], title: &str, keywords: &str) -> Option<i32> {
    score_in(words, title, &[keywords])
}

/// [`score`] with the keywords in parts (an item's own, its subtitle), so
/// nothing is joined for each item at each key typed.
pub fn score_in(words: &[String], title: &str, keywords: &[&str]) -> Option<i32> {
    if words.is_empty() {
        return None;
    }
    let title = title.to_lowercase();
    let mut total = 0;
    for w in words {
        let w = w.as_str();
        total += if title.starts_with(w) {
            100
        } else if title.split(|c: char| !c.is_alphanumeric()).any(|t| t.starts_with(w)) {
            70
        } else if title.contains(w) {
            40
        } else if keywords.iter().any(|k| k.split(' ').any(|k| k.starts_with(w))) {
            45
        } else if keywords.iter().any(|k| k.contains(w)) {
            20
        } else if in_order(w, &title) {
            10
        } else {
            return None;
        };
    }
    let whole = words.join(" ");
    if title == whole {
        total += 60;
    }
    // Shorter titles are closer matches.
    total -= (title.chars().count() as i32) / 6;
    Some(total)
}

/// Whether the letters of `w` appear in `s` in order, each run starting
/// at a word (so "nw" finds "New Window", not "dowNload Window" alone).
fn in_order(w: &str, s: &str) -> bool {
    if w.chars().count() < 2 {
        return false;
    }
    let initials: String = s.split(|c: char| !c.is_alphanumeric()).filter_map(|t| t.chars().next()).collect();
    if initials.contains(w) {
        return true;
    }
    let mut it = s.chars();
    w.chars().all(|c| it.any(|x| x == c)) && w.chars().count() >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks() {
        let q = words("tile");
        let left = score(&q, "Left half", "tile left windows").unwrap();
        assert!(score(&q, "Tile all windows on the display", "").unwrap() > left);
        assert!(score(&words("term"), "Terminal", "").unwrap() > score(&words("term"), "Determine", "").unwrap_or(0));
        assert!(score(&words("nw"), "New Window", "").is_some());
        assert!(score(&words("zzz"), "Terminal", "").is_none());
        assert!(score(&words("left half"), "Left half", "").unwrap() > score(&words("left half"), "Left half of it", "").unwrap());
        assert!(score(&[], "x", "").is_none());
    }
}
