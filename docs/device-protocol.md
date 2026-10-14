# M10 discovery and Dirac transport evidence

Reviewed 13 October 2026. This is a bounded read-only investigation of the user's installed Dirac Live libraries and the known NAD M10 at `<M10_IP>`. No playback, volume change, calibration session, filter upload, slot change, or device write was attempted.

## Finding ledger

| Question | Result | Evidence and limits |
| --- | --- | --- |
| Is the known device an M10? | Observed live metadata | The saved HTTP `/SyncStatus` response on TCP 11000 identifies brand NAD, model M10, name Living Room, and BluOS version 4.16.6. `/Status` contains `dirac=1`. This does not establish the Dirac license, proprietary transport port, or filter slot capabilities. |
| How does installed Dirac Live discover a Dirac endpoint? | Observed static discovery parser | `libupnp.dylib` contains XML matching code for `x_port` in `urn:schemas-dirac-se:diracio-1-0` and `urn:schemas-dirac-com:diracio:2`. It also matches `x_type` and `x_rest_api` in the latter namespace. These are descriptor requirements in the installed library, not values obtained from this M10. |
| What is this M10's Dirac endpoint/port? | Observed live descriptor | A bounded direct `upnp:rootdevice` search returned `LOCATION: http://<M10_IP>:8080/Device.xml`. Read-only GET of that supplied URL identified NAD/M10/Living Room and advertised Dirac v2 `X_port=5000`, `X_rest_api=8090`, `X_type=Stimuli;Filtering`. These are live descriptor values, not proof that a particular request protocol or independent FIR payload is accepted. |
| Can saved project files recover the port? | Not in inspected metadata | The project investigator reports absent `playdev_host` field 18 and `playdev_port` field 19 in the three parseable saved measurement projects. The embedded schema identifies their types as string and int32. Opaque blobs were not interpreted as port evidence. |
| Is transport framing recoverable? | Strong static evidence for installed legacy path | The installed proxy's DiracIODispatcher code constructs and parses marker bytes, big-endian length, message ID, payload hash, and payload. Sender and receiver code agree. No live device exchange verified that this M10 uses this legacy transport. |
| What transport does this M10 actually use? | Verified live read-only gRPC | A passive TCP connection to advertised port 5000 received HTTP/2 SETTINGS, WINDOW_UPDATE, and PING. The source-established `/dio.endpoint.EndpointDevice/GetDeviceInfo` request with an empty Void message returned HTTP/2 200 and final `grpc-status: 0`. Its protobuf identifies NAD/M10, two speakers, five internal API slots, and Concord component version 3.34.1. This verifies the v2 service; it does not validate device writes. |
| What native filter topology is reported? | Verified live metadata | Section 0 reports a 2x2 LIVE section. Diagonal cells support dual-rate variants 0/2/4; cross cells support 1/3/5. Rates are 32,000/44,100/48,000 Hz. Diagonal variants have 1024 high taps plus 1024 low taps; cross variants have zero high taps and 1024 low taps. Every variant reports subsampling factor 4 and cascade count 2. |
| Are filter-upload messages present? | Observed embedded schema | The installed `diracio.proto` includes `Filter`, `LoadFilter`, `FilteringState`, and transfer messages. `LoadFilter` has deviceIndex, slotIndex, and Filter metadata; Filter contains name, technology, channel count, and byte count. This establishes message shape, not filter payload compatibility or a complete upload sequence. |
| Can this application independently upload its FIR output? | Unresolved | Native discovery and read-only metadata now work in independently authored Rust. Typed coefficient schemas and parts of the installed transfer lifecycle are recovered. Acceptance of independent coefficients, shared low-pass processing, high-only behavior, commit/rollback semantics, and actual audio response remain unverified. See `native-transfer.md`. |

## Static transport observations

Ordinary macOS `otool` disassembly was used after the REA native engine was unavailable. These observations are **not REA retained Evidence IDs** and are not runtime execution. No new analysis engine was installed or launched. Original binary hashes and exact arm64 locations below make the observations reproducible.

The constructor at arm64 virtual addresses `0x22ad8`–`0x22aec` allocates three marker bytes and writes `0x0121` as a little-endian halfword followed by `0xff`: marker `21 01 ff`.

