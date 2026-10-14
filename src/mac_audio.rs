//! macOS audio metadata through read-only AudioObject properties.
//! This module never creates an AudioUnit or registers an IOProc. Run property
//! queries off the GUI thread: CoreAudio does not provide per-call deadlines.
use anyhow::{anyhow, bail, Context, Result};
use core_foundation::{base::TCFType, string::CFString};
use coreaudio_sys::*;
use cpal::{SampleFormat, SampleRate, SupportedBufferSize, SupportedStreamConfig};
use std::{
    collections::{HashMap, HashSet},
    mem::{offset_of, size_of},
    ptr,
};

const MAX_PROPERTY_BYTES: usize = 65_536;
const MAX_DEVICES: usize = 512;
const MAX_BUFFERS: usize = 256;
const MAX_CONFIG_RATE: u32 = 192_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone)]
pub struct DeviceMetadata {
    pub id: AudioDeviceID,
    /// Position in kAudioHardwarePropertyDevices, matching CPAL host.devices().
    pub enumeration_index: usize,
    /// Names are disambiguated across the full device list, before filtering.
    pub name: String,
    pub input_channels: u16,
    pub output_channels: u16,
    pub sample_rate: u32,
    pub available_rates: Vec<RateRange>,
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub devices: Vec<DeviceMetadata>,
    pub default_input_name: Option<String>,
    pub default_output_name: Option<String>,
    pub issues: Vec<String>,
}

fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn os_error(status: OSStatus, operation: &str) -> anyhow::Error {
    let raw = (status as u32).to_be_bytes();
    let code = if raw.iter().all(|c| c.is_ascii_graphic()) {
        format!(" ('{}')", String::from_utf8_lossy(&raw))
    } else {
        String::new()
    };
    anyhow!("{operation}: CoreAudio status {status}{code}")
}

/// Allocate aligned, initialized storage and only copy bytes actually returned.
/// A changing property length is retried at most three times.
fn property(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<Vec<u8>> {
    let property = address(selector, scope);
    for _ in 0..3 {
        let mut requested = 0u32;
        // SAFETY: property address and output size point to valid mutable storage.
        let status = unsafe {
            AudioObjectGetPropertyDataSize(object, &property, 0, ptr::null(), &mut requested)
        };
        if status != 0 {
            return Err(os_error(status, "Reading audio property size"));
        }
        let requested = requested as usize;
        if requested > MAX_PROPERTY_BYTES {
            bail!("Audio property exceeds its {MAX_PROPERTY_BYTES}-byte bound");
        }
        if requested == 0 {
            return Ok(Vec::new());
        }
        // AudioBuffer, AudioValueRange, CFStringRef and scalar properties require
        // at most 8-byte alignment. The allocation remains initialized even when
        // CoreAudio writes only part of it.
        let mut storage = vec![0u64; requested.div_ceil(size_of::<u64>())];
        let mut returned = requested as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &property,
                0,
                ptr::null(),
                &mut returned,
                storage.as_mut_ptr().cast(),
            )
        };
        if status as u32 == kAudioHardwareBadPropertySizeError {
            continue;
        }
        if status != 0 {
            return Err(os_error(status, "Reading audio property"));
        }
        if returned as usize > requested {
            bail!("CoreAudio returned an invalid property size");
        }
        // SAFETY: returned <= requested <= initialized allocation size. A byte
        // view is alignment-independent and does not outlive the allocation.
        let bytes =
            unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), returned as usize) };
        return Ok(bytes.to_vec());
    }
    bail!("Audio property changed size repeatedly; refresh after device changes")
}

fn read_u32(bytes: &[u8]) -> Result<u32> {
    Ok(u32::from_ne_bytes(bytes.try_into().map_err(|_| {
        anyhow!("Invalid 32-bit audio property size")
    })?))
}
fn read_f64(bytes: &[u8]) -> Result<f64> {
    Ok(f64::from_ne_bytes(bytes.try_into().map_err(|_| {
        anyhow!("Invalid 64-bit audio property size")
    })?))
}

