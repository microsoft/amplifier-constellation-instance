//! Optional client for the existing private host owner-login.sock protocol.
use crate::config::HostPamConfig;

#[cfg(unix)]
pub(crate) async fn authenticate(config: &HostPamConfig, username: &str, password: &str) -> bool {
    authenticate_with_timeout(
        config,
        username,
        password,
        std::time::Duration::from_secs(20),
    )
    .await
}

#[cfg(unix)]
async fn authenticate_with_timeout(
    config: &HostPamConfig,
    username: &str,
    password: &str,
    timeout: std::time::Duration,
) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    if username != config.owner || password.is_empty() || password.len() > 1024 {
        return false;
    }
    let Ok(info) = std::fs::symlink_metadata(&config.socket) else {
        return false;
    };
    // Match the canonical Unified bridge's private-socket policy. The installing
    // owner supplies the fixed socket; no user-supplied path or PAM service is sent.
    if !info.file_type().is_socket()
        || info.mode() & 0o077 != 0
        || (info.uid() != 0 && info.uid() != unsafe { libc::geteuid() })
    {
        return false;
    }
    let work = async {
        let mut stream = tokio::net::UnixStream::connect(&config.socket).await?;
        let mut request = zeroize::Zeroizing::new(serde_json::to_vec(
            &serde_json::json!({"username":username,"password":password}),
        )?);
        request.push(b'\n');
        stream.write_all(&request).await?;
        drop(request);
        // Three exact bytes and EOF, never unbounded response allocation.
        let mut reply = [0u8; 3];
        stream.read_exact(&mut reply).await?;
        let mut extra = [0u8; 1];
        Ok::<bool, anyhow::Error>(reply == *b"OK\n" && stream.read(&mut extra).await? == 0)
    };
    tokio::time::timeout(timeout, work)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(crate) async fn authenticate(
    _config: &HostPamConfig,
    _username: &str,
    _password: &str,
) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn response_and_eof_share_one_deadline_without_blocking_other_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deadline.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let config = HostPamConfig {
            owner: "owner".into(),
            socket: path.to_str().unwrap().into(),
        };
        let attempt = tokio::spawn(async move {
            authenticate_with_timeout(
                &config,
                "owner",
                "synthetic",
                std::time::Duration::from_millis(60),
            )
            .await
        });
        let (mut peer, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 256];
        assert!(peer.read(&mut request).await.unwrap() > 0);
        peer.write_all(b"OK\n").await.unwrap();
        // A success prefix without EOF is still unfinished, not an accepted login.
        assert!(!attempt.is_finished());
        assert!(!attempt.await.unwrap());
        assert_eq!(peer.read(&mut request).await.unwrap(), 0);
    }
}
