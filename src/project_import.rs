//! Read-only import of the bounded v10 project layout observed in user-owned files.
//! Only explicit metadata, Ogg recordings, and named measured-response graphs are used.
//! Unknown blobs and proprietary filter state are never decoded or transferred.

use crate::dsp::Point;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_PROJECT: u64 = 128 * 1024 * 1024;
const MAX_GRAPH: usize = 32768;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub name: String,
    pub model: Option<String>,
    pub microphone: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<usize>,
    pub positions: Option<usize>,
    pub recordings: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectResponse {
    pub position: usize,
    pub channel: usize,
    /// Stored graph levels; these are not calibrated SPL measurements.
    pub points: Vec<Point>,
}

fn load(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path).context("open project")?;
    ensure!(
        file.metadata()?.len() <= MAX_PROJECT,
        "project exceeds 128 MiB import limit"
    );
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(MAX_PROJECT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_PROJECT,
        "project exceeds import limit"
    );
    Ok(bytes)
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(length).context("length overflow")?;
        let value = self
            .bytes
            .get(self.offset..end)
            .context("truncated project")?;
        self.offset = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into()?))
    }
    fn string(&mut self) -> Result<Option<String>> {
        let length = self.u32()?;
        if length == u32::MAX {
            return Ok(None);
        }
        ensure!(
            length <= 65536 && length % 2 == 0,
            "unsupported string length"
        );
        let words: Vec<u16> = self
            .take(length as usize)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_be_bytes([v[0], v[1]]))
            .collect();
        Ok(Some(
            String::from_utf16(&words).context("invalid UTF-16 project string")?,
        ))
    }
}

fn header(bytes: &[u8]) -> Result<(ProjectSummary, Reader<'_>)> {
    let mut r = Reader { bytes, offset: 0 };
    ensure!(
        r.u32()? == 10,
        "only the observed version 10 project layout is supported"
    );
    let name = r.string()?.context("project has no name")?;
    ensure!(
        r.string()?.as_deref() == Some("Live Desktop"),
        "unsupported project producer"
    );
    // IDs are skipped deliberately: they are not needed for acoustic import.
    r.string()?;
    r.string()?;
    let microphone = r.string()?;
    r.string()?;
    Ok((
        ProjectSummary {
            name,
            model: None,
            microphone,
            sample_rate: None,
            channels: None,
            positions: None,
            recordings: 0,
            warnings: Vec::new(),
        },
        r,
    ))
}

#[derive(Clone, Copy)]
enum Wire<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Fixed,
}
#[derive(Clone, Copy)]
struct Field<'a> {
    number: u32,
    value: Wire<'a>,
}

fn varint(r: &mut Reader<'_>) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let b = r.take(1)?[0];
        if shift == 63 {
            ensure!(b <= 1, "protobuf varint overflow");
        }
        value |= u64::from(b & 127) << shift;
        if b & 128 == 0 {
            return Ok(value);
        }
    }
    bail!("protobuf varint overflow")
}

fn fields(bytes: &[u8]) -> Result<Vec<Field<'_>>> {
    let mut r = Reader { bytes, offset: 0 };
    let mut result = Vec::new();
    while r.offset < bytes.len() {
        ensure!(result.len() < 100000, "too many project fields");
        let key = varint(&mut r)?;
        ensure!(
            key >> 3 > 0 && key >> 3 <= 0x1fff_ffff,
            "invalid protobuf field"
        );
        let value = match key & 7 {
            0 => Wire::Number(varint(&mut r)?),
            1 => {
                r.take(8)?;
                Wire::Fixed
            }
            2 => {
                let n = usize::try_from(varint(&mut r)?)?;
                Wire::Bytes(r.take(n)?)
            }
            5 => {
                r.take(4)?;
                Wire::Fixed
            }
            _ => bail!("unsupported protobuf wire type"),
        };
        result.push(Field {
            number: (key >> 3) as u32,
            value,
        });
    }
    Ok(result)
}

