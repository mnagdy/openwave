//! Experimental original native transfer executor. No explicit slot activation
//! is sent; activation caused by commit itself remains unverified.
//! Public entry rebuilds coefficients from Session and fresh read metadata; JSON
//! packets and readiness flags are never accepted as upload authority.
use crate::{
    nad_transfer::{self, ResponseKind, TransferPackage},
    nad_v2,
    session::Session,
};
use anyhow::{bail, ensure, Context, Result};
use bytes::Bytes;
use serde::Serialize;
use std::{
    future::Future,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tonic::{
    transport::{Channel, Endpoint},
    Request,
};

const MAX_BODY: usize = 1024 * 1024;
const MAX_PROGRESS: usize = 512;
const PREFIX: &str = "/dio.endpoint.EndpointDevice/";

#[derive(Debug, Clone, Serialize)]
pub struct UploadReceipt {
    pub slot: u32,
    pub name: String,
    pub unary_acknowledged: usize,
    pub progress_messages: usize,
    pub commit_acknowledged: bool,
    pub slot_metadata_verified: bool,
    /// Canonical name/family/UUID/time/dimensions, not coefficients or all optional fields.
    pub existing_named_slots_metadata_unchanged: bool,
    pub acoustic_response_verified: bool,
    pub hardware_validated: bool,
    pub explicitly_activated: bool,
    pub commit_activation_behavior_verified: bool,
    pub note: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct UploadFailure {
    pub detail: String,
    pub begin_attempted: bool,
    pub unary_acknowledged: usize,
    pub commit_attempted: bool,
    pub commit_acknowledged: bool,
    pub rollback_verified: bool,
    pub activation_behavior_verified: bool,
}
impl std::fmt::Display for UploadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,"{}; acknowledged {} unary calls; Begin attempted: {}; commit attempted: {}; commit acknowledged: {}. {}",self.detail,self.unary_acknowledged,self.begin_attempted,self.commit_attempted,self.commit_acknowledged,
            if self.begin_attempted {"The device may retain a partial or committed transfer. Rollback, restoration and commit activation behavior are unverified; no delete, activation or retry was sent."}else{"No transfer write was attempted."})
    }
}
impl std::error::Error for UploadFailure {}