fn device_ids(bytes: &[u8]) -> Result<Vec<AudioDeviceID>> {
    if !bytes.len().is_multiple_of(size_of::<AudioDeviceID>()) {
        bail!("Malformed audio device array");
    }
    if bytes.len() / size_of::<AudioDeviceID>() > MAX_DEVICES {
        bail!("Too many audio devices");
    }
    bytes
        .as_chunks::<{ size_of::<AudioDeviceID>() }>()
        .0
        .iter()
        .map(|bytes| read_u32(bytes))
        .collect()
}

fn channel_count(bytes: &[u8]) -> Result<u16> {
    if bytes.len() < size_of::<u32>() {
        bail!("Truncated AudioBufferList");
    }
    let count = read_u32(&bytes[..4])? as usize;
    if count > MAX_BUFFERS {
        bail!("Too many audio buffers");
    }
    if count == 0 {
        return Ok(0);
    }
    let start = offset_of!(AudioBufferList, mBuffers);
    let required = start
        .checked_add(
            count
                .checked_mul(size_of::<AudioBuffer>())
                .ok_or_else(|| anyhow!("Audio buffer length overflow"))?,
        )
        .ok_or_else(|| anyhow!("Audio buffer length overflow"))?;
    if bytes.len() < required {
        bail!("Truncated audio buffer entries");
    }
    let mut channels = 0u32;
    for buffer in bytes[start..required]
        .as_chunks::<{ size_of::<AudioBuffer>() }>()
        .0
    {
        let offset = offset_of!(AudioBuffer, mNumberChannels);
        channels = channels
            .checked_add(read_u32(&buffer[offset..offset + 4])?)
            .ok_or_else(|| anyhow!("Audio channel count overflow"))?;
    }
    u16::try_from(channels).context("Audio channel count exceeds CPAL's supported width")
}

fn rate_ranges(bytes: &[u8]) -> Result<Vec<RateRange>> {
    if !bytes.len().is_multiple_of(size_of::<AudioValueRange>()) {
        bail!("Malformed sample-rate range array");
    }
    let mut result = Vec::new();
    for range in bytes.as_chunks::<{ size_of::<AudioValueRange>() }>().0 {
        let min_offset = offset_of!(AudioValueRange, mMinimum);
        let max_offset = offset_of!(AudioValueRange, mMaximum);
        let min = read_f64(&range[min_offset..min_offset + 8])?;
        let max = read_f64(&range[max_offset..max_offset + 8])?;
        if !min.is_finite() || !max.is_finite() || min < 1.0 || max < min || max > u32::MAX as f64 {
            bail!("Invalid audio sample-rate range");
        }
        result.push(RateRange { min, max });
    }
    Ok(result)
}

fn name(id: AudioDeviceID) -> Result<String> {
    let bytes = property(
        id,
        kAudioObjectPropertyName,
        kAudioObjectPropertyScopeGlobal,
    )?;
    let raw = usize::from_ne_bytes(
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid audio device-name pointer size"))?,
    );
    if raw == 0 {
        bail!("Audio device name is null");
    }
    // kAudioObjectPropertyName returns an owned CFString; the caller releases it.
    // SAFETY: successful CoreAudio property read provides the specified CFString
    // type. wrap_under_create_rule transfers its ownership to an RAII wrapper.
    let value = unsafe { CFString::wrap_under_create_rule(raw as _) };
    if !(0..=1024).contains(&value.char_len()) {
        bail!("Audio device name is too long");
    }
    Ok(value.to_string())
}

fn default_id(selector: AudioObjectPropertySelector) -> Result<AudioDeviceID> {
    read_u32(&property(
        kAudioObjectSystemObject,
        selector,
        kAudioObjectPropertyScopeGlobal,
    )?)
}

fn distinct_names(bases: &[String]) -> Vec<String> {
    let mut counts = HashMap::<String, usize>::new();
    let mut used = HashSet::new();
    bases
        .iter()
        .map(|base| {
            let count = counts.entry(base.clone()).or_default();
            *count += 1;
            let mut candidate = if *count == 1 {
                base.clone()
            } else {
                format!("{base} [{}]", *count)
            };
            while !used.insert(candidate.clone()) {
                *count += 1;
                candidate = format!("{base} [{}]", *count);
            }
            candidate
        })
        .collect()
}

