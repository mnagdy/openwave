# Native v2 transfer contract and remaining validation

Reviewed 13 October 2026. Static inspection of the installed client and schemas, corroborated by read-only M10 metadata. No BeginFilterTransfer, setting RPC, stimulus, commit, or other device write was sent. This report records format facts, not copied application code or a validated uploader.

## Verified device constraints

The live M10 exposes `/dio.endpoint.EndpointDevice` over cleartext HTTP/2/gRPC on its advertised port 5000. Section 0 has two inputs and two outputs and reports LIVE disposition. Its diagonal cells `(0,0)` and `(1,1)` report dual-rate variants 0/2/4. Cross cells `(0,1)` and `(1,0)` report variants 1/3/5.

| Variants | Sample rate | High taps | Low taps | Subsample | Cascades | Reported high-delay range |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| 0 / 1 | 32000 Hz | 1024 / 0 | 1024 | 4 | 2 | 0–638 / 0–0 |
| 2 / 3 | 44100 Hz | 1024 / 0 | 1024 | 4 | 2 | 0–759 / 0–0 |
| 4 / 5 | 48000 Hz | 1024 / 0 | 1024 | 4 | 2 | 0–798 / 0–0 |

`allow_individual_delays` was absent and therefore false under proto3 defaults. Its precise processing semantics were not independently observed. Five internal slot entries were reported; this does not establish five user-selectable or safely writable slots. One entry contains the user's existing named filter, whose coefficients and identifiers are not included in distributed source or this report.

## RPC contracts

All paths have prefix `/dio.endpoint.EndpointDevice/`. Contracts are recovered from generated RPC type identities, descriptor fields, and native stub construction. Read methods have been verified live; the write contracts below remain static observations.

| Method | Request | Response/streaming |
| --- | --- | --- |
| BeginFilterTransfer | SlotInfo | Unary Void |
| SetTransferCell | CellIndex | Unary Void |
| SetCommonDRFIR | DRFIRCOMMON | Unary Void |
| SetCellDRFIR | DRFIR | Unary Void |
| SetInputGainDelay | GainDelay | Unary Void |
| SetFilterChannelNames | FilterChannelInfo | Unary Void |
| SetFilterDesignInfo | FilterDesignInfo | Unary Void |
| EndFilterTransfer | Index | Server stream of Progress; final gRPC status must succeed |
| SendFilterData | Client stream of DataChunk | Unary Void; alternate installed transfer path |

The installed `FilterTransferImpl` constructor invokes BeginFilterTransfer with SlotInfo. Its DRFIR setter first selects the current CellIndex, sends SetTransferCell if input/output/section differ from its cached index, then sends SetCellDRFIR. Each unary status is checked. Its commit method sends EndFilterTransfer with the selected slot index, consumes Progress messages, and checks the terminal status before marking the transfer committed. This establishes **begin → cell selection → coefficient set → commit**, not the complete hardware-side transaction or rollback behavior.

The installed exporter also has an alternate “streaming RPC” path and a legacy typed transfer path. It supports common coefficient, channel-name, gain/delay, design-delay, and crossover setters. Exact global ordering and which common/resample coefficients are mandatory on this firmware remain partially recovered. In particular, source presence of SetCommonDRFIR does not establish that a zero or omitted common filter is accepted. Begin's effects before commit and cancellation/rollback behavior have not been observed. No DeleteSlot or SetActiveSlot action is part of the independent read adapter.

## Protobuf data contract

All messages below are package `dio.endpoint`. These descriptors use proto3: omitted scalar values decode as zero/false/empty, but that does not make a missing field semantically valid for a device write. Numbers are protobuf field numbers. `int32` uses varint wire encoding; `float` uses little-endian IEEE-754 fixed32; repeated float arrays may use packed fixed32 encoding. Indices are zero-based in observed metadata requests.

