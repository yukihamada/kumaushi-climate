/// Multi-zone audio control via GPIO relays + optional I2C digital potentiometer.
///
/// Layout:
///   amp-{zone_id}  → relay: powers zone amplifier (Funktion-One / wall amp)
///   src-{zone_id}  → relay: source select (0 = Line/BT, 1 = DJ matrix)
///
/// Volume is controlled by GPIO PWM on the amp's volume-control input, or by a
/// DS3502 / X9C103 digital potentiometer over I2C (future expansion).
/// For now: volume is tracked in software; real PWM control is wired per zone.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

use kumaushi_common::{AudioSource, AudioZone};
use crate::gpio::GpioController;

pub struct AudioController {
    zones: Arc<RwLock<Vec<AudioZone>>>,
}

impl AudioController {
    pub fn new(gpio: &GpioController) -> Self {
        let zones = vec![
            AudioZone { id: "z1".into(), name: "メインリビング".into(), volume: 0.5, muted: false, source: AudioSource::Off, amp_on: false },
            AudioZone { id: "z2".into(), name: "寝室A".into(),          volume: 0.3, muted: false, source: AudioSource::Off, amp_on: false },
            AudioZone { id: "z3".into(), name: "寝室B".into(),          volume: 0.3, muted: false, source: AudioSource::Off, amp_on: false },
            AudioZone { id: "z4".into(), name: "バス・サウナ".into(),   volume: 0.4, muted: false, source: AudioSource::Off, amp_on: false },
            AudioZone { id: "z5".into(), name: "多目的・ワーク".into(), volume: 0.4, muted: false, source: AudioSource::Off, amp_on: false },
            AudioZone { id: "z6".into(), name: "機械室・廊下".into(),   volume: 0.2, muted: false, source: AudioSource::Off, amp_on: false },
        ];

        // Initialize GPIO: amps off, source = line
        for zone in &zones {
            gpio.set_relay(&format!("amp-{}", zone.id), false);
            gpio.set_relay(&format!("src-{}", zone.id), false);
        }

        Self { zones: Arc::new(RwLock::new(zones)) }
    }

    pub fn all(&self) -> Vec<AudioZone> {
        self.zones.read().unwrap().clone()
    }

    pub fn get(&self, zone_id: &str) -> Option<AudioZone> {
        self.zones.read().unwrap().iter().find(|z| z.id == zone_id).cloned()
    }

    /// Set volume for a zone (0.0–1.0). Turns amp on if volume > 0.
    pub fn set_volume(&self, zone_id: &str, volume: f64, gpio: &GpioController) -> bool {
        let mut zones = self.zones.write().unwrap();
        if let Some(zone) = zones.iter_mut().find(|z| z.id == zone_id) {
            zone.volume = volume.clamp(0.0, 1.0);
            // PWM on volume control input (0V = min, 3.3V = max)
            gpio.set_pwm(&format!("vol-{}", zone_id), if zone.muted { 0.0 } else { zone.volume });
            // Auto power-manage the amp
            let amp_needed = zone.volume > 0.01 && !zone.muted && zone.source != AudioSource::Off;
            if amp_needed != zone.amp_on {
                zone.amp_on = amp_needed;
                gpio.set_relay(&format!("amp-{}", zone_id), amp_needed);
                info!("Audio zone {} amp {}", zone_id, if amp_needed { "ON" } else { "OFF" });
            }
            debug!("Audio zone {} volume={:.0}%", zone_id, zone.volume * 100.0);
            true
        } else {
            false
        }
    }

    /// Set audio source for a zone.
    pub fn set_source(&self, zone_id: &str, source: AudioSource, gpio: &GpioController) -> bool {
        let mut zones = self.zones.write().unwrap();
        if let Some(zone) = zones.iter_mut().find(|z| z.id == zone_id) {
            let is_dj = source == AudioSource::Dj;
            zone.source = source;
            // Relay: HIGH = DJ matrix input, LOW = local line/BT
            gpio.set_relay(&format!("src-{}", zone_id), is_dj);
            // Power amp if source active and volume up
            let amp_needed = zone.volume > 0.01 && !zone.muted && zone.source != AudioSource::Off;
            if amp_needed != zone.amp_on {
                zone.amp_on = amp_needed;
                gpio.set_relay(&format!("amp-{}", zone_id), amp_needed);
            }
            info!("Audio zone {} source={}", zone_id, zone.source);
            true
        } else {
            false
        }
    }

    /// Mute / unmute a zone.
    pub fn set_mute(&self, zone_id: &str, muted: bool, gpio: &GpioController) -> bool {
        let mut zones = self.zones.write().unwrap();
        if let Some(zone) = zones.iter_mut().find(|z| z.id == zone_id) {
            zone.muted = muted;
            gpio.set_pwm(&format!("vol-{}", zone_id), if muted { 0.0 } else { zone.volume });
            debug!("Audio zone {} muted={}", zone_id, muted);
            true
        } else {
            false
        }
    }

    /// DJ broadcast: when a DJ source goes live, auto-switch all active zones to DJ.
    pub fn on_dj_live(&self, live: bool, gpio: &GpioController) {
        let mut zones = self.zones.write().unwrap();
        for zone in zones.iter_mut() {
            if live && zone.amp_on {
                // Switch to DJ source
                zone.source = AudioSource::Dj;
                gpio.set_relay(&format!("src-{}", zone.id), true);
            } else if !live && zone.source == AudioSource::Dj {
                // Fall back to line
                zone.source = AudioSource::Line;
                gpio.set_relay(&format!("src-{}", zone.id), false);
            }
        }
        info!("DJ link {} — auto-switched audio zones", if live { "LIVE" } else { "offline" });
    }

    /// Apply a scene to multiple zones at once.
    pub fn apply_scene(&self, scene: &str, gpio: &GpioController) {
        match scene {
            "party" => {
                // All zones on, DJ source, high volume
                for id in &["z1", "z4", "z5"] {
                    self.set_source(id, AudioSource::Dj, gpio);
                    self.set_volume(id, 0.8, gpio);
                }
                for id in &["z2", "z3"] {
                    self.set_source(id, AudioSource::Dj, gpio);
                    self.set_volume(id, 0.3, gpio);
                }
            }
            "sleep" => {
                for id in &["z1", "z4", "z5", "z6"] {
                    self.set_mute(id, true, gpio);
                }
                for id in &["z2", "z3"] {
                    self.set_source(id, AudioSource::Line, gpio);
                    self.set_volume(id, 0.15, gpio);
                }
            }
            "sauna" => {
                self.set_source("z4", AudioSource::Line, gpio);
                self.set_volume("z4", 0.5, gpio);
            }
            "all_off" => {
                let ids: Vec<String> = self.zones.read().unwrap().iter().map(|z| z.id.clone()).collect();
                for id in &ids {
                    self.set_mute(id, true, gpio);
                    gpio.set_relay(&format!("amp-{}", id), false);
                }
            }
            _ => {}
        }
        info!("Audio scene '{}' applied", scene);
    }
}
