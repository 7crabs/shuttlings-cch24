use axum::{
    body::Bytes,
    extract::{rejection::JsonRejection, Json, Multipart, Path, Query, State},
    http::{
        header::{self, HeaderMap, CONTENT_TYPE},
        HeaderValue, StatusCode,
    },
    response::Html,
    routing::{delete, get, post, put},
    Router,
};
use cargo_manifest::{Manifest, MaybeInherited};
use chrono::{DateTime, Utc};
use jsonwebtoken::{
    decode, decode_header, encode, errors::ErrorKind, Algorithm, DecodingKey, EncodingKey, Header,
    Validation,
};
use leaky_bucket::RateLimiter;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use shuttle_runtime::SecretStore;
use std::sync::OnceLock;
use std::{
    collections::HashMap,
    fmt::Display,
    ops::BitXor,
    sync::{Arc, Mutex},
    time::Duration,
};
use toml;
use tower_http::services::ServeDir;
use uuid::Uuid;

const BUCKET_SIZE: usize = 5;
const REFILL_INTERVAL: u64 = 1;

static SECRET_KEY: OnceLock<String> = OnceLock::new();
static PUBLIC_KEY: OnceLock<String> = OnceLock::new();
static SANTA_PUBLIC_KEY: OnceLock<String> = OnceLock::new();
const ALGORITHM: Algorithm = Algorithm::EdDSA;

static HEADER: OnceLock<Header> = OnceLock::new();

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

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Team {
    Cookie,
    Milk,
}

#[derive(Clone, Copy, Default)]
struct Board {
    board: [[Option<Team>; 4]; 4],
}

impl Display for Board {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut output = String::new();
        for i in 0..4 {
            output.push_str("⬜");
            for j in 0..4 {
                match self.board[j][i] {
                    Some(Team::Cookie) => output.push_str("🍪"),
                    Some(Team::Milk) => output.push_str("🥛"),
                    None => output.push_str("⬛"),
                }
            }
            output.push_str("⬜\n");
        }
        output.push_str("⬜⬜⬜⬜⬜⬜\n");
        write!(f, "{}", output)
    }
}

impl Board {
    fn check_winner(&self) -> Option<Team> {
        // 縦横のチェック
        for i in 0..4 {
            // 横のチェック
            if let Some(team) = self.board[i][0] {
                if self.board[i][1] == Some(team)
                    && self.board[i][2] == Some(team)
                    && self.board[i][3] == Some(team)
                {
                    return Some(team);
                }
            }
            // 縦のチェック
            if let Some(team) = self.board[0][i] {
                if self.board[1][i] == Some(team)
                    && self.board[2][i] == Some(team)
                    && self.board[3][i] == Some(team)
                {
                    return Some(team);
                }
            }
        }

        // 斜めのチェック（左上から右下）
        if let Some(team) = self.board[0][0] {
            if self.board[1][1] == Some(team)
                && self.board[2][2] == Some(team)
                && self.board[3][3] == Some(team)
            {
                return Some(team);
            }
        }

        // 斜めのチェック（右上から左下）
        if let Some(team) = self.board[0][3] {
            if self.board[1][2] == Some(team)
                && self.board[2][1] == Some(team)
                && self.board[3][0] == Some(team)
            {
                return Some(team);
            }
        }

        None
    }

    fn is_draw(&self) -> bool {
        // すべてのマスが埋まっているかチェック
        for row in self.board.iter() {
            for cell in row.iter() {
                if cell.is_none() {
                    return false;
                }
            }
        }
        // 勝者がいない場合は引き分け
        self.check_winner().is_none()
    }

    fn show_result(&self) -> Option<String> {
        let mut result = self.to_string();
        if let Some(winner) = self.check_winner() {
            result.push_str(&format!(
                "{} wins!\n",
                match winner {
                    Team::Cookie => "🍪",
                    Team::Milk => "🥛",
                }
            ));
            Some(result)
        } else if self.is_draw() {
            result.push_str("No winner.\n");
            Some(result)
        } else {
            None
        }
    }

