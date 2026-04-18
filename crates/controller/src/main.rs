mod api;
mod audio;
mod control;
mod db;
mod dj;
mod energy;
mod gpio;
mod lighting;
mod mqtt_client;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};
use tracing::info;

use kumaushi_common::{Setpoints, Zone, ZoneMode, ZoneState};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub db: db::Database,
    pub zones: RwLock<Vec<Zone>>,
    /// last time each node_id sent a reading (for failsafe detection)
    pub last_seen: RwLock<HashMap<String, Instant>>,
    pub sensor_tx: broadcast::Sender<kumaushi_common::SensorReading>,
    pub gpio: gpio::GpioController,
    /// Bearer token for API auth (empty = no auth)
    pub auth_token: String,
    /// Multi-zone audio control
    pub audio: audio::AudioController,
    /// Pioneer DJ Link status monitor
    pub dj: dj::DjMonitor,
    /// Philips Hue lighting client
    pub hue: lighting::HueClient,
    /// Powerwall + Starlink energy monitor (Arc so polling loop can share it)
    pub energy: Arc<energy::EnergyMonitor>,
}

fn default_zones() -> Vec<Zone> {
    vec![
        Zone { id: "z1".into(), name: "メインリビング".into(), containers: vec![1, 2],
               mode: ZoneMode::Auto, setpoints: Setpoints { temperature: 22.0, co2_max: 800.0, humidity: 50.0, cooling_threshold: Some(26.0) }, current: ZoneState::default() },
        Zone { id: "z2".into(), name: "寝室A".into(), containers: vec![3, 4],
               mode: ZoneMode::Auto, setpoints: Setpoints { temperature: 20.0, co2_max: 700.0, humidity: 55.0, cooling_threshold: Some(25.0) }, current: ZoneState::default() },
        Zone { id: "z3".into(), name: "寝室B".into(), containers: vec![5, 6],
               mode: ZoneMode::Auto, setpoints: Setpoints { temperature: 20.0, co2_max: 700.0, humidity: 55.0, cooling_threshold: Some(25.0) }, current: ZoneState::default() },
        Zone { id: "z4".into(), name: "バス・サウナ".into(), containers: vec![7, 8],
               mode: ZoneMode::Auto, setpoints: Setpoints { temperature: 38.0, co2_max: 1200.0, humidity: 70.0, cooling_threshold: None }, current: ZoneState::default() },
        Zone { id: "z5".into(), name: "多目的・ワーク".into(), containers: vec![9, 10],
               mode: ZoneMode::Auto, setpoints: Setpoints { temperature: 21.0, co2_max: 800.0, humidity: 50.0, cooling_threshold: Some(26.0) }, current: ZoneState::default() },
        Zone { id: "z6".into(), name: "機械室・廊下".into(), containers: vec![11, 12],
               mode: ZoneMode::Off, setpoints: Setpoints { temperature: 15.0, co2_max: 1200.0, humidity: 60.0, cooling_threshold: None }, current: ZoneState::default() },
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    eprintln!("[kumaushi] main() entered");
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,kumaushi_controller=debug".into()),
        )
        .init();

    info!("KUMAUSHI CLIMATE — Controller starting");

    let db_path = std::env::var("KUMAUSHI_DB").unwrap_or_else(|_| "kumaushi.db".into());
    let db = db::Database::open(&db_path).await?;
    let gpio = gpio::GpioController::new();
    let audio = audio::AudioController::new(&gpio);
    let hue = lighting::HueClient::new();
    let energy = Arc::new(energy::EnergyMonitor::new());
    let (sensor_tx, _) = broadcast::channel(256);
    let auth_token = std::env::var("AUTH_TOKEN").unwrap_or_default();

    if auth_token.is_empty() {
        tracing::warn!("AUTH_TOKEN not set — API is unauthenticated");
    }

    let dj_monitor = dj::DjMonitor::new();

    let state = Arc::new(AppState {
        db,
        zones: RwLock::new(default_zones()),
        last_seen: RwLock::new(HashMap::new()),
        sensor_tx: sensor_tx.clone(),
        gpio,
        auth_token,
        audio,
        dj: dj_monitor,
        hue,
        energy,
    });

    let mqtt_state = Arc::clone(&state);
    let mqtt_host = std::env::var("MQTT_HOST").unwrap_or_else(|_| "localhost".into());
    let mqtt_port: u16 = std::env::var("MQTT_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(1883);
    tokio::spawn(async move {
        mqtt_client::run(mqtt_state, &mqtt_host, mqtt_port).await;
    });

    let control_state = Arc::clone(&state);
    tokio::spawn(async move {
        control::run_loop(control_state).await;
    });

    // Hue: refresh lights + scenes every 30s
    let hue_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let _ = hue_state.hue.refresh_lights().await;
            let _ = hue_state.hue.refresh_scenes().await;
        }
    });

    // Energy: Powerwall + Starlink polling
    {
        let em = Arc::clone(&state.energy);
        tokio::spawn(async move { energy::EnergyMonitor::run_poll_loop(em).await; });
    }

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let router = api::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("API server listening on http://{}", bind_addr);
    axum::serve(listener, router).await?;

    Ok(())
}