The parser at `0x23ea0`–`0x23ed8` assembles four bytes into a big-endian length. It checks `marker_length + 12 + payload_length < 32768` before accepting the frame. At `0x23df8`–`0x23e10`, it dispatches the data after marker and length, with byte count `payload_length + 8`.

The dispatcher at `0x2341c`–`0x23484` consumes eight header bytes and assembles two big-endian 32-bit words. Code at `0x234c8`–`0x234d4` assembles the first as the message ID. Code at `0x23488`–`0x234b8` checks the second against a payload-only rolling hash, starting at zero and applying `h = 31*h + unsigned_byte` modulo 2^32. Its error paths contain “No handler for message ID” and “Hash mismatch”. Sender code at `0x1a630`–`0x1a700` corroborates big-endian length, message ID, and payload hash emission.

The inferred legacy frame layout is therefore:

```text
21 01 ff | payload_length:u32be | message_id:u32be | payload_hash:u32be | payload
```

The independently authored `src/diracio.rs` implements this offline frame codec, with a hard 1 MiB decoder limit and the observed tighter 32,752-byte legacy encoder limit. It also encodes the embedded Request payload's required fields 1 (`requestType`) and 2 (`deviceIndex`) plus optional field 3 (`channel`). Nested RequestType enum values recovered from the exact descriptor are GetUnit=0, GetVolume=1, GetChannelAttenuation=2, GetActiveFilterSlot=3, GetStimulusState=4, GetFilteringState=5, GetTopologyInfo=6, GetStimulusInfo=7, and GetProtocolInfo=8. Required zero-valued fields are retained. Tests cover exact byte order, empty payloads, an independently calculated hash vector, wrapping/unsigned bytes, coalesced frames, every truncation boundary, corrupt hashes, oversized lengths, and protobuf varints. This module sends nothing and does not choose an unverified message ID.

This layout is a static reconstruction of the legacy path, not the verified M10 v2 protocol. The same installed library includes gRPC types and a local proxy-server address string `127.0.0.1:0`; those static strings alone do not identify a device endpoint. The M10's gRPC service was subsequently established through the independent live evidence below. Legacy message ID initialization hashes descriptor-associated names, but its full runtime mapping was not verified. No guessed legacy message was sent to the device.

The discovery library searches UPnP notifications for a LOCATION field and performs an HTTP GET of the supplied URL. Earlier bounded searches for Dirac v1/v2 namespaces and `ssdp:all` produced no responses; five-second `_musc._tcp` and `_musp._tcp` mDNS browses produced no service additions. A later log-guided search used the installed application's explicit-root-device discovery path and succeeded. These negative results therefore must not be used to imply absent discovery or an unavailable endpoint.

The exported `upnp_do_search_root_device_with_address` function at libupnp arm64 VA `0x4580`–`0x458c` passes the caller's address to the common discovery routine and supplies target `upnp:rootdevice`. At `0x1c3a8`–`0x1c3b4` the library constructs the destination using UDP port 1900. Request assembly at `0x1c430`–`0x1c520` emits `M-SEARCH * HTTP/1.1`, `HOST: 239.255.255.250:1900`, `MAN: "ssdp:discover"`, `MX: 4`, and the chosen ST, followed by an optional supplied USER-AGENT and a blank line.

The successful independent read-only probe bound to the routed local address `<MAC_IP>` with an ephemeral source port, sent one such `upnp:rootdevice` datagram directly to `<M10_IP>:1900`, and listened for six seconds. The known M10 responded from UDP port 1900 with the LOCATION above. Its 748-byte XML description advertises namespace `urn:schemas-dirac-com:diracio:2`. Port 5000 was then verified as HTTP/2/gRPC through read-only metadata requests. Port 8090 root returned HTTP 404 and no API self-description. A `/version_info` string was traced to a local application update file, so it is excluded as evidence of a REST endpoint.

## Verified Rust metadata adapter