fn blob<'a>(r: &mut Reader<'a>) -> Result<&'a [u8]> {
    ensure!(
        r.u32()? == 0,
        "unsupported nonempty project header extension"
    );
    ensure!(r.take(1)?[0] == 1, "project has no recording metadata blob");
    let length = r.u32()? as usize;
    ensure!(length > 0, "empty project metadata blob");
    r.take(length)
}
fn number(fs: &[Field<'_>], n: u32) -> Option<u64> {
    fs.iter().find_map(|f| match f.value {
        Wire::Number(v) if f.number == n => Some(v),
        _ => None,
    })
}
fn bytes<'a>(fs: &[Field<'a>], n: u32) -> Option<&'a [u8]> {
    fs.iter().find_map(|f| match f.value {
        Wire::Bytes(v) if f.number == n => Some(v),
        _ => None,
    })
}
fn text(fs: &[Field<'_>], n: u32) -> Option<String> {
    std::str::from_utf8(bytes(fs, n)?).ok().map(str::to_owned)
}

fn recordings<'a>(fs: &[Field<'a>]) -> Result<Vec<&'a [u8]>> {
    let mut result = Vec::new();
    for f in fs.iter().filter(|f| f.number == 50) {
        let Wire::Bytes(v) = f.value else {
            bail!("invalid recording record");
        };
        let nested = fields(v)?;
        let recording = bytes(&nested, 13).context("recording lacks audio payload")?;
        // Validate an Ogg page containing a Vorbis identification packet, not an inferred PCM layout.
        ensure!(
            recording.len() >= 58 && recording.starts_with(b"OggS\0"),
            "unsupported recording container"
        );
        let segments = recording[26] as usize;
        let packet = 27usize
            .checked_add(segments)
            .context("Ogg header overflow")?;
        ensure!(
            recording.get(packet..packet + 7) == Some(b"\x01vorbis"),
            "recording is not identifiable Ogg Vorbis"
        );
        result.push(recording);
        ensure!(result.len() <= 256, "too many recordings");
    }
    Ok(result)
}

pub fn inspect(path: &Path) -> Result<ProjectSummary> {
    let data = load(path)?;
    let (mut summary, mut r) = header(&data)?;
    let parsed = (|| -> Result<()> {
        let fs = fields(blob(&mut r)?)?;
        summary.model = text(&fs, 17);
        summary.microphone = text(&fs, 24).or(summary.microphone.take());
        summary.sample_rate = number(&fs, 3).and_then(|v| u32::try_from(v).ok());
        summary.channels = number(&fs, 5).and_then(|v| usize::try_from(v).ok());
        summary.positions = Some(fs.iter().filter(|f| f.number == 50).count());
        summary.recordings = recordings(&fs)?.len();
        Ok(())
    })();
    if let Err(e) = parsed {
        summary
            .warnings
            .push(format!("Metadata only: unsupported measurement blob ({e})"));
    }
    summary.warnings.push("Stored response levels are not calibrated SPL. Proprietary filter/target state is not imported.".into());
    Ok(summary)
}

pub fn extract_recordings(path: &Path, output_dir: &Path) -> Result<Vec<PathBuf>> {
    let data = load(path)?;
    let (_, mut r) = header(&data)?;
    let fs = fields(blob(&mut r)?)?;
    let audio = recordings(&fs)?;
    ensure!(!audio.is_empty(), "no supported Ogg recordings found");
    // Require a newly created directory and never overwrite an existing user file.
    fs::create_dir(output_dir).context("select a new, nonexistent recording output directory")?;
    let mut outputs = Vec::new();
    for (i, recording) in audio.iter().enumerate() {
        let dest = output_dir.join(format!("recording-{:02}.ogg", i + 1));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&dest)?;
        file.write_all(recording)?;
        outputs.push(dest);
    }
    Ok(outputs)
}