| Message | Fields |
| --- | --- |
| Index | 1 i:int32 |
| CellIndex | 1 input_idx:int32; 2 output_idx:int32; 3 section_idx:int32 |
| SlotInfo | 1 slot_index:int32; 2 name:string; 3 filter_type_name:string; 4 num_inputs:int32; 5 num_outputs:int32; 6 max_delay:int32; 7 min_gain:float; 8 max_gain:float; 9 num_sections:int32; 10 creation_time:int64; 11 unique_id:UUID; 12 description:string; 13 sub_sampling_factor:repeated int32; 15 slot_metadata:SlotMetadata |
| UUID | 1 low:uint64; 2 high:uint64 |
| SlotMetadata | 1 endpoint_manufacturer:string; 2 endpoint_model:string; 3 endpoint_uuid:UUID; 4 required_capabilities:repeated int64; 5 extra:map<string,string>; 6 is_trial_filter:bool. Field names are schema facts; no corresponding policy behavior was investigated. |
| DRFIRInfo | 1 variant:int32; 2 samplerate:float; 3 num_fir_taps_hi:int32; 4 num_fir_taps_lo:int32; 5 sub_sampling_factor:int32; 6 cascade_number:int32; 7 delay_hi_min:int32; 8 delay_hi_max:int32; 9 allow_individual_delays:bool |
| FIRInfo | 1 variant:int32; 2 samplerate:float; 3 num_fir_taps:int32 |
| FIR | 1 info:FIRInfo; 2 fir_taps:repeated float |
| DRFIR | 1 info:DRFIRInfo; 2 fir_hi:FIR; 3 fir_lo:FIR; 4 delay_hi:int32; 5 max_gain:float |
| DRFIRCOMMON | 1 info:DRFIRInfo; 2 low_pass:FIR; 3 section_index:int32 |
| GainDelay | 1 input_idx:int32; 2 gain_db:float; 3 delay_ms:float |
| FilterChannelInfo | 1 channel_index:int32; 2 name:string; 3 type:FilterChannelType enum |
| FilterDesignInfo | 1 section_index:int32; 2 filter_design_delay_ms:float |
| Progress | 1 progress:float; 2 message:string |
| DataChunk | 1 offset:uint64; 2 data:bytes |

The native DRFIR converter populates top-level DRFIRInfo and high/low FIR tap arrays, then copies delay_hi and max_gain. It does **not** populate nested `fir_hi.info` or `fir_lo.info`. The common converter similarly populates its top-level DRFIRInfo and `low_pass.fir_taps` without setting nested low_pass.info. Therefore independently supplying guessed FIRInfo values for these nested messages is unnecessary for matching this SDK path. Whether empty tap arrays or omitted common messages pass M10 validation remains unknown.

## max_gain is a dB value in the legacy adapter

This is established through a complete assignment path, rather than inferred from the modern field name. The local legacy proxy takes the same incoming `DRFIR.max_gain` float, accumulates its maximum unchanged, and places it into legacy `diracio.FilterChannel.maxGain_dB` field 5. The latter field's serializer emits protobuf tag `0x2d` (field 5, fixed32) from that exact member. Thus 0 represents 0 dB and 1 represents +1 dB in this adapter, rather than a linear gain of one. The installed endpoint client also copies this float unchanged into the native request. Interpreting the API contract as dB is strongly supported; actual v2 firmware processing remains unobserved.

The legacy proxy separately scales transmitted FIR taps by approximately 0.31622776 and adds 10 dB to a legacy channel gain. This is a proxy conversion detail; it must not be automatically applied to native v2 coefficients. The independent application's original full-rate coefficients should not acquire that unexplained scaling.

## High-only independent correction remains a hypothesis

A proposed constrained design is an original 1024-tap full-rate correction in each diagonal high branch, zero low branches, and zero cross cells, with a shared supported delay and 0 dB maximum boost for a cut-only design. This would respect reported dimensions if every required branch is explicitly padded to its declared size and cross variants use zero high taps. However, dimensions alone do not establish correctness: the common low-pass/upsampling chain, branch routing, delay convention, cascade processing, maximum-gain interpretation in firmware, validation rules, and response after commit are unverified. The current 4096-tap generic FIR export is not directly compatible with the reported 1024-tap native high branch.