`src/nad_v2.rs` exposes ten metadata reads: the original GetDeviceInfo, GetSlotInfo, GetSectionInfo, GetCellInfo, GetDRFIRInfo and the five measurement-capability reads documented below. The client uses a raw protobuf codec with Tonic, verifies final gRPC status trailers, requires a local IP address (optionally with its BluOS port), bounds messages to 1 MiB, caps an inspection at 32 requests and 30 seconds, and removes device unique identifiers from its summary. That module exposes no setting, playback, upload or slot-selection RPC. The separate experimental `nad_stimulus` module implements an explicitly triggered playback workflow, which has not been run on hardware.

One live run of this Rust adapter completed in 0.133 seconds with five internal slots, one section, four cells, six dual-rate variants, and complete coverage under its bounds. Synthetic parser tests cover proto3 defaults, packed/unpacked variant fields, invalid/truncated fields, integer overflow, floating-point validation, and identifier exclusion. Native read success does not imply upload success or license validity. The internal five-slot count is distinct from the three user-facing slots documented by NAD.

## Source identity and offsets

| Source | SHA-256 |
| --- | --- |
| `/Applications/DiracLive.app/Contents/Frameworks/libupnp.dylib` | `11658953e4882960f6c5f8d5e3c930d44a5e073f0a2b7fbd518058b77d4295fa` |
| `/Applications/DiracLive.app/Contents/Frameworks/libdioproxy.dylib` | `59ebcf67c19a2c4ea1d4dcf0e1c47184558e92e106ebb22e0965f7bfd2a05947` |
| Saved `/SyncStatus` response | `1b792495cec3c4c140f01a04a551842bcfbb8caccb9a3f3d356d1cd52a9a97f0` |
| Saved `/Status` response | `95a9305ebcd0affe2d38661e805644ddc3934f88893beafc27c65d0be1038514` |
| Live `http://<M10_IP>:8080/Device.xml` response | `4ff17b359884427f2f011185292c48afa81be130b8fa5cd9cde6fb4694d5ac72` |
| Live GetDeviceInfo gRPC response body | `13d72e5594c2cdc32ed21f157087d1a56ad796508b458a967f8c688033597b07` |

Both libraries are universal Mach-O files. The arm64 slice starts at whole-file offset `0x7c000` in libupnp, and `0xe40000` in libdioproxy. For the quoted text addresses, add the respective slice base to obtain original whole-file offsets. The arm64 slice hashes are `dc317ed79b2340bbc363ce12715e5c93ace0c634bea8a3a3f5e9b91f753a8a90` and `5a3b332a02d0b21276740a5fc678906603fdb5ee8ee5f67352acc0438ac1cb7b` respectively.

Useful original whole-file locations:

| Location | Observation |
| --- | --- |
| libupnp `0xc8d5f`, `0xc8d80`, `0xc8d87` | Dirac v1 namespace, `x_port`, Dirac v2 namespace strings. |
| libupnp arm64 VA `0x1838c`–`0x18460` and `0x18514`–`0x185e8` | XML node matching loops combine `x_port` with those namespaces. |
| libupnp `0xc8da7`, `0xc8dae` | `x_type` and `x_rest_api` strings. |
| libdioproxy `0x18c5808`, length 3654 bytes | Embedded arm64 `diracio.proto` descriptor. Duplicate x86_64 descriptor at `0xafa620`. |
| libdioproxy arm64 VA `0x229d4`–`0x229f8` | 32-bit rolling-byte-hash helper. |
| libdioproxy arm64 VA `0x22ad8`–`0x22aec` | Legacy transport marker initialization. |
| libdioproxy arm64 VA `0x23394`–`0x23970` | Dispatcher header decoding and payload-hash check. |
| libdioproxy arm64 VA `0x23d20`–`0x2411c` | Byte-stream framing/parser path. |

Raw responses, extracted descriptors, strings with whole-file offsets, and disassembly remain in the investigation's `work/` directory. Proprietary library bytes are not distributed with this application's source. These observations guide independently written interoperability work; they do not reproduce Dirac's correction algorithm.

## Evidence still required for direct transfer

Native device discovery, transport attribution, and read-only topology inspection are complete for this M10. The remaining work is the transfer/processing validation described in `native-transfer.md`. No native upload should be advertised as working before those checks establish coefficient acceptance and the resulting response. A successful target-curve import into the official application is a separate capability from independent filter upload.

## Native network stimulus contract: additional read-only evidence

