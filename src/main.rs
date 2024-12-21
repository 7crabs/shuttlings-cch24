use axum::{
    body::Bytes,
    extract::{rejection::JsonRejection, Json, Query, State},
    http::{
        header::{self, HeaderMap, CONTENT_TYPE},
        HeaderValue, StatusCode,
    },
    routing::{get, post},
    Router,
};
use cargo_manifest::{Manifest, MaybeInherited};
use leaky_bucket::RateLimiter;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::{
    ops::BitXor,
    sync::{Arc, Mutex},
    time::Duration,
};
use toml;

const BUCKET_SIZE: usize = 5;
const REFILL_INTERVAL: u64 = 1;

#[derive(Deserialize)]
struct Addresses {
    from: String,
    key: String,
}

#[derive(Deserialize)]
struct Addresses2 {
    from: String,
    to: String,
}

#[derive(Clone)]
struct AppState {
    limiter: Arc<Mutex<RateLimiter>>,
}

async fn hello_world() -> &'static str {
    "Hello, bird!"
}

async fn seek() -> (StatusCode, HeaderMap) {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        "https://www.youtube.com/watch?v=9Gc4QTqslN4"
            .parse()
            .unwrap(),
    );
    (StatusCode::FOUND, headers)
}

async fn calc_dest_address(addresses: Query<Addresses>) -> String {
    // split addresses by "." and convert to u8
    let from_parts = addresses
        .from
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    let key_parts = addresses
        .key
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    // wrapping_add every part of the from_parts and convert to string and concatenate with "."
    let dest_address = from_parts
        .iter()
        .zip(key_parts.iter())
        .map(|(from, key)| from.wrapping_add(*key).to_string())
        .collect::<Vec<String>>()
        .join(".");
    dest_address
}

fn parse_ipv6_address(address: &str) -> Vec<u16> {
    let mut parts = address.to_string();
    let count = parts.matches(':').count();
    if count < 7 {
        let tmp = format!(":{}", ":".repeat(8 - count));
        parts = parts.replace("::", &tmp);
    }

    let parts = parts
        .split(":")
        .map(|s| if s.is_empty() { "0" } else { s })
        .map(|s| u16::from_str_radix(&s, 16).unwrap())
        .collect::<Vec<u16>>();
    parts
}

async fn calc_ipv6_dest_address(addresses: Query<Addresses>) -> String {
    let from_parts = parse_ipv6_address(&addresses.from);
    let key_parts = parse_ipv6_address(&addresses.key);

    // wrapping_add every part of the from_parts and convert to string and concatenate with ":"
    let mut dest_address = from_parts
        .iter()
        .zip(key_parts.iter())
        .map(|(from, key)| {
            let result = from.bitxor(*key);
            if result == 0 {
                String::new()
            } else {
                format!("{:x}", result)
            }
        })
        .collect::<Vec<String>>()
        .join(":");

    while dest_address.contains(":::") {
        dest_address = dest_address.replace(":::", "::");
    }
    dest_address
}

async fn calc_key_address(addresses: Query<Addresses2>) -> String {
    // split addresses by "." and convert to u8
    let from_parts = addresses
        .from
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    let to_parts = addresses
        .to
        .split(".")
        .map(|s| s.parse::<u8>().unwrap())
        .collect::<Vec<u8>>();
    // wrapping_sub every part of the to_parts and convert to string and concatenate with "."
    let key_address = to_parts
        .iter()
        .zip(from_parts.iter())
        .map(|(to, from)| to.wrapping_sub(*from).to_string())
        .collect::<Vec<String>>()
        .join(".");
    key_address
}

async fn calc_ipv6_key_address(addresses: Query<Addresses2>) -> String {
    let from_parts = parse_ipv6_address(&addresses.from);
    let to_parts = parse_ipv6_address(&addresses.to);
    let mut key_address = to_parts
        .iter()
        .zip(from_parts.iter())
        // xorの逆演算 1 ^ 1 = 0, 0 ^ 1 = 1, 1 ^ 0 = 1, 0 ^ 0 = 0
        .map(|(from, key)| {
            let result = from.bitxor(*key);
            if result == 0 {
                String::new()
            } else {
                format!("{:x}", result)
            }
        })
        .collect::<Vec<String>>()
        .join(":");
    while key_address.contains(":::") {
        key_address = key_address.replace(":::", "::");
    }
    key_address
}