    fn generate_random(rng: &mut rand::rngs::StdRng) -> Self {
        let mut board = Board::default();
        for i in 0..4 {
            for j in 0..4 {
                board.board[j][i] = Some(if rng.gen::<bool>() {
                    Team::Cookie
                } else {
                    Team::Milk
                });
            }
        }
        board
    }
}

#[derive(sqlx::FromRow, Serialize)]
struct Quote {
    id: Uuid,
    author: String,
    quote: String,
    created_at: DateTime<Utc>,
    version: i32,
}

#[derive(Deserialize)]
struct Draft {
    author: String,
    quote: String,
}

#[derive(Serialize)]
struct QuoteList {
    quotes: Vec<Quote>,
    page: i32,
    next_token: Option<String>,
}

#[derive(Clone)]
struct PaginationState {
    page: i32,
}

#[derive(Clone)]
struct AppState {
    limiter: Arc<Mutex<RateLimiter>>,
    board: Arc<Mutex<Board>>,
    rng: Arc<Mutex<rand::rngs::StdRng>>,
    pool: sqlx::PgPool,
    pagination_tokens: Arc<Mutex<HashMap<String, PaginationState>>>,
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

async fn get_board(State(state): State<AppState>) -> (StatusCode, String) {
    let board = state.board.lock().unwrap();
    if let Some(result) = board.show_result() {
        (StatusCode::OK, result)
    } else {
        (StatusCode::OK, format!("{}", board))
    }
}

async fn reset_board(State(state): State<AppState>) -> (StatusCode, String) {
    let mut board = state.board.lock().unwrap();
    *board = Board::default();
    let mut rng = state.rng.lock().unwrap();
    *rng = rand::rngs::StdRng::seed_from_u64(2024);
    (StatusCode::OK, format!("{}", board))
}

async fn place_piece(
    State(state): State<AppState>,
    Path((team, column)): Path<(Team, usize)>,
) -> (StatusCode, String) {
    if column < 1 || 4 < column {
        return (StatusCode::BAD_REQUEST, "Invalid column".to_string());
    }
    let mut board = state.board.lock().unwrap();
    let result = board.show_result();
    if let Some(result) = result {
        return (StatusCode::SERVICE_UNAVAILABLE, result);
    }

    let column = column - 1;
    // 下から順いている場所を探す
    for row in (0..4).rev() {
        if board.board[column][row].is_none() {
            board.board[column][row] = Some(team);
            let result = board.show_result();
            if let Some(result) = result {
                return (StatusCode::OK, result);
            }
            return (StatusCode::OK, format!("{}", board));
        }
    }

    (StatusCode::SERVICE_UNAVAILABLE, format!("{}", board))
}

async fn random_board(State(state): State<AppState>) -> (StatusCode, String) {
    let mut rng = state.rng.lock().unwrap();
    let board = Board::generate_random(&mut rng);
    let result = board.to_string();
    if let Some(winner) = board.check_winner() {
        (
            StatusCode::OK,
            format!(
                "{}{} wins!",
                result,
                match winner {
                    Team::Cookie => "🍪",
                    Team::Milk => "🥛",
                }
            ),
        )
    } else if board.is_draw() {
        (StatusCode::OK, format!("{}No winner.", result))
    } else {
        (StatusCode::OK, result)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    #[serde(flatten)]
    data: JsonValue,
}

async fn wrap_gift(Json(data): Json<JsonValue>) -> (StatusCode, HeaderMap, &'static str) {
    let header = HEADER.get_or_init(|| Header::new(ALGORITHM));
    let claims = Claims { data };

    let secret_key = SECRET_KEY.get().unwrap();
    let token = encode(
        header,
        &claims,
        &EncodingKey::from_ed_pem(secret_key.as_bytes()).unwrap(),
    )
    .unwrap();

    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("gift={}", token)).unwrap(),
    );

    (StatusCode::OK, headers, "")
}

