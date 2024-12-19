use axum::{routing::get, Router, http::{header::{self, HeaderMap}, StatusCode}};

async fn hello_world() -> &'static str {
    "Hello, bird!"
}

async fn seek() -> (StatusCode, HeaderMap) {
    let mut headers = HeaderMap::new();
    headers.insert(header::LOCATION, "https://www.youtube.com/watch?v=9Gc4QTqslN4".parse().unwrap());
    (StatusCode::FOUND, headers)
}

#[shuttle_runtime::main]
async fn main() -> shuttle_axum::ShuttleAxum {
    let router = Router::new()
        .route("/", get(hello_world))
        .route("/-1/seek", get(seek));
    Ok(router.into())
}