No installed client or metadata evidence examined here proves that the high branch is an independent full-band path when the low branch is zero, or that the firmware accepts this constrained design. The DSP implementation must model and test the complete native topology, or an authorized hardware validation must measure the resulting response. A syntactically valid protobuf and successful write status would still be insufficient to establish correct room correction.

## Reproducible static evidence

`libduff.dylib` SHA-256: `9ef617effd74e0cb9ae8a397a46fbcd23e24abbdf3f4d676ba0a01ddf6149ae0`. Its arm64 slice begins at whole-file offset `0x1234000`. Quoted virtual addresses below map to original whole-file offsets by adding that slice base for these mapped text/data ranges. `libdioproxy.dylib` SHA-256: `59ebcf67c19a2c4ea1d4dcf0e1c47184558e92e106ebb22e0965f7bfd2a05947`, arm64 slice base `0xe40000`. Ordinary `otool` observations are distinguished from REA Evidence IDs; native REA analysis did not yield a usable dossier during this pass.

| Source location | Observation |
| --- | --- |
| libduff VA 0xd99f0–0xd9a20 | RPC name-to-stub metadata offsets for BeginFilterTransfer and SetTransferCell. |
| libduff VA 0xd3240, stub 0xe74a8, method offset 0x508 | Constructor's BeginFilterTransfer call. |
| libduff VA 0xd60d8–0xd61c4, stub 0xe7a60 | CellIndex construction, cached-index comparison, and SetTransferCell. |
| libduff VA 0xd490c–0xd4980, stub 0xe9140 | Index selection followed by SetCellDRFIR. |
| libduff VA 0xd39b4–0xd3bac, stub 0xe8b88 | DRFIRCOMMON conversion and SetCommonDRFIR. |
| libduff VA 0xb95c4–0xb9850 | DRFIR scalar/tap conversion; nested FIRInfo not populated. |
| libduff VA 0xd5e40–0xd5f60, stub 0xec660, method offset 0x6e8 | EndFilterTransfer(Index), Progress consumption, terminal status, commit marker. |
| libduff VA 0xb1514–0xb154c | Status-check helper returns only for zero status; error path handles nonzero RPC status. |
| libduff VA 0x3d858–0x3d94c | Streaming/legacy exporter branch. |
| libduff VA 0x3e488–0x3e658 and callback 0x464cc–0x468d8 | Legacy per-section/rate cell traversal and typed setter dispatch, followed by per-input gain/delay. |
| libdioproxy VA 0x178a4–0x178d0 and 0x179b8–0x179c8 | DRFIR request forwarding; unchanged max_gain maximum accumulator. |
| libdioproxy VA 0x14788–0x14794 | Maximum accumulator copied into FilterChannel member offset 0x2c. |
| libdioproxy VA 0x3bfe8–0x3bff4 | That member serialized as field 5/fixed32, identified as maxGain_dB in diracio.proto. |
| libdioproxy whole-file 0x18d78a0 | filter.proto descriptor: FIR, DRFIR, DRFIRInfo, DRFIRCOMMON. |
| libdioproxy whole-file 0x18d92a0 | filter_slot_info.proto descriptor: SlotInfo. |
| libdioproxy whole-file 0x18dc710 | utility_messages.proto descriptor: Index, CellIndex, DataChunk, Progress. |
| libdioproxy whole-file 0x18d3c98 and 0x18d43d5 | Generated handler identities establish DataChunk→Void client streaming and Index→Progress server streaming. |

Raw native disassembly, schema summaries, live metadata responses, and private device summaries are retained in `work/` for this investigation. They are not packaged with the open-source application. Neither proprietary correction code nor existing filter coefficients are reproduced.
# Native dual-rate topology: evidence and conditional original design

13 October 2026. This addendum extends `docs/native-transfer.md`. It records static client observations and an original offline simulation. No device read or write occurred in this pass. It does not establish firmware execution or authorize filter upload.

## The high branch alone is not a full-band path in the client model

