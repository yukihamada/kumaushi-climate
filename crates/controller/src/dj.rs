/// Pioneer Pro DJ Link listener.
///
/// Pioneer CDJ/DJM players broadcast status packets on UDP port 50001.
/// This module listens for those packets and extracts:
///   - BPM per deck
///   - Track title / artist (from DB server responses)
///   - Link active status (at least one device on network)
///
/// Protocol reference: https://djl-analysis.deepsymmetry.org/

use std::net::UdpSocket;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use kumaushi_common::DjStatus;

/// Magic bytes that start all Pro DJ Link packets
const PDJL_HEADER: &[u8] = b"Qspt1WmJOL";
const PDJL_PORT: u16 = 50001;

/// Packet type for CDJ beat/status
const PKT_STATUS: u8 = 0x0a;
/// Packet type for BPM (beat phase)
const PKT_BEAT: u8 = 0x28;

pub struct DjMonitor {
    status: Arc<RwLock<DjStatus>>,
}

impl DjMonitor {
    pub fn new() -> Self {
        let initial = DjStatus::default();
        let status = Arc::new(RwLock::new(initial));
        let status_clone = Arc::clone(&status);
        Self::spawn_listener(status_clone);
        Self { status }
    }

    pub fn current(&self) -> DjStatus {
        self.status.read().unwrap().clone()
    }

    /// Spawn a background task that listens for Pro DJ Link UDP packets.
    fn spawn_listener(status: Arc<RwLock<DjStatus>>) {
        std::thread::spawn(move || {
            let socket = match UdpSocket::bind(format!("0.0.0.0:{}", PDJL_PORT)) {
                Ok(s) => s,
                Err(e) => {
                    warn!("DJ Link: cannot bind UDP :{} — {}", PDJL_PORT, e);
                    return;
                }
            };
            socket.set_read_timeout(Some(Duration::from_secs(3))).ok();
            info!("DJ Link: listening on UDP :{}", PDJL_PORT);

            let mut last_seen: [Option<Instant>; 5] = [None; 5]; // up to 4 decks + mixer
            let mut buf = [0u8; 512];

            loop {
                match socket.recv_from(&mut buf) {
                    Ok((n, addr)) => {
                        if n < 32 { continue; }
                        if !buf[..10].starts_with(&PDJL_HEADER[..]) {
                            continue;
                        }
                        let pkt_type = buf[10];
                        let device_num = buf[33] as usize; // 1-based player #

                        if device_num >= 1 && device_num <= 4 {
                            last_seen[device_num - 1] = Some(Instant::now());
                        }

                        match pkt_type {
                            PKT_STATUS => {
                                // CDJ status packet: BPM at bytes 92-95 (fixed-point 100ths)
                                if n >= 96 {
                                    let bpm_raw = u32::from_be_bytes([buf[92], buf[93], buf[94], buf[95]]);
                                    let bpm = bpm_raw as f64 / 100.0;
                                    let mut st = status.write().unwrap();
                                    match device_num {
                                        1 => { st.deck1_bpm = Some(bpm); }
                                        2 => { st.deck2_bpm = Some(bpm); }
                                        _ => {}
                                    }
                                    st.master_bpm = st.deck1_bpm.or(st.deck2_bpm);
                                    st.link_active = true;
                                    st.updated_at = Some(chrono::Utc::now());
                                    debug!("DJ Link: deck{} BPM={:.2} from {}", device_num, bpm, addr);
                                }
                            }
                            PKT_BEAT => {
                                // Beat packet — just confirm link alive
                                let mut st = status.write().unwrap();
                                st.link_active = true;
                                st.updated_at = Some(chrono::Utc::now());
                            }
                            _ => {}
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                        // Timeout: check if any deck was recently seen
                        let any_recent = last_seen.iter().any(|t| {
                            t.map(|t| t.elapsed() < Duration::from_secs(10)).unwrap_or(false)
                        });
                        let mut st = status.write().unwrap();
                        if st.link_active != any_recent {
                            st.link_active = any_recent;
                            if !any_recent {
                                st.deck1_bpm = None;
                                st.deck2_bpm = None;
                                st.master_bpm = None;
                                info!("DJ Link: no devices detected");
                            }
                        }
                    }
                    Err(e) => {
                        warn!("DJ Link socket error: {}", e);
                        std::thread::sleep(Duration::from_secs(5));
                    }
                }
            }
        });
    }
}

impl Default for DjMonitor {
    fn default() -> Self { Self::new() }
}
