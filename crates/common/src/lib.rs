use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Sensor reading published via MQTT
/// Topic: `kumaushi/sensors/{node_id}/{sensor_type}`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    /// Sensor node ID (e.g. "node-z1-a")
    pub node_id: String,
    /// Sensor type: "co2" | "temperature" | "humidity" | "water_temp"
    pub sensor_type: SensorType,
    /// Raw value
    pub value: f64,
    /// SI unit string
    pub unit: String,
    /// UTC timestamp
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SensorType {
    Co2,
    Temperature,
    Humidity,
    WaterTemp,
    Pressure,
}

impl SensorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Co2 => "co2",
            Self::Temperature => "temperature",
            Self::Humidity => "humidity",
            Self::WaterTemp => "water_temp",
            Self::Pressure => "pressure",
        }
    }

    pub fn unit(&self) -> &'static str {
        match self {
            Self::Co2 => "ppm",
            Self::Temperature => "°C",
            Self::Humidity => "%RH",
            Self::WaterTemp => "°C",
            Self::Pressure => "hPa",
        }
    }
}

/// HVAC zone (maps to one or more containers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    pub containers: Vec<u8>,
    pub mode: ZoneMode,
    pub setpoints: Setpoints,
    pub current: ZoneState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ZoneMode {
    Auto,
    Manual,
    Off,
}

/// Control setpoints for a zone
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setpoints {
    /// Target temperature °C
    pub temperature: f64,
    /// CO2 upper limit ppm (ventilation kicks in above this)
    pub co2_max: f64,
    /// Target humidity %RH
    pub humidity: f64,
}

impl Default for Setpoints {
    fn default() -> Self {
        Self {
            temperature: 22.0,
            co2_max: 800.0,
            humidity: 50.0,
        }
    }
}

/// Current measured state of a zone
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneState {
    pub temperature: Option<f64>,
    pub humidity: Option<f64>,
    pub co2_ppm: Option<f64>,
    pub water_temp: Option<f64>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Control output command (sent to GPIO)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCommand {
    pub device_id: String,
    pub device_type: DeviceType,
    /// 0.0–1.0 for PWM, true/false mapped to 1.0/0.0 for relay
    pub value: f64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Fan,
    Damper,
    HeatingRelay,
    SaunaRelay,
    PumpRelay,
    Dehumidifier,
}

/// Dashboard snapshot returned by GET /api/v1/dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub zones: Vec<Zone>,
    pub alerts: Vec<Alert>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub zone_id: String,
    pub level: AlertLevel,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

/// PID controller state (serializable for persistence)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidState {
    pub kp: f64,
    pub ki: f64,
    pub kd: f64,
    pub integral: f64,
    pub prev_error: f64,
    pub output_min: f64,
    pub output_max: f64,
}

impl PidState {
    pub fn ventilation() -> Self {
        Self {
            kp: 0.05,
            ki: 0.001,
            kd: 0.01,
            integral: 0.0,
            prev_error: 0.0,
            output_min: 0.0,
            output_max: 1.0,
        }
    }

    pub fn temperature() -> Self {
        Self {
            kp: 2.0,
            ki: 0.1,
            kd: 0.5,
            integral: 0.0,
            prev_error: 0.0,
            output_min: 0.0,
            output_max: 1.0,
        }
    }

    /// Compute next output given setpoint, measured value, dt (seconds)
    pub fn compute(&mut self, setpoint: f64, measured: f64, dt: f64) -> f64 {
        let error = setpoint - measured;
        self.integral += error * dt;
        // Anti-windup: clamp integral
        let max_integral = self.output_max / self.ki.max(1e-9);
        self.integral = self.integral.clamp(-max_integral, max_integral);
        let derivative = if dt > 0.0 {
            (error - self.prev_error) / dt
        } else {
            0.0
        };
        self.prev_error = error;
        let output = self.kp * error + self.ki * self.integral + self.kd * derivative;
        output.clamp(self.output_min, self.output_max)
    }

    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
    }
}
