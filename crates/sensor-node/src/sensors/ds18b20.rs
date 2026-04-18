//! DS18B20 water temperature sensor driver via 1-Wire (emulated over GPIO).
//!
//! The ESP-IDF RMT (Remote Control) peripheral is used to bit-bang 1-Wire timing.
//! This driver uses a simpler software bit-bang via GPIO with μs delays,
//! which is sufficient at 30-second measurement intervals.

use esp_idf_hal::gpio::{Gpio, IOPin, InputOutput, PinDriver};
use esp_idf_hal::delay::Ets;
use log::{debug, error, warn};

const SKIP_ROM: u8 = 0xCC;
const CONVERT_T: u8 = 0x44;
const READ_SCRATCHPAD: u8 = 0xBE;
const RESET_PRESENCE_WAIT_US: u32 = 480;
const PRESENCE_DETECT_US: u32 = 70;

pub struct Ds18b20<'d> {
    pin: PinDriver<'d, esp_idf_hal::gpio::AnyIOPin, InputOutput>,
}

impl<'d> Ds18b20<'d> {
    pub fn new(pin: impl IOPin + 'd) -> anyhow::Result<Self> {
        let pin = PinDriver::input_output_od(pin.into())?;
        Ok(Self { pin })
    }

    /// Read temperature in °C. Returns None on error.
    pub fn read_temp(&mut self) -> Option<f32> {
        // 1. Reset + presence detect
        if !self.reset() {
            warn!("DS18B20: no presence pulse");
            return None;
        }

        // 2. Skip ROM (single device on bus)
        self.write_byte(SKIP_ROM);

        // 3. Start temperature conversion
        self.write_byte(CONVERT_T);

        // 4. Wait for conversion (~750ms for 12-bit resolution)
        Ets::delay_ms(800);

        // 5. Reset again
        if !self.reset() {
            warn!("DS18B20: no presence after convert");
            return None;
        }

        // 6. Skip ROM
        self.write_byte(SKIP_ROM);

        // 7. Read scratchpad
        self.write_byte(READ_SCRATCHPAD);

        // Read 9 bytes (we only need bytes 0-1 for temperature)
        let mut buf = [0u8; 9];
        for b in buf.iter_mut() {
            *b = self.read_byte();
        }

        // CRC-8 check (Dallas/Maxim polynomial 0x31)
        if !crc8_valid(&buf) {
            error!("DS18B20: CRC error");
            return None;
        }

        // Temperature is 16-bit two's complement, LSB first
        let raw = (buf[1] as i16) << 8 | buf[0] as i16;
        let temp = raw as f32 / 16.0;
        debug!("DS18B20: {:.2}°C (raw {})", temp, raw);
        Some(temp)
    }

    fn reset(&mut self) -> bool {
        // Pull low for 480μs
        self.pin.set_low().ok();
        Ets::delay_us(RESET_PRESENCE_WAIT_US);
        // Release
        self.pin.set_high().ok();
        // Wait for presence pulse (60-240μs after release)
        Ets::delay_us(PRESENCE_DETECT_US);
        let present = self.pin.is_low();
        Ets::delay_us(RESET_PRESENCE_WAIT_US - PRESENCE_DETECT_US);
        present
    }

    fn write_bit(&mut self, bit: bool) {
        self.pin.set_low().ok();
        if bit {
            Ets::delay_us(1);
            self.pin.set_high().ok();
            Ets::delay_us(60);
        } else {
            Ets::delay_us(60);
            self.pin.set_high().ok();
            Ets::delay_us(1);
        }
    }

    fn read_bit(&mut self) -> bool {
        self.pin.set_low().ok();
        Ets::delay_us(1);
        self.pin.set_high().ok();
        Ets::delay_us(14);
        let bit = self.pin.is_high();
        Ets::delay_us(45);
        bit
    }

    fn write_byte(&mut self, byte: u8) {
        for i in 0..8 {
            self.write_bit((byte >> i) & 1 == 1);
        }
    }

    fn read_byte(&mut self) -> u8 {
        let mut byte = 0u8;
        for i in 0..8 {
            if self.read_bit() {
                byte |= 1 << i;
            }
        }
        byte
    }
}

fn crc8_valid(data: &[u8; 9]) -> bool {
    let mut crc: u8 = 0;
    for &b in &data[..8] {
        let mut byte = b;
        for _ in 0..8 {
            if (crc ^ byte) & 0x01 != 0 {
                crc = (crc >> 1) ^ 0x8C;
            } else {
                crc >>= 1;
            }
            byte >>= 1;
        }
    }
    crc == data[8]
}
