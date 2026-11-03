//! Write dconf keys through the dconf service (what `gsettings set` does
//! with the default backend), so GTK apps and portals follow our theme.

use std::collections::HashMap;

use zvariant::{LE, Value, serialized::Context};

use crate::{Result, bus};

/// A key's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Setting<'a> {
    Str(&'a str),
    Int(i32),
}

/// Set string keys, e.g. ("/org/gnome/desktop/interface/color-scheme",
/// "prefer-dark"), in one change.
pub fn set_strings(keys: &[(&str, &str)]) -> Result<()> {
    let keys: Vec<(&str, Setting)> = keys.iter().map(|(k, v)| (*k, Setting::Str(v))).collect();
    set(&keys)
}

/// Set keys of any of the kinds [`Setting`] has, in one change.
pub fn set(keys: &[(&str, Setting)]) -> Result<()> {
    let value = |s: &Setting<'_>| match *s {
        Setting::Str(v) => Value::from(v.to_owned()),
        Setting::Int(v) => Value::from(v),
    };
    let changes: HashMap<&str, Option<Value<'_>>> = keys.iter().map(|(k, v)| (*k, Some(value(v)))).collect();
    let blob = changeset(&changes)?;
    let _tag: String = bus::call(bus::session()?, "ca.desrt.dconf", "/ca/desrt/dconf/Writer/user", "ca.desrt.dconf.Writer", "Change", &(blob.as_slice(),))?;
    Ok(())
}

/// A changeset is a GVariant `a{smv}`, serialized little-endian.
// zvariant 5's GVariant support is deprecated in favour of a separate crate
// in 6; it is what we have with zbus 5.
#[allow(deprecated)]
fn changeset(changes: &HashMap<&str, Option<Value<'_>>>) -> Result<Vec<u8>> {
    let data = zvariant::to_bytes(Context::new_gvariant(LE, 0), changes)?;
    Ok(data.bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changeset_is_gvariant() {
        let mut m = HashMap::new();
        m.insert("/a/b", Some(Value::from("x")));
        let blob = changeset(&m).unwrap();
        // GVariant a{smv}: key "/a/b\0", then the maybe-variant: "x\0", a
        // NUL, the type "s", a trailing NUL marking Just, then offsets.
        assert!(blob.starts_with(b"/a/b\0"));
        assert!(blob.windows(4).any(|w| w == b"x\0\0s"));
    }
}
