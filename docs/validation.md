# Validation record

## 14 October 2026 paired speaker measurement

Both local and M10 network measurement actions now run left then right at the same selected placement, save separate speaker responses with a shared take index, and advance only after the pair succeeds. The review selector changes neither the placement nor output routing. Local routing defaults to separate channels 1 and 2 and validates both before playing; Stop test supports both routes. Network recordings use distinct, unique filenames, retain exclusion pending review and preserve the prior per-speaker amplifier restoration flow. If the second capture fails or cancellation occurs between speakers, completed responses are saved and the guide stays in place.

95 library tests and 9 GUI tests passed, including paired ordering, matching placement, partial failure, cancellation between speakers, cancellation before audio or amplifier access, duplicate output-channel rejection, progress events, actual review-selector and stop-button interaction, legacy imports and paired position progression. The unrelated loopback SSDP test was excluded because socket binding is restricted in command execution. All-target Clippy passed with warnings denied. Actual egui paint output was generated for light/dark themes at full and minimum window sizes, and header bounds checked; no speaker playback, microphone capture or amplifier writes were performed. Physical paired capture and network playback/restoration still require hardware validation. The locked Apple Silicon release and ad-hoc app signature verification succeeded.

## 14 October 2026 discovery fix

The installed `/Applications/OpenWave.app` reproduced an empty **Find players** result. In that same running GUI, manual **Connect** to the known M10 succeeded and displayed Living Room, NAD M10, BluOS 4.16.6, active Dirac filtering and a native Dirac service. No playback, microphone recording or amplifier writes were performed.

The button previously searched only `_musc._tcp` and `_musp._tcp` Bonjour services, even though the earlier read-only investigation established UPnP root-device discovery for this M10. Discovery now searches Bonjour and UPnP concurrently, sends SSDP on each eligible local IPv4 interface, validates descriptor locations against the responder, probes BluOS identity on port 11000, deduplicates endpoints and reports search/probe errors. One failed protocol no longer prevents the other from running. Local HTTP identity reads bypass proxy configuration.

The two SSDP parsing regressions and empty-result GUI regression passed. All-target Clippy passed with warnings denied; locked release build and app signature verification succeeded. The added loopback UDP receive/deduplication test has not been run because command execution restricts sockets. The rebuilt app's automatic discovery has not been verified on the physical network. The running installed app was left open with its current session and successful manual connection; it has not been replaced. The updated bundle is in `dist/OpenWave.app`.

13 October 2026, Apple Silicon macOS, Rust 1.99, locked offline dependencies.

## Software checks

The complete library suite passed 80 tests. Following a final cancellation refinement in the native stimulus transport, all six affected lifecycle tests passed again. Clippy passed for all targets with warnings denied. Release compilation succeeded. Tests cover numerical DSP, rate conversion, calibration, file parsing, protobuf bounds, the native package, transfer sequencing/acknowledgements, deadlines, cancellation and state restoration through simulated controls. These checks do not constitute firmware or acoustic validation.

The local app bundle was rebuilt and its ad-hoc signature verified. Source distribution includes the original implementation, Cargo.lock, MIT license and dependency notices; it excludes proprietary binaries, private sessions, raw recordings, build directories and investigation evidence files.

## Observed desktop and hardware reads

- M10 identity and native metadata were read successfully without playback or writes. Explicit successful gRPC status was required.
- The desktop connected to that M10, showed its two speakers, existing filter and finite sweep profile, and bound subsequent actions to the same endpoint.
- A two-second microphone-only check captured 96,768 samples at 48 kHz without clipping. The check retained no recording file.
- The desktop loaded an existing private checkpoint with 18 imported response graphs, nine positions per speaker and 30–500 Hz correction boundaries. The checkpoint was not modified.
- Preparing the native stereo preview succeeded in the desktop, with about 9.0 dB shared digital peak headroom. Worst modeled filter-approximation errors were about 0.46 and 0.28 dB for the two speakers. Predicted room-to-target residuals were much larger (up to about 17.3 and 20.3 dB under the saved cut-only limits). The preview clearly separates these quantities.
- Both playback acknowledgements and the experimental-transfer acknowledgement remained off. No BeginFilterTransfer, coefficient setter, EndFilterTransfer, activation, deletion, volume setter or stimulus request was sent to the physical amplifier.

Local-network permission for OpenRoom was already enabled. Refreshing that same grant after replacing the ad-hoc build restored GUI connectivity. Microphone access was already enabled. Other applications' grants were unchanged.

## Required physical validation

The user requested no sound. Playback, remote stopping and actual restoration must therefore still be checked with a bounded low-volume measurement. Native firmware common/complement behavior, public slot mapping, coefficient acceptance, commit-induced activation and applied response must be established before describing the native workflow as working end to end. Upload acknowledgements and slot identity alone will not establish an acoustic result.
