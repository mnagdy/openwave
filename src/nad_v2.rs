//! Read-only NAD Dirac v2 metadata client. No device-setting or transfer methods.
use anyhow::{anyhow, bail, Context, Result};
use bytes::{Buf, BufMut, Bytes};
use serde::Serialize;
use std::{collections::BTreeSet, net::SocketAddr, time::Duration};
use tonic::{
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    transport::{Channel, Endpoint},
    Request, Status,
};

const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_CALLS: usize = 32;

#[derive(Debug, Clone, Serialize)]
pub struct NativeDevice {
    pub manufacturer: String,
    pub model: String,
    pub name: String,
    pub speakers: u32,
    pub microphones: u32,
    pub reported_slot_count: u32,
    pub slots: Vec<NativeSlot>,
    pub sections: Vec<NativeSection>,
    pub cells: Vec<NativeCell>,
    pub dual_rate_variants: Vec<NativeDualRateInfo>,
    pub measurement: Option<NativeMeasurement>,
    pub coverage_limited: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeMeasurement {
    pub stimuli: Vec<NativeStimulus>,
    pub log_sweeps: Vec<NativeLogSweep>,
    pub gain_min_db: f64,
    pub gain_max_db: f64,
    pub gain_step_db: f64,
    pub speaker_gains_db: Vec<f64>,
    pub operation_mode: NativeOperationMode,
}
#[derive(Debug, Clone, Serialize)]
pub struct NativeStimulus {
    pub kind: u32,
    pub variant: u32,
    pub supports_override: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct NativeLogSweep {
    pub variant: u32,
    pub sample_rate: f32,
    pub start_hz: f32,
    pub end_hz: f32,
    pub samples: u32,
    pub silence_samples: u32,
    pub periods: u32,
    pub fade_out_tail_hz: f64,
}
#[derive(Debug, Clone, Serialize)]
pub struct NativeOperationMode {
    pub gain_enabled: bool,
    pub delay_enabled: bool,
    pub filtering_enabled: bool,
    pub measure_enabled: bool,
    pub streaming_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeSlot {
    pub index: u32,
    pub name: String,
    pub filter_type_name: String,
    pub inputs: u32,
    pub outputs: u32,
    pub sections: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeSection {
    pub index: u32,
    pub inputs: u32,
    pub outputs: u32,
    pub disposition: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeCell {
    pub input: u32,
    pub output: u32,
    pub section: u32,
    pub filter_type: u32,
    pub variants: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeDualRateInfo {
    pub variant: u32,
    pub sample_rate: f32,
    pub high_taps: u32,
    pub low_taps: u32,
    pub sub_sampling_factor: u32,
    pub cascades: u32,
    pub high_delay_min: u32,
    pub high_delay_max: u32,
    pub individual_delays: bool,
}

#[derive(Default)]
pub(crate) struct RawCodec;
pub(crate) struct RawEncoder;
pub(crate) struct RawDecoder;
impl Codec for RawCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = RawEncoder;
    type Decoder = RawDecoder;
    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder
    }
    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder
    }
}
impl Encoder for RawEncoder {
    type Item = Bytes;
    type Error = Status;
    fn encode(&mut self, item: Bytes, dst: &mut EncodeBuf<'_>) -> std::result::Result<(), Status> {
        if item.len() > MAX_MESSAGE {
            return Err(Status::resource_exhausted("Request too large"));
        }
        dst.put_slice(&item);
        Ok(())
    }
}
impl Decoder for RawDecoder {
    type Item = Bytes;
    type Error = Status;
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> std::result::Result<Option<Bytes>, Status> {
        if src.remaining() > MAX_MESSAGE {
            return Err(Status::resource_exhausted("Metadata response too large"));
        }
        Ok(Some(src.copy_to_bytes(src.remaining())))
    }
}

#[derive(Clone, Copy)]
enum ReadMethod {
    Device,
    Slot,
    Section,
    Cell,
    DualRate,
    Stimuli,
    StimuliGainInfo,
    StimuliGain,
    OperationMode,
    LogSweep,
}
impl ReadMethod {
    fn path(self) -> &'static str {
        match self {
            Self::Device => "/dio.endpoint.EndpointDevice/GetDeviceInfo",
            Self::Slot => "/dio.endpoint.EndpointDevice/GetSlotInfo",
            Self::Section => "/dio.endpoint.EndpointDevice/GetSectionInfo",
            Self::Cell => "/dio.endpoint.EndpointDevice/GetCellInfo",
            Self::DualRate => "/dio.endpoint.EndpointDevice/GetDRFIRInfo",
            Self::Stimuli => "/dio.endpoint.EndpointDevice/GetStimuliInfo",
            Self::StimuliGainInfo => "/dio.endpoint.EndpointDevice/GetStimuliGainInfo",
            Self::StimuliGain => "/dio.endpoint.EndpointDevice/GetStimuliGain",
            Self::OperationMode => "/dio.endpoint.EndpointDevice/GetOperationMode",
            Self::LogSweep => "/dio.endpoint.EndpointDevice/GetLogSweepInfo",
        }
    }
}

struct Reader {
    grpc: tonic::client::Grpc<Channel>,
    calls: usize,
}
impl Reader {
    async fn call(&mut self, method: ReadMethod, payload: Vec<u8>) -> Result<Vec<u8>> {
        if self.calls >= MAX_CALLS {
            bail!("Read-only inspection reached its {MAX_CALLS}-request limit");
        }
        self.calls += 1;
        self.grpc
            .ready()
            .await
            .context("Preparing NAD metadata connection")?;
        let path = tonic::codegen::http::uri::PathAndQuery::from_static(method.path());
        let mut request = Request::new(Bytes::from(payload));
        request.set_timeout(Duration::from_secs(3));
        let response = self
            .grpc
            .unary(request, path, RawCodec)
            .await
            .with_context(|| format!("Reading NAD metadata through {}", method.path()))?;
        // Require explicit success: HTTP 200 alone is not a gRPC acknowledgement.
        require_success_status(
            response
                .metadata()
                .get("grpc-status")
                .map(|s| s.to_str())
                .transpose()?,
        )?;
        Ok(response.into_inner().to_vec())
    }
}
pub(crate) fn require_success_status(status: Option<&str>) -> Result<()> {
    if status != Some("0") {
        bail!("Native response did not contain an explicit successful gRPC status");
    }
    Ok(())
}

/// Inspect an explicitly selected IP and advertised Dirac v2 port.
/// At most 32 metadata requests, 1 MiB per response, 30 seconds overall.
/// Returned slots are internal API entries, not a promise of writable UI slots.
pub fn inspect(address: &str, port: u16) -> Result<NativeDevice> {
    let ip = crate::device::address(address)?.ip();
    if port == 0 {
        bail!("NAD native port must be nonzero");
    }
    let endpoint = format!("http://{}", SocketAddr::new(ip, port));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), inspect_async(endpoint))
            .await
            .map_err(|_| anyhow!("NAD metadata inspection exceeded 30 seconds"))?
    })
}

