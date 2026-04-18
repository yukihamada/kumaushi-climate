/// Philips Hue local bridge REST API client.
///
/// Discover bridge via ENV var HUE_BRIDGE_IP (fallback: 192.168.1.2).
/// HUE_USERNAME must be set (register once with bridge button press).
///
/// API reference: https://developers.meethue.com/develop/hue-api/

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

use kumaushi_common::{HueLight, LightScene};

pub struct HueClient {
    bridge_ip: String,
    username: String,
    client: reqwest::Client,
    lights: Arc<RwLock<Vec<HueLight>>>,
    scenes: Arc<RwLock<Vec<LightScene>>>,
}

impl HueClient {
    pub fn new() -> Self {
        let bridge_ip = std::env::var("HUE_BRIDGE_IP").unwrap_or_else(|_| "192.168.1.2".into());
        let username = std::env::var("HUE_USERNAME").unwrap_or_default();
        if username.is_empty() {
            warn!("HUE_USERNAME not set — Hue control disabled");
        }
        // Hue bridge uses self-signed cert; allow self-signed for local API
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        Self {
            bridge_ip,
            username,
            client,
            lights: Arc::new(RwLock::new(vec![])),
            scenes: Arc::new(RwLock::new(vec![])),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/api/{}", self.bridge_ip, self.username)
    }

    pub fn is_configured(&self) -> bool {
        !self.username.is_empty()
    }

    /// Refresh light states from bridge (call periodically).
    pub async fn refresh_lights(&self) -> anyhow::Result<()> {
        if !self.is_configured() { return Ok(()); }
        let url = format!("{}/lights", self.base_url());
        let resp = self.client.get(&url).send().await?.json::<serde_json::Value>().await?;

        let mut lights = Vec::new();
        if let Some(map) = resp.as_object() {
            for (id, v) in map {
                let name = v["name"].as_str().unwrap_or("").to_string();
                let state = &v["state"];
                let on = state["on"].as_bool().unwrap_or(false);
                let bri = state["bri"].as_u64().unwrap_or(0) as u8;
                let ct = state["ct"].as_u64().map(|v| v as u16);
                let reachable = state["reachable"].as_bool().unwrap_or(false);
                lights.push(HueLight { id: id.clone(), name, on, brightness: bri, color_temp: ct, reachable });
            }
        }
        debug!("Hue: {} lights refreshed", lights.len());
        *self.lights.write().unwrap() = lights;
        Ok(())
    }

    /// Refresh available scenes from bridge.
    pub async fn refresh_scenes(&self) -> anyhow::Result<()> {
        if !self.is_configured() { return Ok(()); }
        let url = format!("{}/scenes", self.base_url());
        let resp = self.client.get(&url).send().await?.json::<serde_json::Value>().await?;

        let mut scenes = Vec::new();
        if let Some(map) = resp.as_object() {
            for (id, v) in map {
                let name = v["name"].as_str().unwrap_or("").to_string();
                scenes.push(LightScene { id: id.clone(), name });
            }
        }
        *self.scenes.write().unwrap() = scenes;
        Ok(())
    }

    pub fn all_lights(&self) -> Vec<HueLight> {
        self.lights.read().unwrap().clone()
    }

    pub fn all_scenes(&self) -> Vec<LightScene> {
        self.scenes.read().unwrap().clone()
    }

    /// Set a single light on/off + brightness.
    pub async fn set_light(&self, light_id: &str, on: bool, brightness: Option<u8>) -> anyhow::Result<()> {
        if !self.is_configured() { return Ok(()); }
        let url = format!("{}/lights/{}/state", self.base_url(), light_id);
        let mut body = serde_json::json!({ "on": on });
        if let Some(bri) = brightness {
            body["bri"] = serde_json::json!(bri);
        }
        self.client.put(&url).json(&body).send().await?;
        debug!("Hue: light {} on={} bri={:?}", light_id, on, brightness);
        Ok(())
    }

    /// Activate a scene by scene ID.
    pub async fn activate_scene(&self, scene_id: &str, group_id: &str) -> anyhow::Result<()> {
        if !self.is_configured() { return Ok(()); }
        let url = format!("{}/groups/{}/action", self.base_url(), group_id);
        let body = serde_json::json!({ "scene": scene_id });
        self.client.put(&url).json(&body).send().await?;
        info!("Hue: scene {} activated on group {}", scene_id, group_id);
        Ok(())
    }

    /// Set a group (room) on/off + brightness.
    pub async fn set_group(&self, group_id: &str, on: bool, brightness: Option<u8>, color_temp: Option<u16>) -> anyhow::Result<()> {
        if !self.is_configured() { return Ok(()); }
        let url = format!("{}/groups/{}/action", self.base_url(), group_id);
        let mut body = serde_json::json!({ "on": on });
        if let Some(bri) = brightness { body["bri"] = serde_json::json!(bri); }
        if let Some(ct) = color_temp { body["ct"] = serde_json::json!(ct); }
        self.client.put(&url).json(&body).send().await?;
        debug!("Hue: group {} on={}", group_id, on);
        Ok(())
    }

    /// Preset scene shortcuts mapped to Hue group actions.
    /// Groups: 1=リビング, 2=寝室A, 3=寝室B, 4=バス, 5=多目的
    pub async fn apply_preset(&self, preset: &str) -> anyhow::Result<()> {
        match preset {
            "party" => {
                // Warm low light, living room + multipurpose
                self.set_group("1", true, Some(200), Some(370)).await?;
                self.set_group("5", true, Some(180), Some(300)).await?;
                // Bedrooms: dim
                self.set_group("2", true, Some(80), Some(400)).await?;
                self.set_group("3", true, Some(80), Some(400)).await?;
                info!("Hue preset: party");
            }
            "sleep" => {
                // Very dim warm in bedrooms, off elsewhere
                self.set_group("1", false, None, None).await?;
                self.set_group("5", false, None, None).await?;
                self.set_group("2", true, Some(30), Some(500)).await?;
                self.set_group("3", true, Some(30), Some(500)).await?;
                info!("Hue preset: sleep");
            }
            "morning" => {
                // Bright cool-white everywhere
                for g in &["1", "2", "3", "4", "5"] {
                    self.set_group(g, true, Some(254), Some(233)).await?;
                }
                info!("Hue preset: morning");
            }
            "sauna" => {
                // Warm red-amber only in sauna zone
                self.set_group("4", true, Some(120), Some(500)).await?;
                info!("Hue preset: sauna");
            }
            "all_off" => {
                for g in &["1", "2", "3", "4", "5"] {
                    self.set_group(g, false, None, None).await?;
                }
                info!("Hue preset: all_off");
            }
            _ => {}
        }
        Ok(())
    }
}

impl Default for HueClient {
    fn default() -> Self { Self::new() }
}
