# NAD M10 interface evidence

Reviewed 13 October 2026. Scope: public primary-source documentation, not a live-device test. M10 generation and firmware remain unknown. No audio was played and no device was modified.

## Finding ledger

| Question | Status | Evidence and boundary |
| --- | --- | --- |
| Can the M10 use Dirac calibration? | Documented | NAD lists M10, M10 V2 and M10 V3 among supported products. Exact installed license/features remain unverified. [NAD setup](https://support.nadelectronics.com/hc/en-us/articles/360011867634-How-to-perform-Dirac-Live-Calibration-on-NAD-products), supported-products section. |
| Can a microphone on the Mac be used? | Documented | NAD documents a microphone connected to the local computer and selection of the recording device. It recommends a certified microphone and its correction file. Arbitrary macOS input acceptance by this independent application is a design choice requiring live validation. [NAD setup](https://support.nadelectronics.com/hc/en-us/articles/360011867634-How-to-perform-Dirac-Live-Calibration-on-NAD-products), Microphone setup. |
| Can an adjustment curve be transferred? | Documented indirect route | NAD instructs loading a target curve in Dirac Live, calculating the correction, then exporting a filter to a selected slot. It documents up to three filter slots. The target curve is not itself the device filter payload. [NAD setup](https://support.nadelectronics.com/hc/en-us/articles/360011867634-How-to-perform-Dirac-Live-Calibration-on-NAD-products), steps 24–30. |
| Can independently generated FIR taps be uploaded? | Unresolved | No FIR/Dirac upload endpoint was found in the 64-page public CI API v1.7; whole-document text searches for Dirac and filter returned no matches. The document explicitly covers only a subset of the full protocol, so this does not prove upload impossible. [API v1.7](https://content-bluesound-com.s3.amazonaws.com/uploads/BluOS-Custom-Integration-API_v1.7.pdf), introduction and full contents. |

## Public control interface

API v1.7 specifies HTTP requests with XML responses, typically on TCP 11000. It recommends discovering the actual port with mDNS musc.tcp/musp.tcp. `/SyncStatus` exposes model/name/brand. `/Play?url=encodedStreamURL` starts custom streamed audio. Polling without long polling must be limited to once per 30 seconds. [API v1.7](https://content-bluesound-com.s3.amazonaws.com/uploads/BluOS-Custom-Integration-API_v1.7.pdf), sections 1, 2.2 and 4.1.

Inference: a Mac-hosted sweep file plus custom playback is a plausible measurement transport. Its codec acceptance, per-channel routing, buffering, timing, and repeatability on this M10 need measurement. Public playback support establishes neither phase-synchronized capture nor a calibration/upload session.

## Target text format

Dirac officially accepts both `.txt` and `.targetcurve`. [Dirac FAQ](https://helpdesk.dirac.com/en/faq/Can-I-load-a-target-curve-created-with-previous-versions-of-the-software-ad67).

The official NAD full-frequency text asset was inspected through the current [NAD Downloads](https://nadelectronics.com/pages/downloads) page. [Asset](https://cdn.shopify.com/s/files/1/0784/9751/3722/files/DiracLiveFull_NADTargetCurve.txt?v=1772036916).

Observed structure: a comment, `BREAKPOINTS`, ascending frequency/gain pairs, then named scalar fields. Fields are `HPSLOPEON`, `LPSLOPEON`, `HPCUTOFF`, `LPCUTOFF`, `HPORDER`, `LPORDER`, `LOWLIMITHZ`, `HIGHLIMITHZ`. The sample uses disabled slope flags, fourth-order values, and correction limits near 18 Hz and 20 kHz. Inference: pair units are Hz/dB, consistent with the graph and field names. No comprehensive published grammar was found; export should be described as legacy-text compatibility and checked by importing into installed Dirac Live. A fresh file should carry this project's own provenance comment, not the proprietary generator header.

## Remaining work

To establish direct filter transfer, inspect the installed application or observe an authorized calibration/export on the actual M10. Identify the exact endpoint, session flow, payload encoding, coefficient topology/rate, slot commit behavior, and any integrity/licensing requirements. Independent DSP output must not be called Dirac mixed-phase correction or represented as directly loadable until verified.

API v1.7 also contains distribution-relevant use-policy restrictions concerning copied/reverse-engineered code and competing integrations (pages 2–4). This research identifies the actual published text; it is not a legal conclusion. Preserve independently authored implementation and review the policy before publishing an API integration. [API policy](https://content-bluesound-com.s3.amazonaws.com/uploads/BluOS-Custom-Integration-API_v1.7.pdf).

The Dirac user manual PDF URL returned a retrieval error during this pass and was not used as evidence. No conclusions depend on third-party forum claims.

Local research copies (under `work/protocol`, not packaged application resources) were hashed: API PDF SHA-256 `87fd68fc2bd6584c07f13307c0a8915dace50b74c913e216856997e9cb543c36`; NAD text SHA-256 `bd747fd10f3470af67c3c681f22d8686cdb5bd8c80fa6aa134ae37094b0bbbf9`.

## Native stimulus follow-up

Subsequent installed-artifact analysis and nine bounded metadata reads established this M10's native gRPC stimulus interface. See [device-protocol.md](device-protocol.md) for exact method signatures, protobuf fields, offsets and response hashes.

The device advertises internal log-sweep variants, including a 48 kHz sweep from 10 Hz to 24 kHz with length 240000, silence 24000, one period, and a reported fade-tail parameter of 300 Hz. PrepareStimuli selects that generated sequence; StartStimuli returns a server stream of progress. Native arbitrary-PCM playback is not advertised by GetStreamingInfo. Focused installed-artifact analysis also established the local reference's exponential phase convention, trailing Hann taper and post-sweep silence. This supports an independently written network measurement path without routing speaker output through the Mac, while actual firmware-waveform and cancellation validation remain outstanding.

These metadata reads did not play sound. A finite playback/capture trial is still required to establish sequencing, actual output attenuation, Mac capture synchronization, cancellation and state restoration. Exact Dirac reference-waveform reproduction and algorithm equivalence are not claimed. The public CI API search remains bounded to its documented subset; the independently observed native gRPC endpoint supplies additional evidence beyond that document.
