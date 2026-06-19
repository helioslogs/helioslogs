// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Compile-time embedded SPA, used when `serve` runs without `--frontend-dir`.
//! Mirrors the on-disk fallback: real assets are served by path, everything
//! else returns `index.html` so client-side routes resolve.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist"]
struct Assets;

pub(crate) fn serve(path: &str) -> Response {
    let rel = path.trim_start_matches('/');
    if !rel.is_empty() {
        if let Some(file) = Assets::get(rel) {
            return respond(file.metadata.mimetype(), file.data.into_owned());
        }
    }
    match Assets::get("index.html") {
        Some(file) => respond("text/html; charset=utf-8", file.data.into_owned()),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "embedded frontend is missing index.html",
        )
            .into_response(),
    }
}

fn respond(mime: &str, body: Vec<u8>) -> Response {
    let ct = HeaderValue::from_str(mime)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    ([(header::CONTENT_TYPE, ct)], body).into_response()
}
