//! Real-time audio level metering and waveform history.
//!
//! All structures here use atomic operations and lock-free ring buffers so
//! they are safe to update from inside the real-time audio callback without
//! any allocation or blocking. The TUI thread reads snapshots periodically.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Number of samples to retain for the waveform scroll-back display.
const WAVEFORM_LEN: usize = 120;

/// Number of logarithmic frequency bands for the spectrum display.
pub const NUM_BANDS: usize = 16;

/// Centre frequencies (Hz) for each band — covering ~60 Hz to ~16 kHz.
pub const BAND_FREQS: [f32; NUM_BANDS] = [
    60.0, 80.0, 120.0, 170.0, 250.0, 350.0, 500.0, 700.0, 1000.0, 1400.0, 2000.0, 3000.0, 4000.0,
    6000.0, 9000.0, 13000.0,
];

/// A single channel meter: current peak/RMS levels + waveform history.
///
/// Updated from the audio callback via [`ChannelMeter::update`], read from
/// the TUI via [`ChannelMeter::snapshot`].
pub struct ChannelMeter {
    /// Latest RMS level (linear, 0.0–1.0), stored as AtomicUsize (fixed-point).
    rms: AtomicUsize,
    /// Latest peak level (linear, 0.0–1.0), stored as AtomicUsize (fixed-point).
    peak: AtomicUsize,
    /// Running max peak for clip/peak-hold display.
    peak_hold: AtomicUsize,
    /// Waveform history buffer — fixed-size ring of quantised samples.
    waveform: Vec<AtomicUsize>,
    /// Write cursor for the waveform ring.
    write_idx: AtomicUsize,
    /// Frequency band magnitudes [0.0–1.0] as fixed-point, updated via Goertzel.
    bands: Vec<AtomicUsize>,
    /// Sample rate of the stream feeding this meter (needed for Goertzel freq→coef).
    sample_rate: AtomicUsize,
    /// Whether this channel has clipped (|sample| > 1.0) since last reset.
    clipped: AtomicBool,
}

/// Fixed-point scale: store levels as `u32` with SCALE representing 1.0.
const SCALE: usize = 1_000_000;

impl ChannelMeter {
    pub fn new() -> Self {
        let waveform = (0..WAVEFORM_LEN).map(|_| AtomicUsize::new(0)).collect();
        let bands = (0..NUM_BANDS).map(|_| AtomicUsize::new(0)).collect();
        Self {
            rms: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            peak_hold: AtomicUsize::new(0),
            waveform,
            write_idx: AtomicUsize::new(0),
            bands,
            sample_rate: AtomicUsize::new(48000),
            clipped: AtomicBool::new(false),
        }
    }

    /// Set the sample rate (called when the audio stream is configured).
    pub fn set_sample_rate(&self, sr: u32) {
        self.sample_rate.store(sr as usize, Ordering::Relaxed);
    }

    /// Push a buffer of interleaved samples for this channel into the meter.
    ///
    /// Called from the real-time audio callback. Computes RMS and peak over
    /// the buffer, writes a downsampled waveform segment, and updates
    /// peak-hold / clip flags. No allocation, no locks.
    pub fn update(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }

        let mut sum_sq = 0.0f64;
        let mut max_abs = 0.0f32;
        for &s in samples {
            let abs = s.abs();
            sum_sq += (s as f64) * (s as f64);
            if abs > max_abs {
                max_abs = abs;
            }
        }

        let rms_lin = (sum_sq / samples.len() as f64).sqrt() as f32;
        let rms_fixed = lin_to_fixed(rms_lin);
        let peak_fixed = lin_to_fixed(max_abs);

        self.rms.store(rms_fixed, Ordering::Relaxed);
        self.peak.store(peak_fixed, Ordering::Relaxed);

        // Peak-hold with slow decay handled in snapshot side; here we just
        // track the running max.
        let prev_hold = self.peak_hold.load(Ordering::Relaxed);
        if peak_fixed > prev_hold {
            self.peak_hold.store(peak_fixed, Ordering::Relaxed);
        }

        if max_abs > 1.0 {
            self.clipped.store(true, Ordering::Relaxed);
        }