fn response_name(name: &str) -> Option<(usize, usize)> {
    let rest = name.strip_prefix("measurement::measuredRes_pos")?;
    let (p, c) = rest.split_once("_ch")?;
    if p.is_empty() || c.is_empty() || !p.bytes().chain(c.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((p.parse().ok()?, c.parse().ok()?))
}

fn graph(r: &mut Reader<'_>) -> Result<Vec<Point>> {
    ensure!(
        r.u32()? == 0x12345678 && r.u32()? == 1,
        "unsupported measured-response graph layout"
    );
    let n = r.u32()? as usize;
    ensure!(
        (2..=MAX_GRAPH).contains(&n),
        "invalid measured-response point count"
    );
    let x = r.take(n.checked_mul(8).context("graph overflow")?)?;
    ensure!(
        r.u32()? as usize == n,
        "measured-response coordinate lengths differ"
    );
    let y = r.take(n * 8)?;
    let mut previous = 0.0;
    let mut points = Vec::with_capacity(n);
    for (fx, fy) in x.as_chunks::<8>().0.iter().zip(y.as_chunks::<8>().0.iter()) {
        let hz = f64::from_be_bytes(*fx);
        let db = f64::from_be_bytes(*fy);
        ensure!(
            hz.is_finite() && hz > previous && hz <= 384000.0,
            "invalid measured-response frequency"
        );
        ensure!(
            db.is_finite() && db.abs() <= 200.0,
            "invalid measured-response level"
        );
        points.push(Point { hz, db });
        previous = hz;
    }
    Ok(points)
}

fn responses(data: &[u8]) -> Result<Vec<ProjectResponse>> {
    let (_, h) = header(data)?;
    let prefix: Vec<u8> = "measurement::measuredRes_pos"
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    let mut result = Vec::new();
    // Search only the explicit UTF-16 graph names. Never guess unnamed target/filter arrays.
    for offset in h.offset..data.len().saturating_sub(prefix.len()) + 1 {
        if data.get(offset..offset + prefix.len()) != Some(prefix.as_slice()) || offset < 4 {
            continue;
        }
        let mut r = Reader {
            bytes: data,
            offset: offset - 4,
        };
        let name = r.string()?.context("null graph name")?;
        let (position, channel) = response_name(&name).context("invalid measured-response name")?;
        ensure!(
            position < 256 && channel < 64,
            "measured-response index outside limits"
        );
        ensure!(
            !result
                .iter()
                .any(|v: &ProjectResponse| v.position == position && v.channel == channel),
            "duplicate measured-response graph"
        );
        let points = graph(&mut r)?;
        result.push(ProjectResponse {
            position,
            channel,
            points,
        });
        ensure!(result.len() <= 256, "too many measured-response graphs");
    }
    ensure!(
        !result.is_empty(),
        "no supported named measured-response graphs found"
    );
    result.sort_by_key(|v| (v.position, v.channel));
    Ok(result)
}

pub fn import_responses(path: &Path) -> Result<Vec<ProjectResponse>> {
    responses(&load(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn qstring(out: &mut Vec<u8>, value: &str) {
        let words: Vec<u16> = value.encode_utf16().collect();
        out.extend_from_slice(&((words.len() * 2) as u32).to_be_bytes());
        for word in words {
            out.extend_from_slice(&word.to_be_bytes());
        }
    }
    fn fixture() -> Vec<u8> {
        let mut data = 10u32.to_be_bytes().to_vec();
        for name in ["Example", "Live Desktop", "id-a", "id-b", "Mic", ""] {
            qstring(&mut data, name);
        }
        qstring(&mut data, "measurement::measuredRes_pos0_ch1");
        data.extend_from_slice(&0x12345678u32.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        for values in [[20.0f64, 20000.0], [-3.0f64, -6.0]] {
            data.extend_from_slice(&2u32.to_be_bytes());
            for value in values {
                data.extend_from_slice(&value.to_be_bytes());
            }
        }
        data
    }
    #[test]
    fn imports_named_graph_and_indices() {
        let output = responses(&fixture()).unwrap();
        assert_eq!(output.len(), 1);
        assert_eq!((output[0].position, output[0].channel), (0, 1));
        assert_eq!(output[0].points[1].hz, 20000.0);
        assert_eq!(output[0].points[0].db, -3.0);
    }
    #[test]
    fn rejects_truncation_and_nan() {
        let mut data = fixture();
        data.truncate(data.len() - 1);
        assert!(responses(&data).is_err());
        let mut data = fixture();
        let len = data.len();
        data[len - 8..].copy_from_slice(&f64::NAN.to_be_bytes());
        assert!(responses(&data).is_err());
    }
    #[test]
    fn rejects_varint_overflow_and_missing_payload() {
        assert!(fields(&[0x80; 11]).is_err());
        assert!(fields(&[0x0a, 0x10, 0x01]).is_err());
    }
}
