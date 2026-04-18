// KUMAUSHI CLIMATE — ESP32-S3 Sensor Node Firmware
// Reads CO2 (MH-Z19B) + Temp/Humidity (SHT31) and publishes via MQTT.

mod sensors;

use esp_idf_hal::{
    delay::FreeRtos,
    i2c::{I2cConfig, I2cDriver},
    peripherals::Peripherals,
    prelude::*,
    uart::{config::Config as UartConfig, UartDriver},
};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    mqtt::client::{EspMqttClient, MqttClientConfiguration, QoS},
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use log::{error, info, warn};

// ── Configuration (override with NVS or compile-time env vars) ──────────────

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASS: &str = env!("WIFI_PASS");
const MQTT_URI: &str = env!("MQTT_URI");   // e.g. "mqtt://192.168.1.100:1883"
const NODE_ID: &str = env!("NODE_ID");     // e.g. "node-z1-a"

const PUBLISH_INTERVAL_MS: u32 = 30_000;  // 30 seconds

fn main() -> anyhow::Result<()> {
    esp_idf_hal::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    info!("KUMAUSHI Sensor Node '{}' starting…", NODE_ID);

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // ── WiFi ─────────────────────────────────────────────────────────────────
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?,
        sysloop,
    )?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID.try_into().unwrap(),
        password: WIFI_PASS.try_into().unwrap(),
        ..Default::default()
    }))?;
    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    info!("WiFi connected: {:?}", wifi.wifi().sta_netif().get_ip_info()?);

    // ── MQTT ─────────────────────────────────────────────────────────────────
    let mqtt_cfg = MqttClientConfiguration {
        client_id: Some(NODE_ID),
        ..Default::default()
    };
    let (mut mqtt, _) = EspMqttClient::new(MQTT_URI, &mqtt_cfg)?;
    info!("MQTT connected to {}", MQTT_URI);

    // ── I2C for SHT31 (GPIO8=SDA, GPIO9=SCL) ────────────────────────────────
    let i2c = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio8,   // SDA
        peripherals.pins.gpio9,   // SCL
        &I2cConfig::new().baudrate(400_u32.kHz().into()),
    )?;
    let mut sht31 = sensors::sht31::Sht31::new(i2c);

    // ── UART for MH-Z19B (GPIO17=TX, GPIO18=RX) ──────────────────────────────
    let uart = UartDriver::new(
        peripherals.uart1,
        peripherals.pins.gpio17,  // TX
        peripherals.pins.gpio18,  // RX
        None::<esp_idf_hal::gpio::Gpio0>,
        None::<esp_idf_hal::gpio::Gpio0>,
        &UartConfig::new().baudrate(9600_u32.Hz().into()),
    )?;
    // Wrap in our driver
    let uart_periph = peripherals.uart1;
    drop(uart); // We'll use the sensor struct

    let mut co2_sensor = sensors::co2::MhZ19B::new(
        peripherals.uart2,
        peripherals.pins.gpio17,
        peripherals.pins.gpio18,
    )?;

    info!("Sensors initialized. Publishing every {}s", PUBLISH_INTERVAL_MS / 1000);

    // ── Main loop ─────────────────────────────────────────────────────────────
    loop {
        let now = unix_time_seconds();

        // CO2
        if let Some(co2_ppm) = co2_sensor.read_co2() {
            let payload = format!(r#"{{"v":{},"unit":"ppm","ts":{}}}"#, co2_ppm, now);
            let topic = format!("kumaushi/sensors/{}/co2", NODE_ID);
            if let Err(e) = mqtt.publish(&topic, QoS::AtMostOnce, false, payload.as_bytes()) {
                warn!("MQTT publish co2 failed: {:?}", e);
            } else {
                info!("Published CO2 = {} ppm", co2_ppm);
            }
        }

        // Temperature + Humidity
        if let Some((temp, hum)) = sht31.read() {
            let t_payload = format!(r#"{{"v":{:.2},"unit":"°C","ts":{}}}"#, temp, now);
            let h_payload = format!(r#"{{"v":{:.1},"unit":"%RH","ts":{}}}"#, hum, now);
            let t_topic = format!("kumaushi/sensors/{}/temperature", NODE_ID);
            let h_topic = format!("kumaushi/sensors/{}/humidity", NODE_ID);

            if let Err(e) = mqtt.publish(&t_topic, QoS::AtMostOnce, false, t_payload.as_bytes()) {
                warn!("MQTT publish temperature failed: {:?}", e);
            }
            if let Err(e) = mqtt.publish(&h_topic, QoS::AtMostOnce, false, h_payload.as_bytes()) {
                warn!("MQTT publish humidity failed: {:?}", e);
            }
            info!("Published T={:.2}°C H={:.1}%", temp, hum);
        }

        FreeRtos::delay_ms(PUBLISH_INTERVAL_MS);
    }
}

fn unix_time_seconds() -> i64 {
    // esp_idf_svc::systime provides SNTP-synced time
    unsafe { esp_idf_sys::time(std::ptr::null_mut()) as i64 }
}