pub fn snapshot() -> Result<Snapshot> {
    let ids = device_ids(&property(
        kAudioObjectSystemObject,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    )?)?;
    let mut devices = Vec::new();
    for (enumeration_index, id) in ids.into_iter().enumerate() {
        if id == kAudioObjectUnknown {
            continue;
        }
        let mut issues = Vec::new();
        let name = match name(id) {
            Ok(value) if !value.is_empty() => value,
            Ok(_) => format!("Unnamed audio device {}", enumeration_index + 1),
            Err(error) => {
                issues.push(error.to_string());
                format!("Unnamed audio device {}", enumeration_index + 1)
            }
        };
        let mut channels =
            |scope| match property(id, kAudioDevicePropertyStreamConfiguration, scope)
                .and_then(|b| channel_count(&b))
            {
                Ok(value) => value,
                Err(error) => {
                    issues.push(error.to_string());
                    0
                }
            };
        let input_channels = channels(kAudioObjectPropertyScopeInput);
        let output_channels = channels(kAudioObjectPropertyScopeOutput);
        let sample_rate = match property(
            id,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )
        .and_then(|b| read_f64(&b))
        {
            Ok(rate) if rate.is_finite() && rate >= 1.0 && rate <= u32::MAX as f64 => {
                rate.round() as u32
            }
            Ok(_) => {
                issues.push("Invalid current sample rate".into());
                0
            }
            Err(error) => {
                issues.push(error.to_string());
                0
            }
        };
        let available_rates = match property(
            id,
            kAudioDevicePropertyAvailableNominalSampleRates,
            kAudioObjectPropertyScopeGlobal,
        )
        .and_then(|b| rate_ranges(&b))
        {
            Ok(rates) => rates,
            Err(error) => {
                issues.push(error.to_string());
                vec![]
            }
        };
        devices.push(DeviceMetadata {
            id,
            enumeration_index,
            name,
            input_channels,
            output_channels,
            sample_rate,
            available_rates,
            is_default_input: false,
            is_default_output: false,
            issues,
        });
    }
    let names = distinct_names(&devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>());
    for (device, name) in devices.iter_mut().zip(names) {
        device.name = name;
    }
    let mut issues = Vec::new();
    let mut default = |selector| match default_id(selector) {
        Ok(id) => id,
        Err(error) => {
            issues.push(error.to_string());
            kAudioObjectUnknown
        }
    };
    let input = default(kAudioHardwarePropertyDefaultInputDevice);
    let output = default(kAudioHardwarePropertyDefaultOutputDevice);
    for device in &mut devices {
        device.is_default_input = device.id == input && input != kAudioObjectUnknown;
        device.is_default_output = device.id == output && output != kAudioObjectUnknown;
    }
    let default_input_name = devices
        .iter()
        .find(|d| d.is_default_input)
        .map(|d| d.name.clone());
    let default_output_name = devices
        .iter()
        .find(|d| d.is_default_output)
        .map(|d| d.name.clone());
    Ok(Snapshot {
        devices,
        default_input_name,
        default_output_name,
        issues,
    })
}

/// Pure selection from metadata: no HAL or AudioUnit calls.
pub fn supported_config(
    device: &DeviceMetadata,
    input: bool,
    channel: usize,
    exact_rate: Option<u32>,
) -> Result<SupportedStreamConfig> {
    let channels = if input {
        device.input_channels
    } else {
        device.output_channels
    };
    if channel >= channels as usize {
        bail!(
            "Device does not have channel {} in the selected direction",
            channel + 1
        );
    }
    let supported = |rate: u32| {
        device
            .available_rates
            .iter()
            .any(|range| range.min <= rate as f64 && rate as f64 <= range.max)
    };
    let current_supported = |rate: u32| {
        supported(rate) || (device.available_rates.is_empty() && rate == device.sample_rate)
    };
    let selected = if let Some(rate) = exact_rate {
        if rate == 0 || rate > MAX_CONFIG_RATE || !current_supported(rate) {
            bail!("Device does not advertise the requested {rate} Hz sample rate");
        }
        rate
    } else if device.sample_rate > 0
        && device.sample_rate <= MAX_CONFIG_RATE
        && current_supported(device.sample_rate)
    {
        device.sample_rate
    } else if supported(48_000) {
        48_000
    } else {
        device
            .available_rates
            .iter()
            .find_map(|range| {
                let min = range.min.ceil().max(1.0) as u32;
                let max = range.max.floor().min(MAX_CONFIG_RATE as f64) as u32;
                (min <= max).then(|| 48_000u32.clamp(min, max))
            })
            .ok_or_else(|| anyhow!("Device has no usable advertised sample rate"))?
    };
    Ok(SupportedStreamConfig::new(
        channels,
        SampleRate(selected),
        SupportedBufferSize::Unknown,
        SampleFormat::F32,
    ))
}