The installed arm64 client includes a `dualrateMIMOFilterSignal` model with common high/low preprocessing, per-cell high/low filters, and summation. The high path convolves the input with a common complementary high-pass before applying the cell's high FIR. The low path uses a common low-pass followed by factor-M decimation; cascade 2 repeats that stage, applies the low FIR, then interpolates through two low-pass stages. Therefore setting the low FIR to zero removes the modeled bass path. A 1024-tap high FIR and valid protobuf dimensions cannot establish correct full-band correction.

The effective reduction is `M^cascade`, rather than M. With the observed M10 metadata M=4 and cascade=2, the low sample rates are 2000, 2756.25 and 3000 Hz for the 32000, 44100 and 48000 Hz variants respectively. Fractional 2756.25 Hz must not be rounded to an integer for filter synthesis.

Each modeled decimation selects phase zero and multiplies the result by M. The interpolation inserts zeros and convolves with an unscaled common low-pass. Two decimations therefore provide an overall factor 16 to compensate for the two zero-insertion reductions in average amplitude. This factor belongs to the resampling topology; multiplying native cell FIR coefficients by another factor 16 would be an error.

## Timing

The cascade-2 common prototype contains 75 symmetric double coefficients; its individual group delay is 37 stage samples. The analysis/synthesis paths together introduce `2 × 37 × (1 + 4) = 370` full-rate samples. The common high-pass is constructed as a delayed unit impulse minus the phase-zero low reconstruction, so its reference impulse also belongs at sample 370. The phase-zero low impulse can have fewer stored trailing zero samples after decimation. Its vector length must not determine the reference delay.

A separate client helper returns 185 for `(subFactor=4, cascadeNo=2)`. An observed response-conversion path doubles that result before cropping. Thus 185 is not the complete analysis/synthesis delay. The corresponding single-stage helper returns 149 for `(16,1)`; its stored prototype contains 299 doubles. These facts describe this client artifact, not arbitrary endpoint implementations.

The client pads `delay_hi` zeros before high-branch reconstruction, supporting its interpretation as additional full-rate samples in that model. Actual firmware delay conventions still require verification.

## Original staged implementation

`work/staging/native_dsp.rs` is independently authored and performs no network or device I/O. It includes an original 75-tap Kaiser-windowed sinc low-pass at 0.08 cycles per stage sample with beta 9 and normalized DC. No proprietary coefficient table is copied, loaded or required by that implementation.

The initial linear-phase candidate supports original bass correction between 20 and 500 Hz. It designs a 1023-tap high FIR and a 129-tap low FIR using independent frequency sampling and Kaiser beta 6, then pads both to the reported 1024-element dimensions. The high FIR delay is 511 samples; the low FIR delay is `64 × 16 = 1024` full-rate samples. Setting high delay to 513 aligns both branches at `370 + 1024 = 1394` samples. The observed high-delay maxima 638, 759 and 798 all admit 513. This corresponds to 43.56, 31.61 and 29.04 ms at the three main rates.

All three diagonal variants 0/2/4 are produced. Their cross variants 1/3/5 use no high taps, 1024 zero low taps and no high delay. These are explicit modeled silent cross paths, not omitted unknown cells. A stereo encoder must independently design the two diagonal cells for their respective measured speakers; it must not reuse one speaker's correction for both. It must then call `align_speaker_headroom` on the speaker designs: that chooses the largest attenuation and applies extra cuts to the other speakers' two branches at every rate. This retains relative speaker level without undoing any peak bound by boosting a quieter design.

The full phase-periodic model generates impulses for all 16 input phases. It computes the main response and aggregate alias image amplitude, and rejects a design whose sampled target-shape error exceeds 1 dB or sampled alias amplitude exceeds −70 dB. All measured and target breakpoints within the correction range supplement a logarithmic frequency grid. These are sampled fidelity checks, not a proof of every continuous-frequency extremum.

Both branches and all sample rates share one conservative normalization factor. It is computed from the maximum induced time-domain peak bound over every output phase, plus a small float32 margin. This retains branch balance and makes the modeled system bounded for arbitrary normalized sample streams, rather than merely for sinusoidal inputs. It also attenuates uncorrected frequencies. The coefficient gain must not be compensated with a device input gain unless a new full peak-bound calculation justifies it. `max_gain_db=0` records a conservative 0 dB upper bound in the known dB convention.

