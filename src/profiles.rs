//! Versioned style profiles. Immutable file pairs become active through one
//! atomic pointer replacement, so a prompt cannot combine two revisions.
use crate::{
    auth,
    config::write_private,
    error::{ApiError, ApiResult},
    state::{AppState, now_ms},
};
use anyhow::{Result, ensure};
use axum::{Json, extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc};

const NAMES: [&str; 2] = ["communication.md", "working-style.md"];
const MAX_PROFILE_BYTES: usize = 128 * 1024;

#[derive(Default, Serialize, Deserialize)]
struct Pointer {
    revision: u64,
    active: Option<String>,
    previous: Vec<String>,
    applied_at: Option<i64>,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    id: String,
    created_at: i64,
    kind: String,
    source: Option<Value>,
    files: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    expected_revision: u64,
    package: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Restore {
    expected_revision: u64,
    id: String,
}

fn read_optional(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            ensure!(text.len() <= MAX_PROFILE_BYTES, "Profile too large");
            Ok(text)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}
fn pointer(state: &AppState) -> Result<Pointer> {
    match std::fs::read(state.home.join("profiles/current.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Pointer::default()),
        Err(e) => Err(e.into()),
    }
}
fn snapshot(state: &AppState, id: &str) -> Result<Snapshot> {
    ensure!(
        uuid::Uuid::parse_str(id).is_ok(),
        "Invalid profile snapshot"
    );
    let bytes = std::fs::read(
        state
            .home
            .join("profiles/versions")
            .join(id)
            .join("snapshot.json"),
    )?;
    let value: Snapshot = serde_json::from_slice(&bytes)?;
    ensure!(value.id == id, "Profile snapshot identity changed");
    Ok(value)
}
fn pair(state: &AppState, pointer: &Pointer) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    if let Some(id) = &pointer.active {
        let metadata = snapshot(state, id)?;
        for name in NAMES {
            let text =
                std::fs::read_to_string(state.home.join("profiles/versions").join(id).join(name))?;
            ensure!(text.len() <= MAX_PROFILE_BYTES, "Profile too large");
            ensure!(
                metadata.files.get(name) == Some(&auth::digest(&text)),
                "Profile snapshot changed"
            );
            files.insert(name.into(), text);
        }
    } else {
        for name in NAMES {
            files.insert(
                name.into(),
                read_optional(&state.home.join("profiles").join(name))?,
            );
        }
    }
    Ok(files)
}
fn status_unlocked(state: &AppState, pointer: &Pointer) -> Result<Value> {
    let active = pointer
        .active
        .as_deref()
        .map(|id| snapshot(state, id))
        .transpose()?;
    let previous = pointer
        .previous
        .iter()
        .take(50)
        .map(|id| snapshot(state, id))
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"revision":pointer.revision,"active":active,"previous":previous,"applied_at":pointer.applied_at}),
    )
}
pub fn status(state: &AppState) -> Result<Value> {
    let _lock = state.profile_lock.lock().unwrap();
    status_unlocked(state, &pointer(state)?)
}
pub fn context(state: &AppState) -> Result<(String, Value)> {
    let _lock = state.profile_lock.lock().unwrap();
    let pointer = pointer(state)?;
    let mut text = String::new();
    for (name, content) in pair(state, &pointer)? {
        if !content.trim().is_empty() {
            text.push_str(&format!("\n{name}:\n{content}\n"));
        }
    }
    let identity = read_optional(&state.home.join("profiles/identity.md"))?;
    if !identity.trim().is_empty() {
        text.push_str(&format!("\nidentity.md:\n{identity}\n"));
    }
    Ok((
        text,
        json!({"revision":pointer.revision,"id":pointer.active}),
    ))
}
pub fn read(state: &AppState, name: &str) -> Result<String> {
    let _lock = state.profile_lock.lock().unwrap();
    if name == "identity" {
        return read_optional(&state.home.join("profiles/identity.md"));
    }
    pair(state, &pointer(state)?)?
        .remove(&format!("{name}.md"))
        .ok_or_else(|| anyhow::anyhow!("Unknown profile"))
}
fn save_snapshot(
    state: &AppState,
    files: &BTreeMap<String, String>,
    source: Option<Value>,
    kind: &str,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let directory = state.home.join("profiles/versions").join(&id);
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut hashes = BTreeMap::new();
    for name in NAMES {
        let text = files
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Incomplete profile pair"))?;
        write_private(&directory.join(name), text.as_bytes())?;
        hashes.insert(name.into(), auth::digest(text));
    }
    let metadata = Snapshot {
        id: id.clone(),
        created_at: now_ms(),
        kind: kind.into(),
        source,
        files: hashes,
    };
    write_private(
        &directory.join("snapshot.json"),
        &serde_json::to_vec_pretty(&metadata)?,
    )?;
    sync_directory(&directory)?;
    sync_directory(directory.parent().unwrap())?;
    Ok(id)
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn activate(state: &AppState, mut pointer: Pointer, id: String) -> Result<Value> {
    pointer.previous.retain(|previous| previous != &id);
    if let Some(old) = pointer.active.replace(id) {
        pointer.previous.insert(0, old);
    }
    pointer.revision += 1;
    pointer.applied_at = Some(now_ms());
    write_private(
        &state.home.join("profiles/current.json"),
        &serde_json::to_vec_pretty(&pointer)?,
    )?;
    sync_directory(&state.home.join("profiles"))?;
    status_unlocked(state, &pointer)
}
fn preserve_baseline(state: &AppState, pointer: &mut Pointer) -> Result<()> {
    if pointer.active.is_none() {
        pointer.active = Some(save_snapshot(
            state,
            &pair(state, pointer)?,
            None,
            "baseline",
        )?);
    }
    Ok(())
}

/// Existing editors use the same version store; imports cannot silently mask
/// later edits. Private identity stays independent of style-package rollback.
pub fn write(state: &AppState, name: &str, text: &str) -> Result<()> {
    let _lock = state.profile_lock.lock().unwrap();
    if name == "identity" {
        return write_private(&state.home.join("profiles/identity.md"), text.as_bytes());
    }
    ensure!(
        ["communication", "working-style"].contains(&name),
        "Unknown profile"
    );
    let mut pointer = pointer(state)?;
    let mut files = pair(state, &pointer)?;
    files.insert(format!("{name}.md"), text.into());
    preserve_baseline(state, &mut pointer)?;
    let id = save_snapshot(state, &files, None, "manual")?;
    activate(state, pointer, id)?;
    Ok(())
}

fn validate(package: &Value) -> ApiResult<(BTreeMap<String, String>, Value)> {
    let manifest = &package["manifest"];
    if package["schema"] != "twin-profile/1" || manifest["schema"] != "twin-profile/1" {
        return Err(ApiError::bad(
            "Choose a supported Persona Workshop profile package (twin-profile/1)",
        ));
    }
    if manifest["raw_evidence_included"] != false || manifest["identity_included"] != false {
        return Err(ApiError::bad(
            "The profile package must exclude raw evidence and private identity",
        ));
    }
    for field in ["subject_key", "revision"] {
        if manifest[field]
            .as_str()
            .is_none_or(|s| uuid::Uuid::parse_str(s).is_err())
        {
            return Err(ApiError::bad(
                "Profile package needs valid subject and version identifiers",
            ));
        }
    }
    if manifest["required_context"] != json!(NAMES)
        || manifest
            .get("instruction_format")
            .is_some_and(|v| !matches!(v.as_u64(), Some(1 | 2)))
    {
        return Err(ApiError::bad(
            "Unsupported profile context or instruction format",
        ));
    }
    let files = package["files"]
        .as_object()
        .ok_or_else(|| ApiError::bad("Profile package has no files"))?;
    let hashes = manifest["files"]
        .as_object()
        .ok_or_else(|| ApiError::bad("Profile package has no file digests"))?;
    const ALLOWED: [&str; 5] = [
        "communication.md",
        "working-style.md",
        "claims.json",
        "evaluation-summary.json",
        "manifest.json",
    ];
    if files.keys().any(|name| !ALLOWED.contains(&name.as_str()))
        || hashes
            .keys()
            .any(|name| !files.contains_key(name) || name == "manifest.json")
    {
        return Err(ApiError::bad(
            "Profile package contains unexpected files or digests",
        ));
    }
    for (name, value) in files {
        let text = value
            .as_str()
            .ok_or_else(|| ApiError::bad("Package files must contain text"))?;
        if name == "manifest.json" {
            if serde_json::from_str::<Value>(text).ok().as_ref() != Some(manifest) {
                return Err(ApiError::bad("The package manifests do not match"));
            }
        } else if hashes.get(name).and_then(Value::as_str) != Some(auth::digest(text).as_str()) {
            return Err(ApiError::bad(
                "A profile package file failed its integrity check",
            ));
        }
    }
    let mut pair = BTreeMap::new();
    for name in NAMES {
        let text = files
            .get(name)
            .and_then(Value::as_str)
            .filter(|s| s.len() <= MAX_PROFILE_BYTES)
            .ok_or_else(|| {
                ApiError::bad("Both profile files are required, each at most 128 KiB")
            })?;
        pair.insert(name.into(), text.into());
    }
    let source = json!({"schema":manifest["schema"],"subject_key":manifest["subject_key"],"revision":manifest["revision"],"instruction_format":manifest.get("instruction_format").cloned().unwrap_or(json!(1))});
    Ok((pair, source))
}
pub async fn get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "profiles", false)?;
    Ok(Json(status(&state)?))
}
pub async fn apply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Apply>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "profiles", true)?.actor;
    let (files, source) = validate(&body.package)?;
    let result = {
        let _lock = state.profile_lock.lock().unwrap();
        let mut pointer = pointer(&state)?;
        if pointer.revision != body.expected_revision {
            return Err(ApiError::conflict(json!({"revision":pointer.revision})));
        }
        preserve_baseline(&state, &mut pointer)?;
        let id = save_snapshot(&state, &files, Some(source), "import")?;
        activate(&state, pointer, id)?
    };
    state.event(
        "profile.package_applied",
        json!({"actor":actor,"revision":result["revision"],"id":result["active"]["id"]}),
    )?;
    Ok(Json(result))
}
pub async fn restore(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Restore>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "profiles", true)?.actor;
    let result = {
        let _lock = state.profile_lock.lock().unwrap();
        let pointer = pointer(&state)?;
        if pointer.revision != body.expected_revision {
            return Err(ApiError::conflict(json!({"revision":pointer.revision})));
        }
        if !pointer.previous.contains(&body.id) {
            return Err(ApiError::bad(
                "Choose a previously installed profile version",
            ));
        }
        // Verify both immutable files before making this version active again.
        pair(
            &state,
            &Pointer {
                active: Some(body.id.clone()),
                ..Pointer::default()
            },
        )?;
        activate(&state, pointer, body.id)?
    };
    state.event(
        "profile.package_restored",
        json!({"actor":actor,"revision":result["revision"],"id":result["active"]["id"]}),
    )?;
    Ok(Json(result))
}
