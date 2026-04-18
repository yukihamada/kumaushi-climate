use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{debug, info, warn};

use kumaushi_common::{PidState, ZoneMode};
use crate::SharedState;

/// PID controllers: zone_id → (co2_pid, temp_pid)
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
    let zones = state.zones.read().await;
    let dt = 30.0_f64; // seconds since last step

    for zone in zones.iter() {
        if zone.mode == ZoneMode::Off {
            continue;
        }

        let entry = pids
            .entry(zone.id.clone())
            .or_insert_with(|| (PidState::ventilation(), PidState::temperature()));

        let (co2_pid, temp_pid) = entry;

        // --- CO2 / Ventilation control ---
        if let Some(co2) = zone.current.co2_ppm {
            let fan_id = format!("fan-{}", zone.id);
            let output = if zone.mode == ZoneMode::Auto {
                // PID: error = co2 - setpoint (positive = too high → increase fan)
                co2_pid.compute(zone.setpoints.co2_max, zone.setpoints.co2_max * 2.0 - co2, dt)
                    .clamp(0.0, 1.0)
            } else {
                // Manual: keep current GPIO value
                state.gpio.get(&fan_id)
            };
            state.gpio.set_pwm(&fan_id, output);
            if let Err(e) = state.db.log_control(&fan_id, "fan", output, Some("co2_pid")) {
                warn!("log_control error: {}", e);
            }

            // Alert if CO2 critically high
            if co2 > 1500.0 {
                let _ = state.db.insert_alert(
                    &zone.id,
                    "critical",
                    &format!("CO₂ {:.0} ppm — 換気異常の可能性", co2),
                );
                warn!("Zone {} CO2 CRITICAL: {:.0} ppm", zone.id, co2);
            }
        }

        // --- Temperature / Heating control ---
        if let Some(temp) = zone.current.temperature {
            let relay_id = format!("heat-{}", zone.id);
            let sp = zone.setpoints.temperature;
            let output = if zone.mode == ZoneMode::Auto {
                let raw = temp_pid.compute(sp, temp, dt);
                // Simple hysteresis: on if error > 0.5°C, off if < 0.1°C
                if temp < sp - 0.5 { 1.0 } else if temp > sp + 0.1 { 0.0 } else { state.gpio.get(&relay_id) }
            } else {
                state.gpio.get(&relay_id)
            };
            state.gpio.set_relay(&relay_id, output > 0.5);
            debug!("Zone {} heat relay: {}", zone.id, output > 0.5);
        }

        // --- Humidity control (dehumidifier) ---
        if let Some(hum) = zone.current.humidity {
            let dehu_id = format!("dehu-{}", zone.id);
            let sp = zone.setpoints.humidity;
            if zone.mode == ZoneMode::Auto {
                let on = hum > sp + 5.0; // simple on/off with 5% deadband
                state.gpio.set_relay(&dehu_id, on);
            }
        }

        // --- Sauna zone: water temperature guard ---
        if zone.id == "z4" {
            if let Some(water_temp) = zone.current.water_temp {
                let pump_id = "pump-z4";
                let boiler_id = "boiler-z4";
                // Safety cutoff: never exceed 42°C for bath
                if water_temp > 42.0 {
                    state.gpio.set_relay(boiler_id, false);
                    let _ = state.db.insert_alert(
                        "z4",
                        "warning",
                        &format!("水温 {:.1}°C — ボイラー停止", water_temp),
                    );
                } else if water_temp < 38.0 && zone.mode == ZoneMode::Auto {
                    state.gpio.set_relay(boiler_id, true);
                }
            }
        }
    }
}
