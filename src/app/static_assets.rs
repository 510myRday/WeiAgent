use axum::{http::StatusCode, response::Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

pub async fn serve_frontend(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match FrontendAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", mime.as_ref())
                .body(axum::body::Body::from(file.data))
                .unwrap()
        }
        None => match FrontendAssets::get("index.html") {
            Some(file) => Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html")
                .body(axum::body::Body::from(file.data))
                .unwrap(),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(axum::body::Body::from("Not Found"))
                .unwrap(),
        },
    }
}