        // Downsample into waveform ring: we want at most a few points per
        // update to keep the display smooth without flooding.
        let n_points = samples.len().clamp(1, 8);
        let chunk = samples.len() / n_points;
        if chunk == 0 {
            return;
        }

        for i in 0..n_points {
            let start = i * chunk;
            let end = ((i + 1) * chunk).min(samples.len());
            if start >= end {
                break;
            }
            let chunk_max = samples[start..end]
                .iter()
                .map(|s| s.abs())
                .fold(0.0f32, f32::max);
            let val = lin_to_fixed(chunk_max);
            let idx = self.write_idx.fetch_add(1, Ordering::Relaxed) % WAVEFORM_LEN;
            self.waveform[idx].store(val, Ordering::Relaxed);
        }

        // Goertzel band analysis — compute magnitude for each frequency band.
        // This runs entirely on the stack with no allocation.
        let sr = self.sample_rate.load(Ordering::Relaxed) as f32;
        if sr > 0.0 && samples.len() >= 8 {
            for (bi, &freq) in BAND_FREQS.iter().enumerate() {
                // Only analyse if the frequency is representable (< Nyquist).
                if freq >= sr * 0.5 {
                    continue;
                }
                let k = freq / sr * samples.len() as f32;
                let w = 2.0 * std::f32::consts::PI * k / samples.len() as f32;
                let coeff = 2.0 * w.cos();
                let mut s_prev = 0.0f32;
                let mut s_prev2 = 0.0f32;
                for &s in samples {
                    let s_cur = s + coeff * s_prev - s_prev2;
                    s_prev2 = s_prev;
                    s_prev = s_cur;
                }
                let mag = ((s_prev * s_prev + s_prev2 * s_prev2 - coeff * s_prev * s_prev2).abs())
                    / (samples.len() as f32);
                // Convert to dBFS, then map -60 dB … 0 dB → 0.0 … 1.0.
                let db = if mag > 1e-10 {
                    10.0 * mag.log10()
                } else {
                    -60.0
                };
                let norm = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                self.bands[bi].store(lin_to_fixed(norm), Ordering::Relaxed);
            }
        }
    }

    /// Read a consistent snapshot of all meter values for TUI display.
    pub fn snapshot(&self) -> MeterSnapshot {
        let rms = fixed_to_lin(self.rms.load(Ordering::Relaxed));
        let peak = fixed_to_lin(self.peak.load(Ordering::Relaxed));
        let peak_hold = fixed_to_lin(self.peak_hold.load(Ordering::Relaxed));
        let clipped = self.clipped.load(Ordering::Relaxed);

        // Read waveform ring in chronological order.
        let write_pos = self.write_idx.load(Ordering::Relaxed);
        let mut waveform = Vec::with_capacity(WAVEFORM_LEN);
        // The oldest sample is at (write_pos % LEN) if the buffer is full,
        // otherwise at 0.
        let filled = write_pos.min(WAVEFORM_LEN);
        let start = if write_pos > WAVEFORM_LEN {
            write_pos % WAVEFORM_LEN
        } else {
            0
        };
        for i in 0..filled {
            let idx = (start + i) % WAVEFORM_LEN;
            waveform.push(fixed_to_lin(self.waveform[idx].load(Ordering::Relaxed)));
        }

        MeterSnapshot {
            rms,
            peak,
            peak_hold,
            clipped,
            waveform,
            bands: self
                .bands
                .iter()
                .map(|b| fixed_to_lin(b.load(Ordering::Relaxed)))
                .collect(),
        }
    }

    /// Reset clip indicator and peak-hold (e.g. on user request).
    pub fn reset_peak(&self) {
        self.peak_hold.store(0, Ordering::Relaxed);
        self.clipped.store(false, Ordering::Relaxed);
    }
}

/// A read-only snapshot of meter state, safe to use off the audio thread.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MeterSnapshot {
    pub rms: f32,
    pub peak: f32,
    pub peak_hold: f32,
    pub clipped: bool,
    pub waveform: Vec<f32>,
    /// Per-band normalised magnitudes [0.0–1.0], NUM_BANDS entries.
    pub bands: Vec<f32>,
}

/// Convert linear gain [0.0, ∞) to fixed-point usize.
#[inline]
fn lin_to_fixed(x: f32) -> usize {
    (x.clamp(0.0, 2.0) * SCALE as f32) as usize
}

