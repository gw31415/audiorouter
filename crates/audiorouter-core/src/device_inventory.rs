//! System audio device inventory for dashboard/device-list APIs.

use std::collections::{HashMap, HashSet};

use cpal::traits::HostTrait;
use serde::Serialize;

use crate::devices::{collect_devices, max_channels, preferred_channels};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    pub name: String,
    pub max_input_channels: u16,
    pub max_output_channels: u16,
    pub preferred_input_channels: u16,
    pub preferred_output_channels: u16,
    pub is_default_input: bool,
    pub is_default_output: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct DevicesResponse {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
    pub all: Vec<AudioDeviceInfo>,
}

/// Enumerate currently visible system audio devices without reading audiorouter config.
pub fn list_audio_devices() -> anyhow::Result<DevicesResponse> {
    let host = cpal::default_host();
    let default_input_name = host.default_input_device().map(|d| d.to_string());
    let default_output_name = host.default_output_device().map(|d| d.to_string());

    let input_devices = collect_devices(&host, true)?;
    let output_devices = collect_devices(&host, false)?;

    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for d in input_devices.iter().chain(output_devices.iter()) {
        let name = d.to_string();
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }

    let input_by_name: HashMap<String, _> = input_devices
        .iter()
        .map(|d| (d.to_string(), d.clone()))
        .collect();
    let output_by_name: HashMap<String, _> = output_devices
        .iter()
        .map(|d| (d.to_string(), d.clone()))
        .collect();

    let mut all = Vec::new();
    for name in names {
        let input = input_by_name.get(&name);
        let output = output_by_name.get(&name);
        all.push(AudioDeviceInfo {
            max_input_channels: input.and_then(|d| max_channels(d, true)).unwrap_or(0),
            max_output_channels: output.and_then(|d| max_channels(d, false)).unwrap_or(0),
            preferred_input_channels: input.map(|d| preferred_channels(d, true)).unwrap_or(0),
            preferred_output_channels: output.map(|d| preferred_channels(d, false)).unwrap_or(0),
            is_default_input: default_input_name.as_deref() == Some(name.as_str()),
            is_default_output: default_output_name.as_deref() == Some(name.as_str()),
            name,
        });
    }

    all.sort_by(|a, b| a.name.cmp(&b.name));
    let inputs = all
        .iter()
        .filter(|d| d.max_input_channels > 0)
        .cloned()
        .collect();
    let outputs = all
        .iter()
        .filter(|d| d.max_output_channels > 0)
        .cloned()
        .collect();

    Ok(DevicesResponse {
        inputs,
        outputs,
        all,
    })
}

/// Compact fingerprint of a device inventory snapshot for change detection.
/// Two equal fingerprints guarantee identical device names, channel counts,
/// and default-device flags.
fn device_fingerprint(response: &DevicesResponse) -> String {
    let mut entries: Vec<(&str, u16, u16, bool, bool)> = response
        .all
        .iter()
        .map(|d| {
            (
                d.name.as_str(),
                d.max_input_channels,
                d.max_output_channels,
                d.is_default_input,
                d.is_default_output,
            )
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .into_iter()
        .map(|(n, i, o, di, do_)| format!("{n}:{i}/{o}/{di}/{do_}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// A set of human-readable descriptions of what changed between two snapshots.
/// Returns an empty vec when nothing changed.
pub fn device_diff(prev: &DevicesResponse, curr: &DevicesResponse) -> Vec<String> {
    if device_fingerprint(prev) == device_fingerprint(curr) {
        return Vec::new();
    }

    let prev_map: HashMap<&str, &AudioDeviceInfo> =
        prev.all.iter().map(|d| (d.name.as_str(), d)).collect();
    let curr_map: HashMap<&str, &AudioDeviceInfo> =
        curr.all.iter().map(|d| (d.name.as_str(), d)).collect();

    let prev_names: HashSet<&str> = prev_map.keys().copied().collect();
    let curr_names: HashSet<&str> = curr_map.keys().copied().collect();

    let mut events = Vec::new();

    // Added devices
    let mut added: Vec<&str> = curr_names.difference(&prev_names).copied().collect();
    added.sort_unstable();
    for name in added {
        let d = &curr_map[name];
        events.push(format!(
            "{name} connected (in:{}, out:{})",
            d.max_input_channels, d.max_output_channels
        ));
    }

    // Removed devices
    let mut removed: Vec<&str> = prev_names.difference(&curr_names).copied().collect();
    removed.sort_unstable();
    for name in removed {
        events.push(format!("{name} disconnected"));
    }

    // Changed devices (channel counts or defaults)
    let mut changed: Vec<&str> = curr_names.intersection(&prev_names).copied().collect();
    changed.sort_unstable();
    for name in changed {
        let p = &prev_map[name];
        let c = &curr_map[name];
        if p.max_input_channels != c.max_input_channels
            || p.max_output_channels != c.max_output_channels
        {
            events.push(format!(
                "{name} channels changed (in:{}→{}, out:{}→{})",
                p.max_input_channels,
                c.max_input_channels,
                p.max_output_channels,
                c.max_output_channels
            ));
        }
        if p.is_default_input != c.is_default_input && c.is_default_input {
            events.push(format!("{name} became default input"));
        }
        if p.is_default_output != c.is_default_output && c.is_default_output {
            events.push(format!("{name} became default output"));
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, i: u16, o: u16) -> AudioDeviceInfo {
        AudioDeviceInfo {
            name: name.to_string(),
            max_input_channels: i,
            max_output_channels: o,
            preferred_input_channels: i.min(2),
            preferred_output_channels: o.min(2),
            is_default_input: false,
            is_default_output: false,
        }
    }

    fn resp(devs: &[AudioDeviceInfo]) -> DevicesResponse {
        let all = devs.to_vec();
        let inputs = all
            .iter()
            .filter(|d| d.max_input_channels > 0)
            .cloned()
            .collect();
        let outputs = all
            .iter()
            .filter(|d| d.max_output_channels > 0)
            .cloned()
            .collect();
        DevicesResponse {
            inputs,
            outputs,
            all,
        }
    }

    #[test]
    fn no_change_returns_empty() {
        let r = resp(&[dev("A", 2, 0), dev("B", 0, 2)]);
        assert!(device_diff(&r, &r).is_empty());
    }

    #[test]
    fn device_added() {
        let prev = resp(&[dev("A", 2, 0)]);
        let curr = resp(&[dev("A", 2, 0), dev("B", 0, 2)]);
        let diff = device_diff(&prev, &curr);
        assert_eq!(diff.len(), 1);
        assert!(diff[0].contains("B connected"));
    }

    #[test]
    fn device_removed() {
        let prev = resp(&[dev("A", 2, 0), dev("B", 0, 2)]);
        let curr = resp(&[dev("A", 2, 0)]);
        let diff = device_diff(&prev, &curr);
        assert_eq!(diff.len(), 1);
        assert!(diff[0].contains("B disconnected"));
    }

    #[test]
    fn channel_count_changed() {
        let prev = resp(&[dev("A", 2, 2)]);
        let curr = resp(&[dev("A", 4, 2)]);
        let diff = device_diff(&prev, &curr);
        assert_eq!(diff.len(), 1);
        assert!(diff[0].contains("channels changed"));
    }

    #[test]
    fn fingerprint_order_independent() {
        let r1 = resp(&[dev("A", 2, 0), dev("B", 0, 2)]);
        let r2 = resp(&[dev("B", 0, 2), dev("A", 2, 0)]);
        assert_eq!(device_fingerprint(&r1), device_fingerprint(&r2));
    }
}
