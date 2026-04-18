//! SHT31 Temperature & Humidity sensor driver via I2C.
//!
//! Address: 0x44 (ADDR pin low) or 0x45 (ADDR pin high).
//! Single-shot high-repeatability measurement: cmd 0x2400.

use esp_idf_hal::i2c::I2cDriver;
use log::{debug, error};

const SHT31_ADDR: u8 = 0x44;
const CMD_SINGLE_HIGH: [u8; 2] = [0x24, 0x00];

pub struct Sht31<'d> {
    i2c: I2cDriver<'d>,
    addr: u8,
}

impl<'d> Sht31<'d> {
    pub fn new(i2c: I2cDriver<'d>) -> Self {
        Self { i2c, addr: SHT31_ADDR }
    }

    pub fn with_addr(i2c: I2cDriver<'d>, addr: u8) -> Self {
        Self { i2c, addr }
    }

    /// Read temperature (°C) and relative humidity (%).
    /// Returns None on CRC or I2C error.
    pub fn read(&mut self) -> Option<(f32, f32)> {
        // Send measurement command
        if self.i2c.write(self.addr, &CMD_SINGLE_HIGH, 50).is_err() {
            error!("SHT31: I2C write cmd failed");
            return None;
        }

        // Wait for measurement (~15ms for high repeatability)
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Read 6 bytes: T_MSB T_LSB T_CRC H_MSB H_LSB H_CRC
        let mut buf = [0u8; 6];
        if self.i2c.read(self.addr, &mut buf, 50).is_err() {
            error!("SHT31: I2C read failed");
            return None;
        }

        // CRC-8 check (polynomial 0x31, init 0xFF)
        if !crc8_check(&buf[0..2], buf[2]) || !crc8_check(&buf[3..5], buf[5]) {
            error!("SHT31: CRC mismatch");
            return None;
        }

        let raw_t = (buf[0] as u16) << 8 | buf[1] as u16;
        let raw_h = (buf[3] as u16) << 8 | buf[4] as u16;

        let temp = -45.0 + 175.0 * raw_t as f32 / 65535.0;
        let hum = 100.0 * raw_h as f32 / 65535.0;

        debug!("SHT31: T={:.2}°C H={:.1}%", temp, hum);
        Some((temp, hum))
    }
}

fn crc8_check(data: &[u8], expected: u8) -> bool {
    let mut crc: u8 = 0xFF;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x31;
            } else {
                crc <<= 1;
            }
        }
    }
    crc == expected
}