On 13 October 2026, nine source-established, bounded unary metadata requests were sent to this same M10 gRPC endpoint. Each request used a three-second gRPC deadline, five-second transport deadline and 1 MiB response limit; every response had final `grpc-status: 0`. No prepare/start/setting RPC was sent, no sound played, and no calibration or transfer state changed.

The service prefix for every method below is `/dio.endpoint.EndpointDevice/`. Types and streaming direction were established from installed `libdioproxy.dylib` method strings, RTTI and stub initialization. Numeric fields come from its embedded `dio/endpoint/*.proto` descriptors. In particular, StartStimuli is **server streaming**, not a unary call.

| Method | Request → response | Direction | Live coverage |
| --- | --- | --- | --- |
| GetStimuliInfo | Void → StimuliInfo | Unary | Successful |
| GetLogSweepInfo | Variant → LogSweepInfo | Unary | Variants 0 and 1 successful |
| GetPinkNoiseInfo | Variant → PinkNoiseInfo | Unary | Variant 0 successful |
| GetStimuliGainInfo | Void → GainInfo | Unary | Successful |
| GetStimuliGain | Index → StimuliGain | Unary | Speaker indices 0 and 1 successful |
| GetOperationMode | Void → OperationMode | Unary | Successful |
| GetStreamingInfo | Void → StreamingInfo | Unary | Successful |
| SetStimuliGain | StimuliGain → Void | Unary | Schema only; not invoked |
| SetOperationMode | OperationMode → Void | Unary | Schema only; not invoked |
| PrepareStimuli | StimuliPrepare → Void | Unary | Schema/static caller only; not invoked |
| StartStimuli | StimuliStart → Progress | Server streaming | Static caller only; not invoked |
| StartRecordingStimuli | StimuliStart → AudioBuffers | Server streaming | Static caller only; not invoked |
| QueryStimuliProgress | Void → StimuliProgress | Server streaming | Schema/static stub only; not invoked |

### Observed capabilities and current state

GetStimuliInfo returns LOG_SWEEP (type 3) variants 0/1, PINK_NOISE (type 1) variant 0, and OVERLAPPED_LOG_SWEEP (type 4) variant 0. It omits all stimulus parameter submessages. `supports_override` is absent, hence false under proto3 defaults. Dedicated GetLogSweepInfo requests resolve the actual parameters; absence in the initial list does not mean the parameters are unavailable.

| Parameter | Log sweep variant 0 | Log sweep variant 1 |
| --- | ---: | ---: |
| samplerate | 48000 Hz | 48000 Hz |
| start_freq | 10 Hz | 10 Hz |
| end_freq | 24000 Hz | 300 Hz |
| length | 240000 | 24000 |
| silence | 24000 | 0 (omitted default) |
| num_periods | 1 | 1 |
| fade_out_tail_hz | 300 | 0 (omitted default) |

The sample-count interpretation yields five seconds plus half a second for variant 0 and half a second for variant 1. Installed libduff prepareStimuli at VA `0xc4acc`–`0xc4afc` fetches sweep info and calculates expected capture length as speaker count multiplied by `(silence + num_periods * length)`; the recording routine then uses that length to resize captured sample arrays. This corroborates 264000 samples per selected speaker for variant 0, matching the saved Feb 2026 LogSweepInfo. A later focused static pass recovered the local reference generator's phase/fade and sequence silence placement below; actual device-output comparison remains unperformed.

GetStimuliGainInfo reports minimum -15 dB, maximum 0 dB (omitted default), step 0.5 dB. Both speakers currently report stimulus gain 0 dB (omitted doubles). These are **stimulus gain controls**, not calibrated sound pressure or proof of the amplifier's master output level.

GetOperationMode reports gain/delay/filtering enabled, measure/streaming disabled (omitted defaults). GetStreamingInfo reports 44100 Hz and 512 buffer size, but playback/recording support flags are absent and therefore false. Native arbitrary-PCM streaming is consequently not advertised by this device. Existing general PlaybackStreaming method strings do not establish capability on this M10.

### Exact message fields for an independent implementation

All listed endpoint descriptors use proto3; omit zero/default values when encoding, but restore them explicitly in the semantic model when decoding. Preserve the distinction between missing optional **messages** and scalar defaults.

