//! Structured clone: V8's value serialisation, for messages between the
//! page and its workers (typed arrays, maps, dates and the rest travel
//! whole; functions and DOM nodes throw DataCloneError, as in browsers).
use v8::{ValueDeserializerHelper, ValueSerializerHelper};

struct Serializer;
impl v8::ValueSerializerImpl for Serializer {
    fn throw_data_clone_error<'s>(&self, scope: &mut v8::PinScope<'s, '_>, message: v8::Local<'s, v8::String>) {
        let error = v8::Exception::error(scope, message);
        scope.throw_exception(error);
    }
}
struct Deserializer;
impl v8::ValueDeserializerImpl for Deserializer {}

/// The value as bytes; None when it cannot be cloned (an exception is
/// pending in the scope).
pub fn serialize(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> Option<Vec<u8>> {
    let context = scope.get_current_context();
    let serializer = v8::ValueSerializer::new(scope, Box::new(Serializer));
    serializer.write_header();
    if serializer.write_value(context, value) != Some(true) {
        return None;
    }
    Some(serializer.release())
}
/// The value the bytes hold, in this scope's context.
pub fn deserialize<'s>(scope: &mut v8::PinScope<'s, '_>, bytes: &[u8]) -> Option<v8::Local<'s, v8::Value>> {
    let context = scope.get_current_context();
    let deserializer = v8::ValueDeserializer::new(scope, Box::new(Deserializer), bytes);
    if deserializer.read_header(context) != Some(true) {
        return None;
    }
    deserializer.read_value(context)
}