/// Convert fixed-point usize back to linear gain.
#[inline]
fn fixed_to_lin(x: usize) -> f32 {
    x as f32 / SCALE as f32
}

/// Aggregated meter data for the entire routing graph.
///
/// Contains per-device-channel meters that the TUI reads. Each entry is
/// keyed by `"device_alias:channel_1based"` (e.g. `"vt4:3"`).
pub struct MeterBank {
    pub meters: Vec<(String, Arc<ChannelMeter>)>,
}

impl MeterBank {
    /// Build a meter bank for every channel referenced in the plan.
    ///
    /// Per-channel meters (key `alias:N`, 1-based) are created for every
    /// channel referenced by a route.  Additionally, a representative meter
    /// (key `alias:0`) is created for each device, fed by a mono down-mix of
    /// the device's OS-reported preferred channels.  The TUI visualises this
    /// representative meter so every device shows activity regardless of
    /// which channels are routed.
    pub fn for_plan(
        plan: &crate::validate::ValidatedConfig,
        resolved: &crate::devices::ResolvedAudioDevices,
    ) -> Self {
        use std::collections::BTreeMap;

        let mut keys: BTreeMap<String, ()> = BTreeMap::new();

        for route in &plan.routes {
            // Source device channels — only the channels actually routed.
            for &ch in &route.from_channels {
                keys.insert(format!("{}:{}", route.from, ch), ());
            }

            // Destination device channels — only the channels actually routed.
            for &ch in &route.to_channels {
                keys.insert(format!("{}:{}", route.to, ch), ());
            }
        }

        // Representative meter (mono down-mix of preferred channels) for each
        // resolved device.
        for alias in resolved.devices.keys() {
            keys.insert(format!("{}:0", alias), ());
        }

        let meters = keys
            .into_keys()
            .map(|k| (k, Arc::new(ChannelMeter::new())))
            .collect();

        Self { meters }
    }

    /// Look up the meter for a specific device + channel pair.
    pub fn get(&self, device: &str, channel: usize) -> Option<&Arc<ChannelMeter>> {
        let key = format!("{}:{}", device, channel);
        self.meters.iter().find(|(k, _)| k == &key).map(|(_, m)| m)
    }

    /// Reset clip indicators and peak-hold on all meters.
    pub fn reset_all_peaks(&self) {
        for (_, m) in &self.meters {
            m.reset_peak();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_roundtrip() {
        for x in [0.0f32, 0.1, 0.5, 0.99, 1.0] {
            let fixed = lin_to_fixed(x);
            let back = fixed_to_lin(fixed);
            assert!(
                (back - x).abs() < 0.002,
                "roundtrip failed for {x}: got {back}"
            );
        }
    }

    #[test]
    fn silence_produces_zero() {
        let m = ChannelMeter::new();
        m.update(&[0.0; 256]);
        let s = m.snapshot();
        assert!(s.rms < 0.001);
        assert!(s.peak < 0.001);
    }

    #[test]
    fn full_scale_produces_unity() {
        let m = ChannelMeter::new();
        m.update(&[1.0; 256]);
        let s = m.snapshot();
        assert!((s.peak - 1.0).abs() < 0.002);
        assert!((s.rms - 1.0).abs() < 0.002);
    }

    #[test]
    fn clip_detected() {
        let m = ChannelMeter::new();
        m.update(&[1.5, -1.2, 0.5]);
        let s = m.snapshot();
        assert!(s.clipped);
    }

    #[test]
    fn waveform_is_populated() {
        let m = ChannelMeter::new();
        // Push enough samples to fill waveform.
        let samples: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin()).collect();
        m.update(&samples);
        let s = m.snapshot();
        assert!(!s.waveform.is_empty());
        // At least some non-zero values.
        assert!(s.waveform.iter().any(|&v| v > 0.0));
    }

    #[test]
    fn reset_clears_clip() {
        let m = ChannelMeter::new();
        m.update(&[1.5]);
        assert!(m.snapshot().clipped);
        m.reset_peak();
        assert!(!m.snapshot().clipped);
        assert!(m.snapshot().peak_hold < 0.001);
    }
}
