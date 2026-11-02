//! Remove the wrapper's prepended context from displayed user history.
//! This is presentation cleanup, not an authorization or secrecy boundary.
use serde_json::Value;
use std::collections::BTreeMap;

const OPEN: &str = "[Configured agent instance context]";
const CLOSE: &str = "[/Configured agent instance context]";

#[derive(Default)]
struct Message {
    id: Option<String>,
    first: Option<Value>,
    text: String,
    decided: bool,
}

#[derive(Default)]
pub(crate) struct ContextReplay {
    messages: BTreeMap<String, Message>,
}

impl ContextReplay {
    pub fn finish(&mut self, session: &str) -> Vec<Value> {
        self.messages
            .remove(session)
            .and_then(|mut message| {
                message.first.take().map(|mut value| {
                    value["params"]["update"]["content"]["text"] = message.text.into();
                    value
                })
            })
            .into_iter()
            .collect()
    }

    pub fn push(&mut self, value: Value, limit: usize) -> Vec<Value> {
        let session = value["params"]["sessionId"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        let update = &value["params"]["update"];
        let Some(text) = update["content"]["text"].as_str().filter(|_| {
            update["sessionUpdate"] == "user_message_chunk" && update["content"]["type"] == "text"
        }) else {
            let mut out = self.finish(&session);
            out.push(value);
            return out;
        };
        let id = update["messageId"].as_str().map(str::to_owned);
        let mut out = Vec::new();
        if self.messages.get(&session).is_some_and(|m| m.id != id) {
            out.extend(self.finish(&session));
        }
        let message = self
            .messages
            .entry(session.clone())
            .or_insert_with(|| Message {
                id,
                ..Default::default()
            });
        if message.decided {
            out.push(value);
            return out;
        }
        message.text.push_str(text);
        message.first.get_or_insert(value);
        // Only one envelope is ours. A user's literal copy after it must remain.
        let stripped = message.text.strip_prefix(OPEN).and_then(|rest| {
            rest.find(CLOSE)
                .map(|end| rest[end + CLOSE.len()..].to_owned())
        });
        if let Some(rest) = stripped {
            let mut first = message.first.take().unwrap();
            if !rest.is_empty() {
                first["params"]["update"]["content"]["text"] = rest.into();
                out.push(first);
            }
            message.text.clear();
            message.decided = true;
        } else if message.text.len() > limit
            || !(message.text.starts_with(OPEN) || OPEN.starts_with(&message.text))
        {
            let mut first = message.first.take().unwrap();
            first["params"]["update"]["content"]["text"] = std::mem::take(&mut message.text).into();
            out.push(first);
            message.decided = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn chunk(sid: &str, mid: &str, text: &str) -> Value {
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,
            "update":{"sessionUpdate":"user_message_chunk","messageId":mid,"content":{"type":"text","text":text}}}})
    }
    fn text(v: &Value) -> &str {
        v["params"]["update"]["content"]["text"].as_str().unwrap()
    }

    #[test]
    fn removes_one_context_across_chunks_and_keeps_literal_user_text() {
        let mut f = ContextReplay::default();
        assert!(
            f.push(chunk("s", "m", "[Configured agent"), 4096)
                .is_empty()
        );
        assert!(
            f.push(chunk("s", "m", " instance context]\nName: Helper\n"), 4096)
                .is_empty()
        );
        let literal = format!("I wrote {OPEN} myself");
        let out = f.push(chunk("s", "m", &format!("{CLOSE}{literal}")), 4096);
        assert_eq!(text(&out[0]), literal);
        let further = f.push(chunk("s", "m", CLOSE), 4096);
        assert_eq!(text(&further[0]), CLOSE);
    }

    #[test]
    fn preserves_incomplete_unrelated_and_non_user_content() {
        let mut f = ContextReplay::default();
        assert!(f.push(chunk("s", "m", OPEN), 4096).is_empty());
        assert_eq!(
            text(&f.push(chunk("other", "m", "ordinary user text"), 4096)[0]),
            "ordinary user text"
        );
        assert_eq!(text(&f.finish("s")[0]), OPEN);
        let mut assistant = chunk("s", "a", &format!("{OPEN}quoted{CLOSE}"));
        assistant["params"]["update"]["sessionUpdate"] = "agent_message_chunk".into();
        assert_eq!(f.push(assistant.clone(), 4096), vec![assistant]);
        assert!(f.push(chunk("s", "m", OPEN), 4096).is_empty());
        let out = f.push(chunk("s", "next", "new message"), 4096);
        assert_eq!(text(&out[0]), OPEN);
        assert_eq!(text(&out[1]), "new message");
    }
}