Tests check unity reconstruction at every phase and rate, exact silent cross paths, a broad bass cut, a broad boost with shared peak headroom, speaker-level matching using extra attenuation, rejection of an unresolved narrow bass feature, supported dimensions/delays, and failure of the high-only bass path. The final isolated harness passed all 15 inherited and native tests. Its model report is retained separately; no user recordings or native filter taps appear in it.

## Firmware gap

The client sends only common `low_pass.fir_taps` in `DRFIRCOMMON`; it does not send an explicit high-pass complement. Static client analysis therefore does **not** prove that the M10 constructs its high complement from an independently supplied low-pass. The firmware may use a fixed complement, may derive one differently, or may impose undocumented common-filter validation. Replacing the common table while the device keeps a different complementary high path would create an incorrect crossover response even if all RPC statuses succeeded.

The original staged design sets `hardware_validated=false` and describes its assumptions. Before it is a usable native encoder, independent firmware evidence or an explicitly authorized bounded hardware measurement must establish common/complement derivation, resampling phases and gain, full-rate high delay, response across all supported variants, and actual commit/cancellation behavior. An offline simulation proves behavior of the declared model only.

## Static source locations

The inspected `libduff.dylib` has SHA-256 `9ef617effd74e0cb9ae8a397a46fbcd23e24abbdf3f4d676ba0a01ddf6149ae0`; arm64 slice base is whole-file offset `0x1234000`. These observations come from retained ordinary native disassembly, not a new successful REA native dossier. Retained artifact-graph identity remains REA Evidence `ev_a1f371426415809d70884f0f75c4ffd40de8f805689c0434babdb4b43a7937f5` for the installed application, distinct from these manual conclusions.

| Client virtual address | Observation |
| --- | --- |
| 0x988610–0x9886c4 | Matrix dimensional checks, cascade count 1/2 and allowed subFactor 4/16. |
| 0x98873c–0x9887b4 | Common high preprocessing followed by per-cell high FIR processing. |
| 0x9887d8–0x988820 | One-stage common low-pass, decimation, low-cell FIR and interpolation. |
| 0x988864–0x9888f0 | Two low-pass/decimation stages followed by two interpolation stages around the low-cell FIR. |
| 0x989068–0x989080 | Summation of reconstructed high and low output vectors. |
| 0x986b44–0x986c7c | Phase-zero every-Mth sample selection. |
| 0x986d68–0x986d80 and 0xb40c0c–0xb40cd0 | Decimation selection followed by scalar M multiplication, verified as `fmul`. |
| 0x986130–0x986264 / 0x9862d4–0x98654c | Zero-insertion interpolation and common filter convolution with retained length handling. |
| 0x98745c–0x9877d8 | Common low reconstruction and complement construction: negate low reconstruction, add center unit impulse. |
| 0x987a14–0x987a20 | Effective reduction computed with `pow(subFactor,cascadeNo)`. |
| 0x98a42c–0x98a4d0 | Stored prototype initialization: 299 doubles (0x958 bytes) at VA0xfa93e0 and 75 doubles (0x258 bytes) at VA0xfa9d38. Only dimensions/statistics were retained, not their coefficient values. |
| 0x989bac–0x989bf4 | Delay helper returns 149 for (16,1) and 185 for (4,2). |
| 0x69fd5c–0x69fdd4 | High coefficient vector padded by stored `delay_hi` before response conversion. |
| 0x69fec0–0x69fec8 | Helper delay explicitly doubled before response cropping. |

Private summary-only inspection found the stored 75-tap prototype symmetric with DC sum approximately 0.9992337063 and the 299-tap prototype symmetric with DC sum approximately 0.9992473676. These numeric observations corroborate the prototype identity and differ from the original unit-DC prototype. They are not coefficients for redistribution or for the new implementation.
## Original offline package implementation

Added 13 October 2026. `src/nad_transfer.rs` implements `build_package(&NativeDevice, &[NativeDesign], slot, name) -> Result<TransferPackage>`. It builds protobuf request bodies and a bounded proposed call sequence. It contains no network, file, playback, device-setting, or activation operation. System time and OS entropy provide a new independent filter identity. This is an inspectable candidate for later validation, not an observed successful upload.