async fn unwrap_gift(headers: HeaderMap) -> Result<Json<JsonValue>, StatusCode> {
    let cookie_header = match headers.get(header::COOKIE) {
        Some(cookie_header) => cookie_header,
        None => return Err(StatusCode::BAD_REQUEST),
    };

    let cookie_str = match cookie_header.to_str() {
        Ok(s) => s,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };

    let gift_token = cookie_str
        .split(';')
        .find(|s| s.trim().starts_with("gift="))
        .map(|s| s.trim()[5..].to_string())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let mut validation = Validation::new(ALGORITHM);
    validation.required_spec_claims.remove("exp");

    let token_data = decode::<Claims>(
        &gift_token,
        &DecodingKey::from_ed_pem(PUBLIC_KEY.get().unwrap().as_bytes()).unwrap(),
        &validation,
    )
    .map_err(|e| {
        println!("JWT decode error: {:?}", e);
        StatusCode::BAD_REQUEST
    })?;

    Ok(Json(token_data.claims.data))
}

async fn decode_gift(body: String) -> Result<Json<JsonValue>, StatusCode> {
    // 公開鍵をSANTA_PUBLIC_KEYから取得
    let public_key = SANTA_PUBLIC_KEY.get().unwrap();

    // JWTのヘッダーをデコードしてアルゴリズムを取得
    let header: Header = decode_header(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let algorithm = match header.alg {
        Algorithm::RS256 | Algorithm::RS512 => header.alg,
        _ => return Err(StatusCode::BAD_REQUEST),
    };

    // Validationの設定を修正
    let mut validation = Validation::new(algorithm);
    validation.required_spec_claims.remove("exp"); // expの検証を無効化

    // JWTのデコード（署名の検証を有効化）
    let token_data = decode::<Claims>(
        &body,
        &DecodingKey::from_rsa_pem(public_key.as_bytes()).map_err(|_| StatusCode::BAD_REQUEST)?,
        &validation,
    )
    .map_err(|e| {
        match *e.kind() {
            ErrorKind::InvalidToken => StatusCode::BAD_REQUEST, // ヘッダーが無効な場合
            ErrorKind::InvalidSignature => StatusCode::UNAUTHORIZED, // 署名が無効な場合
            _ => StatusCode::BAD_REQUEST,                       // その他の理由で無効な場合
        }
    })?;

    Ok(Json(token_data.claims.data))
}

async fn reset_quotes(State(state): State<AppState>) -> (StatusCode, String) {
    sqlx::query("DELETE FROM quotes")
        .execute(&state.pool)
        .await
        .expect("Failed to reset quotes");
    (StatusCode::OK, "Quotes reset".to_string())
}

async fn get_quotes(State(state): State<AppState>, Path(id): Path<Uuid>) -> (StatusCode, String) {
    let quote = sqlx::query_as::<_, Quote>("SELECT * FROM quotes WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .unwrap();
    if let Some(quote) = quote {
        (StatusCode::OK, serde_json::to_string(&quote).unwrap())
    } else {
        (StatusCode::NOT_FOUND, "Quote not found".to_string())
    }
}

async fn remove_quotes(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> (StatusCode, String) {
    let quote = sqlx::query_as::<_, Quote>("SELECT * FROM quotes WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .unwrap();
    if let Some(quote) = quote {
        sqlx::query("DELETE FROM quotes WHERE id = $1")
            .bind(id)
            .execute(&state.pool)
            .await
            .expect("Failed to remove quote");
        (StatusCode::OK, serde_json::to_string(&quote).unwrap())
    } else {
        (StatusCode::NOT_FOUND, "Quote not found".to_string())
    }
}

async fn undo_quotes(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(draft): Json<Draft>,
) -> (StatusCode, String) {
    let quote = sqlx::query_as::<_, Quote>("SELECT * FROM quotes WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .unwrap();
    if let Some(mut quote) = quote {
        quote.quote = draft.quote;
        quote.author = draft.author;
        quote.version += 1;
        sqlx::query(
            "UPDATE quotes SET quote = $1, author = $2, version = version + 1 WHERE id = $3",
        )
        .bind(&quote.quote)
        .bind(&quote.author)
        .bind(id)
        .execute(&state.pool)
        .await
        .expect("Failed to undo quote");
        (StatusCode::OK, serde_json::to_string(&quote).unwrap())
    } else {
        return (StatusCode::NOT_FOUND, "Quote not found".to_string());
    }
}

async fn add_quote(
    State(state): State<AppState>,
    Json(draft): Json<Draft>,
) -> (StatusCode, String) {
    let quote = sqlx::query_as::<_, Quote>(
        "INSERT INTO quotes (quote, author) VALUES ($1, $2) RETURNING id, author, quote, created_at, version",
    )
    .bind(draft.quote)
    .bind(draft.author)
    .fetch_one(&state.pool)
    .await
    .expect("Failed to add quote");
    (StatusCode::CREATED, serde_json::to_string(&quote).unwrap())
}

fn generate_token(rng: &mut rand::rngs::StdRng) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut token = String::with_capacity(16);
    for _ in 0..16 {
        let idx = rng.gen_range(0..CHARSET.len());
        token.push(CHARSET[idx] as char);
    }
    token
}

#[derive(Deserialize)]
struct ListQuery {
    token: String,
}

async fn list_quotes(
    State(state): State<AppState>,
    query: Option<Query<ListQuery>>,
) -> Result<Json<QuoteList>, StatusCode> {
    const QUOTES_PER_PAGE: i64 = 3;

    let current_page = if let Some(query) = query {
        let tokens = state.pagination_tokens.lock().unwrap();
        if let Some(pagination_state) = tokens.get(&query.token) {
            pagination_state.page
        } else {
            return Err(StatusCode::BAD_REQUEST);
        }
    } else {
        1
    };

    let offset = (current_page - 1) * QUOTES_PER_PAGE as i32;

    let quotes = sqlx::query_as::<_, Quote>(
        "SELECT * FROM quotes ORDER BY created_at ASC LIMIT $1 OFFSET $2",
    )
    .bind(QUOTES_PER_PAGE + 1) // 次のページがあるかチェックするために1つ多く取得
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let has_next_page = quotes.len() > QUOTES_PER_PAGE as usize;
    let quotes = quotes
        .into_iter()
        .take(QUOTES_PER_PAGE as usize)
        .collect::<Vec<_>>();

    let next_token = if has_next_page {
        let mut rng = state.rng.lock().unwrap();
        let token = generate_token(&mut rng);
        let mut tokens = state.pagination_tokens.lock().unwrap();
        tokens.insert(
            token.clone(),
            PaginationState {
                page: current_page + 1,
            },
        );
        Some(token)
    } else {
        None
    };

    Ok(Json(QuoteList {
        quotes,
        page: current_page,
        next_token,
    }))
}

async fn get_light_star() -> Html<&'static str> {
    Html("<div id=\"star\" class=\"lit\"></div>")
}

async fn get_present(Path(color): Path<String>) -> (StatusCode, Html<&'static str>) {
    match color.as_str() {
        "red" => (
            StatusCode::OK,
            Html(
                "<div class=\"present red\" hx-get=\"/23/present/blue\" hx-swap=\"outerHTML\">
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                </div>",
            ),
        ),
        "blue" => (
            StatusCode::OK,
            Html(
                "<div class=\"present blue\" hx-get=\"/23/present/purple\" hx-swap=\"outerHTML\">
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                </div>",
            ),
        ),
        "purple" => (
            StatusCode::OK,
            Html(
                "<div class=\"present purple\" hx-get=\"/23/present/red\" hx-swap=\"outerHTML\">
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                    <div class=\"ribbon\"></div>
                </div>",
            ),
        ),
        _ => (StatusCode::IM_A_TEAPOT, Html("")),
    }
}

async fn get_ornament(Path((state, n)): Path<(String, String)>) -> (StatusCode, Html<String>) {
    let n = html_escape::encode_double_quoted_attribute(&n);
    match state.as_str() {
        "on" => (
            StatusCode::OK,
            Html(format!(
                "<div class=\"ornament on\" id=\"ornament{}\" hx-trigger=\"load delay:2s once\" hx-get=\"/23/ornament/off/{}\" hx-swap=\"outerHTML\"></div>",
                n, n
            )),
        ),
        "off" => (
            StatusCode::OK,
            Html(format!(
                "<div class=\"ornament\" id=\"ornament{}\" hx-trigger=\"load delay:2s once\" hx-get=\"/23/ornament/on/{}\" hx-swap=\"outerHTML\"></div>",
                n, n
            )),
        ),
        _ => (StatusCode::IM_A_TEAPOT, Html("".to_string())),
    }
}

async fn process_lockfile(mut multipart: Multipart) -> Result<Html<String>, StatusCode> {
    let mut lockfile_content = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        if field.name() == Some("lockfile") {
            lockfile_content = Some(field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?);
        }
    }

    let lockfile_content = lockfile_content.ok_or(StatusCode::BAD_REQUEST)?;

    // TOMLとしてパース
    let lockfile: toml::Value = match toml::from_str(&lockfile_content) {
        Ok(l) => l,
        _ => {
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // packagesを取得
    let packages = match lockfile.get("package") {
        Some(toml::Value::Array(packages)) => packages,
        _ => {
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    let mut html = String::new();
    for package in packages.iter() {
        if let Some(checksum) = package.get("checksum") {
            let checksum = match checksum {
                toml::Value::String(s) => s,
                _ => {
                    return Err(StatusCode::BAD_REQUEST);
                }
            };

            // チェックサムは少なくとも5バイト（10文字）必要で、16進数文字列である必要がある
            if checksum.len() < 10 || !checksum.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(StatusCode::UNPROCESSABLE_ENTITY);
            }

            // 最初の6文字を色コードとして使用
            let color = &checksum[..6];
            // 次の2文字をtopとして使用
            let top = u8::from_str_radix(&checksum[6..8], 16)
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
            // その次の2文字をleftとして使用
            let left = u8::from_str_radix(&checksum[8..10], 16)
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;

            if !html.is_empty() {
                html.push('\n');
            }
            html.push_str(&format!(
                "<div style=\"background-color:#{};top:{}px;left:{}px;\"></div>",
                color, top, left
            ));
        }
    }

    Ok(Html(html))
}

#[shuttle_runtime::main]
async fn main(
    #[shuttle_runtime::Secrets] secrets: SecretStore,
    #[shuttle_shared_db::Postgres] pool: sqlx::PgPool,
) -> shuttle_axum::ShuttleAxum {
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    HEADER.set(Header::new(ALGORITHM)).unwrap();
    SECRET_KEY.set(secrets.get("SECRET_KEY").unwrap()).unwrap();
    PUBLIC_KEY.set(secrets.get("PUBLIC_KEY").unwrap()).unwrap();
    SANTA_PUBLIC_KEY
        .set(secrets.get("SANTA_PUBLIC_KEY").unwrap())
        .unwrap();

    let state = AppState {
        limiter: Arc::new(Mutex::new(
            RateLimiter::builder()
                .initial(BUCKET_SIZE)
                .max(BUCKET_SIZE)
                .interval(Duration::from_secs(REFILL_INTERVAL))
                .build(),
        )),
        board: Arc::new(Mutex::new(Board::default())),
        rng: Arc::new(Mutex::new(rand::rngs::StdRng::seed_from_u64(2024))),
        pool,
        pagination_tokens: Arc::new(Mutex::new(HashMap::new())),
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
        .route("/12/board", get(get_board))
        .route("/12/reset", post(reset_board))
        .route("/12/place/:team/:column", post(place_piece))
        .route("/12/random-board", get(random_board))
        .route("/16/wrap", post(wrap_gift))
        .route("/16/unwrap", get(unwrap_gift))
        .route("/16/decode", post(decode_gift))
        .route("/19/reset", post(reset_quotes))
        .route("/19/cite/:id", get(get_quotes))
        .route("/19/remove/:id", delete(remove_quotes))
        .route("/19/undo/:id", put(undo_quotes))
        .route("/19/draft", post(add_quote))
        .route("/19/list", get(list_quotes))
        .route("/23/star", get(get_light_star))
        .route("/23/present/:color", get(get_present))
        .route("/23/ornament/:state/:n", get(get_ornament))
        .route("/23/lockfile", post(process_lockfile))
        .nest_service("/assets", ServeDir::new("assets"))
        .with_state(state);
    Ok(router.into())
}
