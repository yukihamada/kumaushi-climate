use std::sync::Arc;
use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{HeaderMap, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tracing::{debug, error};

use kumaushi_common::{DashboardSnapshot, Schedule, Setpoints, ZoneMode};
use crate::SharedState;

pub fn build_router(state: SharedState) -> Router {
    let cors = CorsLayer::permissive();

    // Public routes (no auth)
    let public = Router::new()
        .route("/", get(dashboard_html))
        .route("/dashboard", get(dashboard_html))
        .route("/ws", get(ws_handler));

    // Protected routes (require Bearer token if AUTH_TOKEN is set)
    let protected = Router::new()
        .route("/api/v1/sensors", get(get_all_sensors))
        .route("/api/v1/sensors/:node_id/history", get(get_sensor_history))
        .route("/api/v1/zones", get(get_zones))
        .route("/api/v1/zones/:zone_id", get(get_zone))
        .route("/api/v1/zones/:zone_id/mode", post(set_zone_mode))
        .route("/api/v1/zones/:zone_id/setpoint", post(set_setpoint))
        .route("/api/v1/controls", get(get_controls))
        .route("/api/v1/controls/:device_id", post(set_control))
        .route("/api/v1/dashboard", get(get_dashboard))
        .route("/api/v1/alerts", get(get_alerts))
        .route("/api/v1/alerts/:id/resolve", post(resolve_alert))
        .route("/api/v1/schedules", get(get_schedules))
        .route("/api/v1/schedules", post(create_schedule))
        .route("/api/v1/schedules/:id", delete(delete_schedule))
        .route_layer(middleware::from_fn_with_state(Arc::clone(&state), auth_middleware));

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
        .layer(cors)
}

// ── Auth middleware ────────────────────────────────────────────────────────

async fn auth_middleware(
    State(state): State<SharedState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // If no token configured, skip auth
    if state.auth_token.is_empty() {
        return next.run(req).await;
    }
    let auth_header = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());
    let expected = format!("Bearer {}", state.auth_token);
    if auth_header.map(|h| h == expected).unwrap_or(false) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "unauthorized"}))).into_response()
    }
}

// ── Dashboard HTML ─────────────────────────────────────────────────────────

static DASHBOARD_HTML: &str = include_str!("../../../dashboard/index.html");

async fn dashboard_html() -> impl IntoResponse {
    axum::response::Html(DASHBOARD_HTML)
}

// ── Sensors ────────────────────────────────────────────────────────────────