The input must contain exactly two speaker designs and complete read metadata for the inspected two-speaker NAD M10. Only an empty public candidate slot 1 or 2 is permitted; a named slot and internal slots 3/4 are rejected. The builder validates the 2x2 LIVE section, four cells, six distinct variants, actual sample rates, advertised high/low tap counts, subsampling/cascade dimensions, high-delay ranges, and shared-delay restriction. It validates finite coefficients, a 75-tap unit-DC common prototype identical between speakers, explicit zero cross paths, and exactly 0 dB `max_gain`. The supplied conditional DSP checks must retain the existing limits: shape error at most 1 dB, crossover phase error at most 5 degrees, alias amplitude at most -70 dB and peak bound at most one. These are model checks, not measured firmware behavior. The package does not independently recalculate acoustic fidelity from the coefficients.

`align_speaker_headroom` runs on clones of both input designs before serialization. The resulting shared attenuation remains in the transmitted coefficients. The builder adds no compensation gain, does not change caller inputs and includes no activation RPC. Each request and the aggregate plan are capped at 1 MiB; at most 32 calls are allowed. `hardware_validated`, `transfer_semantics_verified`, `ready_for_device_write`, and `includes_activation` remain false.

### Concrete SlotInfo candidate

The Begin request contains the selected slot index; independent display name `Open Room — <name>`; a single nonempty compatible filter-family string observed in device slot metadata; 2 inputs; 2 outputs; 1 section; current Unix epoch milliseconds; and a fresh random UUID. The inspected existing family string was `Dirac Live`. This family value identifies the interface format used for compatibility; the independent filter name and coefficients identify Open Room's original implementation. No existing filter UUID is reused. The summary read `work/m10-grpc-slot0-summary.json` reports `creation_time=1770846363693`, corroborating milliseconds rather than epoch seconds. The new UUID is independently mapped into the schema's two uint64 members, low then high, and serialized as varints.

The candidate omits `max_delay`, `min_gain`, `max_gain` and unknown optional description, subsampling-list and SlotMetadata members. It does not invent required capabilities, extra map entries, endpoint identity or trial/license policy. Proto3 allows these fields to be absent on the wire, but firmware acceptance of this particular metadata remains unverified. There is no evidence that these ordinary metadata fields are privileged. An absent or conflicting observed family causes a build error instead of selecting a guessed value.

### Proposed call plan

| Calls | Method and request | State of evidence |
| --- | --- | --- |
| 1 | `BeginFilterTransfer(SlotInfo)` | Concrete offline candidate; no device acceptance test. |
| 3 | `SetCommonDRFIR(DRFIRCOMMON)` for diagonal variants 0, 2, 4 | Original prototype and typed serializer; exact global ordering/firmware common semantics unverified. |
| 24 | For each rate and all four cells: `SetTransferCell(CellIndex)` then `SetCellDRFIR(DRFIR)` | Cell selection before coefficient set corroborated by the installed typed client; the complete plan remains proposed. |
| 1 | `EndFilterTransfer(Index)` | Server-streaming Progress marker; the executor consumes the stream and requires successful terminal gRPC status. No commit was executed. |

The 31-call plan explicitly includes both zero off-diagonal paths and two SetInputGainDelay messages setting the new filter's input gains and added delays to zero. DRFIR and DRFIRCOMMON contain the advertised top-level DRFIRInfo. FIR messages contain packed little-endian IEEE-754 float32 taps in field 2; nested FIRInfo is omitted, matching the recovered SDK converters. Empty high FIR messages for cross variants are explicit, while their 1024 low taps are explicit zeros. Section/index defaults and exactly 0 dB max gain are omitted scalar zeros, matching proto3 encoding. No DeleteSlot or explicit SetActiveSlot request is included; commit-induced activation remains unknown.

Remaining hardware questions are acceptance of this SlotInfo candidate; complete common/cell ordering and required optional setters; effects before commit; interruption/rollback behavior; the firmware's complement and resampling phases/gain; and actual acoustic response. These gaps keep the package unavailable for device writes. The user's no-sound constraint remains in force.

