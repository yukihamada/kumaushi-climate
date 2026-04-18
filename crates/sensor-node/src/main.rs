// KUMAUSHI CLIMATE — ESP32-S3 Sensor Node Firmware
// Reads CO2 (MH-Z19B) + Temp/Humidity (SHT31) + Water Temp (DS18B20) and publishes via MQTT.

mod sensors;

use esp_idf_hal::{
    delay::FreeRtos,
    i2c::{I2cConfig, I2cDriver},
    peripherals::Peripherals,
    prelude::*,
};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    mqtt::client::{EspMqttClient, EspMqttConnection, MqttClientConfiguration, QoS},
    nvs::EspDefaultNvsPartition,
    sntp::{EspSntp, SyncStatus},
    wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::{error, info, warn};

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");
const MQTT_URI: &str = env!("MQTT_URI");
const NODE_ID: &str = env!("NODE_ID");

/// Z4 water temperature node publishes DS18B20. Other zones skip it.
const HAS_WATER_TEMP: bool = cfg!(feature = "water_temp");

const PUBLISH_INTERVAL_MS: u32 = 30_000;
const WIFI_RETRY_DELAY_MS: u32 = 5_000;
const MQTT_RETRY_DELAY_MS: u32 = 5_000;

fn main() -> anyhow::Result<()> {
    esp_idf_hal::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("KUMAUSHI Sensor Node '{}' starting…", NODE_ID);

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // ── WiFi (with reconnect loop) ────────────────────────────────────────
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;
    connect_wifi(&mut wifi);

    // ── NTP time sync ────────────────────────────────────────────────────
    let sntp = EspSntp::new_default()?;
    info!("Waiting for NTP sync…");
    let mut ntp_tries = 0u32;
    while sntp.get_sync_status() != SyncStatus::Completed && ntp_tries < 20 {
        FreeRtos::delay_ms(500);
        ntp_tries += 1;
    }
    if sntp.get_sync_status() == SyncStatus::Completed {
        info!("NTP synced");
    } else {
        warn!("NTP sync timeout — using monotonic time");
    }

    // ── I2C for SHT31 (GPIO8=SDA, GPIO9=SCL) ────────────────────────────
    let i2c = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio8,
        peripherals.pins.gpio9,
        &I2cConfig::new().baudrate(400_u32.kHz().into()),
    )?;
    let mut sht31 = sensors::sht31::Sht31::new(i2c);

    // ── UART for MH-Z19B (GPIO17=TX, GPIO18=RX) ──────────────────────────
    let mut co2_sensor = sensors::co2::MhZ19B::new(
        peripherals.uart2,
        peripherals.pins.gpio17,
        peripherals.pins.gpio18,
    )?;

    // ── DS18B20 water temperature (GPIO5, Z4 nodes only) ─────────────────
    #[cfg(feature = "water_temp")]
    let mut ds18b20 = sensors::ds18b20::Ds18b20::new(peripherals.pins.gpio5)
        .expect("DS18B20 init failed");

    info!("Sensors initialized. Publishing every {}s", PUBLISH_INTERVAL_MS / 1000);

    // ── MQTT connection (reconnects on failure) ────────────────────────────
    let mqtt_cfg = MqttClientConfiguration {
        client_id: Some(NODE_ID),
        ..Default::default()
    };
    let (mut mqtt, _conn) = connect_mqtt(MQTT_URI, &mqtt_cfg);

    // ── Main loop ─────────────────────────────────────────────────────────
    loop {
        // Check WiFi and reconnect if needed
        if !wifi.is_connected().unwrap_or(false) {
            warn!("WiFi disconnected — reconnecting…");
            connect_wifi(&mut wifi);
            // Reconnect MQTT after WiFi restored
            let (new_mqtt, _conn) = connect_mqtt(MQTT_URI, &mqtt_cfg);
            mqtt = new_mqtt;
        }

        let now = unix_time_seconds();

        // CO2
        if let Some(co2) = co2_sensor.read_co2() {
            let payload = format!(r#"{{"v":{},"unit":"ppm","ts":{}}}"#, co2, now);
            publish(&mut mqtt, &format!("kumaushi/sensors/{}/co2", NODE_ID), &payload);
        }

        // Temperature + Humidity
        if let Some((temp, hum)) = sht31.read() {
            let t_payload = format!(r#"{{"v":{:.2},"unit":"°C","ts":{}}}"#, temp, now);
            let h_payload = format!(r#"{{"v":{:.1},"unit":"%RH","ts":{}}}"#, hum, now);
            publish(&mut mqtt, &format!("kumaushi/sensors/{}/temperature", NODE_ID), &t_payload);
            publish(&mut mqtt, &format!("kumaushi/sensors/{}/humidity", NODE_ID), &h_payload);
        }

        // Water temperature (Z4 only)
        #[cfg(feature = "water_temp")]
        if let Some(wt) = ds18b20.read_temp() {
            let payload = format!(r#"{{"v":{:.2},"unit":"°C","ts":{}}}"#, wt, now);
            publish(&mut mqtt, &format!("kumaushi/sensors/{}/water_temp", NODE_ID), &payload);
        }

        FreeRtos::delay_ms(PUBLISH_INTERVAL_MS);
    }
}

fn connect_wifi(wifi: &mut BlockingWifi<EspWifi>) {
    loop {
        let result = (|| -> anyhow::Result<()> {
            wifi.set_configuration(&Configuration::Client(ClientConfiguration {
                ssid: WIFI_SSID.try_into().unwrap(),
                password: WIFI_PASS.try_into().unwrap(),
                ..Default::default()
            }))?;
            wifi.start()?;
            wifi.connect()?;
            wifi.wait_netif_up()?;
            Ok(())
        })();
        match result {
            Ok(_) => {
                info!("WiFi connected: {:?}", wifi.wifi().sta_netif().get_ip_info());
                return;
            }
            Err(e) => {
                error!("WiFi connect failed: {} — retrying in {}ms", e, WIFI_RETRY_DELAY_MS);
                wifi.stop().ok();
                FreeRtos::delay_ms(WIFI_RETRY_DELAY_MS);
            }
        }
    }
}

fn connect_mqtt(uri: &str, cfg: &MqttClientConfiguration) -> (EspMqttClient<'static>, EspMqttConnection) {
    loop {
        match EspMqttClient::new(uri, cfg) {
            Ok(pair) => {
                info!("MQTT connected to {}", uri);
                return pair;
            }
            Err(e) => {
                error!("MQTT connect failed: {} — retrying in {}ms", e, MQTT_RETRY_DELAY_MS);
                FreeRtos::delay_ms(MQTT_RETRY_DELAY_MS);
            }
        }
    }
}

fn publish(mqtt: &mut EspMqttClient, topic: &str, payload: &str) {
    if let Err(e) = mqtt.publish(topic, QoS::AtMostOnce, false, payload.as_bytes()) {
        warn!("MQTT publish {} failed: {:?}", topic, e);
    } else {
        info!("→ {} {}", topic, payload);
    }
}

fn unix_time_seconds() -> i64 {
    unsafe { esp_idf_sys::time(std::ptr::null_mut()) as i64 }
}
