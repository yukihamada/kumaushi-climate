use std::sync::Arc;
use std::time::Instant;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tracing::{debug, error, info};

use kumaushi_common::{SensorReading, SensorType};

pub async fn run(state: Arc<crate::AppState>, host: &str, port: u16) {
    let client_id = format!("kumaushi-controller-{}", std::process::id());
    let mut opts = MqttOptions::new(client_id, host, port);
    opts.set_keep_alive(std::time::Duration::from_secs(30));
    opts.set_clean_session(true);

    info!("Connecting to MQTT broker {}:{}", host, port);

    loop {
        let (client, mut eventloop) = AsyncClient::new(opts.clone(), 64);

        if let Err(e) = client.subscribe("kumaushi/sensors/#", QoS::AtMostOnce).await {
            error!("MQTT subscribe error: {}", e);
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            continue;
        }

        info!("MQTT subscribed to kumaushi/sensors/#");

        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    let payload = match std::str::from_utf8(&p.payload) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if let Some(reading) = parse_topic_payload(&p.topic, payload) {
                        debug!("Sensor: {} {} {:.1}", reading.node_id, reading.sensor_type.as_str(), reading.value);

                        // Update last_seen for failsafe tracking
                        {
                            let mut last_seen = state.last_seen.write().await;
                            last_seen.insert(reading.node_id.clone(), Instant::now());
                        }

                        if let Err(e) = state.db.insert_reading(&reading) {
                            error!("DB insert: {}", e);
                        }

                        update_zone_state(&state, &reading).await;
                        let _ = state.sensor_tx.send(reading);
                    }
                }
                Ok(Event::Incoming(Packet::ConnAck(_))) => info!("MQTT connected"),
                Ok(_) => {}
                Err(e) => {
                    error!("MQTT error: {} — reconnecting in 5s", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    break;
                }
            }
        }
    }
}

fn parse_topic_payload(topic: &str, payload: &str) -> Option<SensorReading> {
    let parts: Vec<&str> = topic.splitn(4, '/').collect();
    if parts.len() != 4 || parts[0] != "kumaushi" || parts[1] != "sensors" {
        return None;
    }
    let node_id = parts[2].to_string();
    let sensor_type = match parts[3] {
        "co2" => SensorType::Co2,
        "temperature" => SensorType::Temperature,
        "humidity" => SensorType::Humidity,
        "water_temp" => SensorType::WaterTemp,
        "pressure" => SensorType::Pressure,
        _ => return None,
    };
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let value = v["v"].as_f64()?;
    let unit = sensor_type.unit().to_string();
    let timestamp = if let Some(ts) = v["ts"].as_i64() {
        chrono::DateTime::from_timestamp(ts, 0).unwrap_or_else(chrono::Utc::now)
    } else {
        chrono::Utc::now()
    };
    Some(SensorReading { node_id, sensor_type, value, unit, timestamp })
}

fn node_to_zone(node_id: &str) -> Option<&str> {
    if node_id.starts_with("node-z1") { Some("z1") }
    else if node_id.starts_with("node-z2") { Some("z2") }
    else if node_id.starts_with("node-z3") { Some("z3") }
    else if node_id.starts_with("node-z4") { Some("z4") }
    else if node_id.starts_with("node-z5") { Some("z5") }
    else if node_id.starts_with("node-z6") { Some("z6") }
    else { None }
}

async fn update_zone_state(state: &Arc<crate::AppState>, reading: &SensorReading) {
    let Some(zone_id) = node_to_zone(&reading.node_id) else { return };
    let mut zones = state.zones.write().await;
    if let Some(zone) = zones.iter_mut().find(|z| z.id == zone_id) {
        let now = Some(chrono::Utc::now());
        match reading.sensor_type {
            SensorType::Temperature => zone.current.temperature = Some(reading.value),
            SensorType::Humidity    => zone.current.humidity = Some(reading.value),
            SensorType::Co2         => zone.current.co2_ppm = Some(reading.value),
            SensorType::WaterTemp   => zone.current.water_temp = Some(reading.value),
            SensorType::Pressure    => {}
        }
        zone.current.updated_at = now;
        zone.current.sensor_stale = false;
    }
}
