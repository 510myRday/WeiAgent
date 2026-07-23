use axum::{http::StatusCode, response::Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

pub async fn serve_frontend(uri: axum::http::Uri, api_token: &str) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match FrontendAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let cache_control = if path == "index.html" {
                "no-cache"
            } else if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "public, max-age=3600"
            };
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", mime.as_ref())
                .header("cache-control", cache_control)
                .body(axum::body::Body::from(frontend_body(
                    path, file.data, api_token,
                )))
                .unwrap()
        }
        None => match FrontendAssets::get("index.html") {
            Some(file) => Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/html")
                .header("cache-control", "no-cache")
                .body(axum::body::Body::from(frontend_body(
                    "index.html",
                    file.data,
                    api_token,
                )))
                .unwrap(),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(axum::body::Body::from("Not Found"))
                .unwrap(),
        },
    }
}

fn frontend_body(path: &str, data: std::borrow::Cow<'static, [u8]>, api_token: &str) -> Vec<u8> {
    if path != "index.html" {
        return data.into_owned();
    }
    let html = String::from_utf8_lossy(&data);
    html.replace(
        "</head>",
        &format!("<meta name=\"weiagent-api-token\" content=\"{api_token}\" /></head>"),
    )
    .into_bytes()
}