- `Variant`: int32 `v=1`; `Index`: int32 `i=1`; `Void`: empty message.
- `StimuliInfo`: repeated nested `stimuli=1`.
- `Stimulus`: enum `type=1`, int32 `variant=2`, nested MLS `=10`, periodic sweep `=11`, pink noise `=12`, log sweep `=13`, overlapped sweep `=14`, bool `supports_override=15`.
- `StimuliType`: PERIODIC_SWEEP=0, PINK_NOISE=1, MLS=2, LOG_SWEEP=3, OVERLAPPED_LOG_SWEEP=4.
- `LogSweepInfo`: int32 `variant=1`, float `samplerate=2`, float `start_freq=3`, float `end_freq=4`, int32 `length=5`, int32 `silence=6`, int32 `num_periods=7`, double `fade_out_tail_hz=8`.
- `StimuliPrepare`: nested `stimulus=1`, repeated int32 `speaker_idxs=4`, bool `stream_recording=5`, repeated int32 `speaker_offsets=6`, bool `loop_mode=7`.
- `StimuliStart`: uint64 `start_time=1`.
- `Progress`: float `progress=1`, string `message=2`.
- `StimuliProgress`: bool `stimuli_playing=1`, float `progress=2`, repeated int32 `active_channels=3`.
- `GainInfo`: double `min_value=1`, double `step_size=2`, double `max_value=3`, string `units=4`.
- `StimuliGain`: int32 `index=1`, double `gain=2`.
- `OperationMode`: bool `gain_enabled=1`, `delay_enabled=2`, `filtering_enabled=3`, `measure_enabled=4`, `streaming_enabled=5`.

For Mac-connected capture, an initial finite test should choose reported LOG_SWEEP/variant 0, one speaker index, `stream_recording=false`, `loop_mode=false`, and no overlapped offsets. Prepare selects a device-generated sequence, not an uploaded waveform. The installed client constructs a default-empty StimuliStart, so start_time=0 is statically observed; nonzero timestamps and their clock units remain unknown. This candidate request is not yet hardware-validated.

### Lifecycle and cleanup evidence

Installed `libduff` playStimuli at arm64 VA `0xc6988` opens the StartStimuli stream through the stub, consumes Progress until Read fails, then calls Finish and checks status. A timeout/cancellation monitor is initialized with 5000 at `0xc6a28` and its timer is refreshed on each progress message at `0xc6a98`–`0xc6aa4`; RTTI identifies it as `RPCTimeoutAndCancellationHandler`.

The exported `duff_dev_cancel_stimuli` at VA `0x1c144` delegates through its callable at `0x368d8` to `0x127bc`; that routine sets a local cancellation byte through `0xb5b828` and waits for completion. This is static evidence for local cancellation machinery. No explicit StopStimuli RPC exists in the complete installed EndpointDevice method table. Stream cancellation/HTTP2 reset is therefore a candidate stop mechanism, but remote stop behavior and restoration after cancellation have not been tested. Do not claim it is verified merely because the client token is understood.

The independently written lifecycle should snapshot all operation-mode flags and both stimulus gains before modifying anything, require one active acquisition owner, start Mac capture before the stimulus stream, and impose both an absolute duration bound and progress-idle deadline. On completion/cancel/error, cancel the stream, wait boundedly, query progress with a bounded cancellable read, restore any gains/mode changed by this acquisition, and read back the restored values. Restoration must preserve the complete snapshot rather than assuming normal listening flags. Unexpected concurrent device changes or failed restoration must be surfaced to the user. These are proposed safeguards, not observed device behavior.

### Reference waveform and remaining validation

No GetStimulusSamples/GetStimulusData method was found in the complete endpoint method table. StartRecordingStimuli concerns endpoint microphones, not a reference-output waveform; it is unnecessary for a Mac microphone. An independently generated exponential sine sweep can now reproduce the mathematical local-reference convention recovered below. Numeric/device equivalence still needs comparison before claiming exact deconvolution compatibility. Retain the unmodified recording and distinguish independently analyzed response from Dirac optimization.

