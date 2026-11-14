//! Property lists as AirPlay's messages carry them: binary, read and
//! written by the `plist` crate; here, the few shapes a play needs.

use plist::Value;

/// A dictionary of these.
pub(crate) fn dict<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Dictionary(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

pub(crate) fn write(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    value.to_writer_binary(&mut out).expect("a property list written to memory");
    out
}

pub(crate) fn read(bytes: &[u8]) -> Option<Value> {
    Value::from_reader(std::io::Cursor::new(bytes)).ok()
}

/// What is at these keys, one inside the other.
pub(crate) fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |v, key| v.as_dictionary()?.get(key))
}

pub(crate) fn text<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    at(value, path)?.as_string()
}

/// A number, whichever way it was written.
pub(crate) fn number(value: &Value) -> Option<f64> {
    value.as_real().or_else(|| value.as_signed_integer().map(|n| n as f64)).or_else(|| value.as_unsigned_integer().map(|n| n as f64))
}

/// Seconds: a number of them, or a time as Apple's players count it
/// (`value` ticks of `timescale` to the second).
pub(crate) fn seconds(value: &Value) -> Option<f64> {
    number(value).or_else(|| {
        let (ticks, scale) = (number(at(value, &["value"])?)?, number(at(value, &["timescale"])?)?);
        (scale > 0.0).then(|| ticks / scale)
    })
}

/// Seconds as such a time, to the millisecond.
pub(crate) fn time(seconds: f64) -> Value {
    dict([("value", Value::from((seconds * 1000.0).round() as i64)), ("timescale", Value::from(1000i64)), ("flags", Value::from(1i64)), ("epoch", Value::from(0i64))])
}

/// A list in a line, for a person to read: its keys and values, with
/// what is only bytes said as how many.
pub(crate) fn line(value: &Value) -> String {
    match value {
        Value::Dictionary(d) => format!("{{{}}}", d.iter().map(|(k, v)| format!("{k}: {}", line(v))).collect::<Vec<_>>().join(", ")),
        Value::Array(a) => format!("[{}]", a.iter().map(line).collect::<Vec<_>>().join(", ")),
        Value::String(s) => format!("{s:?}"),
        Value::Boolean(b) => b.to_string(),
        Value::Data(d) => format!("<{} bytes>", d.len()),
        other => number(other).map_or_else(|| format!("{other:?}"), |n| n.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_and_read_back() {
        let inner = dict([("type", Value::from("setRate")), ("rate", Value::from(1.0))]);
        let outer = dict([("params", dict([("data", Value::Data(write(&inner)))]))]);
        let bytes = write(&outer);
        assert!(bytes.starts_with(b"bplist00"));
        let back = read(&bytes).unwrap();
        let nested = read(at(&back, &["params", "data"]).unwrap().as_data().unwrap()).unwrap();
        assert_eq!(text(&nested, &["type"]), Some("setRate"));
        assert_eq!(at(&nested, &["rate"]).and_then(number), Some(1.0));
        assert_eq!(read(b"not a list"), None);
    }

    #[test]
    fn in_a_line() {
        let list = dict([
            ("eventPort", Value::from(42815i64)),
            ("name", Value::from("Front \"Room\"")),
            ("ok", Value::from(true)),
            ("rate", Value::from(1.5)),
            ("key", Value::Data(vec![1, 2, 3])),
            ("streams", Value::Array(vec![dict([("streamID", Value::from(7i64))])])),
        ]);
        assert_eq!(line(&list), r#"{eventPort: 42815, name: "Front \"Room\"", ok: true, rate: 1.5, key: <3 bytes>, streams: [{streamID: 7}]}"#);
    }

    #[test]
    fn seconds_either_way() {
        assert_eq!(seconds(&Value::from(2.5)), Some(2.5));
        assert_eq!(seconds(&Value::from(90i64)), Some(90.0));
        assert_eq!(seconds(&time(12.3456)), Some(12.346));
        assert_eq!(seconds(&dict([("value", Value::from(5i64)), ("timescale", Value::from(0i64))])), None);
        assert_eq!(seconds(&Value::from("soon")), None);
    }
}