async fn inspect_async(address: String) -> Result<NativeDevice> {
    let channel = Endpoint::from_shared(address)?
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(4))
        .connect()
        .await
        .context("Connecting to NAD Dirac v2")?;
    let grpc = tonic::client::Grpc::new(channel)
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE);
    let mut reader = Reader { grpc, calls: 0 };
    let data = reader.call(ReadMethod::Device, vec![]).await?;
    let mut device = parse_device(&data)?;
    if device.reported_slot_count > 16 || device.speakers > 64 || device.microphones > 64 {
        bail!("Device reports implausible metadata counts");
    }
    for index in 0..device.reported_slot_count {
        let data = reader.call(ReadMethod::Slot, index_payload(index)).await?;
        device.slots.push(parse_slot(&data, index)?);
    }
    let section_count = device.slots.iter().map(|s| s.sections).max().unwrap_or(0);
    if section_count > 4 {
        device.coverage_limited = true;
        device
            .notes
            .push("Only the first four sections were inspected.".into());
    }
    let mut variants = BTreeSet::new();
    'sections: for section in 0..section_count.min(4) {
        if reader.calls >= MAX_CALLS - 1 {
            device.coverage_limited = true;
            break;
        }
        let data = match reader
            .call(ReadMethod::Section, index_payload(section))
            .await
        {
            Ok(data) => data,
            Err(error) => {
                device.coverage_limited = true;
                device.notes.push(error.to_string());
                continue;
            }
        };
        let info = parse_section(&data, section)?;
        if info.inputs > 64 || info.outputs > 64 {
            bail!("Device reports implausible section channel counts");
        }
        if info.inputs > 4 || info.outputs > 4 {
            device.coverage_limited = true;
        }
        for input in 0..info.inputs.min(4) {
            for output in 0..info.outputs.min(4) {
                if reader.calls >= MAX_CALLS - 1 {
                    device.coverage_limited = true;
                    device.sections.push(info);
                    break 'sections;
                }
                let data = match reader
                    .call(ReadMethod::Cell, cell_payload(input, output, section))
                    .await
                {
                    Ok(data) => data,
                    Err(error) => {
                        device.coverage_limited = true;
                        device.notes.push(error.to_string());
                        continue;
                    }
                };
                let cell = parse_cell(&data, input, output, section)?;
                if cell.filter_type == 3 {
                    variants.extend(cell.variants.iter().copied());
                }
                device.cells.push(cell);
            }
        }
        device.sections.push(info);
    }
    for variant in variants {
        if reader.calls >= MAX_CALLS {
            device.coverage_limited = true;
            break;
        }
        match reader
            .call(ReadMethod::DualRate, index_payload(variant))
            .await
        {
            Ok(data) => device
                .dual_rate_variants
                .push(parse_dual_rate(&data, variant)?),
            Err(error) => {
                device.coverage_limited = true;
                device.notes.push(error.to_string());
            }
        }
    }
    match read_measurement(&mut reader, device.speakers).await {
        Ok(measurement) => device.measurement = Some(measurement),
        Err(error) => device
            .notes
            .push(format!("Measurement metadata unavailable: {error:#}")),
    }
    if device.coverage_limited && device.notes.is_empty() {
        device
            .notes
            .push("Metadata inspection was limited by channel or request bounds.".into());
    }
    Ok(device)
}