#[derive(Default)]
struct State {
    begin: bool,
    unary: usize,
    commit: bool,
    acknowledged: bool,
}
impl State {
    fn failure(&self, e: impl std::fmt::Display) -> anyhow::Error {
        UploadFailure {
            detail: e.to_string(),
            begin_attempted: self.begin,
            unary_acknowledged: self.unary,
            commit_attempted: self.commit,
            commit_acknowledged: self.acknowledged,
            rollback_verified: false,
            activation_behavior_verified: false,
        }
        .into()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Begin,
    Common,
    Select,
    Coefficient,
    Gain,
    Commit,
}
impl Method {
    fn name(self) -> &'static str {
        match self {
            Self::Begin => "BeginFilterTransfer",
            Self::Common => "SetCommonDRFIR",
            Self::Select => "SetTransferCell",
            Self::Coefficient => "SetCellDRFIR",
            Self::Gain => "SetInputGainDelay",
            Self::Commit => "EndFilterTransfer",
        }
    }
    fn request_type(self) -> &'static str {
        match self {
            Self::Begin => "SlotInfo",
            Self::Common => "DRFIRCOMMON",
            Self::Select => "CellIndex",
            Self::Coefficient => "DRFIR",
            Self::Gain => "GainDelay",
            Self::Commit => "Index",
        }
    }
    fn path(self) -> tonic::codegen::http::uri::PathAndQuery {
        tonic::codegen::http::uri::PathAndQuery::from_static(match self {
            Self::Begin => "/dio.endpoint.EndpointDevice/BeginFilterTransfer",
            Self::Common => "/dio.endpoint.EndpointDevice/SetCommonDRFIR",
            Self::Select => "/dio.endpoint.EndpointDevice/SetTransferCell",
            Self::Coefficient => "/dio.endpoint.EndpointDevice/SetCellDRFIR",
            Self::Gain => "/dio.endpoint.EndpointDevice/SetInputGainDelay",
            Self::Commit => "/dio.endpoint.EndpointDevice/EndFilterTransfer",
        })
    }
}
trait Transport {
    fn unary(&mut self, method: Method, body: Vec<u8>) -> impl Future<Output = Result<()>>;
    /// Success requires the complete Progress stream and successful final status.
    fn commit(&mut self, body: Vec<u8>) -> impl Future<Output = Result<usize>>;
}
struct NativeTransport {
    grpc: tonic::client::Grpc<Channel>,
    cancel: Arc<AtomicBool>,
}
impl NativeTransport {
    async fn connect(address: &str, port: u16, cancel: Arc<AtomicBool>) -> Result<Self> {
        ensure!(port != 0, "Native port must be nonzero");
        let ip = crate::device::address(address)?.ip();
        let channel = Endpoint::from_shared(format!("http://{}", SocketAddr::new(ip, port)))?
            .connect_timeout(Duration::from_secs(2))
            .connect()
            .await
            .context("Connecting experimental transfer channel")?;
        Ok(Self {
            grpc: tonic::client::Grpc::new(channel)
                .max_encoding_message_size(MAX_BODY)
                .max_decoding_message_size(MAX_BODY),
            cancel,
        })
    }
    async fn read_slot(&mut self, index: u32) -> Result<SlotIdentity> {
        ensure!(index < 16, "Slot index exceeds inspected bounds");
        self.grpc.ready().await?;
        let mut request = Request::new(Bytes::from(index_body(index)));
        request.set_timeout(Duration::from_secs(3));
        let response = self
            .grpc
            .unary(
                request,
                tonic::codegen::http::uri::PathAndQuery::from_static(
                    "/dio.endpoint.EndpointDevice/GetSlotInfo",
                ),
                nad_v2::RawCodec,
            )
            .await?;
        final_status(
            response
                .metadata()
                .get("grpc-status")
                .map(|v| v.to_str())
                .transpose()?,
            None,
            1,
        )?;
        let identity = slot_identity(&response.into_inner())?;
        ensure!(
            identity.index == index,
            "Slot metadata index differs from requested index"
        );
        Ok(identity)
    }
}
impl Transport for NativeTransport {
    async fn unary(&mut self, method: Method, body: Vec<u8>) -> Result<()> {
        ensure!(
            method != Method::Commit,
            "Commit requires server-streaming transport"
        );
        self.grpc.ready().await?;
        // Readiness can resume between the outer cancellation polls. Recheck
        // immediately before dispatch so a stopped transfer sends no next write.
        ensure!(
            !self.cancel.load(Ordering::Acquire),
            "Transfer cancelled before dispatch"
        );
        let mut request = Request::new(Bytes::from(body));
        request.set_timeout(Duration::from_secs(4));
        let response = self
            .grpc
            .unary(request, method.path(), nad_v2::RawCodec)
            .await?;
        // Unary completion includes final gRPC-status verification by Tonic.
        final_status(
            response
                .metadata()
                .get("grpc-status")
                .map(|v| v.to_str())
                .transpose()?,
            None,
            1,
        )?;
        ensure!(
            response.into_inner().is_empty(),
            "Expected empty Void response"
        );
        Ok(())
    }
    async fn commit(&mut self, body: Vec<u8>) -> Result<usize> {
        self.grpc.ready().await?;
        // Readiness can resume between the outer cancellation polls. Recheck
        // immediately before dispatch so a stopped transfer sends no next write.
        ensure!(
            !self.cancel.load(Ordering::Acquire),
            "Transfer cancelled before dispatch"
        );
        let mut request = Request::new(Bytes::from(body));
        request.set_timeout(Duration::from_secs(30));
        let response = self
            .grpc
            .server_streaming(request, Method::Commit.path(), nad_v2::RawCodec)
            .await?;
        let initial_status = response
            .metadata()
            .get("grpc-status")
            .map(|v| v.to_str().map(str::to_owned))
            .transpose()?;
        let mut stream = response.into_inner();
        let mut count = 0;
        let mut bytes = 0usize;
        while let Some(body) = stream.message().await? {
            count += 1;
            bytes = bytes
                .checked_add(body.len())
                .context("Progress byte overflow")?;
            ensure!(
                count <= MAX_PROGRESS && bytes <= MAX_BODY,
                "Commit progress stream exceeds bounds"
            );
            validate_progress(&body)?;
        }
        // Tonic0.12 may tolerate HTTP200 without grpc-status. Require explicit
        // success, including legitimate zero-message trailers-only responses.
        let trailers = stream.trailers().await?;
        let status = trailers
            .as_ref()
            .and_then(|m| m.get("grpc-status"))
            .map(|v| v.to_str())
            .transpose()?;
        final_status(status, initial_status.as_deref(), count)?;
        Ok(count)
    }
}

