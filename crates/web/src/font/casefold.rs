use super::*;
use std::borrow::Cow;
#[path = "casefold_data.rs"]
mod data;

/// CSS family names require full default Unicode case folding, without
/// normalization and without language-specific Turkic mappings.
pub(super) fn fold<'a>(name: &'a str, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Cow<'a, str>> {
    check(cancelled)?;
    if name.len() > limits.name_bytes || name.len() > limits.work {
        return Err(Error::Limit);
    }
    if name.is_ascii() {
        return Ok(if name.bytes().any(|b| b.is_ascii_uppercase()) { Cow::Owned(name.to_ascii_lowercase()) } else { Cow::Borrowed(name) });
    }
    let mut output = String::with_capacity(name.len());
    for c in name.chars() {
        check(cancelled)?;
        if let Ok(i) = data::FOLD.binary_search_by_key(&c, |v| v.0) {
            output.push_str(data::FOLD[i].1);
        } else {
            output.push(c);
        }
        if output.len() > limits.name_bytes {
            return Err(Error::Limit);
        }
    }
    Ok(Cow::Owned(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_normative_default_full_mapping() {
        let mut count = 0;
        for line in include_str!("../../tests/font-native-fixtures/ucd/CaseFolding.txt").lines() {
            let record = line.split('#').next().unwrap().trim();
            if record.is_empty() {
                continue;
            }
            let fields: Vec<_> = record.split(';').map(str::trim).collect();
            if !matches!(fields[1], "C" | "F") {
                continue;
            }
            let input = char::from_u32(u32::from_str_radix(fields[0], 16).unwrap()).unwrap().to_string();
            let expected: String = fields[2].split_whitespace().map(|s| char::from_u32(u32::from_str_radix(s, 16).unwrap()).unwrap()).collect();
            assert_eq!(fold(&input, Limits::default(), None).unwrap(), expected, "{}", fields[0]);
            count += 1;
        }
        assert_eq!(count, 1606);
        assert_eq!(fold("Iİı", Limits::default(), None).unwrap(), "ii\u{307}ı");
        assert_ne!(fold("Å", Limits::default(), None).unwrap(), fold("A\u{30a}", Limits::default(), None).unwrap());
    }
}
