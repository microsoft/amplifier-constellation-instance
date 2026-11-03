//! Portable presentation identity. This is never prompt content or an authority grant.
use crate::{
    auth,
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader, Rgba, RgbaImage};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Cursor, sync::Arc};

pub const NAMESPACE: &str = "com.microsoft.amplifier.identity";
pub const METHOD: &str = "_amplifier/identity/get";
pub const MAX_UPLOAD: usize = 5 * 1024 * 1024;
pub const MAX_IMAGE: usize = 1024 * 1024;

fn png(image: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

pub fn normalize(bytes: &[u8], media_type: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_UPLOAD,
        "Choose an image smaller than 5 MB"
    );
    let format = match media_type {
        "image/png" => ImageFormat::Png,
        "image/jpeg" => ImageFormat::Jpeg,
        "image/webp" => ImageFormat::WebP,
        _ => anyhow::bail!("Choose a PNG, JPEG or WebP image"),
    };
    anyhow::ensure!(
        image::guess_format(bytes)? == format,
        "The image content does not match its type"
    );
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode()?;
    anyhow::ensure!(
        decoded.width() >= 32 && decoded.height() >= 32,
        "Choose an image at least 32 pixels wide and high"
    );
    // Center-crop and decode/re-encode: original metadata and embedded content
    // are not retained. All clients receive the same static, bounded PNG.
    let output = png(&decoded
        .resize_to_fill(512, 512, image::imageops::FilterType::Lanczos3)
        .to_rgba8())?;
    anyhow::ensure!(
        output.len() <= MAX_IMAGE,
        "The normalized image is too large"
    );
    Ok(output)
}

fn fallback() -> anyhow::Result<Vec<u8>> {
    // A neutral code-drawn agent glyph, also usable to clear remote portraits.
    let mut image = RgbaImage::from_pixel(512, 512, Rgba([232, 239, 237, 255]));
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        if (96..416).contains(&x) && (128..384).contains(&y) {
            *pixel = Rgba([32, 91, 83, 255]);
        }
        if ((160..208).contains(&x) || (304..352).contains(&x)) && (216..272).contains(&y) {
            *pixel = Rgba([248, 251, 249, 255]);
        }
    }
    png(&image)
}

pub fn document(state: &AppState, inline: bool) -> anyhow::Result<Value> {
    let saved = state
        .get_record("identity", "avatar")?
        .unwrap_or(json!({"revision":0}));
    let config = state.instance_config();
    let uploaded = saved["data"].as_str();
    let data = match uploaded {
        Some(data) => data.to_owned(),
        None => STANDARD.encode(fallback()?),
    };
    let bytes = STANDARD.decode(&data)?;
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let revision = format!(
        "{:x}",
        Sha256::digest(format!(
            "{}:{}:{}:{}",
            config.instance_id, config.name, saved["revision"], sha
        ))
    );
    let mut avatar = json!({"kind":if uploaded.is_some(){"uploaded"}else{"generated"},"media_type":"image/png","sha256":sha,"width":512,"height":512,"bytes":bytes.len()});
    if inline {
        avatar["data"] = data.into();
    }
    Ok(
        json!({"schema":"agent-identity/1","agent_id":config.instance_id,"display_name":config.name,"revision":revision,"avatar_revision":saved["revision"],"avatar":avatar}),
    )
}

pub async fn get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    auth::check(&state, &headers, "observe", false)?;
    Ok(Json(document(&state, false)?))
}
pub async fn image_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    auth::check(&state, &headers, "observe", false)?;
    let doc = document(&state, true)?;
    let bytes = STANDARD
        .decode(doc["avatar"]["data"].as_str().unwrap())
        .map_err(|_| ApiError::bad("Image unavailable"))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "private, no-store"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=agent-avatar.png",
            ),
        ],
        bytes,
    )
        .into_response())
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub expected_revision: u64,
    pub media_type: String,
    pub data: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub expected_revision: u64,
}

fn save(state: &AppState, expected: u64, data: Option<Vec<u8>>, actor: &str) -> ApiResult<Value> {
    let _guard = state.profile_lock.lock().unwrap();
    let current = document(state, false)?;
    if current["avatar_revision"].as_u64() != Some(expected) {
        return Err(ApiError::conflict(current));
    }
    state.set_record(
        "identity",
        "avatar",
        json!({"revision":expected + 1,"data":data.map(|v|STANDARD.encode(v))}),
    )?;
    let doc = document(state, false)?;
    state.event("identity.avatar.changed", json!({"actor":actor,"avatar_revision":doc["avatar_revision"],"kind":doc["avatar"]["kind"]}))?;
    Ok(doc)
}
pub async fn put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Upload>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    if body.data.len() > MAX_UPLOAD.div_ceil(3) * 4 {
        return Err(ApiError::bad("Choose an image smaller than 5 MB"));
    }
    let decoded = STANDARD
        .decode(body.data)
        .map_err(|_| ApiError::bad("Image data must be base64"))?;
    let bytes = tokio::task::spawn_blocking(move || normalize(&decoded, &body.media_type))
        .await
        .map_err(|_| ApiError::bad("Could not read this image"))?
        .map_err(|e| ApiError::bad(e.to_string()))?;
    Ok(Json(save(
        &state,
        body.expected_revision,
        Some(bytes),
        &actor,
    )?))
}
pub async fn delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(body): Query<Revision>,
) -> ApiResult<Json<Value>> {
    let actor = auth::check(&state, &headers, "admin", true)?.actor;
    Ok(Json(save(&state, body.expected_revision, None, &actor)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_bounds_types_and_strips_metadata() {
        let bytes = png(&RgbaImage::from_pixel(640, 480, Rgba([10, 20, 30, 255]))).unwrap();
        let out = normalize(&bytes, "image/png").unwrap();
        let im = image::load_from_memory(&out).unwrap();
        assert_eq!((im.width(), im.height()), (512, 512));
        assert!(normalize(&bytes, "image/jpeg").is_err());
        assert!(normalize(b"<svg/>", "image/svg+xml").is_err());
        assert!(normalize(&vec![0; MAX_UPLOAD + 1], "image/png").is_err());
        assert!(normalize(&png(&RgbaImage::new(16, 16)).unwrap(), "image/png").is_err());
        assert!(fallback().unwrap().len() < MAX_IMAGE);
    }
}
