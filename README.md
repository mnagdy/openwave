# OpenWave

**Shape your sound.**

Open-source room measurement and frequency-response correction, built in Rust for macOS. Import existing measurements, explore how your speakers interact with your room, shape a target curve, and export an independently designed FIR filter or a target for Dirac Live.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Language: Rust](https://img.shields.io/badge/Language-Rust-orange.svg)
![Platform: macOS](https://img.shields.io/badge/Platform-macOS-lightgrey.svg)
![Status: Experimental](https://img.shields.io/badge/Status-Experimental-yellow.svg)

> **Early prototype:** curve editing, project import, local filter design, microphone capture, and read-only NAD M10 connectivity have validation evidence. Speaker playback and direct filter transfer have **not** been validated on physical hardware. See [current status](#current-status).

OpenWave is the project name. The current source package, executable, app bundle, and session extension still use the original names: `open-room`, `OpenRoom.app`, and `.openroom.json`.

## What you can do

- **Bring in measurements:** read measured response graphs from the observed version 10 Dirac Live `.liveproject` format without modifying the original file.
- **Shape your response:** edit target points on an interactive graph, choose correction frequency limits, and set bounded boost.
- **Combine listening positions:** include or exclude captures, normalize relative levels, and average responses by power for each speaker.
- **Calibrate your microphone:** load a compatible frequency-response calibration text file for raw captures.
- **Export your work:** save sessions, export `.txt` / `.targetcurve` targets, or generate a linear-phase FIR WAV for an external convolver.
- **Explore NAD integration:** discover BluOS players, read M10 identity and native capabilities, and prepare an offline preview of an experimental bass-correction package.

The desktop app and CLI share the same measurement, session, and DSP modules. Filter design is independently authored; proprietary Dirac binaries and libraries are not needed to build it.

## Quick start

### Requirements

- macOS 12 or newer; the recorded build and desktop checks used Apple Silicon.
- A Rust toolchain compatible with `Cargo.lock`. The manifest declares Rust 1.88 as its minimum; the recorded build used Rust 1.99.
- macOS development tools. Microphone access is needed only when capturing audio; local-network access is needed to connect to a player.

### Build and explore

```sh
git clone https://github.com/mnagdy/openwave.git
cd openwave
cargo build --release --locked

# Open a synthetic demo without playing audio.
cargo run --release --locked -- demo

# Open the desktop app.
cargo run --release --locked
```

To connect to a player at an example address:

```sh
cargo run --release --locked -- --device 192.168.1.20
```

Build the macOS app bundle with:

```sh
sh scripts/build-macos.sh
open dist/OpenRoom.app
```

The script creates an app with a local ad-hoc signature. It is not a Developer ID signed or notarized release.

## Start with existing measurements

1. In **Connect**, choose **Import a saved Dirac Live project** and select a `.liveproject` file.
2. In **Shape curve**, select a speaker and review the included listening positions.
3. Adjust target points, lower and upper correction limits, and maximum boost.
4. Save your session, then choose an export:
   - **Export target curve** for use in licensed Dirac Live. Dirac calculates and transfers its own filters.
   - **Calculate FIR → Save FIR WAV** for use in an external software convolver. OpenWave does not provide real-time convolution playback.

Targets are currently shared across speakers; each speaker's filter uses its own averaged response. Included positions are normalized around 500–2000 Hz. Imported Dirac graphs retain their existing processing; an added microphone calibration applies only to raw app/WAV captures.

Import reads named response graphs, rather than proprietary filter coefficients or unidentified target state. Unsupported metadata is reported. The importer has been checked against five supplied projects; compatibility with every Dirac Live version is not established.

## Take new measurements

Select an audio input and its physical microphone channel. Use a measurement microphone and its matching manufacturer calibration file when available. A built-in microphone can capture audio, but it is not automatically calibrated for room measurement.

For the local playback route, connect a selected Mac audio output to the amplifier's analogue or optical input, using an interface or DAC if needed. A BluOS network connection by itself does not make the M10 a Mac audio output. Choose the matching amplifier source, disable existing correction and tone controls, and verify the selected output channel feeds the intended speaker.

Start with low amplifier volume. **Play sweep and capture position** emits a −30 dBFS sweep with a 0.5-second lead-in and a 2-second tail. Digital signal level does not establish acoustic SPL. Repeat around the listening seat and review each position before averaging.

The experimental **Network measurement** route uses the M10's advertised finite sweep and a Mac microphone. Capture starts before playback; the controller attempts to restore and read back the prior stimulus gain and operation mode. These lifecycle paths have simulated-endpoint tests, but physical playback, stopping, and restoration remain unverified. New network captures start excluded from averaging until reviewed.

Independent playback and microphone clocks can drift. Absolute SPL calibration, clock-drift correction, phase alignment, and uncertainty estimates are not implemented. See [audio measurement](docs/audio.md).

## Current status

| Capability | Evidence and limits |
| --- | --- |
| Desktop curve editor, session save/load, target export, and local FIR design | Exercised in the desktop workflow; numerical and format tests recorded. |
| Saved Dirac Live project import | Five supplied projects parsed; an 18-response session was loaded in the desktop. |
| macOS device enumeration and microphone capture | Devices listed and a two-second microphone-only capture completed at 48 kHz without clipping. |
| NAD M10 discovery, identity, and native metadata | Read successfully on a physical M10 without changing playback or settings. |
| Local sweep playback and acoustic response measurement | Implemented; physical speaker playback and resulting room response remain unverified. |
| M10 network measurement | Implemented and tested with simulated controls; no physical sweep has been run. |
| Native bass-filter design and package preview | Original dual-rate model, offline validation, and desktop preview; firmware behavior remains unverified. |
| Direct native filter transfer | Experimental executor and simulated lifecycle tests; no transfer has been attempted on the physical M10. |

The [13 October 2026 validation record](docs/validation.md) reports 80 passing library tests, a later rerun of six affected stimulus tests, clean all-target Clippy, and a successful release build. These results establish software checks and the listed observations; they do not establish an end-to-end acoustic correction result.

### Experimental native transfer

The native model supports correction from 20–500 Hz, up to 6 dB boost, and shared stereo peak headroom. Offline predictions distinguish filter approximation error from the room's remaining deviation from the target. A good modeled filter fit does not imply a corrected room response.

The desktop first prepares a preview without writing to the amplifier. A separate acknowledgement enables **Send experimental filter to M10**. The executor rereads capabilities and targets an empty candidate slot 1 or 2; it sends no explicit activation or deletion. **Commit may activate correction, and failure or cancellation may leave a partial slot.** The app does not claim rollback or retry automatically.

Firmware common/complement behavior, public slot mapping, coefficient acceptance, commit activation, and applied response need physical evidence. A successful acknowledgement or slot identity is not proof of the applied frequency response. See [native transfer](docs/native-transfer.md) and [DSP design](docs/DSP.md).

OpenWave does not implement Dirac's proprietary mixed-phase optimizer, Bass Control, or ART. The documented route for using a shaped target on the M10 is to export the target, load it into licensed Dirac Live, calculate there, and export using Dirac Live.

## Command-line usage

After building, the executable is `target/release/open-room`. The examples below use that path and an example player IP. Channel indexes start at zero.

```sh
# Device enumeration and read-only discovery.
target/release/open-room audio-devices
target/release/open-room discover
target/release/open-room probe 192.168.1.20
target/release/open-room native-info 192.168.1.20

# Import measurements, export a target, and design a local FIR.
target/release/open-room inspect-project "room.liveproject"
target/release/open-room import-project "room.liveproject" "room.openroom.json"
target/release/open-room export-target "room.openroom.json" "target.txt"
target/release/open-room filter "room.openroom.json" "left-fir.wav" --channel 0

# Record a microphone only, generate a reference file, or analyze saved audio.
target/release/open-room record "Microphone name" "capture.wav" --channel 0 --seconds 10
target/release/open-room sweep "reference.wav" --rate 48000 --seconds 8
target/release/open-room analyze "reference.wav" "capture.wav" "response.json"

# Prepare native reference/package files without playback or filter upload.
target/release/open-room native-reference 192.168.1.20 "native-reference.wav"
target/release/open-room native-package 192.168.1.20 "room.openroom.json" "filter-preview.json" --slot 1 --name OpenWave

# Extract original Ogg recordings into a new directory.
target/release/open-room extract-recordings "room.liveproject" "new-recordings-directory"
```

`record` does not start playback. `sweep` writes a reference WAV; it does not play it. `native-info`, `native-reference`, and `native-package` read amplifier metadata without playback or filter writes. `analyze` requires the full reference sweep, the complete recording and tail, and matching sample rates; an unrelated microphone recording will not produce a meaningful response.

The local FIR uses 4097 taps and introduces about 42.7 ms delay at 48 kHz. Requested boosts reserve overall gain headroom. FIR export is intended for a convolver, not direct import into the M10.

## Development

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

Tests cover DSP response recovery, FIR limits and gain, microphone calibration, resampling, project parsing, malformed inputs, audio-file normalization, framing, and simulated native transport and cancellation. Hardware validation is tracked separately.

| Area | Files |
| --- | --- |
| Desktop and CLI | `src/gui.rs`, `src/main.rs` |
| Sessions and project import | `src/session.rs`, `src/project_import.rs` |
| Audio and permissions | `src/audio.rs`, `src/mac_audio.rs`, `src/mac_permission.rs` |
| DSP and rate conversion | `src/dsp.rs`, `src/native_dsp.rs`, `src/resample.rs` |
| Device integration | `src/device.rs`, `src/nad_v2.rs`, `src/nad_stimulus.rs`, `src/nad_transfer.rs`, `src/nad_upload.rs` |

Contributions are welcome, especially reproducible format fixtures, numerical checks, and carefully documented hardware validation. Use synthetic fixtures in tests and keep private sessions, recordings, identifying device metadata, and proprietary artifacts out of contributions.

## Documentation

- [DSP design and limitations](docs/DSP.md)
- [Audio measurement](docs/audio.md)
- [Saved project format](docs/project-format.md)
- [Native transfer and evidence](docs/native-transfer.md)
- [Device protocol](docs/device-protocol.md)
- [Public NAD / BluOS interfaces](docs/protocol-research.md)
- [Investigation checklist](docs/investigation.md)
- [Validation record](docs/validation.md)

## License

Independently authored source is available under the [MIT License](LICENSE). Dependency notices are included in [THIRD_PARTY_LICENSES.txt](THIRD_PARTY_LICENSES.txt).

OpenWave is an independent project and is not affiliated with Dirac, NAD, or BluOS. Their names identify compatible formats or devices; trademarks remain with their respective owners. The source distribution excludes proprietary binaries, private measurement projects, and recordings.
