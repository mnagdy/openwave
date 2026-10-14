mod gui;
use open_room::{audio, device, dsp, nad_stimulus, nad_transfer, nad_v2, project_import, session};

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "OpenRoom — independent room measurement and curve design"
)]
struct Args {
    #[arg(long, global = true)]
    device: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// List available microphone and playback devices
    AudioDevices,
    /// Extract original Ogg recordings to a new directory
    ExtractRecordings { path: PathBuf, output: PathBuf },
    /// Record a microphone WAV without playing a test signal
    Record {
        input: String,
        output: PathBuf,
        #[arg(long, default_value_t = 0)]
        channel: usize,
        #[arg(long, default_value_t = 10.)]
        seconds: f64,
    },
    /// Discover BluOS players on the local network
    Discover,
    /// Read player identity/status without changing playback
    Probe { address: String },
    /// Read native M10 filter capabilities without changing settings
    NativeInfo { address: String },
    /// Save an independently reconstructed native sweep reference; plays no sound
    NativeReference {
        address: String,
        output: PathBuf,
        #[arg(long, default_value_t = 0)]
        speaker: u32,
    },
    /// Prepare an experimental native filter package offline; does not upload
    NativePackage {
        address: String,
        session: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 1)]
        slot: u32,
        #[arg(long, default_value = "OpenRoom")]
        name: String,
    },
    /// Inspect a saved Dirac Live project
    InspectProject { path: PathBuf },
    /// Import measured response graphs into an OpenRoom session
    ImportProject { path: PathBuf, output: PathBuf },
    /// Generate a low-level logarithmic test sweep WAV
    Sweep {
        output: PathBuf,
        #[arg(long, default_value_t = 48000)]
        rate: u32,
        #[arg(long, default_value_t = 8.0)]
        seconds: f64,
    },
    /// Analyze a recording made with the generated reference WAV
    Analyze {
        reference: PathBuf,
        recording: PathBuf,
        output: PathBuf,
    },
    /// Generate independent FIR WAV from a saved OpenRoom session
    Filter {
        session: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 0)]
        channel: usize,
    },
    /// Export a target curve for loading in Dirac Live
    ExportTarget { session: PathBuf, output: PathBuf },
    /// Open a synthetic demonstration without playing audio
    Demo,
}
fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        None => gui::run(args.device, false),
        Some(Command::Demo) => gui::run(args.device, true),
        Some(Command::AudioDevices) => {
            println!("Inputs:");
            for d in audio::inputs()? {
                println!("{} — {} ch @ {} Hz", d.name, d.channels, d.sample_rate);
            }
            println!("Outputs:");
            for d in audio::outputs()? {
                println!("{} — {} ch @ {} Hz", d.name, d.channels, d.sample_rate);
            }
            Ok(())
        }
        Some(Command::ExtractRecordings { path, output }) => {
            let files = project_import::extract_recordings(&path, &output)?;
            println!("Extracted {} Ogg recordings", files.len());
            Ok(())
        }
        Some(Command::Record {
            input,
            output,
            channel,
            seconds,
        }) => {
            let r = audio::record(&input, channel, seconds, &output)?;
            println!(
                "Recorded {} samples @ {} Hz, peak {:.1} dBFS, clipped={}",
                r.samples.len(),
                r.sample_rate,
                r.peak_db,
                r.clipped
            );
            Ok(())
        }
        Some(Command::Discover) => {
            let report = device::discover_report()?;
            for warning in report.warnings {
                eprintln!("{warning}");
            }
            println!("{}", serde_json::to_string_pretty(&report.devices)?);
            Ok(())
        }
        Some(Command::Probe { address }) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&device::probe(&address)?)?
            );
            Ok(())
        }
        Some(Command::NativeInfo { address }) => {
            let d = device::inspect_dirac(&address)?;
            anyhow::ensure!(d.version == 2, "Native reader requires interface v2");
            println!(
                "{}",
                serde_json::to_string_pretty(&nad_v2::inspect(&address, d.control_port)?)?
            );
            Ok(())
        }
        Some(Command::NativeReference {
            address,
            output,
            speaker,
        }) => {
            let d = device::inspect_dirac(&address)?;
            anyhow::ensure!(d.version == 2, "Native reference requires interface v2");
            let metadata = nad_v2::inspect(&address, d.control_port)?;
            let plan = nad_stimulus::plan(&metadata, speaker)?;
            let reference = nad_stimulus::reference(&plan)?;
            audio::write_wav(&output, &reference, 48000)?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
            Ok(())
        }
        Some(Command::NativePackage {
            address,
            session,
            output,
            slot,
            name,
        }) => {
            let s = session::Session::load(&session)?;
            let d = device::inspect_dirac(&address)?;
            anyhow::ensure!(d.version == 2, "Native package requires interface v2");
            let metadata = nad_v2::inspect(&address, d.control_port)?;
            let package = nad_transfer::prepare_session_package(&s, &metadata, slot, &name)?;
            std::fs::write(&output, serde_json::to_vec_pretty(&package)?)?;
            println!("Saved {} proposed calls, {} payload bytes; no device writes. Hardware verification pending.",package.calls.len(),package.total_payload_bytes);
            Ok(())
        }
        Some(Command::InspectProject { path }) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&project_import::inspect(&path)?)?
            );
            Ok(())
        }
        Some(Command::ImportProject { path, output }) => {
            let s = session::Session::from_dirac(&path)?;
            s.save(&output)?;
            println!(
                "Imported {} measured responses into {}",
                s.measurements.len(),
                output.display()
            );
            Ok(())
        }
        Some(Command::Sweep {
            output,
            rate,
            seconds,
        }) => {
            let samples = dsp::sweep(&dsp::SweepConfig {
                sample_rate: rate,
                seconds,
                start_hz: 20.,
                end_hz: 20000_f64.min(rate as f64 * 0.45),
                level_db: -30.,
            })?;
            audio::write_wav(&output, &samples, rate)
        }
        Some(Command::Analyze {
            reference,
            recording,
            output,
        }) => {
            let x = audio::read_wav(&reference)?;
            let y = audio::read_wav(&recording)?;
            anyhow::ensure!(
                x.sample_rate == y.sample_rate,
                "Reference and recording rates differ"
            );
            anyhow::ensure!(!y.clipped, "Recording is clipped");
            let points = dsp::analyze(&x.samples, &y.samples, x.sample_rate)?;
            std::fs::write(output, serde_json::to_string_pretty(&points)?)?;
            Ok(())
        }
        Some(Command::Filter {
            session: path,
            output,
            channel,
        }) => {
            let s = session::Session::load(&path)?;
            let measured = s.average(channel)?;
            let taps = dsp::design_filter(
                &measured,
                &s.target,
                s.sample_rate,
                4097,
                s.low_hz,
                s.high_hz,
                s.max_boost_db,
            )?;
            audio::write_wav(&output, &taps, s.sample_rate)
        }
        Some(Command::ExportTarget {
            session: path,
            output,
        }) => {
            let s = session::Session::load(&path)?;
            std::fs::write(output, dsp::export_target(&s.target, s.low_hz, s.high_hz)?)?;
            Ok(())
        }
    }
}
