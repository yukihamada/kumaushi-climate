use chrono::{DateTime, Utc};
use kumaushi_common::SensorReading;
use rusqlite::{Connection, params};
use std::sync::Mutex;
use tracing::{error, info};

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub async fn open(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Self::init_schema(&conn)?;
        info!("Database opened: {}", path);
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn init_schema(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS sensor_readings (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                node_id     TEXT    NOT NULL,
                sensor_type TEXT    NOT NULL,
                value       REAL    NOT NULL,
                unit        TEXT    NOT NULL,
                recorded_at TEXT    NOT NULL  -- ISO8601
            );

            CREATE INDEX IF NOT EXISTS idx_sensor_node_type_time
                ON sensor_readings (node_id, sensor_type, recorded_at DESC);

            CREATE TABLE IF NOT EXISTS control_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                device_id   TEXT    NOT NULL,
                device_type TEXT    NOT NULL,
                value       REAL    NOT NULL,
                reason      TEXT,
                logged_at   TEXT    NOT NULL
            );

            CREATE TABLE IF NOT EXISTS zone_setpoints (
                zone_id     TEXT PRIMARY KEY,
                temperature REAL NOT NULL DEFAULT 22.0,
                co2_max     REAL NOT NULL DEFAULT 800.0,
                humidity    REAL NOT NULL DEFAULT 50.0,
                mode        TEXT NOT NULL DEFAULT 'auto',
                updated_at  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS alerts (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                zone_id     TEXT    NOT NULL,
                level       TEXT    NOT NULL,
                message     TEXT    NOT NULL,
                resolved    INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT    NOT NULL
            );
        ")?;
        Ok(())
    }

    pub fn insert_reading(&self, reading: &SensorReading) -> anyhow::Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        conn.execute(
            "INSERT INTO sensor_readings (node_id, sensor_type, value, unit, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                reading.node_id,
                reading.sensor_type.as_str(),
                reading.value,
                reading.unit,
                reading.timestamp.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn latest_readings(
        &self,
        zone_node_ids: &[&str],
    ) -> anyhow::Result<Vec<SensorReading>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let placeholders: String = zone_node_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "SELECT node_id, sensor_type, value, unit, recorded_at
             FROM sensor_readings
             WHERE node_id IN ({placeholders})
             GROUP BY node_id, sensor_type
             HAVING recorded_at = MAX(recorded_at)"
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut readings = Vec::new();

        // Build params dynamically
        use rusqlite::types::ToSql;
        let params: Vec<Box<dyn ToSql>> = zone_node_ids
            .iter()
            .map(|id| Box::new(id.to_string()) as Box<dyn ToSql>)
            .collect();
        let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query(rusqlite::params_from_iter(param_refs))?;
        let mut rows = rows;
        while let Some(row) = rows.next()? {
            let sensor_type_str: String = row.get(1)?;
            let sensor_type = match sensor_type_str.as_str() {
                "co2" => kumaushi_common::SensorType::Co2,
                "humidity" => kumaushi_common::SensorType::Humidity,
                "water_temp" => kumaushi_common::SensorType::WaterTemp,
                "pressure" => kumaushi_common::SensorType::Pressure,
                _ => kumaushi_common::SensorType::Temperature,
            };
            let ts_str: String = row.get(4)?;
            let timestamp = ts_str.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now());
            readings.push(SensorReading {
                node_id: row.get(0)?,
                sensor_type,
                value: row.get(2)?,
                unit: row.get(3)?,
                timestamp,
            });
        }
        Ok(readings)
    }

    pub fn history(
        &self,
        node_id: &str,
        sensor_type: &str,
        hours: u32,
    ) -> anyhow::Result<Vec<(DateTime<Utc>, f64)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let cutoff = Utc::now() - chrono::Duration::hours(hours as i64);
        let mut stmt = conn.prepare(
            "SELECT recorded_at, value FROM sensor_readings
             WHERE node_id = ?1 AND sensor_type = ?2 AND recorded_at >= ?3
             ORDER BY recorded_at ASC",
        )?;
        let rows = stmt.query_map(
            params![node_id, sensor_type, cutoff.to_rfc3339()],
            |row| {
                let ts_str: String = row.get(0)?;
                let value: f64 = row.get(1)?;
                Ok((ts_str, value))
            },
        )?;
        let mut result = Vec::new();
        for row in rows {
            let (ts_str, value) = row?;
            if let Ok(ts) = ts_str.parse::<DateTime<Utc>>() {
                result.push((ts, value));
            }
        }
        Ok(result)
    }

    pub fn log_control(
        &self,
        device_id: &str,
        device_type: &str,
        value: f64,
        reason: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        conn.execute(
            "INSERT INTO control_log (device_id, device_type, value, reason, logged_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![device_id, device_type, value, reason, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn insert_alert(
        &self,
        zone_id: &str,
        level: &str,
        message: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        conn.execute(
            "INSERT INTO alerts (zone_id, level, message, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![zone_id, level, message, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}
