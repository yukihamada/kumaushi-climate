use std::sync::Arc;
use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tracing::{debug, error};

use kumaushi_common::{DashboardSnapshot, SensorReading, ZoneMode, Setpoints};
use crate::SharedState;

pub fn build_router(state: SharedState) -> Router {
    let cors = CorsLayer::permissive();

    Router::new()
        // Sensor readings
        .route("/api/v1/sensors", get(get_all_sensors))
        .route("/api/v1/sensors/:node_id/history", get(get_sensor_history))
        // Zone control
        .route("/api/v1/zones", get(get_zones))
        .route("/api/v1/zones/:zone_id", get(get_zone))
        .route("/api/v1/zones/:zone_id/mode", post(set_zone_mode))
        .route("/api/v1/zones/:zone_id/setpoint", post(set_setpoint))
        // Device control
        .route("/api/v1/controls", get(get_controls))
        .route("/api/v1/controls/:device_id", post(set_control))
        // Dashboard
        .route("/api/v1/dashboard", get(get_dashboard))
        // Alerts
        .route("/api/v1/alerts", get(get_alerts))
        // WebSocket live feed
        .route("/ws", get(ws_handler))
        // Static dashboard HTML
        .route("/", get(dashboard_html))
        .route("/dashboard", get(dashboard_html))
        .with_state(state)
        .layer(cors)
}

static DASHBOARD_HTML: &str = include_str!("../../../dashboard/index.html");

async fn dashboard_html() -> impl IntoResponse {
    axum::response::Html(DASHBOARD_HTML)
}

// GET /api/v1/sensors — returns latest reading for every known sensor
async fn get_all_sensors(State(state): State<SharedState>) -> impl IntoResponse {
    let zones = state.zones.read().await;
    let all_node_ids: Vec<String> = zones
        .iter()
        .flat_map(|z| {
            let zone_id = z.id.clone();
            let containers = z.containers.clone();
            containers.into_iter().flat_map(move |c| {
                let zid = zone_id.clone();
                ["a", "b"].iter().map(move |s| format!("node-{}-{}{}", zid, c, s))
            })
        })
        .collect();
    let ids: Vec<&str> = all_node_ids.iter().map(|s| s.as_str()).collect();
    match state.db.latest_readings(&ids) {
        Ok(readings) => Json(readings).into_response(),
        Err(e) => {
            error!("DB error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
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
            let result: Vec<serde_json::Value> = data
                .into_iter()
                .map(|(ts, v)| serde_json::json!({"ts": ts.to_rfc3339(), "v": v}))
                .collect();
            Json(result).into_response()
        }
        Err(e) => {
            error!("DB history error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

async fn get_zones(State(state): State<SharedState>) -> impl IntoResponse {
    let zones = state.zones.read().await;
    Json(zones.clone()).into_response()
}

async fn get_zone(
    State(state): State<SharedState>,
    Path(zone_id): Path<String>,
) -> impl IntoResponse {
    let zones = state.zones.read().await;
    if let Some(zone) = zones.iter().find(|z| z.id == zone_id) {
        Json(zone.clone()).into_response()
    } else {
        (StatusCode::NOT_FOUND, "zone not found").into_response()
    }
}

#[derive(Deserialize)]
struct ModeBody {
    mode: String,
}

async fn set_zone_mode(
    State(state): State<SharedState>,
    Path(zone_id): Path<String>,
    Json(body): Json<ModeBody>,
) -> impl IntoResponse {
    let mode = match body.mode.as_str() {
        "auto" => ZoneMode::Auto,
        "manual" => ZoneMode::Manual,
        "off" => ZoneMode::Off,
        _ => return (StatusCode::BAD_REQUEST, "invalid mode").into_response(),
    };
    let mut zones = state.zones.write().await;
    if let Some(zone) = zones.iter_mut().find(|z| z.id == zone_id) {
        zone.mode = mode;
        Json(zone.clone()).into_response()
    } else {
        (StatusCode::NOT_FOUND, "zone not found").into_response()
    }
}

async fn set_setpoint(
    State(state): State<SharedState>,
    Path(zone_id): Path<String>,
    Json(body): Json<Setpoints>,
) -> impl IntoResponse {
    let mut zones = state.zones.write().await;
    if let Some(zone) = zones.iter_mut().find(|z| z.id == zone_id) {
        zone.setpoints = body;
        Json(zone.clone()).into_response()
    } else {
        (StatusCode::NOT_FOUND, "zone not found").into_response()
    }
}

async fn get_controls(State(state): State<SharedState>) -> impl IntoResponse {
    Json(state.gpio.all()).into_response()
}

#[derive(Deserialize)]
struct ControlBody {
    value: f64,
}

async fn set_control(
    State(state): State<SharedState>,
    Path(device_id): Path<String>,
    Json(body): Json<ControlBody>,
) -> impl IntoResponse {
    let value = body.value.clamp(0.0, 1.0);
    state.gpio.set_pwm(&device_id, value);
    if let Err(e) = state.db.log_control(&device_id, "manual", value, Some("api")) {
        error!("log_control: {}", e);
    }
    Json(serde_json::json!({"device_id": device_id, "value": value})).into_response()
}

async fn get_dashboard(State(state): State<SharedState>) -> impl IntoResponse {
    let zones = state.zones.read().await.clone();
    let snapshot = DashboardSnapshot {
        zones,
        alerts: vec![], // TODO: fetch active alerts from DB
        timestamp: chrono::Utc::now(),
    };
    Json(snapshot).into_response()
}

async fn get_alerts(State(state): State<SharedState>) -> impl IntoResponse {
    // Return last 50 alerts (stub)
    Json(serde_json::json!([])).into_response()
}

// WebSocket live sensor feed
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: SharedState) {
    let mut rx = state.sensor_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(reading) => {
                let json = match serde_json::to_string(&reading) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                if socket.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                debug!("WS lagged {} messages", n);
            }
            Err(_) => break,
        }
    }
}
