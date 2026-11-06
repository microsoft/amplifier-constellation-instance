//! Deterministic advertised maintenance commands, never decorated model prompts.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
pub const META: &str = "com.microsoft.amplifier.maintenance";
pub const METHOD: &str = "_amplifier/maintenance_receipt";
#[derive(Default)]
pub struct Maintenance {
    commands: BTreeSet<String>,
    active: BTreeMap<String, Operation>,
    seen: BTreeSet<(String, String)>,
}
struct Operation {
    session: String,
    id: String,
    compaction_id: Option<String>,
    status: String,
}
pub fn capability() -> Value {
    json!({"version":1,"promptMetadata":META,"receiptsMethod":METHOD,"requiresAdvertisedCommand":true,"command":"compact","completion":"typed_compaction_and_prompt_terminal"})
}
fn requested(v: &Value) -> bool {
    v["method"] == "session/prompt"
        && (v["params"]["_meta"].get(META).is_some()
            || v["params"]["prompt"].as_array().is_some_and(|p| {
                p.len() == 1
                    && p[0]["text"]
                        .as_str()
                        .is_some_and(|s| s.trim() == "/compact")
            }))
}
impl Maintenance {
    pub fn busy(&self, sid: &str) -> bool {
        self.active.values().any(|o| o.session == sid)
    }
    pub fn has_active(&self) -> bool {
        !self.active.is_empty()
    }
    pub fn begin(&mut self, v: &mut Value, session_busy: bool) -> Result<bool> {
        if !requested(v) {
            return Ok(false);
        }
        let p = &v["params"];
        let sid = p["sessionId"].as_str().unwrap_or("");
        ensure!(
            !session_busy && !self.busy(sid),
            "Wait for this session to settle before compacting"
        );
        ensure!(
            self.commands.contains(sid),
            "This session has not advertised the compact command"
        );
        let blocks = p["prompt"].as_array();
        ensure!(
            blocks.is_some_and(|b| b.len() == 1
                && b[0]["type"] == "text"
                && b[0]["text"]
                    .as_str()
                    .is_some_and(|t| t.trim() == "/compact")),
            "Maintenance requires exactly the advertised /compact command"
        );
        ensure!(
            p["_meta"].get(crate::attention::META).is_none(),
            "Maintenance cannot consume an attention offer"
        );
        let meta = &p["_meta"][META];
        if !meta.is_null() {
            ensure!(
                meta["version"] == 1 && meta["operation"] == "compact",
                "Unsupported maintenance operation"
            );
        }
        let id = meta["operationId"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        ensure!(
            !id.is_empty() && id.len() <= 128,
            "Invalid maintenance operation ID"
        );
        v["params"]["_meta"][META] = json!({"version":1,"operation":"compact","operationId":id});
        self.active.insert(
            v["id"].to_string(),
            Operation {
                session: v["params"]["sessionId"].as_str().unwrap_or("").into(),
                id,
                compaction_id: None,
                status: "unknown".into(),
            },
        );
        Ok(true)
    }
    pub fn update(&mut self, v: &Value) {
        let sid = v["params"]["sessionId"].as_str().unwrap_or("");
        let u = &v["params"]["update"];
        if u["sessionUpdate"] == "available_commands_update" {
            if u["availableCommands"]
                .as_array()
                .is_some_and(|a| a.iter().any(|c| c["name"] == "compact"))
            {
                self.commands.insert(sid.into());
            } else {
                self.commands.remove(sid);
            }
        }
        if u["sessionUpdate"] != "compaction_update" {
            return;
        }
        let Some(id) = u["compactionId"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512)
        else {
            return;
        };
        let already_seen = self.seen.contains(&(sid.into(), id.into()));
        for o in self.active.values_mut().filter(|o| o.session == sid) {
            // ACP permits a single completed update, without in_progress.
            // A previously observed ID cannot settle a new operation.
            if o.compaction_id.is_none() && !already_seen {
                o.compaction_id = Some(id.into());
            }
            if o.compaction_id.as_deref() == Some(id) {
                o.status = u["status"].as_str().unwrap_or("unknown").into();
            }
        }
        self.seen.insert((sid.into(), id.into()));
    }
    pub fn finish(&mut self, v: &mut Value) -> Option<Value> {
        let o = self.active.remove(&v["id"].to_string())?;
        let status = if v["result"]["stopReason"] == "end_turn" && o.status == "completed" {
            "completed"
        } else if v["result"]["stopReason"] == "cancelled" {
            "cancelled"
        } else if v.get("error").is_some() || o.status == "failed" {
            "failed"
        } else {
            "unknown"
        };
        let receipt = json!({"version":1,"operation":"compact","operationId":o.id,"sessionId":o.session,"status":status,"compactionId":o.compaction_id});
        if v["result"].is_object() {
            v["result"]["_meta"][META] = receipt.clone();
        }
        Some(receipt)
    }
    pub fn closed(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.active).into_values().map(|o|json!({"operationId":o.id,"sessionId":o.session,"status":"unknown","reason":"transport_closed"})).collect()
    }
}