/// Resolve by the snapshot's distinct name. The caller can then select the same
/// CPAL host.devices() entry by enumeration_index, before starting authorized IO.
pub fn select_config(
    snapshot: &Snapshot,
    name: &str,
    input: bool,
    channel: usize,
    exact_rate: Option<u32>,
) -> Result<(DeviceMetadata, SupportedStreamConfig)> {
    let device = snapshot
        .devices
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| anyhow!("Audio device '{name}' is unavailable; refresh the device list"))?;
    Ok((
        device.clone(),
        supported_config(device, input, channel, exact_rate)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn descriptor() -> DeviceMetadata {
        DeviceMetadata {
            id: 1,
            enumeration_index: 0,
            name: "Mic".into(),
            input_channels: 2,
            output_channels: 0,
            sample_rate: 44_100,
            available_rates: vec![RateRange {
                min: 44_100.0,
                max: 48_000.0,
            }],
            is_default_input: true,
            is_default_output: false,
            issues: vec![],
        }
    }
    #[test]
    fn channel_count_uses_layout_without_dereferencing_audio_pointers() {
        let offset = offset_of!(AudioBufferList, mBuffers);
        let mut bytes = vec![0; offset + 2 * size_of::<AudioBuffer>()];
        bytes[..4].copy_from_slice(&2u32.to_ne_bytes());
        bytes[offset..offset + 4].copy_from_slice(&2u32.to_ne_bytes());
        let next = offset + size_of::<AudioBuffer>();
        bytes[next..next + 4].copy_from_slice(&6u32.to_ne_bytes());
        assert_eq!(channel_count(&bytes).unwrap(), 8);
        assert!(channel_count(&bytes[..next + 3]).is_err());
        assert_eq!(channel_count(&0u32.to_ne_bytes()).unwrap(), 0);
        assert!(channel_count(&[0; 3]).is_err());
        assert!(channel_count(&(MAX_BUFFERS as u32 + 1).to_ne_bytes()).is_err());
    }
    #[test]
    fn validates_array_lengths_and_nonfinite_rates() {
        assert!(device_ids(&[0; 3]).is_err());
        assert_eq!(device_ids(&[1, 0, 0, 0, 2, 0, 0, 0]).unwrap(), vec![1, 2]);
        let mut ranges = 44_100f64.to_ne_bytes().to_vec();
        ranges.extend_from_slice(&48_000f64.to_ne_bytes());
        assert_eq!(
            rate_ranges(&ranges).unwrap(),
            vec![RateRange {
                min: 44_100.0,
                max: 48_000.0
            }]
        );
        assert!(rate_ranges(&ranges[..15]).is_err());
        ranges[..8].copy_from_slice(&f64::NAN.to_ne_bytes());
        assert!(rate_ranges(&ranges).is_err());
    }
    #[test]
    fn duplicate_names_stay_distinct_even_with_literal_suffix_collisions() {
        let bases = ["Mic", "Mic", "Mic [2]", "Mic"].map(String::from);
        assert_eq!(
            distinct_names(&bases),
            vec!["Mic", "Mic [2]", "Mic [2] [2]", "Mic [3]"]
        );
    }
    #[test]
    fn config_selection_is_pure_and_enforces_channel_and_exact_rate() {
        let d = descriptor();
        let c = supported_config(&d, true, 1, Some(48_000)).unwrap();
        assert_eq!(
            (c.channels(), c.sample_rate().0, c.sample_format()),
            (2, 48_000, SampleFormat::F32)
        );
        assert!(matches!(c.buffer_size(), SupportedBufferSize::Unknown));
        assert_eq!(
            supported_config(&d, true, 0, None).unwrap().sample_rate().0,
            44_100
        );
        assert!(supported_config(&d, true, 2, None).is_err());
        assert!(supported_config(&d, false, 0, None).is_err());
        assert!(supported_config(&d, true, 0, Some(96_000)).is_err());
    }
}
