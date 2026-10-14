# Validation record

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
