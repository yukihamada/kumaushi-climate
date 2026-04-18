/// Energy monitoring: Tesla Powerwall (local API) + Starlink status.
///
/// Powerwall local gateway: https://192.168.91.1
///   GET /api/meters/aggregates  → solar, grid, battery, load watts
///   GET /api/system_status/soe  → battery state-of-charge %
///
/// Starlink gRPC dishStatusRequest is complex; we use the simpler
/// HTTP stats endpoint at http://192.168.100.1/api/status (dish API v2).
///
/// ENV vars:
///   POWERWALL_IP    = 192.168.91.1 (default)
///   POWERWALL_PASS  = Tesla password
///   STARLINK_IP     = 192.168.100.1 (default, Starlink dishy)

use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

use kumaushi_common::EnergySnapshot;

pub struct EnergyMonitor {
    snapshot: Arc<RwLock<EnergySnapshot>>,
    client: reqwest::Client,
    powerwall_ip: String,
    powerwall_pass: String,
    starlink_ip: String,
    pw_token: Arc<RwLock<Option<String>>>,
}

impl EnergyMonitor {
    pub fn new() -> Self {
        let powerwall_ip = std::env::var("POWERWALL_IP").unwrap_or_else(|_| "192.168.91.1".into());
        let powerwall_pass = std::env::var("POWERWALL_PASS").unwrap_or_default();
        let starlink_ip = std::env::var("STARLINK_IP").unwrap_or_else(|_| "192.168.100.1".into());

        if powerwall_pass.is_empty() {
            warn!("POWERWALL_PASS not set — Powerwall monitoring disabled");
        }

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)  // Powerwall uses self-signed cert
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap();

        Self {
            snapshot: Arc::new(RwLock::new(EnergySnapshot {
                timestamp: chrono::Utc::now(),
                powerwall_online: false,
                ..Default::default()
            })),
            client,
            powerwall_ip,
            powerwall_pass,
            starlink_ip,
            pw_token: Arc::new(RwLock::new(None)),
        }
    }

    pub fn current(&self) -> EnergySnapshot {
        self.snapshot.read().unwrap().clone()
    }

    /// Authenticate with Powerwall gateway and store token.
    async fn pw_authenticate(&self) -> anyhow::Result<String> {
        let url = format!("https://{}/api/login/Basic", self.powerwall_ip);
        let body = serde_json::json!({
            "username": "customer",
            "password": self.powerwall_pass,
            "email": "kumaushi@soluna.jp",
            "force_sm_off": false
        });
        let resp = self.client.post(&url).json(&body).send().await?;
        let json: serde_json::Value = resp.json().await?;
        let token = json["token"].as_str().ok_or_else(|| anyhow::anyhow!("no token"))?.to_string();
        info!("Powerwall: authenticated");
        Ok(token)
    }

    /// Ensure we have a valid token, re-authenticating if needed.
    async fn pw_token(&self) -> Option<String> {
        if self.powerwall_pass.is_empty() { return None; }
        let existing = self.pw_token.read().unwrap().clone();
        if existing.is_some() { return existing; }
        match self.pw_authenticate().await {
            Ok(t) => {
                *self.pw_token.write().unwrap() = Some(t.clone());
                Some(t)
            }
            Err(e) => {
                warn!("Powerwall auth failed: {}", e);
                None
            }
        }
    }

    /// Poll Powerwall for current energy data.
    pub async fn poll_powerwall(&self) {
        let Some(token) = self.pw_token().await else { return };

        // --- Aggregated meters ---
        let agg_url = format!("https://{}/api/meters/aggregates", self.powerwall_ip);
        let agg = self.client
            .get(&agg_url)
            .header("Cookie", format!("AuthCookie={}", token))
            .send().await;

        let (solar_w, grid_w, battery_w, load_w) = match agg {
            Ok(r) => {
                match r.json::<serde_json::Value>().await {
                    Ok(json) => {
                        let solar = json["solar"]["instant_power"].as_f64().unwrap_or(0.0);
                        let grid  = json["site"]["instant_power"].as_f64().unwrap_or(0.0);
                        let batt  = json["battery"]["instant_power"].as_f64().unwrap_or(0.0);
                        let load  = json["load"]["instant_power"].as_f64().unwrap_or(0.0);
                        debug!("Powerwall: solar={:.0}W grid={:.0}W bat={:.0}W load={:.0}W", solar, grid, batt, load);
                        (solar, grid, batt, load)
                    }
                    Err(e) => {
                        warn!("Powerwall aggregates parse error: {}", e);
                        // Token might be stale
                        *self.pw_token.write().unwrap() = None;
                        return;
                    }
                }
            }
            Err(e) => { warn!("Powerwall aggregates request failed: {}", e); return; }
        };

        // --- State of charge ---
        let soe_url = format!("https://{}/api/system_status/soe", self.powerwall_ip);
        let battery_pct = match self.client
            .get(&soe_url)
            .header("Cookie", format!("AuthCookie={}", token))
            .send().await
        {
            Ok(r) => r.json::<serde_json::Value>().await
                .ok()
                .and_then(|j| j["percentage"].as_f64())
                .unwrap_or(0.0),
            Err(_) => 0.0,
        };

        let mut snap = self.snapshot.write().unwrap();
        snap.solar_w = solar_w;
        snap.grid_w = grid_w;
        snap.battery_w = battery_w;
        snap.battery_pct = battery_pct;
        snap.load_w = load_w;
        snap.powerwall_online = true;
        snap.timestamp = chrono::Utc::now();
    }

    /// Poll Starlink dish status for throughput metrics.
    pub async fn poll_starlink(&self) {
        // Starlink dish HTTP API (firmware ≥ 2024.x)
        let url = format!("http://{}/api/status", self.starlink_ip);
        match self.client.get(&url).send().await {
            Ok(r) => {
                if let Ok(json) = r.json::<serde_json::Value>().await {
                    let dl = json["downlinkThroughputBps"].as_f64().map(|v| v / 1_000_000.0);
                    let ul = json["uplinkThroughputBps"].as_f64().map(|v| v / 1_000_000.0);
                    let mut snap = self.snapshot.write().unwrap();
                    snap.starlink_dl_mbps = dl;
                    snap.starlink_ul_mbps = ul;
                    if let (Some(d), Some(u)) = (dl, ul) {
                        debug!("Starlink: ↓{:.1}Mbps ↑{:.1}Mbps", d, u);
                    }
                }
            }
            Err(_) => {
                // Starlink not reachable — not critical, leave values as None
                let mut snap = self.snapshot.write().unwrap();
                snap.starlink_dl_mbps = None;
                snap.starlink_ul_mbps = None;
            }
        }
    }

    /// Run polling loop: Powerwall every 30s, Starlink every 60s.
    pub async fn run_poll_loop(monitor: Arc<EnergyMonitor>) {
        let mut interval_pw = tokio::time::interval(std::time::Duration::from_secs(30));
        let mut starlink_counter = 0u32;
        loop {
            interval_pw.tick().await;
            monitor.poll_powerwall().await;
            starlink_counter += 1;
            if starlink_counter >= 2 {
                monitor.poll_starlink().await;
                starlink_counter = 0;
            }
        }
    }
}

impl Default for EnergyMonitor {
    fn default() -> Self { Self::new() }
}
