use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time;
use tracing::{debug, info, warn};

use kumaushi_common::{PidState, ZoneMode};
use crate::SharedState;

const SENSOR_STALE_SECS: u64 = 90;
/// Minimum fan output when sensor is stale (failsafe ventilation)
const FAILSAFE_FAN: f64 = 0.20;

type PidMap = HashMap<String, (PidState, PidState)>;

pub async fn run_loop(state: Arc<crate::AppState>) {
    let mut interval = time::interval(Duration::from_secs(30));
    let mut pids: PidMap = HashMap::new();

    loop {
        interval.tick().await;
        step(&state, &mut pids).await;
    }
}

async fn step(state: &Arc<crate::AppState>, pids: &mut PidMap) {
    let dt = 30.0_f64;
    let now = Instant::now();

    // Apply active schedules: override setpoints while schedule window is active
    apply_schedules(state).await;

    // Check sensor staleness
    let stale_zones = detect_stale_zones(state, now).await;

    let zones = state.zones.read().await;

    for zone in zones.iter() {
        if zone.mode == ZoneMode::Off {
            continue;
        }

        let is_stale = stale_zones.contains(&zone.id);
        let entry = pids
            .entry(zone.id.clone())
            .or_insert_with(|| (PidState::ventilation(), PidState::temperature()));
        let (co2_pid, temp_pid) = entry;

        // --- CO2 / Ventilation ---
        let fan_id = format!("fan-{}", zone.id);
        let fan_output = if is_stale {
            // Failsafe: sensor lost → minimum ventilation, reset PID integral
            co2_pid.reset();
            warn!("Zone {} sensor stale — failsafe fan {:.0}%", zone.id, FAILSAFE_FAN * 100.0);
            FAILSAFE_FAN
        } else if let Some(co2) = zone.current.co2_ppm {
            if zone.mode == ZoneMode::Auto {
                // Error: how much above setpoint? Positive error → more fan
                let error_input = (zone.setpoints.co2_max * 2.0 - co2).clamp(0.0, zone.setpoints.co2_max * 2.0);
                co2_pid.compute(zone.setpoints.co2_max, error_input, dt).clamp(0.0, 1.0)
            } else {
                state.gpio.get(&fan_id)
            }
        } else {
            state.gpio.get(&fan_id)
        };
        state.gpio.set_pwm(&fan_id, fan_output);
        let _ = state.db.log_control(&fan_id, "fan", fan_output, Some(if is_stale { "failsafe" } else { "co2_pid" }));

        // CO2 critical alert
        if let Some(co2) = zone.current.co2_ppm {
            if co2 > 1500.0 {
                let _ = state.db.insert_alert(&zone.id, "critical",
                    &format!("CO₂ {:.0} ppm — 換気異常の可能性", co2));
                warn!("Zone {} CO2 CRITICAL: {:.0} ppm", zone.id, co2);
            }
        }

        // Stale sensor alert (only insert once; debounce via DB lookup is skipped for simplicity)
        if is_stale {
            let _ = state.db.insert_alert(&zone.id, "warning",
                "センサーデータ未受信（90秒超）— フェイルセーフ換気中");
        }

        // --- Temperature / Heating ---
        if let Some(temp) = zone.current.temperature {
            let relay_id = format!("heat-{}", zone.id);
            let sp = zone.setpoints.temperature;
            let on = if is_stale {
                // Failsafe: keep heat on to prevent freezing in Hokkaido winter
                temp < 10.0
            } else if zone.mode == ZoneMode::Auto {
                temp_pid.compute(sp, temp, dt);
                temp < sp - 0.5
            } else {
                state.gpio.get(&relay_id) > 0.5
            };
            state.gpio.set_relay(&relay_id, on);
            debug!("Zone {} heat: {} (T={:.1}°C sp={:.1}°C)", zone.id, on, temp, sp);
        }

        // --- Humidity ---
        if let Some(hum) = zone.current.humidity {
            let dehu_id = format!("dehu-{}", zone.id);
            if zone.mode == ZoneMode::Auto && !is_stale {
                state.gpio.set_relay(&dehu_id, hum > zone.setpoints.humidity + 5.0);
            }
        }

        // --- Z4: Sauna / Water temp safety ---
        if zone.id == "z4" {
            if let Some(wt) = zone.current.water_temp {
                let boiler_id = "boiler-z4";
                if wt > 42.0 {
                    state.gpio.set_relay(boiler_id, false);
                    let _ = state.db.insert_alert("z4", "warning",
                        &format!("水温 {:.1}°C — ボイラー安全遮断", wt));
                    warn!("Z4 water temp {:.1}°C — boiler OFF", wt);
                } else if wt < 38.0 && zone.mode == ZoneMode::Auto && !is_stale {
                    state.gpio.set_relay(boiler_id, true);
                }
            }
        }
    }
}

/// Mark zones as stale if no sensor reading in SENSOR_STALE_SECS seconds
async fn detect_stale_zones(state: &Arc<crate::AppState>, now: Instant) -> std::collections::HashSet<String> {
    let last_seen = state.last_seen.read().await;
    let zones = state.zones.read().await;
    let mut stale = std::collections::HashSet::new();

    for zone in zones.iter() {
        // Check if any node in this zone has a recent reading
        let has_fresh = zone.containers.iter().any(|c| {
            ["a", "b"].iter().any(|s| {
                let node_id = format!("node-{}-{}{}", zone.id, c, s);
                last_seen.get(&node_id)
                    .map(|t| now.duration_since(*t).as_secs() < SENSOR_STALE_SECS)
                    .unwrap_or(false)
            })
        });

        // Mark stale only if we've ever seen data (skip uninstalled zones)
        let ever_seen = zone.containers.iter().any(|c| {
            ["a", "b"].iter().any(|s| {
                let node_id = format!("node-{}-{}{}", zone.id, c, s);
                last_seen.contains_key(&node_id)
            })
        });

        if ever_seen && !has_fresh {
            stale.insert(zone.id.clone());
        }
    }

    // Update zone.current.sensor_stale flag
    drop(zones);
    let mut zones_w = state.zones.write().await;
    for zone in zones_w.iter_mut() {
        zone.current.sensor_stale = stale.contains(&zone.id);
    }

    stale
}

/// Apply active schedule setpoints to zones (auto mode only)
async fn apply_schedules(state: &Arc<crate::AppState>) {
    let schedules = match state.db.get_active_schedules() {
        Ok(s) => s,
        Err(e) => { warn!("schedule fetch error: {}", e); return; }
    };

    if schedules.is_empty() { return; }

    let mut zones = state.zones.write().await;
    for sched in &schedules {
        if let Some(zone) = zones.iter_mut().find(|z| z.id == sched.zone_id) {
            if zone.mode == ZoneMode::Auto {
                zone.setpoints = sched.setpoints.clone();
                debug!("Schedule applied to zone {}: T={}°C CO2={}", zone.id,
                       zone.setpoints.temperature, zone.setpoints.co2_max);
            }
        }
    }
}