/// Maintenance and ordinary turns cannot overlap on the same saved session,
/// even when different clients load it. Ordinary-turn concurrency is otherwise
/// left to the backend. Guards release on all reply/disconnect paths.
pub struct Guard {
    state: std::sync::Arc<crate::state::AppState>,
    sid: String,
}
pub fn admit(
    state: &std::sync::Arc<crate::state::AppState>,
    sid: &str,
    maintenance: bool,
) -> Result<Guard> {
    let mut active = state.maintenance_activity.lock().unwrap();
    let entry = active.entry(sid.into()).or_default();
    ensure!(
        !entry.1 && (!maintenance || entry.0 == 0),
        "Conversation is busy; wait for it to settle before maintenance"
    );
    entry.0 += 1;
    entry.1 = maintenance;
    Ok(Guard {
        state: state.clone(),
        sid: sid.into(),
    })
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut active = self.state.maintenance_activity.lock().unwrap();
        if let Some(entry) = active.get_mut(&self.sid) {
            entry.0 -= 1;
            if entry.0 == 0 {
                active.remove(&self.sid);
            }
        }
    }
}

pub fn claim(state: &crate::state::AppState, principal: &str, params: &Value) -> Result<()> {
    let id = params["_meta"][META]["operationId"].as_str().unwrap_or("");
    let row = json!({"version":1,"operation":"compact","operationId":id,"sessionId":params["sessionId"],"principal":principal,"status":"unknown","reason":"submitted"});
    let inserted = state.db.lock().unwrap().execute("INSERT OR IGNORE INTO records(collection,id,value,updated_at) VALUES('maintenance',?1,?2,?3)",rusqlite::params![id,row.to_string(),crate::state::now_ms()])?;
    ensure!(
        inserted == 1,
        "This maintenance operation was already recorded; inspect its receipt rather than repeat it"
    );
    Ok(())
}
pub fn save(state: &crate::state::AppState, principal: &str, receipt: &Value) -> Result<()> {
    let mut row = receipt.clone();
    row["principal"] = json!(principal);
    state.set_record("maintenance", receipt["operationId"].as_str().unwrap(), row)
}
pub fn query(state: &crate::state::AppState, principal: &str, params: &Value) -> Result<Value> {
    let mut row = state
        .get_record("maintenance", params["operationId"].as_str().unwrap_or(""))?
        .ok_or_else(|| anyhow::anyhow!("No maintenance receipt"))?;
    ensure!(
        row["sessionId"] == params["sessionId"] && row["principal"] == principal,
        "No maintenance receipt"
    );
    row.as_object_mut().unwrap().remove("principal");
    Ok(row)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn req() -> Value {
        json!({"id":9,"method":"session/prompt","params":{"sessionId":"s","prompt":[{"type":"text","text":"/compact"}]}})
    }
    fn update(id: &str, status: &str) -> Value {
        json!({"params":{"sessionId":"s","update":{"sessionUpdate":"compaction_update","compactionId":id,"status":status}}})
    }
    #[test]
    fn acceptance_is_not_completion_and_unrelated_terminal_does_not_settle() {
        let mut m = Maintenance::default();
        assert!(m.begin(&mut req(), false).is_err());
        m.commands.insert("s".into());
        assert!(m.begin(&mut req(), true).is_err());
        m.begin(&mut req(), false).unwrap();
        m.update(&update("a", "in_progress"));
        m.update(&update("other", "completed"));
        let mut end = json!({"id":9,"result":{"stopReason":"end_turn"}});
        assert_eq!(m.finish(&mut end).unwrap()["status"], "unknown");
        m.begin(&mut req(), false).unwrap();
        m.update(&update("b", "in_progress"));
        m.update(&update("b", "completed"));
        assert!(m.busy("s"));
        assert_eq!(m.finish(&mut end).unwrap()["status"], "completed");
    }
    #[test]
    fn disconnect_and_cancel_never_report_success() {
        let mut m = Maintenance::default();
        m.commands.insert("s".into());
        m.begin(&mut req(), false).unwrap();
        m.update(&update("c", "in_progress"));
        m.update(&update("c", "completed"));
        let mut v = json!({"id":9,"result":{"stopReason":"cancelled"}});
        assert_eq!(m.finish(&mut v).unwrap()["status"], "cancelled");
        m.begin(&mut req(), false).unwrap();
        assert_eq!(m.closed()[0]["status"], "unknown");
    }
    #[test]
    fn one_completed_update_is_valid_but_replayed_ids_are_not() {
        let mut m = Maintenance::default();
        m.commands.insert("s".into());
        m.begin(&mut req(), false).unwrap();
        m.update(&update("single", "completed"));
        assert_eq!(
            m.finish(&mut json!({"id":9,"result":{"stopReason":"end_turn"}}))
                .unwrap()["status"],
            "completed"
        );
        m.begin(&mut req(), false).unwrap();
        m.update(&update("single", "completed"));
        assert_eq!(
            m.finish(&mut json!({"id":9,"result":{"stopReason":"end_turn"}}))
                .unwrap()["status"],
            "unknown"
        );
        m.begin(&mut req(), false).unwrap();
        m.update(&update("future", "new_status"));
        assert_eq!(
            m.finish(&mut json!({"id":9,"result":{"stopReason":"end_turn"}}))
                .unwrap()["status"],
            "unknown"
        );
    }
}