Bounded next steps: implement/test codecs with scalar defaults, implement native metadata reads and display reported parameters, derive the fade convention from focused static stimulus-generator evidence, and add an offline fake streaming-server test for cancellation/timeout/restoration. The user explicitly instructed that sound must not be played yet: Prepare/Start/Set RPCs remain excluded from live testing. A later explicit release of that hold is required before a finite low-level playback test measures real sequencing and stop behavior. Direct filter transfer remains a separate validation task.

### Additional source identity

Installed `libduff.dylib` SHA-256: `9ef617effd74e0cb9ae8a397a46fbcd23e24abbdf3f4d676ba0a01ddf6149ae0`; its arm64 slice base is whole-file `0x1234000`. Its quoted __text VAs map by adding that slice base. Existing libdioproxy hash above remains applicable.

Relevant whole-file libdioproxy locations: stimuli descriptor `0x18da8c4` (1770 bytes); gain descriptor `0x18d9894`; operation mode `0x18d9c68`; utility messages `0x18dc710`. GetLogSweepInfo path `0x1844b11`, unary RTTI `0x18ceb28`; PrepareStimuli RTTI `0x18d0f5f`; StartStimuli streaming RTTI `0x18d1186`; QueryStimuliProgress streaming RTTI `0x18d656e`. Stub registration at arm64 VA `0x49f60`–`0x49fa4` corroborates Prepare/Start directions.

Raw response hashes (including five-byte gRPC message envelope): GetStimuliInfo `4067f50784bb0ab81f6845a18579f3f5af95ec43bd2529eb491f552dc2119e92`; GetLogSweepInfo variant0 `c4d3d080a33b2cc72b18f968625a20cd34c490073290d1575c5dedc9977b03ba`; variant1 `58b839d399ed22bcbf2434c045c57fe1e2db399cf776f37d510881afd986a5d8`; GetStimuliGainInfo `acd2d2ff03cbe33d4b4782094203acbf4ff7789056c04ac55b78aed23ee9d9dd`; GetOperationMode `11af09c486cdb47856137fd4cd2213bcce1a96fee45fb9aac16557bba11d37fb`; GetStreamingInfo `d5afd54099a7d5a95b2a4de33603605695d43d38784a346fba3711b1b23264d4`. Request scripts enforce explicit metadata-only method whitelists. No proprietary library bytes are distributed.

## Focused static log-sweep reference reconstruction

This pass read only installed binaries and retained disassembly. No application/library code was executed and no device requests were issued. Five shipped dylibs were inventoried: libupnp, librecendpoint, libdioproxy, libdc and libduff. The relevant stimulus generator is linked directly into libduff, with a retained diagnostic path ending in `dodsp/src/dodsp_stimuli.c`; a separate smaller generator library was not found within the installed Frameworks inventory.

The `log_sweep_v2` reference wrapper is identified by its sample-count diagnostic and call chain at arm64 VA `0x6d76c4`–`0x6d77d8`. It passes sample rate, sample length, starting frequency, Nyquist ending frequency, fade-tail width and a soft-start flag into the local generator initializer at `0xb602b8`, then renders exactly the requested sample count through `0xb607a4`. The generator's default configuration at VA `0xfac274` contains int32 48000, int32 240000, float32 10, float32 24000, float32 300 and two zero flags. This matches the live full-bandwidth sweep dimensions.

Retained REA/Ghidra wrapper Evidence `ev_406ecfc3e925eedc12587cf9afb6677545c4de60562290eb0e65d827e00e08e1`, entry `0x6d76c4`, corroborates that argument mapping and the initializer/render calls. Its immutable subject is the derived arm64 libduff artifact with SHA-256 `928a853eff1699019898a3995771f32aff13a5613b0b1c99d18dca626906cc4b`; original universal-library identity is recorded above. This is static analysis, not execution of library functions or device firmware.

The following mathematical description was independently derived from the small setup/render blocks; it describes standard swept-sine stimulus construction, not room-correction optimization or a copied implementation.

Let `N` be sweep samples, `fs` sample rate, `f0` start frequency, `f1` end frequency, and `fw` the fade-tail width in Hz. For the normal, non-soft-start path:

```text
L = ln(f1 / f0)
A = 2π f0 N / (fs L)
B = exp(L / (N - 1))
phase[n] = A (B^n - 1), 0 <= n < N
sweep[n] = sin(phase[n])
```

