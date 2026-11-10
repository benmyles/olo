//! OpenAI Chat Completions streaming. Only a complete choice may admit tools.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) struct Stream {
    message: Value,
    calls: BTreeMap<u64, Value>,
    finish: Option<String>,
    usage: Value,
}
impl Default for Stream {
    fn default() -> Self {
        Self { message: json!({"role":"assistant","content":null}), calls: BTreeMap::new(), finish: None, usage: json!({}) }
    }
}
impl Stream {
    pub fn push(&mut self, value: Value, event: &mut impl FnMut(&str, Value)) -> Result<()> {
        ensure!(value.get("error").is_none(), "custom endpoint returned an error event");
        if value["usage"].is_object() {
            self.usage = value["usage"].clone();
        }
        let choices = value["choices"].as_array().context("chat chunk has no choices")?;
        if choices.is_empty() {
            return Ok(());
        }
        ensure!(choices.len() == 1 && choices[0]["index"] == 0, "expected one chat completion choice");
        let choice = &choices[0];
        let delta = choice["delta"].as_object().context("chat chunk has no delta")?;
        ensure!(self.finish.is_none() || delta.is_empty(), "chat data after finish reason");
        for (key, value) in delta {
            if value.is_null() {
                continue;
            }
            match key.as_str() {
                "role" => ensure!(value == "assistant", "unexpected completion role"),
                "tool_calls" => {
                    for call in value.as_array().context("tool_calls must be an array")? {
                        let index = call["index"].as_u64().context("tool call has no index")?;
                        ensure!(index < 4096, "too many tool calls");
                        let target = self.calls.entry(index).or_insert_with(|| json!({"type":"function","function":{"name":"","arguments":""}}));
                        if let Some(id) = call.get("id").filter(|v| !v.is_null()) {
                            ensure!(id.is_string() && (target.get("id").is_none() || target["id"] == *id), "conflicting tool call id");
                            target["id"] = id.clone();
                        }
                        if let Some(kind) = call.get("type").filter(|v| !v.is_null()) {
                            ensure!(kind == "function", "unsupported tool call type");
                        }
                        if let Some(function) = call.get("function") {
                            for key in ["name", "arguments"] {
                                if let Some(part) = function.get(key).filter(|v| !v.is_null()) {
                                    append(&mut target["function"], key, part)?;
                                }
                            }
                        }
                    }
                }
                "reasoning_details" => {
                    // Gateway reasoning blocks (including opaque encrypted data)
                    // are an ordered sequence, not a last-chunk-wins field.
                    let details = value.as_array().context("reasoning_details must be an array")?;
                    if self.message.get(key).is_none() {
                        self.message[key] = json!([]);
                    }
                    self.message[key].as_array_mut().unwrap().extend(details.iter().cloned());
                }
                "content" | "reasoning_content" | "reasoning" | "refusal" => {
                    append(&mut self.message, key, value)?;
                    if key == "content" {
                        event("delta", json!({"text":value}));
                    }
                    if matches!(key.as_str(), "reasoning_content" | "reasoning") {
                        event("reasoning", json!({"text":value}));
                    }
                }
                _ => {
                    self.message[key] = value.clone();
                }
            }
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            ensure!(self.finish.is_none(), "duplicate chat finish reason");
            self.finish = Some(reason.into());
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<Value> {
        let reason = self.finish.context("chat stream ended without a finish reason; no tools executed")?;
        if !self.calls.is_empty() {
            ensure!(self.calls.keys().copied().eq(0..self.calls.len() as u64), "missing streamed tool call index");
            self.message["tool_calls"] = self.calls.into_values().collect::<Vec<_>>().into();
        }
        Ok(json!({"choices":[{"index":0,"message":self.message,"finish_reason":reason}],"usage":self.usage}))
    }
}
fn append(message: &mut Value, key: &str, value: &Value) -> Result<()> {
    let part = value.as_str().context("chat text delta must be a string")?;
    if let Some(Value::String(text)) = message.get_mut(key) {
        text.push_str(part);
    } else {
        message[key] = part.into();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_reasoning_blocks_keep_their_order_and_unknown_fields() {
        let blocks = [json!({"type":"reasoning.summary","summary":"brief","index":0}), json!({"type":"reasoning.encrypted","data":"opaque+/=","index":1,"future":{"keep":true}})];
        let mut stream = Stream::default();
        for block in &blocks {
            stream.push(json!({"choices":[{"index":0,"delta":{"reasoning_details":[block]}}]}), &mut |_, _| {}).unwrap();
        }
        stream.push(json!({"choices":[{"index":0,"delta":{"content":"done"},"finish_reason":"stop"}]}), &mut |_, _| {}).unwrap();
        let reply = stream.finish().unwrap();
        assert_eq!(reply["choices"][0]["message"]["reasoning_details"], json!(blocks));
    }
}
