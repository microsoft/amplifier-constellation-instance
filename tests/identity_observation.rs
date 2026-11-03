use agent_instance::{acp, host, identity, state::AppState};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

struct Peer {
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Value>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}
impl Peer {
    async fn send(&self, id: u64, method: &str, params: Value) {
        self.tx
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
            .unwrap();
    }
    async fn read(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(3), self.rx.recv())
            .await
            .unwrap()
            .unwrap()
    }
}
fn connect(state: Arc<AppState>, headers: HeaderMap) -> Peer {
    let (tx, incoming) = mpsc::channel(16);
    let (outgoing, rx) = mpsc::channel(16);
    let task = tokio::spawn(acp::relay_authenticated(
        state,
        "observer".into(),
        headers,
        incoming,
        outgoing,
    ));
    Peer { tx, rx, task }
}
fn image() -> String {
    let mut output = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(64, 64, image::Rgba([30, 80, 90, 255]))
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    STANDARD.encode(output.into_inner())
}

#[tokio::test]
async fn authenticated_identity_events_need_no_backend_and_reconnect_to_current_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    host::init(
        dir.path(),
        "Example teammate".into(),
        None,
        "127.0.0.1".into(),
        0,
    )
    .unwrap();
    let state = Arc::new(AppState::open(dir.path().to_owned()).unwrap());
    // Blank setup, disabled runtime, no backend: any backend launch would fail.
    assert!(!state.enabled());
    assert!(state.instance_config().backend.is_none());
    let token = std::fs::read_to_string(dir.path().join("secrets/control-token")).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", token.trim()).parse().unwrap(),
    );
    let mut peer = connect(state.clone(), headers.clone());
    peer.send(
        1,
        "initialize",
        json!({"protocolVersion":1,"_meta":{identity::NAMESPACE:{"mode":"observe"}}}),
    )
    .await;
    let init = peer.read().await;
    assert_eq!(
        init["result"]["_meta"][identity::NAMESPACE]["observation_only"],
        true
    );
    peer.send(2, identity::SUBSCRIBE, json!({})).await;
    let initial = peer.read().await;
    assert_eq!(initial["result"]["display_name"], "Example teammate");
    assert!(
        state
            .connections
            .lock()
            .unwrap()
            .values()
            .all(|v| v["backend"].is_null())
    );
    peer.send(3, "session/new", json!({})).await;
    assert_eq!(peer.read().await["error"]["code"], -32601);
    let _ = identity::put(
        State(state.clone()),
        headers.clone(),
        Json(identity::Upload {
            expected_revision: 0,
            media_type: "image/png".into(),
            data: image(),
        }),
    )
    .await
    .unwrap();
    let changed = peer.read().await;
    assert_eq!(changed["method"], identity::CHANGED);
    assert_eq!(changed["params"]["avatar"]["kind"], "uploaded");
    assert!(changed["params"]["avatar"]["data"].is_string());
    // Unrelated changes and repeated identity events do not emit duplicate data.
    state.event("identity.avatar.changed", json!({})).unwrap();
    state
        .event("view.changed", json!({"private":"not forwarded"}))
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), peer.rx.recv())
            .await
            .is_err()
    );
    state.config.write().unwrap().config.name = "Renamed teammate".into();
    state.event("config.changed", json!({})).unwrap();
    assert_eq!(
        peer.read().await["params"]["display_name"],
        "Renamed teammate"
    );
    let _ = identity::delete(
        State(state.clone()),
        headers.clone(),
        Query(identity::Revision {
            expected_revision: 1,
        }),
    )
    .await
    .unwrap();
    let removed = peer.read().await;
    assert_eq!(removed["params"]["avatar"]["kind"], "generated");
    drop(peer.tx);
    peer.task.await.unwrap().unwrap();
    assert!(state.connections.lock().unwrap().is_empty());
    let mut peer = connect(state.clone(), headers);
    peer.send(
        1,
        "initialize",
        json!({"protocolVersion":1,"_meta":{identity::NAMESPACE:{"mode":"observe"}}}),
    )
    .await;
    peer.read().await;
    peer.send(2, identity::SUBSCRIBE, json!({})).await;
    assert_eq!(peer.read().await["result"], removed["params"]);
    drop(peer.tx);
    peer.task.await.unwrap().unwrap();
    // An ACP-scoped credential is enough; revocation closes the observer before
    // another presentation event can disclose data.
    let grant =
        agent_instance::auth::issue_token(&state, "presentation", &["acp".into()], 600).unwrap();
    let mut scoped = HeaderMap::new();
    scoped.insert(
        "authorization",
        format!("Bearer {}", grant["token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    let mut scoped_peer = connect(state.clone(), scoped);
    scoped_peer
        .send(
            1,
            "initialize",
            json!({"protocolVersion":1,"_meta":{identity::NAMESPACE:{"mode":"observe"}}}),
        )
        .await;
    scoped_peer.read().await;
    scoped_peer.send(2, identity::SUBSCRIBE, json!({})).await;
    scoped_peer.read().await;
    state
        .db
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM tokens WHERE id=?1",
            [grant["id"].as_str().unwrap()],
        )
        .unwrap();
    state
        .event("token.revoked", json!({"id":grant["id"]}))
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), scoped_peer.task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(scoped_peer.rx.recv().await, None);
    let mut bad = connect(state, HeaderMap::new());
    assert!(bad.task.await.unwrap().is_err());
    assert_eq!(bad.rx.recv().await, None);
}