Setup at VA `0xb60594`–`0xb606a0` computes those factors. Reset at `0xb60808`–`0xb60818` initializes phase state to A; rendering subtracts A before sine evaluation, so the mathematical first sample has zero phase. The installed implementation advances a quantized fixed-point recurrence and evaluates single-precision sine. A direct f64 formula is mathematically equivalent but must not be described as bit-for-bit identical.

The fade parameter is a **width below the ending frequency**, not an absolute 300 Hz fade-start frequency. Setup at `0xb60618`–`0xb60660` subtracts fw from f1 and computes:

```text
k = round_to_nearest_away_from_zero(N * ln((f1 - fw) / f0) / L)
M = N - k
window[n] = 1                                      for n < k
window[n] = 0.5 * (1 + cos(π * (2*(n-k)+1)/(2*M+1))) for k <= n < N
reference[n] = sweep[n] * window[n]
```

The trailing cosine/Hann calculation is corroborated by the scalar render path at `0xb61108`–`0xb61150` and vector path `0xb6101c`–`0xb610e4`. For this M10's full variant, k=239612 and M=388: the final approximately 8.083 ms are tapered, beginning near 23700 Hz. Zero soft-start flags avoid the separate startup-envelope branch. This exact recovered local-reference convention improves on a generic arbitrary fade, while firmware output equivalence remains untested.

REA/Ghidra render Evidence `ev_5bd52c6c6c77a31750e7581bac815081966aedba20674dd9920ffa1abac0f74e`, function `0xb607a4`, subsequently corroborates reset of phase state at index zero, the normal-path fixed-point A/B recurrence, single-precision sine, and the stated trailing cosine window. The function also includes other stimulus and soft-start branches; the mathematical reconstruction deliberately covers only the observed normal full-bandwidth log sweep.

A subsequent REA/Ghidra analysis corroborates the setup block: Evidence `ev_2611e49c19e39e9f4cd96887e7292076f63a19048199d4484795810c486ba47d`, derived arm64 artifact SHA-256 `928a853eff1699019898a3995771f32aff13a5613b0b1c99d18dca626906cc4b`, function `0xb60570`. Its pseudocode presents the fade-index conversion as a plain integer cast, which would incorrectly suggest truncation. Both its assembly and an independent macOS disassembly decode instruction `0xb60634` as `FCVTAS w8,d0`; the original bytes are `08 00 64 1e` (instruction word `1e640008`). Arm specifies nearest rounding with ties away from zero for this instruction. Therefore the recovered formula retains **round**, not floor. For the current full profile the unrounded value is approximately 239612.12632152045, so those choices happen to produce the same k; other profiles could differ. Treat the instruction's behavior as controlling evidence when decompiler notation loses that detail. [Arm assembler reference, section 7.10](https://documentation-service.arm.com/static/5f3fa899428f7a6b3328fd44).

Silence placement is corroborated by sequence preparation at `0xb61400`–`0xb61430`: the first sequence starts at sample 0, each next sequence starts at the prior start plus N and the requested silence. Rendering at `0xb61988`–`0xb619a0` first zeros output buffers; `0xb61ac8`–`0xb61af4` renders only the finite sweep samples and marks that sequence complete. Thus the local sequence is sweep **followed by** silence, including the final silent gap. For one speaker and one period, the reference has N waveform samples followed by 24000 zeros; total 264000.

Retained REA/Ghidra sequence-builder Evidence `ev_c9a154e5ee999e39c562589994bb949902cc31a526f899deb1601e6ff8db0295`, entry `0xb6138c`, corroborates initial start zero, successive increments by waveform length plus supplied silence, and a sentinel after the selected speaker list. Its subject/profile match the wrapper analysis above. The zero-filled renderer behavior remains independently supported by assembly. These records establish the local SDK's finite sequence contract; they do not demonstrate that this M10 has played or reproduced that waveform.

Limits: this establishes the installed SDK/local-reference mathematical behavior, not measured firmware output, absolute acoustic gain, clock synchronization or correction-filter equivalence. The shorter 300 Hz variant and overlapped/soft-start forms need their own bounded reference-path validation; this derivation targets the live full-bandwidth variant. The user's no-sound hold remains in force.
