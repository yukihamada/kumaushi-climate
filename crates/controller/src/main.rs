mod api;
mod control;
mod db;
mod gpio;
mod mqtt_client;

use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::info;

use kumaushi_common::{SensorReading, Zone, ZoneMode, Setpoints, ZoneState};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub db: db::Database,
    pub zones: RwLock<Vec<Zone>>,
    /// Broadcast channel for live WebSocket pushes
    pub sensor_tx: broadcast::Sender<SensorReading>,
    pub gpio: gpio::GpioController,
}

fn default_zones() -> Vec<Zone> {
    vec![
        Zone {
            id: "z1".into(),
            name: "メインリビング".into(),
            containers: vec![1, 2],
            mode: ZoneMode::Auto,
            setpoints: Setpoints { temperature: 22.0, co2_max: 800.0, humidity: 50.0 },
            current: ZoneState::default(),
        },
        Zone {
            id: "z2".into(),
            name: "寝室A".into(),
            containers: vec![3, 4],
            mode: ZoneMode::Auto,
            setpoints: Setpoints { temperature: 20.0, co2_max: 700.0, humidity: 55.0 },
            current: ZoneState::default(),
        },
        Zone {
            id: "z3".into(),
            name: "寝室B".into(),
            containers: vec![5, 6],
            mode: ZoneMode::Auto,
            setpoints: Setpoints { temperature: 20.0, co2_max: 700.0, humidity: 55.0 },
            current: ZoneState::default(),
        },
        Zone {
            id: "z4".into(),
            name: "バス・サウナ".into(),
            containers: vec![7, 8],
            mode: ZoneMode::Auto,
            setpoints: Setpoints { temperature: 38.0, co2_max: 1200.0, humidity: 70.0 },
            current: ZoneState::default(),
        },
        Zone {
            id: "z5".into(),
            name: "多目的・ワーク".into(),
            containers: vec![9, 10],
            mode: ZoneMode::Auto,
            setpoints: Setpoints { temperature: 21.0, co2_max: 800.0, humidity: 50.0 },
            current: ZoneState::default(),
        },
        Zone {
            id: "z6".into(),
            name: "機械室・廊下".into(),
            containers: vec![11, 12],
            mode: ZoneMode::Off,
            setpoints: Setpoints::default(),
            current: ZoneState::default(),
        },
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,kumaushi_controller=debug".into()),
        )
        .init();

    info!("KUMAUSHI CLIMATE — Controller starting");

    let db = db::Database::open("kumaushi.db").await?;
    let gpio = gpio::GpioController::new();
    let (sensor_tx, _) = broadcast::channel(256);

    let state = Arc::new(AppState {
        db,
        zones: RwLock::new(default_zones()),
        sensor_tx: sensor_tx.clone(),
        gpio,
    });

    // MQTT subscriber task
    let mqtt_state = Arc::clone(&state);
    let mqtt_host = std::env::var("MQTT_HOST").unwrap_or_else(|_| "localhost".into());
    let mqtt_port: u16 = std::env::var("MQTT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(1883);

    tokio::spawn(async move {
        mqtt_client::run(mqtt_state, &mqtt_host, mqtt_port).await;
    });

    // PID control loop task (runs every 30 seconds)
    let control_state = Arc::clone(&state);
    tokio::spawn(async move {
        control::run_loop(control_state).await;
    });

    // HTTP API server
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let router = api::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    info!("API server listening on http://{}", bind_addr);
    axum::serve(listener, router).await?;

    Ok(())
}
