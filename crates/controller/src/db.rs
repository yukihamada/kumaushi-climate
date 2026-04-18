use chrono::{DateTime, Utc};
use kumaushi_common::{Alert, AlertLevel, Schedule, Setpoints, SensorReading, SensorType};
use rusqlite::{Connection, params};
use std::sync::Mutex;
use tracing::info;

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
                recorded_at TEXT    NOT NULL
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

            CREATE TABLE IF NOT EXISTS alerts (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                zone_id     TEXT    NOT NULL,
                level       TEXT    NOT NULL,
                message     TEXT    NOT NULL,
                resolved    INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT    NOT NULL
            );

            CREATE TABLE IF NOT EXISTS schedules (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                zone_id     TEXT    NOT NULL,
                weekdays    TEXT    NOT NULL DEFAULT '',  -- comma-separated e.g. '1,2,3'
                time_from   TEXT    NOT NULL,             -- 'HH:MM'
                time_until  TEXT    NOT NULL,
                temperature REAL    NOT NULL DEFAULT 22.0,
                co2_max     REAL    NOT NULL DEFAULT 800.0,
                humidity    REAL    NOT NULL DEFAULT 50.0,
                enabled     INTEGER NOT NULL DEFAULT 1,
                created_at  TEXT    NOT NULL
            );
        ")?;
        Ok(())
    }

    // ── Sensor readings ────────────────────────────────────────────────────

    pub fn insert_reading(&self, reading: &SensorReading) -> anyhow::Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        conn.execute(
            "INSERT INTO sensor_readings (node_id, sensor_type, value, unit, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![reading.node_id, reading.sensor_type.as_str(), reading.value,
                    reading.unit, reading.timestamp.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn latest_readings(&self, zone_node_ids: &[&str]) -> anyhow::Result<Vec<SensorReading>> {
        if zone_node_ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let placeholders: String = (1..=zone_node_ids.len())
            .map(|i| format!("?{}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT node_id, sensor_type, value, unit, recorded_at
             FROM sensor_readings WHERE node_id IN ({placeholders})
             GROUP BY node_id, sensor_type HAVING recorded_at = MAX(recorded_at)"
        );
        let mut stmt = conn.prepare(&sql)?;
        use rusqlite::types::ToSql;
        let params: Vec<Box<dyn ToSql>> = zone_node_ids
            .iter().map(|id| Box::new(id.to_string()) as Box<dyn ToSql>).collect();
        let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let mut rows = stmt.query(rusqlite::params_from_iter(param_refs))?;
        let mut readings = Vec::new();
        while let Some(row) = rows.next()? {
            let st: String = row.get(1)?;
            let sensor_type = parse_sensor_type(&st);
            let ts: String = row.get(4)?;
            readings.push(SensorReading {
                node_id: row.get(0)?,
                sensor_type,
                value: row.get(2)?,
                unit: row.get(3)?,
                timestamp: ts.parse().unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(readings)
    }

    pub fn history(&self, node_id: &str, sensor_type: &str, hours: u32) -> anyhow::Result<Vec<(DateTime<Utc>, f64)>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let cutoff = Utc::now() - chrono::Duration::hours(hours as i64);
        let mut stmt = conn.prepare(
            "SELECT recorded_at, value FROM sensor_readings
             WHERE node_id=?1 AND sensor_type=?2 AND recorded_at>=?3
             ORDER BY recorded_at ASC"
        )?;
        let rows = stmt.query_map(
            params![node_id, sensor_type, cutoff.to_rfc3339()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )?;
        let mut result = Vec::new();
        for row in rows {
            let (ts, v) = row?;
            if let Ok(dt) = ts.parse::<DateTime<Utc>>() {
                result.push((dt, v));
            }
        }
        Ok(result)
    }

    // ── Control log ────────────────────────────────────────────────────────

    pub fn log_control(&self, device_id: &str, device_type: &str, value: f64, reason: Option<&str>) -> anyhow::Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        conn.execute(
            "INSERT INTO control_log (device_id, device_type, value, reason, logged_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![device_id, device_type, value, reason, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // ── Alerts ─────────────────────────────────────────────────────────────

    pub fn insert_alert(&self, zone_id: &str, level: &str, message: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        conn.execute(
            "INSERT INTO alerts (zone_id, level, message, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![zone_id, level, message, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_alerts(&self, limit: usize, include_resolved: bool) -> anyhow::Result<Vec<Alert>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let sql = if include_resolved {
            "SELECT id, zone_id, level, message, resolved, created_at
             FROM alerts ORDER BY created_at DESC LIMIT ?1"
        } else {
            "SELECT id, zone_id, level, message, resolved, created_at
             FROM alerts WHERE resolved=0 ORDER BY created_at DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut alerts = Vec::new();
        for row in rows {
            let (id, zone_id, level_str, message, resolved, ts) = row?;
            let level = match level_str.as_str() {
                "critical" => AlertLevel::Critical,
                "warning" => AlertLevel::Warning,
                _ => AlertLevel::Info,
            };
            alerts.push(Alert {
                id,
                zone_id,
                level,
                message,
                resolved: resolved != 0,
                timestamp: ts.parse().unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(alerts)
    }

    pub fn resolve_alert(&self, id: i64) -> anyhow::Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        conn.execute("UPDATE alerts SET resolved=1 WHERE id=?1", params![id])?;
        Ok(())
    }

    // ── Schedules ──────────────────────────────────────────────────────────

    pub fn get_schedules(&self, zone_id: Option<&str>) -> anyhow::Result<Vec<Schedule>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let sql = match zone_id {
            Some(_) => "SELECT id, zone_id, weekdays, time_from, time_until, temperature, co2_max, humidity, enabled
                        FROM schedules WHERE zone_id=?1 ORDER BY time_from",
            None => "SELECT id, zone_id, weekdays, time_from, time_until, temperature, co2_max, humidity, enabled
                     FROM schedules ORDER BY zone_id, time_from",
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(zid) = zone_id {
            stmt.query_map(params![zid], parse_schedule_row)?
        } else {
            stmt.query_map(params![], parse_schedule_row)?
        };
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn create_schedule(&self, s: &Schedule) -> anyhow::Result<i64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let weekdays = s.weekdays.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(",");
        conn.execute(
            "INSERT INTO schedules (zone_id, weekdays, time_from, time_until, temperature, co2_max, humidity, enabled, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![s.zone_id, weekdays, s.time_from, s.time_until,
                    s.setpoints.temperature, s.setpoints.co2_max, s.setpoints.humidity,
                    s.enabled as i64, Utc::now().to_rfc3339()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_schedule(&self, id: i64) -> anyhow::Result<bool> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let rows = conn.execute("DELETE FROM schedules WHERE id=?1", params![id])?;
        Ok(rows > 0)
    }

    pub fn get_active_schedules(&self) -> anyhow::Result<Vec<Schedule>> {
        let all = self.get_schedules(None)?;
        Ok(all.into_iter().filter(|s| s.enabled && s.is_active_now()).collect())
    }
}

fn parse_sensor_type(s: &str) -> SensorType {
    match s {
        "co2" => SensorType::Co2,
        "humidity" => SensorType::Humidity,
        "water_temp" => SensorType::WaterTemp,
        "pressure" => SensorType::Pressure,
        _ => SensorType::Temperature,
    }
}

fn parse_schedule_row(row: &rusqlite::Row) -> rusqlite::Result<Schedule> {
    let id: i64 = row.get(0)?;
    let zone_id: String = row.get(1)?;
    let weekdays_str: String = row.get(2)?;
    let weekdays: Vec<u8> = weekdays_str.split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    Ok(Schedule {
        id,
        zone_id,
        weekdays,
        time_from: row.get(3)?,
        time_until: row.get(4)?,
        setpoints: Setpoints {
            temperature: row.get(5)?,
            co2_max: row.get(6)?,
            humidity: row.get(7)?,
        },
        enabled: row.get::<_, i64>(8)? != 0,
    })
}
