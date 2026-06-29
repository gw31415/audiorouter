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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
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
