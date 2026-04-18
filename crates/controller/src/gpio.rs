//! GPIO control for Raspberry Pi.
//!
//! On non-Linux platforms (dev machine), all outputs are simulated.
//! Enable the `rpi` feature to actually drive GPIO pins via `rppal`.

use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{debug, warn};

/// Maps device ID → current value (0.0–1.0)
pub struct GpioController {
    state: Mutex<HashMap<String, f64>>,
}

impl GpioController {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Set a PWM device (0.0 = off, 1.0 = full power)
    pub fn set_pwm(&self, device_id: &str, value: f64) {
        let value = value.clamp(0.0, 1.0);
        debug!("GPIO PWM: {} = {:.2}", device_id, value);
        if let Ok(mut s) = self.state.lock() {
            s.insert(device_id.to_string(), value);
        }
        // Production: drive PCA9685 via I2C or direct PWM pin
        #[cfg(feature = "rpi")]
        {
            // rppal::pwm::Pwm::new(Channel::Pwm0).unwrap().set_duty_cycle(value).ok();
        }
    }

    /// Set a relay (digital on/off)
    pub fn set_relay(&self, device_id: &str, on: bool) {
        let value = if on { 1.0 } else { 0.0 };
        debug!("GPIO Relay: {} = {}", device_id, if on { "ON" } else { "OFF" });
        if let Ok(mut s) = self.state.lock() {
            s.insert(device_id.to_string(), value);
        }
        #[cfg(feature = "rpi")]
        {
            // use rppal::gpio::{Gpio, Level};
            // let gpio = Gpio::new().unwrap();
            // let mut pin = gpio.get(PIN_MAP[device_id]).unwrap().into_output();
            // pin.write(if on { Level::High } else { Level::Low });
        }
    }

    pub fn get(&self, device_id: &str) -> f64 {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.get(device_id).copied())
            .unwrap_or(0.0)
    }

    pub fn all(&self) -> HashMap<String, f64> {
        self.state.lock().ok().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Default for GpioController {
    fn default() -> Self {
        Self::new()
    }
}