### Retained static corroboration

The original installed universal libduff hash is `9ef617effd74e0cb9ae8a397a46fbcd23e24abbdf3f4d676ba0a01ddf6149ae0`; the immutable arm64 analysis slice hash is `928a853eff1699019898a3995771f32aff13a5613b0b1c99d18dca626906cc4b`. These identify privately retained analysis inputs; no proprietary binary or copied implementation is bundled with the source.

| Retained evidence | Scope and limit |
| --- | --- |
| `ev_ab20d431ac7c541329fe0aa16c7ecaf799d9cc0225f61330ee11c9c84e2c33f7`, `work/ghidra-control/slot-export-evidence.json` | Installed exporter has distinct typed and streaming paths; not firmware execution or a successful transfer. |
| `ev_76e6fd48379395d542b0cb8041336355a494d5fe2df1e6ec273e90587807356e`, `work/ghidra-control/responses/07-typed-export.evidence.json`, function VA `0x3e328` | Higher model exporter passes name/family/description/identity inputs into transfer construction and traverses rate/section coefficient state. It does not prove our complete candidate sequence is accepted. |
| `ev_a9dee790d41d73bd6d0a3dd6e16ece9094d7464debcba620e44c741ef3bf5f29`, `work/ghidra-control/responses/08-transfer-constructor.evidence.json`, function VA `0xd2f30` | Constructor creates SlotInfo and invokes BeginFilterTransfer stub `0xe74a8`; nonempty name is checked. Its higher-model metadata inputs establish ordinary field population, not license/capability policy. |

Evidence paths are relative to the privately retained investigation base. The prior descriptor and converter ledger remains the source of protobuf field numbers, nested FIRInfo omission, cell selection, commit streaming and maximum-gain units. No installed client code or firmware was executed for this static corroboration.

### Verification

Seven focused Rust tests passed in an isolated harness using the actual DSP and native metadata types. They include independently checked literal wire vectors for packed float taps, CellIndex, DRFIRInfo and SlotInfo; bounded call count and commit-stream response type; independent UUID v4 identities; rejection of occupied/internal slots, absent/conflicting family, malformed names and incomplete metadata; NaNs, tap/rate variants and advertised-delay violations; nonzero cross coefficients and mismatched common preprocessing; and shared attenuation without mutation or boost. Synthetic fixtures are original and contain no user's coefficients or identifying metadata. These tests confirm the original serializer and validation logic, not device interoperability.


## Current native design and experimental executor

The final `src/native_dsp.rs` first checks that linear candidate against a 1 dB approximation limit. The actual imported room curves exceeded the limit, so the implementation adds an original minimum-phase low branch and matches the high branch to that same complex transfer. It retains the 1 dB gate and adds a 5-degree crossover phase gate; it does not relax them to accept a poor fit. See `docs/DSP.md` for the strategy, alignment and actual-session summary. Reported filter-approximation error is distinct from modeled room-to-target residual, which can be much larger under bounded gain and correction limits.

`src/nad_upload.rs` now implements a separate experimental executor. It does not consume preview JSON or trust its readiness flags. It rereads metadata, rebuilds the package from the validated Session, and targets only a still-empty candidate 1 or 2. Fixed typed paths restrict writes to BeginFilterTransfer, SetCommonDRFIR, SetTransferCell, SetCellDRFIR, SetInputGainDelay and EndFilterTransfer. Each unary requires explicit successful gRPC status; commit consumes bounded Progress messages and requires successful final status. There is no DeleteSlot, SetActiveSlot, operation-mode, amplifier-volume or retry request.

On completion it rereads the new slot's name, family, UUID, creation time and dimensions and verifies that existing named slots' corresponding metadata is unchanged. This does not read back coefficients or verify sound. Cancellation and deadlines return acknowledged counts and explicitly report a potentially partial/committed slot; no rollback or activation semantics are claimed. The desktop exposes a separate experimental-transfer acknowledgement after preview. No transfer has been executed on the physical M10, and the user's no-sound hold remains in effect.