async fn bounded<F: Future<Output = Result<T>>, T>(
    future: F,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<T> {
    let mut future = std::pin::pin!(future);
    loop {
        ensure!(!cancel.load(Ordering::Acquire), "Transfer cancelled");
        let remaining = deadline.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "Transfer deadline exceeded");
        match tokio::time::timeout(remaining.min(Duration::from_millis(50)), future.as_mut()).await
        {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}
fn index_body(index: u32) -> Vec<u8> {
    if index == 0 {
        vec![]
    } else {
        vec![8, index as u8]
    }
}

/// Explicit experimental upload entry. Calling it writes a new filter candidate,
/// although it never explicitly activates, changes volume or sets operation mode.
/// The caller must apply the user's current hardware-validation/no-sound hold.
pub fn upload_candidate(
    address: &str,
    port: u16,
    session: &Session,
    slot: u32,
    name: &str,
    cancel: Arc<AtomicBool>,
) -> Result<UploadReceipt> {
    let mut state = State::default();
    let outcome = (|| -> Result<UploadReceipt> {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Transfer cancelled before inspection"
        );
        ensure!(
            slot == 1 || slot == 2,
            "Only public candidate slots 1 or 2 are supported"
        );
        session.validate()?;
        let metadata =
            nad_v2::inspect(address, port).context("Fresh read-only device inspection")?;
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Transfer cancelled before design"
        );
        let package = nad_transfer::prepare_session_package(session, &metadata, slot, name)?;
        validate_plan(&package)?;
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Transfer cancelled before connection"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let mut transport=bounded(NativeTransport::connect(address,port,cancel.clone()),&cancel,Instant::now()+Duration::from_secs(5)).await?;
            let snapshot_deadline=Instant::now()+Duration::from_secs(30);
            let mut preserved=Vec::new();
            for entry in metadata.slots.iter().filter(|s|!s.name.is_empty()) {
                let identity=bounded(transport.read_slot(entry.index),&cancel,snapshot_deadline).await?;
                ensure!(identity.name==entry.name && identity.family==entry.filter_type_name,"Named slot metadata changed during preparation; transfer not started");
                ensure!(identity.uuid.is_some(),"Named slot identity unavailable; transfer not started");
                preserved.push(identity);
            }
            let candidate=bounded(transport.read_slot(slot),&cancel,snapshot_deadline).await?;
            ensure!(candidate.name.is_empty() && (candidate.inputs,candidate.outputs,candidate.sections)==(2,2,1),"Candidate slot became occupied or changed topology; transfer not started");
            let progress=execute(&mut transport,&package,&cancel,&mut state).await?;
            // After commit acknowledgment, perform read-only verification even if
            // cancellation was subsequently requested; no further write is sent.
            let verify_cancel=AtomicBool::new(false);
            let verify_deadline=Instant::now()+Duration::from_secs(30);
            let installed=bounded(transport.read_slot(slot),&verify_cancel,verify_deadline).await?;
            let expected=slot_identity(package.calls[0].payload.as_ref().context("Missing Begin candidate")?)?;
            confirm_identity(&installed,&expected).context("Commit acknowledged but installed slot identity/name/topology differs from candidate")?;
            for before in &preserved {
                let after=bounded(transport.read_slot(before.index),&verify_cancel,verify_deadline).await?;
                confirm_identity(&after,before).with_context(||format!("Commit acknowledged but pre-existing named slot {} identity metadata changed",before.index))?;
            }
            Ok(UploadReceipt {slot,name:package.name.clone(),unary_acknowledged:state.unary,progress_messages:progress,commit_acknowledged:true,slot_metadata_verified:true,existing_named_slots_metadata_unchanged:true,acoustic_response_verified:false,hardware_validated:false,explicitly_activated:false,commit_activation_behavior_verified:false,
                note:"Commit status and slot metadata acknowledged only; coefficients and acoustic response are unverified. No explicit activation was sent. Firmware activation behavior on commit is unknown.".into()})
        })
    })();
    outcome.map_err(|e| state.failure(e))
}

