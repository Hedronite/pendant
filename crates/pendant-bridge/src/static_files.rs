use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use pendant_web::Assets;

pub async fn static_handler(req: Request) -> Response {
    let path = req.uri().path();
    let key = asset_key(path);
    match Assets::get(&key) {
        Some(file) => {
            let mime = mime_guess::from_path(&key).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::CACHE_CONTROL, cache_control(&key))
                .body(Body::from(file.data.into_owned()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => {
            if looks_like_file(path) {
                StatusCode::NOT_FOUND.into_response()
            } else if let Some(index) = Assets::get("index.html") {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .body(Body::from(index.data.into_owned()))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
    }
}

fn asset_key(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        "index.html".into()
    } else {
        trimmed.to_string()
    }
}

fn looks_like_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext != "html")
}

fn cache_control(key: &str) -> &'static str {
    if key == "index.html" || key.ends_with(".webmanifest") || key == "sw.js" {
        "no-cache"
    } else {
        "public, max-age=60"
    }
}