async fn read_measurement(reader: &mut Reader, speakers: u32) -> Result<NativeMeasurement> {
    let data = reader.call(ReadMethod::Stimuli, vec![]).await?;
    let stimuli = parse_stimuli(&data)?;
    let data = reader.call(ReadMethod::StimuliGainInfo, vec![]).await?;
    let f = fields(&data)?;
    if text(&f, 4)? != "dB" {
        bail!("Unsupported stimulus gain units");
    }
    let gain_min_db = double(&f, 1)?;
    let gain_step_db = double(&f, 2)?;
    let gain_max_db = double(&f, 3)?;
    if gain_min_db < -200.0
        || gain_min_db > gain_max_db
        || gain_max_db > 24.0
        || gain_step_db <= 0.0
    {
        bail!("Implausible stimulus gain range");
    }
    if speakers > 4 {
        bail!("Measurement metadata limited to four speakers");
    }
    let mut speaker_gains_db = Vec::new();
    for speaker in 0..speakers {
        let data = reader
            .call(ReadMethod::StimuliGain, index_payload(speaker))
            .await?;
        let f = fields(&data)?;
        if integer(&f, 1)? != speaker {
            bail!("Unexpected stimulus gain speaker index");
        }
        let gain = double(&f, 2)?;
        if gain < gain_min_db || gain > gain_max_db {
            bail!("Stimulus gain outside advertised range");
        }
        speaker_gains_db.push(gain);
    }
    let data = reader.call(ReadMethod::OperationMode, vec![]).await?;
    let operation_mode = parse_operation_mode(&data)?;
    let mut log_sweeps = Vec::new();
    for stimulus in stimuli.iter().filter(|s| s.kind == 3).take(4) {
        let data = reader
            .call(ReadMethod::LogSweep, index_payload(stimulus.variant))
            .await?;
        log_sweeps.push(parse_log_sweep(&data, stimulus.variant)?);
    }
    Ok(NativeMeasurement {
        stimuli,
        log_sweeps,
        gain_min_db,
        gain_max_db,
        gain_step_db,
        speaker_gains_db,
        operation_mode,
    })
}