async fn parse_manifest(headers: HeaderMap, body: Bytes) -> (StatusCode, String) {
    let content_type_header = headers.get(CONTENT_TYPE);
    let content_type = match content_type_header {
        Some(content_type_header) => content_type_header,
        None => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, String::new()),
    };

    let toml_str = if content_type == "application/json" {
        // JSONをTOMLに変換
        let json_value: JsonValue = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid JSON".to_string()),
        };
        match toml::to_string_pretty(&json_value) {
            Ok(s) => s,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "Failed to convert JSON to TOML".to_string(),
                )
            }
        }
    } else if content_type == "application/yaml" {
        // YAMLをTOMLに変換
        let yaml_value: YamlValue = match serde_yaml::from_slice(&body) {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid YAML".to_string()),
        };
        match toml::to_string_pretty(&yaml_value) {
            Ok(s) => s,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "Failed to convert YAML to TOML".to_string(),
                )
            }
        }
    } else if content_type == "application/toml" {
        match String::from_utf8(body.to_vec()) {
            Ok(s) => s,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid TOML".to_string()),
        }
    } else {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, String::new());
    };

    let manifest = match Manifest::from_slice(toml_str.as_bytes()) {
        Ok(m) => m,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid manifest".to_string()),
    };

    let package = match manifest.package {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Magic keyword not provided".to_string(),
            )
        }
    };

    let keywords = match package.keywords {
        Some(k) => k,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Magic keyword not provided".to_string(),
            )
        }
    };

    let keywords = match keywords {
        MaybeInherited::Inherited { .. } => {
            return (
                StatusCode::BAD_REQUEST,
                "Magic keyword not provided".to_string(),
            )
        }
        MaybeInherited::Local(k) => k,
    };

    if !keywords.contains(&"Christmas 2024".to_string()) {
        return (
            StatusCode::BAD_REQUEST,
            "Magic keyword not provided".to_string(),
        );
    }

    let metadata = match package.metadata {
        Some(m) => m,
        None => return (StatusCode::NO_CONTENT, String::new()),
    };

    let orders = match metadata.get("orders") {
        Some(o) => o,
        None => return (StatusCode::NO_CONTENT, String::new()),
    };
    let orders = match orders.as_array() {
        Some(o) => o,
        None => return (StatusCode::NO_CONTENT, String::new()),
    };

    let mut outputs = Vec::new();
    for order in orders {
        let item = match order.get("item") {
            Some(i) => i,
            None => continue,
        };
        let quantity = match order.get("quantity") {
            Some(q) => q,
            None => continue,
        };
        let item = match item.as_str() {
            Some(i) => i,
            None => continue,
        };
        let quantity = match quantity.as_integer() {
            Some(q) => q,
            None => continue,
        };
        outputs.push(format!("{}: {}", item, quantity));
    }
    if outputs.is_empty() {
        return (StatusCode::NO_CONTENT, String::new());
    }
    (StatusCode::OK, outputs.join("\n"))
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
enum Volume {
    Gallons(f32),
    Liters(f32),
    Pints(f32),
    Litres(f32),
}

async fn withdraw_milk(
    State(state): State<AppState>,
    headers: HeaderMap,
    volume: Result<Json<Volume>, JsonRejection>,
) -> (StatusCode, String) {
    let limiter = state.limiter.lock().unwrap();
    let success = limiter.try_acquire(1);
    if !success {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "No milk available\n".to_string(),
        );
    }
    let content_type_header = headers.get(CONTENT_TYPE);
    let is_json = content_type_header == Some(&HeaderValue::from_static("application/json"));
    if is_json {
        let Json(volume) = match volume {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, String::new()),
        };
        let volume = match volume {
            Volume::Gallons(v) => Volume::Liters(v * 3.785411784),
            Volume::Liters(v) => Volume::Gallons(v / 3.785411784),
            Volume::Pints(v) => Volume::Litres(v * 0.56826125),
            Volume::Litres(v) => Volume::Pints(v / 0.56826125),
        };
        // JSONに変換
        let json_value = serde_json::to_value(volume).unwrap();
        return (StatusCode::OK, json_value.to_string());
    } else {
        (StatusCode::OK, "Milk withdrawn\n".to_string())
    }
}

async fn refill_milk(State(state): State<AppState>) -> (StatusCode, String) {
    let mut limiter = state.limiter.lock().unwrap();
    *limiter = RateLimiter::builder()
        .initial(BUCKET_SIZE)
        .max(BUCKET_SIZE)
        .interval(Duration::from_secs(REFILL_INTERVAL))
        .build();
    (StatusCode::OK, String::new())
}

#[shuttle_runtime::main]
async fn main() -> shuttle_axum::ShuttleAxum {
    let state = AppState {
        limiter: Arc::new(Mutex::new(
            RateLimiter::builder()
                .initial(BUCKET_SIZE)
                .max(BUCKET_SIZE)
                .interval(Duration::from_secs(REFILL_INTERVAL))
                .build(),
        )),
    };

    let router = Router::new()
        .route("/", get(hello_world))
        .route("/-1/seek", get(seek))
        .route("/2/dest", get(calc_dest_address))
        .route("/2/key", get(calc_key_address))
        .route("/2/v6/dest", get(calc_ipv6_dest_address))
        .route("/2/v6/key", get(calc_ipv6_key_address))
        .route("/5/manifest", post(parse_manifest))
        .route("/9/milk", post(withdraw_milk))
        .route("/9/refill", post(refill_milk))
        .with_state(state);
    Ok(router.into())
}
