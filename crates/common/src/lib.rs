use chrono::{DateTime, Datelike, NaiveTime, Utc, Weekday};
use serde::{Deserialize, Serialize};

/// Sensor reading published via MQTT
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub node_id: String,
    pub sensor_type: SensorType,
    pub value: f64,
    pub unit: String,
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
    pub temperature: f64,
    pub co2_max: f64,
    pub humidity: f64,
    /// If Some, cooling (AC) kicks in above this temperature (°C)
    #[serde(default)]
    pub cooling_threshold: Option<f64>,
}

impl Default for Setpoints {
    fn default() -> Self {
        Self { temperature: 22.0, co2_max: 800.0, humidity: 50.0, cooling_threshold: Some(26.0) }
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
    /// true if any sensor for this zone is stale (>90s)
    pub sensor_stale: bool,
}

/// Control output command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCommand {
    pub device_id: String,
    pub device_type: DeviceType,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    Fan,
    Damper,
    HeatingRelay,
    /// Wall-mount AC unit (cooling mode)
    CoolingRelay,
    SaunaRelay,
    PumpRelay,
    Dehumidifier,
}

/// Weekly schedule entry: override setpoints during a time window on given weekdays
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: i64,
    pub zone_id: String,
    /// ISO weekdays 1=Mon … 7=Sun, empty = every day
    pub weekdays: Vec<u8>,
    /// "HH:MM" local time (UTC+9 for Hokkaido)
    pub time_from: String,
    pub time_until: String,
    pub setpoints: Setpoints,
    pub enabled: bool,
}

impl Schedule {
    /// Returns true if the schedule is active right now (UTC time, +9 offset applied internally)
    pub fn is_active_now(&self) -> bool {
        use chrono::TimeZone;
        let jst_offset = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
        let now_jst = jst_offset.from_utc_datetime(&Utc::now().naive_utc());
        let weekday_num = match now_jst.weekday() {
            Weekday::Mon => 1u8,
            Weekday::Tue => 2,
            Weekday::Wed => 3,
            Weekday::Thu => 4,
            Weekday::Fri => 5,
            Weekday::Sat => 6,
            Weekday::Sun => 7,
        };
        if !self.weekdays.is_empty() && !self.weekdays.contains(&weekday_num) {
            return false;
        }
        let now_time = now_jst.time();
        let from = NaiveTime::parse_from_str(&self.time_from, "%H:%M").unwrap_or(NaiveTime::MIN);
        let until = NaiveTime::parse_from_str(&self.time_until, "%H:%M")
            .unwrap_or(NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        now_time >= from && now_time < until
    }
}

/// Dashboard snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub zones: Vec<Zone>,
    pub alerts: Vec<Alert>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: i64,
    pub zone_id: String,
    pub level: AlertLevel,
    pub message: String,
    pub resolved: bool,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

/// PID controller state
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
        Self { kp: 0.05, ki: 0.001, kd: 0.01, integral: 0.0, prev_error: 0.0, output_min: 0.0, output_max: 1.0 }
    }

    pub fn temperature() -> Self {
        Self { kp: 2.0, ki: 0.1, kd: 0.5, integral: 0.0, prev_error: 0.0, output_min: 0.0, output_max: 1.0 }
    }

    pub fn compute(&mut self, setpoint: f64, measured: f64, dt: f64) -> f64 {
        let error = setpoint - measured;
        self.integral += error * dt;
        let max_integral = self.output_max / self.ki.max(1e-9);
        self.integral = self.integral.clamp(-max_integral, max_integral);
        let derivative = if dt > 0.0 { (error - self.prev_error) / dt } else { 0.0 };
        self.prev_error = error;
        (self.kp * error + self.ki * self.integral + self.kd * derivative)
            .clamp(self.output_min, self.output_max)
    }

    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
    }
}