fn parse_stimuli(data: &[u8]) -> Result<Vec<NativeStimulus>> {
    let mut stimuli = Vec::new();
    for field in fields(data)?.iter().filter(|f| f.number == 1) {
        let Value::Bytes(data) = field.value else {
            bail!("Unexpected stimulus-list wire type");
        };
        let f = fields(data)?;
        stimuli.push(NativeStimulus {
            kind: integer(&f, 1)?,
            variant: integer(&f, 2)?,
            supports_override: boolean(&f, 15)?,
        });
        if stimuli.len() > 32 {
            bail!("Too many stimulus variants");
        }
    }
    Ok(stimuli)
}

pub(crate) fn parse_operation_mode(data: &[u8]) -> Result<NativeOperationMode> {
    let f = fields(data)?;
    Ok(NativeOperationMode {
        gain_enabled: boolean(&f, 1)?,
        delay_enabled: boolean(&f, 2)?,
        filtering_enabled: boolean(&f, 3)?,
        measure_enabled: boolean(&f, 4)?,
        streaming_enabled: boolean(&f, 5)?,
    })
}
pub(crate) fn parse_stimulus_gain(data: &[u8], expected: u32) -> Result<f64> {
    let f = fields(data)?;
    if integer(&f, 1)? != expected {
        bail!("Unexpected stimulus gain speaker index");
    }
    double(&f, 2)
}

fn parse_log_sweep(data: &[u8], expected: u32) -> Result<NativeLogSweep> {
    let f = fields(data)?;
    let s = NativeLogSweep {
        variant: integer(&f, 1)?,
        sample_rate: float(&f, 2)?,
        start_hz: float(&f, 3)?,
        end_hz: float(&f, 4)?,
        samples: integer(&f, 5)?,
        silence_samples: integer(&f, 6)?,
        periods: integer(&f, 7)?,
        fade_out_tail_hz: double(&f, 8)?,
    };
    if s.variant != expected
        || !(8000.0..=192000.0).contains(&s.sample_rate)
        || s.start_hz <= 0.0
        || s.end_hz <= s.start_hz
        || s.end_hz > s.sample_rate / 2.0
        || s.samples == 0
        || s.samples as f32 > s.sample_rate * 30.0
        || s.silence_samples as f32 > s.sample_rate * 10.0
        || !(1..=4).contains(&s.periods)
        || s.fade_out_tail_hz < 0.0
        || s.fade_out_tail_hz > s.sample_rate as f64 / 2.0
    {
        bail!("Invalid or unsupported native log-sweep dimensions");
    }
    Ok(s)
}

fn put_varint(mut number: u32, output: &mut Vec<u8>) {
    while number > 127 {
        output.push((number as u8 & 127) | 128);
        number >>= 7;
    }
    output.push(number as u8);
}
fn index_payload(index: u32) -> Vec<u8> {
    let mut result = vec![8];
    put_varint(index, &mut result);
    result
}
fn cell_payload(input: u32, output: u32, section: u32) -> Vec<u8> {
    let mut result = Vec::new();
    for (tag, value) in [(8, input), (16, output), (24, section)] {
        result.push(tag);
        put_varint(value, &mut result);
    }
    result
}