async fn get_all_sensors(State(state): State<SharedState>) -> impl IntoResponse {
    let zones = state.zones.read().await;
    let all_node_ids: Vec<String> = zones.iter()
        .flat_map(|z| {
            let zid = z.id.clone();
            let containers = z.containers.clone();
            containers.into_iter().flat_map(move |c| {
                let zid = zid.clone();
                ["a", "b"].iter().map(move |s| format!("node-{}-{}{}", zid, c, s))
            })
        })
        .collect();
    let ids: Vec<&str> = all_node_ids.iter().map(|s| s.as_str()).collect();
    match state.db.latest_readings(&ids) {
        Ok(readings) => Json(readings).into_response(),
        Err(e) => { error!("{}", e); (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response() }
    }
}

#[derive(Deserialize)]
struct HistoryQuery {
    sensor_type: Option<String>,
    hours: Option<u32>,
}

async fn get_sensor_history(
    State(state): State<SharedState>,
    Path(node_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> impl IntoResponse {
    let sensor_type = q.sensor_type.as_deref().unwrap_or("co2");
    let hours = q.hours.unwrap_or(24);
    match state.db.history(&node_id, sensor_type, hours) {
        Ok(data) => {
            let result: Vec<_> = data.iter()
                .map(|(ts, v)| serde_json::json!({"ts": ts.to_rfc3339(), "v": v}))
                .collect();
            Json(result).into_response()
        }
        Err(e) => { error!("{}", e); (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response() }
    }
}

// ── Zones ──────────────────────────────────────────────────────────────────

async fn get_zones(State(state): State<SharedState>) -> impl IntoResponse {
    Json(state.zones.read().await.clone()).into_response()
}

async fn get_zone(State(state): State<SharedState>, Path(zone_id): Path<String>) -> impl IntoResponse {
    let zones = state.zones.read().await;
    match zones.iter().find(|z| z.id == zone_id) {
        Some(z) => Json(z.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, "zone not found").into_response(),
    }
}

#[derive(Deserialize)]
struct ModeBody { mode: String }

async fn set_zone_mode(
    State(state): State<SharedState>,
    Path(zone_id): Path<String>,
    Json(body): Json<ModeBody>,
) -> impl IntoResponse {
    let mode = match body.mode.as_str() {
        "auto" => ZoneMode::Auto,
        "manual" => ZoneMode::Manual,
        "off" => ZoneMode::Off,
        _ => return (StatusCode::BAD_REQUEST, "invalid mode: use auto|manual|off").into_response(),
    };
    let mut zones = state.zones.write().await;
    match zones.iter_mut().find(|z| z.id == zone_id) {
        Some(z) => { z.mode = mode; Json(z.clone()).into_response() }
        None => (StatusCode::NOT_FOUND, "zone not found").into_response(),
    }
}

async fn set_setpoint(
    State(state): State<SharedState>,
    Path(zone_id): Path<String>,
    Json(body): Json<Setpoints>,
) -> impl IntoResponse {
    let mut zones = state.zones.write().await;
    match zones.iter_mut().find(|z| z.id == zone_id) {
        Some(z) => { z.setpoints = body; Json(z.clone()).into_response() }
        None => (StatusCode::NOT_FOUND, "zone not found").into_response(),
    }
}

// ── Controls ───────────────────────────────────────────────────────────────

async fn get_controls(State(state): State<SharedState>) -> impl IntoResponse {
    Json(state.gpio.all()).into_response()
}

#[derive(Deserialize)]
struct ControlBody { value: f64 }

async fn set_control(
    State(state): State<SharedState>,
    Path(device_id): Path<String>,
    Json(body): Json<ControlBody>,
) -> impl IntoResponse {
    let value = body.value.clamp(0.0, 1.0);
    state.gpio.set_pwm(&device_id, value);
    let _ = state.db.log_control(&device_id, "manual", value, Some("api"));
    Json(serde_json::json!({"device_id": device_id, "value": value})).into_response()
}

// ── Dashboard ──────────────────────────────────────────────────────────────

async fn get_dashboard(State(state): State<SharedState>) -> impl IntoResponse {
    let zones = state.zones.read().await.clone();
    let alerts = state.db.get_alerts(20, false).unwrap_or_default();
    let snapshot = DashboardSnapshot { zones, alerts, timestamp: chrono::Utc::now() };
    Json(snapshot).into_response()
}

// ── Alerts ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AlertQuery { limit: Option<usize>, all: Option<bool> }

async fn get_alerts(
    State(state): State<SharedState>,
    Query(q): Query<AlertQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50);
    let include_resolved = q.all.unwrap_or(false);
    match state.db.get_alerts(limit, include_resolved) {
        Ok(alerts) => Json(alerts).into_response(),
        Err(e) => { error!("{}", e); (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response() }
    }
}

async fn resolve_alert(State(state): State<SharedState>, Path(id): Path<i64>) -> impl IntoResponse {
    match state.db.resolve_alert(id) {
        Ok(_) => Json(serde_json::json!({"id": id, "resolved": true})).into_response(),
        Err(e) => { error!("{}", e); (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response() }
    }
}

// ── Schedules ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ScheduleQuery { zone_id: Option<String> }

async fn get_schedules(
    State(state): State<SharedState>,
    Query(q): Query<ScheduleQuery>,
) -> impl IntoResponse {
    match state.db.get_schedules(q.zone_id.as_deref()) {
        Ok(schedules) => Json(schedules).into_response(),
        Err(e) => { error!("{}", e); (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response() }
    }
}

#[derive(Deserialize)]
struct CreateScheduleBody {
    zone_id: String,
    weekdays: Option<Vec<u8>>,
    time_from: String,
    time_until: String,
    temperature: Option<f64>,
    co2_max: Option<f64>,
    humidity: Option<f64>,
}

async fn create_schedule(
    State(state): State<SharedState>,
    Json(body): Json<CreateScheduleBody>,
) -> impl IntoResponse {
    let sched = Schedule {
        id: 0,
        zone_id: body.zone_id,
        weekdays: body.weekdays.unwrap_or_default(),
        time_from: body.time_from,
        time_until: body.time_until,
        setpoints: Setpoints {
            temperature: body.temperature.unwrap_or(22.0),
            co2_max: body.co2_max.unwrap_or(800.0),
            humidity: body.humidity.unwrap_or(50.0),
            cooling_threshold: Some(26.0),
        },
        enabled: true,
    };
    match state.db.create_schedule(&sched) {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response(),
        Err(e) => { error!("{}", e); (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response() }
    }
}

async fn delete_schedule(State(state): State<SharedState>, Path(id): Path<i64>) -> impl IntoResponse {
    match state.db.delete_schedule(id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "schedule not found").into_response(),
        Err(e) => { error!("{}", e); (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response() }
    }
}

// ── WebSocket ──────────────────────────────────────────────────────────────

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<SharedState>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: SharedState) {
    let mut rx = state.sensor_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(reading) => {
                let Ok(json) = serde_json::to_string(&reading) else { continue };
                if socket.send(Message::Text(json)).await.is_err() { break; }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                debug!("WS lagged {} messages", n);
            }
            Err(_) => break,
        }
    }
}
