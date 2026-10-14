# Read-only project import evidence

Observed 13 October 2026 in five user-provided `.liveproject` files and the installed Dirac Live 3.14.1 libraries. Original files were read only. This is a bounded compatibility importer, not a complete project format specification. Ordinary byte parsers and embedded protobuf descriptor inspection supplied this evidence; no REA provider Evidence IDs were produced.

## Evidence identity

| Source name | Bytes | SHA-256 | Named response graphs |
| --- | ---: | --- | ---: |
| 2026 Feb 11.liveproject | 15214090 | `7b1e95b4a86b00a59085fc0020ee5954b044bb1cbe717272ecdb9b7ac1de01d4` | 18 |
| Living Room.liveproject | 15214090 | `6918f1c3400f1f6c6624ee4cef3a38fcd461f61dfb9203ce3358f59dbd2988a3` | 18 |
| NAD M10 filter 2022.liveproject | 21076464 | `8d02682669b5b401d24607b63609896a41ad865e8c1060081d32d3edc1d47f0d` | 34 |
| aldan.liveproject | 9886735 | `364c7447c7374749278d34f04d9cb0018ca1235b03e9a6600400f18df5c722f7` | 34 |
| M10 - D266.liveproject | 9886717 | `b7263f86fef54115b9592cb11f2995aa382c6a1a91a789b3c457cff746e2c0f3` | 34 |

Source projects and personal recording payloads are not included in this application distribution. Output metadata omits internal account/device IDs and saved network addresses.

## Container prefix

All five files start with BE u32 value 10, followed by six strings encoded as BE u32 byte length and UTF-16BE characters. A length of `FFFFFFFF` denotes a null string. String values correspond to project name, producer (`Live Desktop`), two internal IDs, optional microphone friendly name, and optional previous project filename. The last field's semantics remain inferred from the observed filenames.

The tested importer requires this exact prefix, then zero u32, byte value 1, a BE u32 blob length, and blob bytes. Unsupported versions/extensions fail clearly. The older `aldan` and M10 autosave files have opaque first blobs: the importer reports header metadata and a warning, while the independent named-graph importer still works.

For `2026 Feb 11.liveproject`, string fields begin at offsets 4, 30, 58, 134, 210 and 258; the subsequent flag is at 266, blob length at 267, and blob at 271 with 5768683 bytes. `NAD M10 filter 2022.liveproject` first blob begins at 243. These are absolute offsets in the hashed files.

## Measurement protobuf

The Feb 2026 and 2022 first blobs parse fully as protobuf wire records. The installed library's embedded descriptor corroborates the layout: `commondata.LiveMeasurement` field 3 is sample rate, 5 is speaker count, 16/17 manufacturer/model, 24 recording-device friendly name, and repeated field 50 is `MeasurePosition`. Each position has field 13 `waveform_data`; its bytes identify an Ogg Vorbis recording. The Feb 2026 file supplies nine mono 48 kHz recordings, while the 2022 file supplies seventeen. Field 12 `raw_model` contains impulse-related arrays; those arrays are deliberately excluded from this importer.

Descriptor evidence is `connect/commondata/measurement.proto` at absolute library offset 12464752 in `libdc.dylib`, and at 15649568 in `libduff.dylib`; duplicate architecture copies corroborate the field names. Library hashes and broader protocol evidence are tracked in the main investigation ledger. The local Feb 2026 metadata has no stored play-device host or port fields.

`extract_recordings` preserves those Ogg payloads without decoding or altering them. It requires a new output directory and creates sequential new files. Recordings alone are not analyzed responses: acquisition timing, stimulus and processing would be needed for a fresh interpretation.

## Named response graphs

The nonopaque project tail contains explicit UTF-16 names `measurement::measuredRes_pos{position}_ch{channel}`. The importer selects only these names. Their following record is BE u32 magic `12345678`, BE u32 version 1, BE u32 x count, x values as BE f64, BE u32 y count, then y values as BE f64. All observed graphs have 2048 coordinates, x frequencies from 1 Hz through 24 kHz, in increasing order.

In the Feb 2026 project, position 0/channel 0's name is at 11149580, magic at 11149650, x count at 11149658, first x at 11149662, y count at 11166046 and first y at 11166050. Its first y is approximately -51.55 and final y -108.55. Its last named graph is position 8/channel 1 at 11708234. The corresponding first name in the 2022 file is at 16169209. Other projects use the same record layout. Graph names in old files are stored lexicographically (position 10 precedes 2), so importer results are sorted numerically.

Interpretation: the named records are saved measured-response graph coordinates, with y values used as relative dB levels. They are not calibrated sound-pressure measurements. Absolute acoustic calibration and any earlier smoothing/normalization are not established by the graph container. When used for independent filter design, retain provenance and normalize deliberately; combine responses only with an explicit averaging policy.

The parser enforces a 128 MiB file limit, bounded string/field/graph counts, matching coordinate lengths, finite levels, positive increasing frequencies and unique position/channel pairs. Synthetic tests cover correct import, truncated graph data, NaNs and malformed protobuf lengths/varints. No private project fixtures are bundled.

Validation: the Rust importer compiled and passed its three synthetic tests in an isolated harness. It imported all five real projects: 18/34/34/18/34 named graphs respectively. The supported measurement blobs exposed 9/17/9 Ogg recordings; unsupported blobs returned metadata warnings. A local test extraction of all nine Feb 2026 Ogg payloads matched each original payload's SHA-256 exactly.

## Unsupported state

An opaque second blob and unnamed filter-design state remain uninterpreted. The importer does not recover target curves from `.liveproject`, decode proprietary filter state, or treat stored curves as device-compatible correction filters. Existing exported `.txt` target curves have a separately documented import route. Recovery of saved graph data establishes neither equivalence to Dirac optimization nor an independently implemented NAD upload path.