#[derive(Clone, Copy)]
enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed32(&'a [u8]),
    Fixed64(&'a [u8]),
}
struct Field<'a> {
    number: u32,
    value: Value<'a>,
}
fn varint(data: &[u8], offset: &mut usize) -> Result<u64> {
    let mut result = 0u64;
    for index in 0..10 {
        let byte = *data
            .get(*offset)
            .ok_or_else(|| anyhow!("Truncated protobuf varint"))?;
        *offset += 1;
        if index == 9 && byte > 1 {
            bail!("Protobuf varint overflow");
        }
        result |= ((byte & 127) as u64) << (index * 7);
        if byte < 128 {
            return Ok(result);
        }
    }
    bail!("Protobuf varint overflow")
}
fn fields(data: &[u8]) -> Result<Vec<Field<'_>>> {
    if data.len() > MAX_MESSAGE {
        bail!("Protobuf metadata exceeds size limit");
    }
    let mut offset = 0;
    let mut result = Vec::new();
    while offset < data.len() {
        let tag = varint(data, &mut offset)?;
        let number = u32::try_from(tag >> 3)?;
        if number == 0 || number > 0x1fff_ffff {
            bail!("Invalid protobuf field number");
        }
        let value = match tag & 7 {
            0 => Value::Varint(varint(data, &mut offset)?),
            wire @ (1 | 2 | 5) => {
                let length = if wire == 2 {
                    usize::try_from(varint(data, &mut offset)?)?
                } else if wire == 1 {
                    8
                } else {
                    4
                };
                let end = offset
                    .checked_add(length)
                    .ok_or_else(|| anyhow!("Protobuf length overflow"))?;
                let bytes = data
                    .get(offset..end)
                    .ok_or_else(|| anyhow!("Truncated protobuf field"))?;
                offset = end;
                match wire {
                    1 => Value::Fixed64(bytes),
                    2 => Value::Bytes(bytes),
                    _ => Value::Fixed32(bytes),
                }
            }
            _ => bail!("Unsupported protobuf wire type"),
        };
        result.push(Field { number, value });
        if result.len() > 65536 {
            bail!("Too many protobuf fields");
        }
    }
    Ok(result)
}
fn integer(items: &[Field<'_>], number: u32) -> Result<u32> {
    match items
        .iter()
        .rev()
        .find(|f| f.number == number)
        .map(|f| f.value)
    {
        None => Ok(0),
        Some(Value::Varint(value)) => Ok(u32::try_from(value)?),
        _ => bail!("Unexpected wire type for metadata integer field {number}"),
    }
}
fn text(items: &[Field<'_>], number: u32) -> Result<String> {
    match items
        .iter()
        .rev()
        .find(|f| f.number == number)
        .map(|f| f.value)
    {
        None => Ok(String::new()),
        Some(Value::Bytes(value)) => {
            if value.len() > 4096 {
                bail!("Metadata string is too long");
            }
            Ok(std::str::from_utf8(value)?.to_owned())
        }
        _ => bail!("Unexpected wire type for metadata text field {number}"),
    }
}
fn float(items: &[Field<'_>], number: u32) -> Result<f32> {
    match items
        .iter()
        .rev()
        .find(|f| f.number == number)
        .map(|f| f.value)
    {
        None => Ok(0.0),
        Some(Value::Fixed32(value)) => {
            let value = f32::from_le_bytes(value.try_into()?);
            if !value.is_finite() {
                bail!("Non-finite metadata float");
            }
            Ok(value)
        }
        _ => bail!("Unexpected wire type for metadata float field {number}"),
    }
}
fn double(items: &[Field<'_>], number: u32) -> Result<f64> {
    match items
        .iter()
        .rev()
        .find(|f| f.number == number)
        .map(|f| f.value)
    {
        None => Ok(0.0),
        Some(Value::Fixed64(bytes)) => {
            let value = f64::from_le_bytes(bytes.try_into()?);
            if !value.is_finite() {
                bail!("Non-finite metadata double");
            }
            Ok(value)
        }
        _ => bail!("Unexpected wire type for metadata double field {number}"),
    }
}
fn boolean(items: &[Field<'_>], number: u32) -> Result<bool> {
    match integer(items, number)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => bail!("Invalid metadata boolean field {number}"),
    }
}
fn parse_device(data: &[u8]) -> Result<NativeDevice> {
    let f = fields(data)?;
    let manufacturer = text(&f, 1)?;
    let model = text(&f, 3)?;
    if manufacturer.is_empty() || model.is_empty() {
        bail!("Device response lacks manufacturer/model identity");
    }
    Ok(NativeDevice {
        manufacturer,
        model,
        name: text(&f, 2)?,
        speakers: integer(&f, 5)?,
        microphones: integer(&f, 6)?,
        reported_slot_count: integer(&f, 7)?,
        slots: vec![],
        sections: vec![],
        cells: vec![],
        dual_rate_variants: vec![],
        measurement: None,
        coverage_limited: false,
        notes: vec![],
    })
}
fn parse_slot(data: &[u8], expected: u32) -> Result<NativeSlot> {
    let f = fields(data)?;
    let index = integer(&f, 1)?;
    if index != expected {
        bail!("NAD returned a different slot index");
    }
    Ok(NativeSlot {
        index,
        name: text(&f, 2)?,
        filter_type_name: text(&f, 3)?,
        inputs: integer(&f, 4)?,
        outputs: integer(&f, 5)?,
        sections: integer(&f, 9)?,
    })
}
fn parse_section(data: &[u8], expected: u32) -> Result<NativeSection> {
    let f = fields(data)?;
    let index = integer(&f, 1)?;
    if index != expected {
        bail!("NAD returned a different section index");
    }
    Ok(NativeSection {
        index,
        inputs: integer(&f, 3)?,
        outputs: integer(&f, 4)?,
        disposition: integer(&f, 6)?,
    })
}
fn parse_cell(data: &[u8], input: u32, output: u32, section: u32) -> Result<NativeCell> {
    let f = fields(data)?;
    let mut variants = Vec::new();
    for field in f.iter().filter(|f| f.number == 2) {
        match field.value {
            Value::Varint(value) => variants.push(u32::try_from(value)?),
            Value::Bytes(bytes) => {
                let mut offset = 0;
                while offset < bytes.len() {
                    variants.push(u32::try_from(varint(bytes, &mut offset)?)?);
                    if variants.len() > 256 {
                        bail!("Too many filter variants");
                    }
                }
            }
            _ => bail!("Invalid filter-variant wire type"),
        }
    }
    if variants.len() > 256 {
        bail!("Too many filter variants");
    }
    Ok(NativeCell {
        input,
        output,
        section,
        filter_type: integer(&f, 1)?,
        variants,
    })
}
fn parse_dual_rate(data: &[u8], expected: u32) -> Result<NativeDualRateInfo> {
    let f = fields(data)?;
    let variant = integer(&f, 1)?;
    let sample_rate = float(&f, 2)?;
    if variant != expected {
        bail!("NAD returned a different filter variant");
    }
    if !(1.0..=192000.0).contains(&sample_rate) {
        bail!("Invalid reported filter sample rate");
    }
    Ok(NativeDualRateInfo {
        variant,
        sample_rate,
        high_taps: integer(&f, 3)?,
        low_taps: integer(&f, 4)?,
        sub_sampling_factor: integer(&f, 5)?,
        cascades: integer(&f, 6)?,
        high_delay_min: integer(&f, 7)?,
        high_delay_max: integer(&f, 8)?,
        individual_delays: integer(&f, 9)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_acknowledgement_requires_explicit_success() {
        assert!(require_success_status(Some("0")).is_ok());
        for status in [None, Some(""), Some("13"), Some("00")] {
            assert!(require_success_status(status).is_err());
        }
    }
    #[test]
    fn parse_identity_and_proto3_defaults_without_retaining_unique_id() {
        let data = b"\x0a\x03NAD\x12\x04Room\x1a\x03M10\x22\x06secret\x28\x02\x38\x05";
        let d = parse_device(data).unwrap();
        assert_eq!(
            (d.manufacturer.as_str(), d.model.as_str(), d.name.as_str()),
            ("NAD", "M10", "Room")
        );
        assert_eq!(
            (d.speakers, d.microphones, d.reported_slot_count),
            (2, 0, 5)
        );
        assert!(!serde_json::to_string(&d).unwrap().contains("secret"));
    }
    #[test]
    fn packed_and_unpacked_variants_are_accepted() {
        assert_eq!(
            parse_cell(&[8, 3, 18, 3, 0, 2, 4], 0, 0, 0)
                .unwrap()
                .variants,
            vec![0, 2, 4]
        );
        assert_eq!(
            parse_cell(&[8, 3, 16, 1, 16, 3, 16, 5], 0, 1, 0)
                .unwrap()
                .variants,
            vec![1, 3, 5]
        );
        assert!(parse_cell(&[8, 3, 18, 1, 128], 0, 0, 0).is_err());
    }
    #[test]
    fn malformed_unknown_fields_and_overflows_are_bounded() {
        assert!(fields(&[0]).is_err());
        assert!(fields(&[10, 100, 1]).is_err());
        assert!(fields(&[8, 255, 255, 255, 255, 255, 255, 255, 255, 255, 2]).is_err());
        assert!(fields(&[8, 128]).is_err());
        assert!(parse_slot(&[8, 1], 0).is_err());
        assert!(fields(&vec![0; MAX_MESSAGE + 1]).is_err());
    }
    #[test]
    fn dual_rate_numeric_metadata_and_zero_variant_are_preserved() {
        let mut data = vec![21];
        data.extend_from_slice(&48_000f32.to_le_bytes());
        data.extend_from_slice(&[24, 128, 8, 32, 128, 8, 40, 4, 48, 2, 64, 158, 6]);
        let info = parse_dual_rate(&data, 0).unwrap();
        assert_eq!(
            (info.high_taps, info.low_taps, info.high_delay_max),
            (1024, 1024, 798)
        );
        assert_eq!(info.sample_rate, 48_000.0);
        data[1..5].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(parse_dual_rate(&data, 0).is_err());
    }
    #[test]
    fn only_source_established_read_methods_and_exact_request_fields() {
        assert_eq!(index_payload(300), vec![8, 172, 2]);
        assert_eq!(cell_payload(0, 1, 0), vec![8, 0, 16, 1, 24, 0]);
        assert!(ReadMethod::Device.path().ends_with("/GetDeviceInfo"));
        assert!(inspect("not-an-ip", 5000).is_err());
    }
    #[test]
    fn stimulus_defaults_and_malformed_state_are_checked() {
        let stimuli = parse_stimuli(&[10, 2, 8, 3, 10, 4, 8, 3, 16, 1]).unwrap();
        assert_eq!(
            (
                stimuli[0].kind,
                stimuli[0].variant,
                stimuli[0].supports_override
            ),
            (3, 0, false)
        );
        assert_eq!(stimuli[1].variant, 1);
        assert_eq!(parse_stimulus_gain(&[], 0).unwrap(), 0.);
        assert!(parse_stimulus_gain(&[], 1).is_err());
        assert!(parse_operation_mode(&[8, 2]).is_err());
        let mut invalid = vec![17];
        invalid.extend_from_slice(&f64::NAN.to_le_bytes());
        assert!(parse_stimulus_gain(&invalid, 0).is_err());
        assert!(parse_stimuli(&[8, 3]).is_err());
    }
}
