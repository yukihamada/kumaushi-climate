//! MH-Z19B CO2 sensor driver via UART.
//!
//! Communication: 9600 baud, 8N1.
//! Read command: 0xFF 0x01 0x86 0x00 × 6 0x79
//! Response:     0xFF 0x86 HIGH LOW ...

use esp_idf_hal::uart::{config::Config, Uart, UartDriver};
use esp_idf_hal::gpio::{InputPin, OutputPin};
use esp_idf_hal::peripheral::Peripheral;
use log::{debug, error, warn};

/// MH-Z19B read command
const CMD_READ: [u8; 9] = [0xFF, 0x01, 0x86, 0x00, 0x00, 0x00, 0x00, 0x00, 0x79];

pub struct MhZ19B<'d> {
    uart: UartDriver<'d>,
}

impl<'d> MhZ19B<'d> {
    pub fn new<UART: Uart>(
        uart: impl Peripheral<P = UART> + 'd,
        tx: impl Peripheral<P = impl OutputPin> + 'd,
        rx: impl Peripheral<P = impl InputPin> + 'd,
    ) -> anyhow::Result<Self> {
        let config = Config::new().baudrate(esp_idf_hal::units::Hertz(9600));
        let driver = UartDriver::new(uart, tx, rx, None::<esp_idf_hal::gpio::Gpio0>,
                                    None::<esp_idf_hal::gpio::Gpio0>, &config)?;
        Ok(Self { uart: driver })
    }

    /// Read CO2 concentration in ppm. Returns None on checksum or timeout error.
    pub fn read_co2(&mut self) -> Option<u16> {
        // Send read command
        if self.uart.write(&CMD_READ).is_err() {
            error!("MH-Z19B: UART write failed");
            return None;
        }

        // Wait for response (9 bytes)
        let mut buf = [0u8; 9];
        match self.uart.read(&mut buf, 1000 / esp_idf_hal::delay::TickType::new_millis(1).0) {
            Ok(n) if n == 9 => {}
            Ok(n) => {
                warn!("MH-Z19B: short read {}/9 bytes", n);
                return None;
            }
            Err(e) => {
                error!("MH-Z19B: UART read error: {:?}", e);
                return None;
            }
        }

        // Validate start byte and command echo
        if buf[0] != 0xFF || buf[1] != 0x86 {
            warn!("MH-Z19B: unexpected response header {:02X} {:02X}", buf[0], buf[1]);
            return None;
        }

        // Checksum: 0xFF - sum(buf[1..8]) + 1
        let checksum = (0xFFu16 - buf[1..8].iter().map(|&b| b as u16).sum::<u16>() + 1) as u8;
        if checksum != buf[8] {
            warn!("MH-Z19B: checksum mismatch {:02X} != {:02X}", checksum, buf[8]);
            return None;
        }

        let ppm = (buf[2] as u16) << 8 | buf[3] as u16;
        debug!("MH-Z19B: CO2 = {} ppm", ppm);
        Some(ppm)
    }
}
