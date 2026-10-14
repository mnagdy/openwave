# Reconstruction status

Reviewed 13 October 2026. The source implements an independently designed room-measurement and curve-editing application. It does not claim to reproduce Dirac's proprietary optimizer or constitute a fully verified replacement.

| Requested capability | Status | Evidence and remaining boundary |
| --- | --- | --- |
| Identify and connect to the NAD M10 | Observed / implemented read-only | Live BluOS status identified M10, BluOS 4.16.6 and active Dirac filtering. Log-guided direct UPnP discovery located the native version 2 service. The source contains the read-only discovery and status client. |
| Discover native filter capabilities | Observed read-only | The advertised control port accepted HTTP/2 and the exact recovered GetDeviceInfo RPC. Typed metadata reads establish the slot and dual-rate filter layout; see the transport ledger for details and internal-slot versus public-slot limits. |
| Use any Mac microphone | Built-in capture observed / other hardware unverified | Lightweight CoreAudio metadata discovery and microphone authorization are implemented. The packaged app captured 96000 samples at 48 kHz from MacBook Pro Microphone, peak −44.5 dBFS without clipping. No recording was saved by that two-second check. Other connected-device types use the same selection path but were not physically tested. |
| Measure the M10 speakers | Local and experimental native transport implemented / acoustically unverified | Native sweep dimensions, gain bounds and current mode were read live. The independently reconstructed reference and a finite network-control/capture workflow are implemented, with simulated failure/restoration tests. Native waveform formulas are corroborated by Ghidra/REA. User requested no sound, so playback, remote stop and physical measurement remain untested. Clock-drift correction is not implemented. |
| Reuse saved Dirac measurements | Observed / implemented | All five supplied version 10 projects yield explicitly named response graphs: two have 18 graphs and three have 34. Supported metadata and Ogg recordings were independently checked; the source contains no private project or recording fixture. |
| Create adjustment curves | Implemented / desktop interaction observed | Response and target curves rendered after importing 18 graphs. Session saving, target export and 4097-tap FIR generation worked through the desktop UI. Shared target editing, microphone response calibration and per-speaker averaging have numerical checks. |
| Generate independent correction filters | Implemented / numerical tests | Original magnitude-only linear-phase FIR, bounded boost/cuts and gain headroom. Synthetic delayed transfer recovery and filter-response tests pass. It is not mixed-phase Dirac correction. |
| Transfer a target through Dirac Live | Documented handoff / actual import unverified | Independently generated legacy target text matches the observed official grammar. Official NAD instructions describe loading a target into Dirac Live, calculating and exporting a filter. Actual GUI import/export has not been observed in this pass. |
| Independently transfer filters to the M10 | Experimental implementation / hardware unverified | Original dual-rate synthesis and a bounded typed 31-call transfer executor are implemented with simulated endpoint checks. Actual room graphs pass the declared filter-approximation model. Public slot mapping, firmware complement reconstruction, coefficient acceptance, commit activation and applied sound remain unverified. No hardware upload has been attempted. |
| Open-source Rust delivery | Implemented | Source, locked dependency manifest, MIT license, CLI, desktop UI and macOS bundling script. No original Dirac binaries or copied implementation are included. |

## REA evidence used

REA 6.4.0 was used through its CLI because this session did not expose a connected REA MCP server. The original Dirac Live 3.14.1 bundle was inventoried without execution. Retained evidence:

- `inspect_artifact`: `ev_a1f371426415809d70884f0f75c4ffd40de8f805689c0434babdb4b43a7937f5`
- completed inventory: `ev_7c820094b832297f9ae85d0b646e4d426fed85f043c38f70ee70df7743282229`
- graph manifest: `agm_296d2935318e5491770ef00bbfd9055e623038e15c97a4da8cd2d6ab1ec6b966`
- bundle digest: `4acb4e6faf2ad78875bc7d98d21306f6928211909a875e408eecf60e2c5c2786`

Inventory comprised 1,614 nodes and 2,116 edges. This establishes shipped-file identity/inventory, not executed behavior or original source. Complete retained JSON remains in the private investigation work directory; the source distribution contains this summary only.

The initial Hopper attempt at native REA deep analysis did not yield usable retained evidence. After correcting the Hopper launcher path, the helper imported the library but the bridge timed out before completing the request. The demo license prevented saving the automatically generated analysis document; that temporary document was discarded and closed. No original library or user analysis document was changed. No global configuration was changed and no unrelated analysis engine was installed. Focused native follow-up used ordinary static strings, embedded format descriptors and `otool`; these are separately labeled and anchored by file hashes/offsets in the relevant ledgers. Live network metadata is identified separately from those static observations.

The failed Hopper helper left its temporary resource directory intact; its CLI was stopped after the generated document was closed. Subsequent Ghidra analysis succeeded through an owned REA CLI/MCP session, using a byte-preserving arm64 extraction of the same library. Original library SHA-256 is unchanged; the derived slice is `928a853eff1699019898a3995771f32aff13a5613b0b1c99d18dca626906cc4b`. Retained sweep dossiers are `ev_2611e49c19e39e9f4cd96887e7292076f63a19048199d4484795810c486ba47d`, `ev_5bd52c6c6c77a31750e7581bac815081966aedba20674dd9920ffa1abac0f74e`, `ev_406ecfc3e925eedc12587cf9afb6677545c4de60562290eb0e65d827e00e08e1` and `ev_c9a154e5ee999e39c562589994bb949902cc31a526f899deb1601e6ff8db0295`. These are static client observations, not firmware execution or original source. The machine instruction controls the rounding interpretation where decompiler pseudocode loses it; see the protocol ledger.

Project originals were read-only. The two-second Mac microphone check succeeded. No speaker playback, amplifier-volume change, calibration session or filter-slot write has been performed. User's no-sound instruction remains in effect.