async fn execute<T: Transport>(
    transport: &mut T,
    package: &TransferPackage,
    cancel: &AtomicBool,
    state: &mut State,
) -> Result<usize> {
    let methods = validate_plan(package)?;
    let overall = Instant::now() + Duration::from_secs(90);
    for (method, call) in methods.into_iter().zip(&package.calls) {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Transfer cancelled between requests"
        );
        ensure!(
            Instant::now() < overall,
            "Transfer overall deadline exceeded"
        );
        let body = call.payload.clone().context("Missing request body")?;
        if method == Method::Commit {
            state.commit = true;
            let count = bounded(
                transport.commit(body),
                cancel,
                overall.min(Instant::now() + Duration::from_secs(30)),
            )
            .await?;
            state.acknowledged = true;
            return Ok(count);
        }
        if method == Method::Begin {
            state.begin = true;
        }
        bounded(
            transport.unary(method, body),
            cancel,
            overall.min(Instant::now() + Duration::from_secs(4)),
        )
        .await?;
        state.unary += 1;
    }
    bail!("Transfer plan did not reach commit")
}

fn validate_plan(package: &TransferPackage) -> Result<Vec<Method>> {
    ensure!(
        package.slot == 1 || package.slot == 2,
        "Invalid public candidate slot"
    );
    ensure!(
        package.calls.len() == 31 && package.calls.len() <= 32,
        "Expected exactly 31 typed transfer calls"
    );
    ensure!(
        !package.includes_activation,
        "Activation is not permitted in candidate upload"
    );
    let mut methods = vec![Method::Begin];
    methods.extend([Method::Common; 3]);
    for _ in 0..12 {
        methods.extend([Method::Select, Method::Coefficient]);
    }
    methods.extend([Method::Gain, Method::Gain, Method::Commit]);
    let mut total = 0usize;
    for (method, call) in methods.iter().zip(&package.calls) {
        ensure!(
            call.method == format!("{PREFIX}{}", method.name())
                && call.request_type == method.request_type(),
            "Transfer method/type/sequence outside strict whitelist"
        );
        ensure!(
            matches!(
                (method, &call.response),
                (Method::Commit, ResponseKind::ServerStreamingProgress)
            ) || (*method != Method::Commit && matches!(call.response, ResponseKind::UnaryVoid)),
            "Transfer response contract differs"
        );
        let body = call
            .payload
            .as_ref()
            .context("Unresolved transfer payload")?;
        ensure!(body.len() <= MAX_BODY, "Transfer request exceeds 1 MiB");
        fields(body)?;
        total = total
            .checked_add(body.len())
            .context("Transfer byte count overflow")?;
    }
    ensure!(
        total <= MAX_BODY && total == package.total_payload_bytes,
        "Aggregate transfer body bound/count differs"
    );
    let begin = slot_identity(package.calls[0].payload.as_ref().unwrap())?;
    let uuid = uuid::Uuid::parse_str(&package.unique_id)?;
    let bits = uuid.as_u128();
    ensure!(
        begin.index == package.slot
            && begin.name == package.name
            && begin.family == package.filter_family
            && begin.time == u64::try_from(package.creation_time_ms)?
            && begin.uuid == Some((bits as u64, (bits >> 64) as u64))
            && (begin.inputs, begin.outputs, begin.sections) == (2, 2, 1),
        "Begin metadata differs from fresh package"
    );
    for (i, variant) in [0u64, 2, 4].into_iter().enumerate() {
        ensure!(
            variant_field(package.calls[1 + i].payload.as_ref().unwrap())? == variant,
            "Common coefficient variant differs"
        );
        for (j, (input, output)) in [(0, 0), (0, 1), (1, 0), (1, 1)].into_iter().enumerate() {
            let select = 4 + i * 8 + j * 2;
            let mut expected = Vec::new();
            if input != 0 {
                expected.extend([8, input]);
            }
            if output != 0 {
                expected.extend([16, output]);
            }
            ensure!(
                package.calls[select].payload.as_ref().unwrap() == &expected,
                "Cell selection differs from prescribed matrix order"
            );
            ensure!(
                variant_field(package.calls[select + 1].payload.as_ref().unwrap())?
                    == variant + u64::from(input != output),
                "Cell coefficient variant differs"
            );
        }
    }
    ensure!(
        package.calls[28].payload.as_ref().unwrap().is_empty()
            && package.calls[29].payload.as_ref().unwrap() == &vec![8, 1],
        "Input gain/delay must remain explicit zero"
    );
    ensure!(
        package.calls[30].payload.as_ref().unwrap() == &index_body(package.slot),
        "Commit slot differs"
    );
    Ok(methods)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlotIdentity {
    index: u32,
    name: String,
    family: String,
    inputs: u32,
    outputs: u32,
    sections: u32,
    time: u64,
    uuid: Option<(u64, u64)>,
}
enum Value<'a> {
    Varint(u64),
    Fixed32([u8; 4]),
    Fixed64,
    Bytes(&'a [u8]),
}
struct Field<'a> {
    number: u32,
    value: Value<'a>,
}
fn read_varint(bytes: &[u8], position: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*position).context("Truncated protobuf varint")?;
        *position += 1;
        ensure!(shift < 63 || byte <= 1, "Protobuf varint overflow");
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    bail!("Invalid protobuf varint")
}
fn fields(bytes: &[u8]) -> Result<Vec<Field<'_>>> {
    ensure!(bytes.len() <= MAX_BODY, "Protobuf body exceeds 1 MiB");
    let mut p = 0;
    let mut result = Vec::new();
    while p < bytes.len() {
        ensure!(result.len() < 256, "Protobuf has too many fields");
        let tag = read_varint(bytes, &mut p)?;
        let number = u32::try_from(tag >> 3)?;
        ensure!(
            number > 0 && number <= 0x1fffffff,
            "Invalid protobuf field number"
        );
        let value = match tag & 7 {
            0 => Value::Varint(read_varint(bytes, &mut p)?),
            1 => {
                let end = p.checked_add(8).context("Field overflow")?;
                ensure!(end <= bytes.len(), "Truncated fixed64");
                p = end;
                Value::Fixed64
            }
            2 => {
                let len = usize::try_from(read_varint(bytes, &mut p)?)?;
                let end = p.checked_add(len).context("Field overflow")?;
                ensure!(end <= bytes.len(), "Truncated protobuf bytes");
                let value = &bytes[p..end];
                p = end;
                Value::Bytes(value)
            }
            5 => {
                let end = p.checked_add(4).context("Field overflow")?;
                let v: [u8; 4] = bytes.get(p..end).context("Truncated fixed32")?.try_into()?;
                p = end;
                Value::Fixed32(v)
            }
            _ => bail!("Unsupported protobuf wire type"),
        };
        result.push(Field { number, value });
    }
    Ok(result)
}
fn one<'a, 'b>(fields: &'b [Field<'a>], number: u32) -> Result<Option<&'b Value<'a>>> {
    let mut values = fields.iter().filter(|f| f.number == number);
    let first = values.next();
    ensure!(values.next().is_none(), "Duplicate singular protobuf field");
    Ok(first.map(|f| &f.value))
}
fn integer(fields: &[Field<'_>], number: u32) -> Result<u64> {
    match one(fields, number)? {
        None => Ok(0),
        Some(Value::Varint(v)) => Ok(*v),
        _ => bail!("Wrong integer wire type"),
    }
}
fn nested<'a>(fields: &[Field<'a>], number: u32) -> Result<Option<&'a [u8]>> {
    match one(fields, number)? {
        None => Ok(None),
        Some(Value::Bytes(v)) => Ok(Some(v)),
        _ => bail!("Wrong nested wire type"),
    }
}
fn text(fields: &[Field<'_>], number: u32) -> Result<String> {
    let value = nested(fields, number)?.unwrap_or(&[]);
    ensure!(value.len() <= 4096, "Slot string exceeds bound");
    Ok(std::str::from_utf8(value)?.into())
}
fn slot_identity(bytes: &[u8]) -> Result<SlotIdentity> {
    let f = fields(bytes)?;
    let index = u32::try_from(integer(&f, 1)?)?;
    let inputs = u32::try_from(integer(&f, 4)?)?;
    let outputs = u32::try_from(integer(&f, 5)?)?;
    let sections = u32::try_from(integer(&f, 9)?)?;
    ensure!(
        index < 16 && inputs <= 64 && outputs <= 64 && sections <= 16,
        "Slot fields exceed inspected metadata bounds"
    );
    let time = integer(&f, 10)?;
    ensure!(
        time <= i64::MAX as u64,
        "Negative/overflowing creation time"
    );
    let uuid = if let Some(raw) = nested(&f, 11)? {
        let f = fields(raw)?;
        let pair = (integer(&f, 1)?, integer(&f, 2)?);
        if pair == (0, 0) {
            None
        } else {
            Some(pair)
        }
    } else {
        None
    };
    Ok(SlotIdentity {
        index,
        name: text(&f, 2)?,
        family: text(&f, 3)?,
        inputs,
        outputs,
        sections,
        time,
        uuid,
    })
}
fn confirm_identity(actual: &SlotIdentity, expected: &SlotIdentity) -> Result<()> {
    ensure!(
        actual == expected,
        "Slot identity metadata differs at index {}",
        expected.index
    );
    Ok(())
}
fn variant_field(bytes: &[u8]) -> Result<u64> {
    let f = fields(bytes)?;
    let info = nested(&f, 1)?.context("Missing DRFIRInfo")?;
    integer(&fields(info)?, 1)
}
fn validate_progress(bytes: &[u8]) -> Result<()> {
    let f = fields(bytes)?;
    let progress = match one(&f, 1)? {
        None => 0.,
        Some(Value::Fixed32(v)) => f32::from_le_bytes(*v),
        _ => bail!("Wrong Progress wire type"),
    };
    ensure!(
        progress.is_finite() && (0. ..=1.).contains(&progress),
        "Nonfinite/out-of-range commit progress"
    );
    text(&f, 2)?;
    Ok(())
}
fn final_status(trailer: Option<&str>, initial: Option<&str>, messages: usize) -> Result<()> {
    let status = trailer
        .or(if messages == 0 { initial } else { None })
        .context("Terminal grpc-status missing; RPC completion is unconfirmed")?;
    ensure!(
        status == "0",
        "Terminal grpc-status was not success: {status}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nad_transfer::TransferCall;
    fn vi(mut value: u64) -> Vec<u8> {
        let mut b = Vec::new();
        while value >= 128 {
            b.push(value as u8 | 128);
            value >>= 7;
        }
        b.push(value as u8);
        b
    }
    fn int(b: &mut Vec<u8>, field: u8, value: u64) {
        if value != 0 {
            b.extend(vi(u64::from(field) << 3));
            b.extend(vi(value));
        }
    }
    fn blob(b: &mut Vec<u8>, field: u8, value: &[u8]) {
        b.extend(vi((u64::from(field) << 3) | 2));
        b.extend(vi(value.len() as u64));
        b.extend(value);
    }
    fn package() -> TransferPackage {
        let name = "Open Room — fixture";
        let family = "Dirac Live";
        let time = 1_770_846_363_693i64;
        let mut begin = Vec::new();
        int(&mut begin, 1, 1);
        blob(&mut begin, 2, name.as_bytes());
        blob(&mut begin, 3, family.as_bytes());
        int(&mut begin, 4, 2);
        int(&mut begin, 5, 2);
        int(&mut begin, 9, 1);
        int(&mut begin, 10, time as u64);
        blob(&mut begin, 11, &[8, 1, 16, 2]);
        let make = |method: Method, body: Vec<u8>| TransferCall {
            method: format!("{PREFIX}{}", method.name()),
            request_type: method.request_type().into(),
            payload: Some(body),
            response: if method == Method::Commit {
                ResponseKind::ServerStreamingProgress
            } else {
                ResponseKind::UnaryVoid
            },
            note: "original synthetic protocol fixture".into(),
        };
        let info = |variant: u64| {
            let mut nested = Vec::new();
            int(&mut nested, 1, variant);
            let mut b = Vec::new();
            blob(&mut b, 1, &nested);
            b
        };
        let mut calls = vec![make(Method::Begin, begin)];
        for variant in [0, 2, 4] {
            calls.push(make(Method::Common, info(variant)));
        }
        for variant in [0, 2, 4] {
            for (input, output) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let mut selected = Vec::new();
                int(&mut selected, 1, input);
                int(&mut selected, 2, output);
                calls.push(make(Method::Select, selected));
                calls.push(make(
                    Method::Coefficient,
                    info(variant + u64::from(input != output)),
                ));
            }
        }
        calls.push(make(Method::Gain, vec![]));
        calls.push(make(Method::Gain, vec![8, 1]));
        calls.push(make(Method::Commit, vec![8, 1]));
        let total_payload_bytes = calls
            .iter()
            .map(|c| c.payload.as_ref().unwrap().len())
            .sum();
        TransferPackage {
            version: 1,
            slot: 1,
            name: name.into(),
            filter_family: family.into(),
            creation_time_ms: time,
            unique_id: uuid::Uuid::from_u128((2u128 << 64) | 1).to_string(),
            attenuation_db: 0.,
            hardware_validated: false,
            transfer_semantics_verified: false,
            ready_for_device_write: false,
            calls,
            blockers: vec!["unverified".into()],
            topology_assumptions: vec![],
            total_payload_bytes,
            includes_activation: false,
            model_summaries: vec![],
        }
    }
    struct Mock {
        seen: Vec<Method>,
        fail: Option<usize>,
        cancel_after: Option<usize>,
        cancel: Arc<AtomicBool>,
        terminal: Option<&'static str>,
        commit_cancel: bool,
    }
    impl Mock {
        fn new(cancel: Arc<AtomicBool>) -> Self {
            Self {
                seen: vec![],
                fail: None,
                cancel_after: None,
                cancel,
                terminal: Some("0"),
                commit_cancel: false,
            }
        }
    }
    impl Transport for Mock {
        async fn unary(&mut self, method: Method, _body: Vec<u8>) -> Result<()> {
            self.seen.push(method);
            if self.fail == Some(self.seen.len()) {
                bail!("Simulated unary trailer failure");
            }
            if self.cancel_after == Some(self.seen.len()) {
                self.cancel.store(true, Ordering::Release);
            }
            Ok(())
        }
        async fn commit(&mut self, _body: Vec<u8>) -> Result<usize> {
            self.seen.push(Method::Commit);
            validate_progress(&[13, 0, 0, 0, 0x3f])?;
            if self.commit_cancel {
                self.cancel.store(true, Ordering::Release);
                std::future::pending::<()>().await;
            }
            final_status(self.terminal, None, 1)?;
            Ok(1)
        }
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    #[test]
    fn simulated_success_requires_complete_exact_sequence() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut mock = Mock::new(cancel.clone());
        let mut state = State::default();
        let count = runtime()
            .block_on(execute(&mut mock, &package(), &cancel, &mut state))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(mock.seen.len(), 31);
        assert_eq!(state.unary, 30);
        assert!(state.acknowledged);
        assert_eq!(mock.seen.last(), Some(&Method::Commit));
    }
    #[test]
    fn cancellation_before_begin_and_between_requests_never_commits() {
        let cancel = Arc::new(AtomicBool::new(true));
        let mut mock = Mock::new(cancel.clone());
        let mut state = State::default();
        assert!(runtime()
            .block_on(execute(&mut mock, &package(), &cancel, &mut state))
            .is_err());
        assert!(mock.seen.is_empty());
        assert!(!state.begin);
        cancel.store(false, Ordering::Release);
        mock.cancel_after = Some(1);
        let error = runtime()
            .block_on(execute(&mut mock, &package(), &cancel, &mut state))
            .unwrap_err();
        assert_eq!(mock.seen, vec![Method::Begin]);
        assert_eq!(state.unary, 1);
        assert!(!state.commit);
        let failure = state.failure(error);
        assert!(failure.to_string().contains("partial or committed"));
    }
    #[test]
    fn transport_and_commit_status_failures_do_not_claim_acknowledgment() {
        for (fail, terminal) in [
            (Some(1), Some("0")),
            (Some(5), Some("0")),
            (None, Some("13")),
            (None, None),
        ] {
            let cancel = Arc::new(AtomicBool::new(false));
            let mut mock = Mock::new(cancel.clone());
            mock.fail = fail;
            mock.terminal = terminal;
            let mut state = State::default();
            assert!(runtime()
                .block_on(execute(&mut mock, &package(), &cancel, &mut state))
                .is_err());
            assert!(!state.acknowledged);
            assert!(state.begin);
            assert_eq!(state.commit, fail.is_none());
        }
    }
    #[test]
    fn cancellation_during_commit_and_deadline_leave_outcome_unknown() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut mock = Mock::new(cancel.clone());
        mock.commit_cancel = true;
        let mut state = State::default();
        assert!(runtime()
            .block_on(execute(&mut mock, &package(), &cancel, &mut state))
            .is_err());
        assert!(state.commit && !state.acknowledged);
        cancel.store(false, Ordering::Release);
        assert!(runtime()
            .block_on(bounded(
                std::future::pending::<Result<()>>(),
                &cancel,
                Instant::now() + Duration::from_millis(2)
            ))
            .is_err());
    }
    #[test]
    fn whitelist_and_bounds_reject_loaded_or_mutated_packet_authority() {
        let mut p = package();
        p.calls[4].method = format!("{PREFIX}SetActiveSlot");
        assert!(validate_plan(&p).is_err());
        let mut p = package();
        p.calls.swap(1, 4);
        assert!(validate_plan(&p).is_err());
        let mut p = package();
        p.calls[5].payload = Some(vec![0x0a, 2, 8, 4]);
        assert!(validate_plan(&p).is_err());
        let mut p = package();
        p.calls[29].payload = Some(vec![8, 1, 21, 0, 0, 0x80, 0x3f]);
        p.total_payload_bytes += 5;
        assert!(validate_plan(&p).is_err());
        let mut p = package();
        p.calls[0].payload = Some(vec![0; MAX_BODY + 1]);
        assert!(validate_plan(&p).is_err());
        let mut p = package();
        p.total_payload_bytes += 1;
        assert!(validate_plan(&p).is_err());
        let mut p = package();
        p.calls[30].response = ResponseKind::UnaryVoid;
        assert!(validate_plan(&p).is_err());
    }
    #[test]
    fn metadata_and_progress_parser_fail_closed_on_malformed_fields() {
        assert!(slot_identity(&[8, 16]).is_err());
        assert!(slot_identity(&[0x5a, 0]).unwrap().uuid.is_none());
        assert!(slot_identity(&[32, 65]).is_err());
        assert!(slot_identity(&[8, 1, 8, 2]).is_err());
        assert!(fields(&[0]).is_err());
        assert!(fields(&[18, 127]).is_err());
        assert!(fields(&[8, 255, 255, 255, 255, 255, 255, 255, 255, 255, 2]).is_err());
        assert!(validate_progress(&[13, 0, 0, 0xc0, 0x7f]).is_err());
        assert!(validate_progress(&[13, 0, 0, 0, 0x40]).is_err());
        assert!(validate_progress(&[18, 1, 255]).is_err());
        assert!(final_status(None, None, 0).is_err());
        assert!(final_status(None, Some("0"), 1).is_err());
        assert!(final_status(Some("13"), Some("0"), 0).is_err());
        assert!(final_status(None, Some("0"), 0).is_ok());
        let a = slot_identity(package().calls[0].payload.as_ref().unwrap()).unwrap();
        let mut b = a.clone();
        b.uuid = Some((2, 1));
        assert!(confirm_identity(&a, &a).is_ok());
        assert!(confirm_identity(&b, &a).is_err());
        let mut b = a.clone();
        b.name = "Different filter".into();
        assert!(confirm_identity(&b, &a).is_err());
    }
}
