use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints, Points};
use open_room::{
    audio,
    device::{self, Device},
    dsp::{self, Point},
    nad_stimulus, nad_transfer, nad_upload, nad_v2, resample,
    session::{Measurement, Session},
};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub fn run(address: Option<String>, demo: bool) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180., 790.])
            .with_min_inner_size([900., 650.]),
        ..Default::default()
    };
    eframe::run_native(
        "OpenRoom",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let mut style = (*cc.egui_ctx.style()).clone();
            style.spacing.item_spacing = egui::vec2(10., 9.);
            style.visuals.selection.bg_fill = Color32::from_rgb(38, 106, 102);
            cc.egui_ctx.set_style(style);
            Ok(Box::new(App::new(address, demo)))
        }),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}

enum Done {
    Devices(Vec<Device>),
    Native(nad_v2::NativeDevice, String, u16),
    Audio(
        Vec<audio::AudioInput>,
        Vec<audio::AudioInput>,
        Option<String>,
        Option<String>,
    ),
    Session(Session),
    Measurement(Measurement),
    Filter(Vec<f32>),
    Message(String),
    Package(nad_transfer::TransferPackage),
    Upload(nad_upload::UploadReceipt),
}
struct App {
    session: Session,
    tab: usize,
    address: String,
    devices: Vec<Device>,
    native: Option<nad_v2::NativeDevice>,
    native_endpoint: Option<(String, u16)>,
    native_package: Option<nad_transfer::TransferPackage>,
    native_slot: u32,
    transfer_ack: bool,
    inputs: Vec<audio::AudioInput>,
    outputs: Vec<audio::AudioInput>,
    input: String,
    output: String,
    input_channel: usize,
    output_channel: usize,
    channel: usize,
    seconds: f64,
    status: String,
    error: Option<String>,
    job: Option<Receiver<std::result::Result<Done, String>>>,
    filter: Vec<f32>,
    filter_channel: usize,
    plot_edit: bool,
    play_ack: bool,
    network_ack: bool,
    cancel: Arc<AtomicBool>,
    averages: BTreeMap<usize, Vec<Point>>,
}
impl App {
    fn new(address: Option<String>, demo: bool) -> Self {
        let mut a = Self {
            session: if demo {
                Session::demo()
            } else {
                Session::default()
            },
            tab: if demo { 2 } else { 0 },
            address: address.clone().unwrap_or_default(),
            devices: vec![],
            native: None,
            native_endpoint: None,
            native_package: None,
            native_slot: 1,
            transfer_ack: false,
            inputs: vec![],
            outputs: vec![],
            input: String::new(),
            output: String::new(),
            input_channel: 0,
            output_channel: 0,
            channel: 0,
            seconds: 8.,
            status: "Ready. Choose your player and microphone.".into(),
            error: None,
            job: None,
            filter: vec![],
            filter_channel: 0,
            plot_edit: false,
            play_ack: false,
            network_ack: false,
            cancel: Arc::new(AtomicBool::new(false)),
            averages: BTreeMap::new(),
        };
        if let Some(ip) = address {
            a.start("Reading player identity…", move || {
                Ok(Done::Devices(vec![device::probe(&ip)?]))
            })
        } else {
            a.refresh_audio()
        };
        a
    }
    fn start<F>(&mut self, label: &str, f: F)
    where
        F: FnOnce() -> Result<Done> + Send + 'static,
    {
        if self.job.is_some() {
            return;
        }
        self.error = None;
        self.cancel.store(false, Ordering::Relaxed);
        self.status = label.into();
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        std::thread::spawn(move || {
            let result = f().map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
    }
    fn refresh_audio(&mut self) {
        self.start("Finding microphones and audio outputs…", || {
            let devices = audio::device_snapshot()?;
            Ok(Done::Audio(
                devices.inputs,
                devices.outputs,
                devices.default_input,
                devices.default_output,
            ))
        })
    }
    fn poll(&mut self) {
        let res = match &self.job {
            Some(rx) => match rx.try_recv() {
                Ok(x) => Some(x),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Background operation ended unexpectedly".into()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            },
            None => None,
        };
        if let Some(r) = res {
            self.job = None;
            match r {
                Err(e) => {
                    self.status = "Operation could not complete.".into();
                    self.error = Some(e)
                }
                Ok(Done::Devices(ds)) => {
                    self.native = None;
                    self.native_endpoint = None;
                    self.native_package = None;
                    self.transfer_ack = false;
                    self.status = if ds.is_empty() {
                        "No players found. Enter the M10 IP address.".into()
                    } else {
                        format!("Connected to {} player(s).", ds.len())
                    };
                    if self.address.is_empty() {
                        if let Some(d) = ds.first() {
                            self.address = d.address.clone();
                        }
                    }
                    self.devices = ds;
                }
                Ok(Done::Native(d, address, port)) => {
                    self.native_package = None;
                    self.transfer_ack = false;
                    self.native_endpoint = Some((address, port));
                    self.status = format!(
                        "Native service verified: {} {} · {} speakers",
                        d.manufacturer, d.model, d.speakers
                    );
                    self.native = Some(d);
                }
                Ok(Done::Audio(i, o, preferred_input, preferred_output)) => {
                    if !i.iter().any(|d| d.name == self.input) {
                        self.input = preferred_input
                            .filter(|n| i.iter().any(|d| d.name == *n))
                            .or_else(|| i.first().map(|d| d.name.clone()))
                            .unwrap_or_default();
                    }
                    if !o.iter().any(|d| d.name == self.output) {
                        self.output = preferred_output
                            .filter(|n| o.iter().any(|d| d.name == *n))
                            .or_else(|| o.first().map(|d| d.name.clone()))
                            .unwrap_or_default();
                    }
                    self.inputs = i;
                    self.outputs = o;
                    self.status = "Audio device list updated.".into()
                }
                Ok(Done::Session(s)) => {
                    self.session = s;
                    self.channel = 0;
                    self.invalidate();
                    self.tab = 2;
                    self.status = format!(
                        "Loaded {} measured responses.",
                        self.session.measurements.len()
                    )
                }
                Ok(Done::Measurement(m)) => {
                    let needs_review = !m.included;
                    self.session.measurements.push(m);
                    self.invalidate();
                    self.status = if needs_review {
                        "Recording saved. Review the new position before including it in curve averaging.".into()
                    } else {
                        "Measurement captured. Move the microphone for the next position.".into()
                    }
                }
                Ok(Done::Filter(f)) => {
                    self.filter = f;
                    self.filter_channel = self.channel;
                    self.status =
                        "Independent FIR calculated. Export it for a local convolver.".into()
                }
                Ok(Done::Message(s)) => self.status = s,
                Ok(Done::Package(p)) => {
                    self.status = "Amplifier filter package prepared. Hardware validation is pending; nothing was transferred.".into();
                    self.native_package = Some(p);
                }
                Ok(Done::Upload(receipt)) => {
                    self.status = format!("Transfer acknowledged for {} and slot identity checked. Applied sound is unverified. Read amplifier capabilities again to refresh slots.", receipt.name);
                    self.native = None;
                    self.native_endpoint = None;
                    self.native_package = None;
                    self.transfer_ack = false;
                }
            }
        }
    }
    fn invalidate(&mut self) {
        self.filter.clear();
        self.averages.clear();
        self.native_package = None;
        self.transfer_ack = false;
    }
    fn import_project(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Dirac Live project", &["liveproject"])
            .pick_file()
        {
            self.start("Reading saved measured responses…", move || {
                Ok(Done::Session(Session::from_dirac(&path)?))
            });
        }
    }
    fn open_session(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("OpenRoom session", &["json"])
            .pick_file()
        {
            self.start("Opening session…", move || {
                Ok(Done::Session(Session::load(&path)?))
            });
        }
    }
    fn save_session(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("room.openroom.json")
            .save_file()
        {
            let s = self.session.clone();
            self.start("Saving session…", move || {
                s.save(&path)?;
                Ok(Done::Message(format!("Saved {}", path.display())))
            });
        }
    }
    fn draw_devices(&mut self, ui: &mut egui::Ui) {
        ui.heading("Connect your listening room");
        ui.label("Read player status over your local network, then choose any microphone attached to this Mac.");
        ui.add_space(14.);
        ui.horizontal(|ui| {
            ui.label("Player address");
            ui.add(
                egui::TextEdit::singleline(&mut self.address)
                    .hint_text("192.168.1.20 or IP:port")
                    .desired_width(280.),
            );
            if ui.button("Connect").clicked() {
                let ip = self.address.clone();
                self.start("Reading player identity…", move || {
                    Ok(Done::Devices(vec![device::probe(&ip)?]))
                })
            }
            if ui.button("Find players").clicked() {
                self.start("Discovering BluOS players…", || {
                    Ok(Done::Devices(device::discover()?))
                });
            }
        });
        for d in self.devices.clone() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("●").color(Color32::from_rgb(89, 211, 174)));
                    ui.heading(&d.name);
                    ui.label(format!("NAD {} · BluOS {}", d.model, d.firmware));
                });
                ui.label(&d.address);
                ui.label(match d.dirac_active {
                    Some(true) => "A Dirac filter is currently active.",
                    Some(false) => "Dirac filtering is currently off.",
                    None => "Dirac filtering state was not reported.",
                });
                if let Some(native) = &d.dirac_interface {
                    ui.label("Native Dirac service detected.");
                    if native.version == 2 && ui.button("Read amplifier capabilities").clicked() {
                        let address = d.address.clone();
                        let port = native.control_port;
                        self.start("Reading amplifier capabilities…", move || {
                            Ok(Done::Native(
                                nad_v2::inspect(&address, port)?,
                                address,
                                port,
                            ))
                        });
                    }
                }
                if ui.button("Use this player").clicked() {
                    if self
                        .native_endpoint
                        .as_ref()
                        .is_some_and(|(address, _)| address != &d.address)
                    {
                        self.native = None;
                        self.native_endpoint = None;
                        self.native_package = None;
                        self.transfer_ack = false;
                    }
                    self.address = d.address;
                }
            });
        }
        if let Some(d) = &self.native {
            if let Some((address, _)) = &self.native_endpoint {
                ui.label(format!("Amplifier capabilities from {address}"));
            }
            ui.label(format!(
                "Native connection verified · {} speakers · {}",
                d.speakers, d.model
            ));
            for slot in &d.slots {
                if !slot.name.is_empty() {
                    ui.label(format!("Saved filter: {}", slot.name));
                }
            }
            if let Some(m) = &d.measurement {
                if let Some(s) = m.log_sweeps.iter().find(|s| s.variant == 0) {
                    ui.label(format!(
                        "Network measurement signal available · {:.1} seconds · {:.0} Hz to {:.0} Hz",
                        s.samples as f64 / s.sample_rate as f64, s.start_hz, s.end_hz
                    ));
                    ui.label("Network playback is under verification. No test signal is played when reading capabilities.");
                }
            }
        }
        ui.add_space(15.);
        ui.horizontal(|ui| {
            ui.heading("Microphone and output");
            if ui.button("Refresh devices").clicked() {
                self.refresh_audio();
            }
        });
        selector(ui, "Microphone", "input", &mut self.input, &self.inputs);
        selector(
            ui,
            "Playback output",
            "output",
            &mut self.output,
            &self.outputs,
        );
        ui.horizontal(|ui| {
            ui.label("Microphone channel (0 based)");
            ui.add(egui::DragValue::new(&mut self.input_channel).range(0..=63));
            ui.label("Playback channel");
            ui.add(egui::DragValue::new(&mut self.output_channel).range(0..=63));
        });
        if ui
            .add_enabled(
                !self.input.is_empty(),
                egui::Button::new("Check microphone · 2 seconds, no playback"),
            )
            .clicked()
        {
            let input = self.input.clone();
            let ch = self.input_channel;
            let rate = self
                .inputs
                .iter()
                .find(|d| d.name == input)
                .map(|d| d.sample_rate)
                .unwrap_or(48000);
            self.start("Checking microphone for two seconds…", move || {
                let r = audio::capture_with_action(&input, ch, rate, 2., || Ok(()))?;
                let message = if r.peak_db.is_finite() {
                    format!(
                        "Microphone captured {} samples at {} Hz · peak {:.1} dBFS · {}",
                        r.samples.len(),
                        r.sample_rate,
                        r.peak_db,
                        if r.clipped {
                            "clipped — reduce gain"
                        } else {
                            "no clipping"
                        }
                    )
                } else {
                    "Microphone stream works but captured silence. Check input gain.".into()
                };
                Ok(Done::Message(message))
            });
        }
        ui.label("For local sweeps, route the selected Mac output into the M10’s analogue or optical input. Select that input on the M10. Choose one speaker channel at a time.");
        ui.add_space(16.);
        ui.horizontal(|ui|{if ui.button("Load microphone calibration…").clicked(){if let Some(path)=rfd::FileDialog::new().add_filter("Calibration text",&["txt","cal"]).pick_file(){match std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t|dsp::import_calibration(&t)){Ok(c)=>{self.session.calibration=c;self.invalidate();self.status="Microphone calibration loaded for raw captures. Imported Dirac graphs retain their processing.".into();},Err(e)=>self.error=Some(e.to_string())}}}
if !self.session.calibration.is_empty(){ui.label(format!("{} calibration points",self.session.calibration.len()));if ui.button("Remove").clicked(){self.session.calibration.clear();self.invalidate();}}});
        ui.small("Without a calibration file, results show the microphone’s response as well as the room. Imported Dirac graphs retain their existing processing.");
        ui.add_space(20.);
        ui.heading("Continue from your existing work");
        if ui.button("Import a saved Dirac Live project…").clicked() {
            self.import_project();
        }
        ui.label("Reads saved measured-response graphs. Your original project is kept intact.");
    }
    fn draw_measure(&mut self, ui: &mut egui::Ui) {
        ui.heading("Measure one speaker at a time");
        ui.label("Use several microphone positions around your listening seat. Start with the M10 volume low. Turn off existing room correction and tone controls before measuring.");
        ui.add_space(12.);
        ui.horizontal(|ui| {
            ui.label("Save as");
            let old = self.channel;
            speaker_selector(
                ui,
                &mut self.channel,
                self.session
                    .measurements
                    .iter()
                    .map(|m| m.channel)
                    .max()
                    .unwrap_or(1)
                    .max(1),
            );
            if old != self.channel {
                self.output_channel = self.channel;
            }
            ui.label("Sweep duration");
            ui.add(
                egui::DragValue::new(&mut self.seconds)
                    .range(2. ..=30.)
                    .suffix(" s"),
            );
            ui.label("Sample rate");
            egui::ComboBox::from_id_salt("rate")
                .selected_text(self.session.sample_rate.to_string())
                .show_ui(ui, |ui| {
                    for rate in [44100, 48000, 96000] {
                        if ui
                            .selectable_value(&mut self.session.sample_rate, rate, rate.to_string())
                            .changed()
                        {
                            self.invalidate();
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Physical playback channel (0 based)");
            ui.add(egui::DragValue::new(&mut self.output_channel).range(0..=63));
            ui.label("Microphone channel");
            ui.add(egui::DragValue::new(&mut self.input_channel).range(0..=63));
        });
        ui.label(format!(
            "Input: {} (channel {})",
            if self.input.is_empty() {
                "Select a microphone"
            } else {
                &self.input
            },
            self.input_channel
        ));
        ui.label(format!(
            "Output: {} (channel {})",
            if self.output.is_empty() {
                "Select an output"
            } else {
                &self.output
            },
            self.output_channel
        ));
        self.draw_network_measurement(ui);
        ui.add_space(12.);
        ui.heading("Measurement through a connected Mac output");
        ui.checkbox(
            &mut self.play_ack,
            "My selected output feeds the M10, and its volume is set low",
        );
        if ui
            .add_enabled(
                self.play_ack && !self.input.is_empty() && !self.output.is_empty(),
                egui::Button::new("Play sweep and capture position"),
            )
            .clicked()
        {
            let input = self.input.clone();
            let output = self.output.clone();
            let ic = self.input_channel;
            let oc = self.output_channel;
            let ch = self.channel;
            let rate = self.session.sample_rate;
            let seconds = self.seconds;
            let pos = self
                .session
                .measurements
                .iter()
                .filter(|m| m.channel == ch)
                .map(|m| m.position)
                .max()
                .map_or(0, |p| p + 1);
            self.start("Playing a −30 dBFS sweep and recording…", move || {
                let sweep = dsp::sweep(&dsp::SweepConfig {
                    sample_rate: rate,
                    seconds,
                    start_hz: 20.,
                    end_hz: 20000_f64.min(rate as f64 * 0.45),
                    level_db: -30.,
                })?;
                let captured = audio::measure(&input, ic, &output, oc, &sweep, rate)?;
                anyhow::ensure!(
                    !captured.clipped,
                    "Microphone clipped; lower the M10 volume or microphone gain"
                );
                anyhow::ensure!(
                    captured.peak_db > -65.,
                    "Recorded signal was too quiet; check routing and gain"
                );
                let points = dsp::analyze(&sweep, &captured.samples, rate)?;
                Ok(Done::Measurement(Measurement {
                    channel: ch,
                    position: pos,
                    points,
                    source: "OpenRoom local sweep capture".into(),
                    included: true,
                    calibration_applied: false,
                }))
            });
        }
        ui.add_space(12.);
        ui.horizontal(|ui| {
            if ui.button("Save reference sweep…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_file_name("sweep.wav")
                    .save_file()
                {
                    let rate = self.session.sample_rate;
                    let seconds = self.seconds;
                    self.start("Saving reference sweep…", move || {
                        let x = dsp::sweep(&dsp::SweepConfig {
                            sample_rate: rate,
                            seconds,
                            start_hz: 20.,
                            end_hz: 20000_f64.min(rate as f64 * 0.45),
                            level_db: -30.,
                        })?;
                        audio::write_wav(&path, &x, rate)?;
                        Ok(Done::Message(format!("Saved {}", path.display())))
                    });
                }
            }
            if ui.button("Analyze reference + recorded WAV…").clicked() {
                if let Some(reference) = rfd::FileDialog::new()
                    .set_title("Select original reference sweep WAV")
                    .add_filter("WAV", &["wav"])
                    .pick_file()
                {
                    if let Some(recording) = rfd::FileDialog::new()
                        .set_title("Select full microphone recording WAV")
                        .add_filter("WAV", &["wav"])
                        .pick_file()
                    {
                        let rate = self.session.sample_rate;
                        let ch = self.channel;
                        let pos = self
                            .session
                            .measurements
                            .iter()
                            .filter(|m| m.channel == ch)
                            .count();
                        self.start("Analyzing recorded sweep…", move || {
                            let x = audio::read_wav(&reference)?;
                            let y = audio::read_wav(&recording)?;
                            anyhow::ensure!(
                                x.sample_rate == y.sample_rate && x.sample_rate == rate,
                                "WAV sample rates must match session rate"
                            );
                            anyhow::ensure!(
                                !y.clipped,
                                "Recording clipped; do not design a filter from it"
                            );
                            Ok(Done::Measurement(Measurement {
                                channel: ch,
                                position: pos,
                                points: dsp::analyze(&x.samples, &y.samples, rate)?,
                                source: "Imported WAV reference + recording".into(),
                                included: true,
                                calibration_applied: false,
                            }))
                        });
                    }
                }
            }
        });
        ui.small("Keep the complete sweep and decay tail. Separate recording/playback clocks can affect results; timing and mixed-phase correction are not estimated.");
        ui.add_space(16.);
        self.measurement_list(ui);
    }
    fn draw_network_measurement(&mut self, ui: &mut egui::Ui) {
        let Some(native) = self.native.as_ref() else {
            ui.label("For measurement over Wi-Fi or Ethernet, read amplifier capabilities in Connect first.");
            return;
        };
        let p = match nad_stimulus::plan(native, self.channel as u32) {
            Ok(p) => p,
            Err(e) => {
                ui.label(format!("Network measurement unavailable: {e}"));
                return;
            }
        };
        let Some((address, port)) = self.native_endpoint.clone() else {
            return;
        };
        let mic_rate = self
            .inputs
            .iter()
            .find(|d| d.name == self.input)
            .map(|d| d.sample_rate);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Measurement over your network · experimental");
            ui.label(format!("The M10 plays a {:.1}-second sweep through speaker {} while this Mac records. No audio cable from the Mac is needed.",p.sequence_seconds,self.channel+1));
            ui.label("Playback and amplifier-state restoration still need a live validation test. Keep other calibration apps closed. Raw audio is saved locally so the result can be checked.");
            ui.checkbox(&mut self.network_ack,"My speakers are ready and the M10 volume is set low");
            if ui.add_enabled(self.network_ack && mic_rate.is_some_and(|r|(8000..=192000).contains(&r)),egui::Button::new("Play network sweep and record this position…")).clicked() {
                if let Some(path) = rfd::FileDialog::new().set_title("Save the microphone recording").add_filter("WAV",&["wav"]).set_file_name("m10-measurement.wav").save_file() {
                    let input=self.input.clone();let mic=self.input_channel;let ch=self.channel;
                    let rate=mic_rate.unwrap();
                    let pos=self.session.measurements.iter().filter(|m|m.channel==ch).map(|m|m.position).max().map_or(0,|p|p+1);
                    let cancel=self.cancel.clone();
                    self.start("Recording the M10 network sweep…",move || {
                        let mut played=None;
                        let captured=audio::capture_with_action(&input,mic,rate,2.,|| {
                            played=Some(nad_stimulus::play_log_sweep(&address,port,ch as u32,cancel)?); Ok(())
                        })?;
                        audio::write_wav(&path,&captured.samples,captured.sample_rate)?;
                        anyhow::ensure!(!captured.clipped,"Recording clipped. Lower the M10 volume; the raw recording has been saved.");
                        anyhow::ensure!(captured.peak_db > -65.,"Recording is too quiet; the raw recording has been saved for checking.");
                        let reference=nad_stimulus::reference(&played.context("Native playback did not return its waveform profile")?)?;
                        let reference=resample::resample_reference(&reference,48000,rate)?;
                        let points=dsp::analyze(&reference,&captured.samples,rate)?;
                        Ok(Done::Measurement(Measurement {channel:ch,position:pos,points,source:format!("M10 native sweep estimate; raw audio: {}",path.display()),included:false,calibration_applied:false}))
                    });
                }
            }
            ui.small("New network measurements start excluded from curve averaging until you review them.");
        });
    }
    fn measurement_list(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!(
            "{} saved responses",
            self.session.measurements.len()
        ));
        let mut delete = None;
        let mut changed = false;
        egui::ScrollArea::vertical()
            .max_height(220.)
            .show(ui, |ui| {
                for (i, m) in self.session.measurements.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        changed |= ui
                            .checkbox(
                                &mut m.included,
                                format!("Speaker {} · position {}", m.channel + 1, m.position + 1),
                            )
                            .changed();
                        ui.small(&m.source);
                        if ui.small_button("Remove").clicked() {
                            delete = Some(i)
                        }
                    });
                }
            });
        if let Some(i) = delete {
            self.session.measurements.remove(i);
            changed = true;
        }
        if changed {
            self.invalidate();
        }
    }
    fn draw_curve(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Shape your target curve");
            speaker_selector(
                ui,
                &mut self.channel,
                self.session
                    .measurements
                    .iter()
                    .map(|m| m.channel)
                    .max()
                    .unwrap_or(1),
            );
        });
        ui.label("Measured responses are normalized around 500–2000 Hz and averaged by power. This target is shared across speakers; measured corrections remain per speaker.");
        let average = if let Some(p) = self.averages.get(&self.channel) {
            Ok(p.clone())
        } else {
            let r = self.session.average(self.channel);
            if let Ok(p) = &r {
                self.averages.insert(self.channel, p.clone());
            }
            r
        };
        if let Err(e) = &average {
            ui.label(RichText::new(e.to_string()).color(Color32::LIGHT_YELLOW));
        }
        let target =
            PlotPoints::from_iter(self.session.target.iter().map(|p| [p.hz.log10(), p.db]));
        ui.checkbox(&mut self.plot_edit, "Click the graph to add a target point");
        let mut pointer = None;
        let response = Plot::new("response")
            .height(310.)
            .allow_drag(!self.plot_edit)
            .include_x(20f64.log10())
            .include_x(20000f64.log10())
            .include_y(-15.)
            .include_y(15.)
            .legend(egui_plot::Legend::default())
            .x_axis_label("Frequency (Hz)")
            .x_axis_formatter(|mark, _| {
                let hz = 10f64.powf(mark.value);
                if hz >= 1000. {
                    format!("{:.0}k", hz / 1000.)
                } else {
                    format!("{hz:.0}")
                }
            })
            .label_formatter(|name, p| format!("{name}\n{:.0} Hz · {:.1} dB", 10f64.powf(p.x), p.y))
            .y_axis_label("Relative dB")
            .show(ui, |p| {
                if let Ok(measured) = &average {
                    p.line(
                        Line::new(PlotPoints::from_iter(
                            measured.iter().map(|x| [x.hz.log10(), x.db]),
                        ))
                        .name("Measured average")
                        .color(Color32::from_rgb(126, 146, 190)),
                    );
                }
                p.line(
                    Line::new(target)
                        .name("Target")
                        .color(Color32::from_rgb(103, 223, 185))
                        .width(2.5_f32),
                );
                p.points(
                    Points::new(PlotPoints::from_iter(
                        self.session.target.iter().map(|x| [x.hz.log10(), x.db]),
                    ))
                    .radius(4_f32)
                    .color(Color32::from_rgb(103, 223, 185)),
                );
                p.vline(egui_plot::VLine::new(self.session.low_hz.log10()).color(Color32::GRAY));
                p.vline(egui_plot::VLine::new(self.session.high_hz.log10()).color(Color32::GRAY));
                pointer = p.pointer_coordinate();
            });
        if self.plot_edit && response.response.clicked() {
            if let Some(p) = pointer {
                let hz = 10f64.powf(p.x);
                if (20. ..=20000.).contains(&hz) && p.y.is_finite() {
                    let db = p.y.clamp(-20., 12.);
                    if let Some(existing) = self
                        .session
                        .target
                        .iter_mut()
                        .find(|x| (x.hz / hz).ln().abs() < 0.01)
                    {
                        existing.db = db;
                    } else {
                        self.session.target.push(Point { hz, db });
                    }
                    self.session.target.sort_by(|a, b| a.hz.total_cmp(&b.hz));
                    self.invalidate();
                }
            }
        }
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Correction from");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut self.session.low_hz)
                        .range(10. ..=10000.)
                        .suffix(" Hz"),
                )
                .changed();
            ui.label("to");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut self.session.high_hz)
                        .range(20. ..=20000.)
                        .suffix(" Hz"),
                )
                .changed();
            ui.label("Maximum boost");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut self.session.max_boost_db)
                        .range(0. ..=6.)
                        .suffix(" dB"),
                )
                .changed();
        });
        ui.horizontal(|ui| {
            egui::ScrollArea::vertical()
                .min_scrolled_height(150.)
                .max_height(150.)
                .show(ui, |ui| {
                    let mut del = None;
                    egui::Grid::new("curve_points")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label("Frequency");
                            ui.label("Target level");
                            ui.label("");
                            ui.end_row();
                            for (i, p) in self.session.target.iter_mut().enumerate() {
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut p.hz)
                                            .range(10. ..=22000.)
                                            .suffix(" Hz"),
                                    )
                                    .changed();
                                changed |= ui
                                    .add(
                                        egui::DragValue::new(&mut p.db)
                                            .range(-30. ..=12.)
                                            .suffix(" dB"),
                                    )
                                    .changed();
                                if ui.small_button("×").clicked() {
                                    del = Some(i);
                                }
                                ui.end_row();
                            }
                        });
                    if let Some(i) = del {
                        if self.session.target.len() > 2 {
                            self.session.target.remove(i);
                            changed = true;
                        }
                    }
                });
            ui.vertical(|ui| {
                if ui.button("Add point").clicked() {
                    let hz = (1..2200)
                        .map(|i| i as f64 * 10.)
                        .find(|h| self.session.target.iter().all(|p| p.hz != *h))
                        .unwrap_or(1000.);
                    self.session.target.push(Point { hz, db: 0. });
                    changed = true;
                }
                if ui.button("Import target…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Dirac target", &["txt", "targetcurve"])
                        .pick_file()
                    {
                        match std::fs::read_to_string(path)
                            .map_err(anyhow::Error::from)
                            .and_then(|t| dsp::import_target(&t))
                        {
                            Ok((p, l, h)) => {
                                self.session.target = p;
                                self.session.low_hz = l;
                                self.session.high_hz = h;
                                changed = true;
                            }
                            Err(e) => self.error = Some(e.to_string()),
                        }
                    }
                }
                if ui.button("Flat target").clicked() {
                    self.session.target =
                        vec![Point { hz: 20., db: 0. }, Point { hz: 20000., db: 0. }];
                    changed = true;
                }
            });
        });
        if changed {
            self.session.target.sort_by(|a, b| a.hz.total_cmp(&b.hz));
            self.invalidate();
        }
    }
    fn draw_export(&mut self, ui: &mut egui::Ui) {
        ui.heading("Take your curves to the M10");
        self.draw_native_package(ui);
        ui.add_space(14.);
        ui.label("You can also export a target curve for installed Dirac Live to calculate and transfer.");
        ui.add_space(14.);
        egui::Frame::group(ui.style()).show(ui,|ui|{ui.heading("Target curve → Dirac Live → M10");ui.label("1. Export the target curve below.");ui.label("2. Open your saved project in Dirac Live and load this target for each speaker group.");ui.label("3. Calculate, then export to a chosen M10 filter slot.");if ui.button("Export target curve…").clicked(){if let Some(path)=rfd::FileDialog::new().set_file_name("openroom-target.txt").save_file(){match dsp::export_target(&self.session.target,self.session.low_hz,self.session.high_hz).and_then(|t|{std::fs::write(&path,t)?;Ok(())}){Ok(())=>self.status=format!("Saved {}. Load it in Dirac Live to transfer.",path.display()),Err(e)=>self.error=Some(e.to_string())}}}
if ui.button("Open installed Dirac Live").clicked(){self.start("Opening Dirac Live…",||{let s=std::process::Command::new("/usr/bin/open").args(["-a","DiracLive"]).status()?;anyhow::ensure!(s.success(),"Could not open Dirac Live");Ok(Done::Message("Dirac Live opened. Load your saved project and exported target.".into()))});}});
        ui.add_space(15.);
        egui::Frame::group(ui.style()).show(ui,|ui|{ui.heading("Independent FIR for a local convolver");ui.label("OpenRoom’s own magnitude correction uses a linear-phase FIR. This WAV is for a software convolver; M10 filter slots do not accept it through a verified interface.");speaker_selector(ui,&mut self.channel,self.session.measurements.iter().map(|m|m.channel).max().unwrap_or(1));
   if ui.button("Calculate FIR").clicked(){let s=self.session.clone();let channel=self.channel;self.start("Designing independent correction filter…",move||{s.validate()?;let measured=s.average(channel)?;Ok(Done::Filter(dsp::design_filter(&measured,&s.target,s.sample_rate,4097,s.low_hz,s.high_hz,s.max_boost_db)?))});}
   if !self.filter.is_empty(){ui.label(format!("{} taps @ {} Hz · speaker {} · linear-phase delay {:.1} ms",self.filter.len(),self.session.sample_rate,self.filter_channel+1,(self.filter.len()-1)as f64/2./self.session.sample_rate as f64*1000.));if ui.button("Save FIR WAV…").clicked(){if let Some(path)=rfd::FileDialog::new().set_file_name(format!("speaker-{}-fir.wav",self.filter_channel+1)).save_file(){let f=self.filter.clone();let rate=self.session.sample_rate;self.start("Saving FIR…",move||{audio::write_wav(&path,&f,rate)?;Ok(Done::Message(format!("Saved {}",path.display())))});}}}
  });
        ui.add_space(16.);
        ui.label(
            RichText::new("Direct M10 filter upload remains unverified.")
                .color(Color32::LIGHT_YELLOW),
        );
        ui.label("The independent transfer is experimental. Successful acknowledgements do not establish the applied frequency response.");
    }
    fn draw_native_package(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Independent amplifier filter · experimental");
            ui.small("Native correction currently supports 20–500 Hz and up to 6 dB boost. Set those limits in Shape curve before preparing.");
            ui.label("Prepare both speakers’ filters for the M10’s native format. Preparing and saving a preview sends nothing. Sending a filter is a separate experimental action; hardware acceptance and the applied sound still need validation.");
            if let Some(metadata)=self.native.clone() {
                egui::ComboBox::from_id_salt("native_slot").selected_text(format!("Empty slot candidate {}",self.native_slot)).show_ui(ui,|ui| {
                    for slot in metadata.slots.iter().filter(|s|[1,2].contains(&s.index) && s.name.is_empty()) {
                        if ui.selectable_value(&mut self.native_slot,slot.index,format!("Empty slot candidate {}",slot.index)).changed() { self.native_package=None; self.transfer_ack=false; }
                    }
                });
                ui.small("The mapping between these candidates and NAD’s visible slot labels has not been verified.");
                if ui.button("Prepare stereo amplifier filter").clicked() {
                    let s=self.session.clone();let slot=self.native_slot;
                    self.start("Preparing stereo amplifier filters…",move || Ok(Done::Package(nad_transfer::prepare_session_package(&s,&metadata,slot,"OpenRoom")?)));
                }
            } else {
                ui.label("Read amplifier capabilities in Connect before preparing a filter.");
            }
            if let Some(package)=self.native_package.clone() {
                ui.label(format!("Prepared for both speakers · {:.1} dB shared headroom",package.attenuation_db));
                ui.label("The filter model passed its approximation checks. This does not establish that your room matches the target or that the M10 accepts this package.");
                for speaker in 0..2 {
                    let models=package.model_summaries.iter().filter(|m|m.speaker==speaker).collect::<Vec<_>>();
                    let fit=models.iter().map(|m|m.filter_approximation_error_db).fold(0.,f64::max);
                    let residual=models.iter().map(|m|m.room_target_residual_db).fold(0.,f64::max);
                    ui.label(format!("Speaker {} · filter approximation {:.2} dB · modeled target gap up to {:.1} dB",speaker+1,fit,residual));
                }
                ui.small("Gain limits and the correction boundaries can leave room-response dips unfilled. These values are predictions, not new speaker measurements.");
                if ui.button("Save amplifier filter preview…").clicked() {
                    if let Some(path)=rfd::FileDialog::new().set_file_name("openroom-m10-filter-preview.json").save_file() {
                        let preview=package.clone();
                        self.start("Saving amplifier filter preview…",move || {std::fs::write(&path,serde_json::to_vec_pretty(&preview)?)?;Ok(Done::Message(format!("Saved {}. No amplifier settings changed.",path.display())))});
                    }
                }
                ui.separator();
                ui.small("Transfer checks that the chosen candidate is still empty and leaves named slots untargeted. Slot mapping, firmware filter behavior and commit-induced activation are unverified. A failed transfer may leave a partial slot.");
                ui.checkbox(&mut self.transfer_ack, "Test this filter in an empty slot; sending may select the new correction");
                let destination = self.native_endpoint.clone();
                if ui.add_enabled(self.transfer_ack && destination.is_some(),egui::Button::new("Send experimental filter to M10")).clicked() {
                    let (address,port)=destination.unwrap();
                    let session=self.session.clone();
                    let slot=package.slot;
                    let name="OpenRoom".to_owned();
                    let cancel=self.cancel.clone();
                    self.transfer_ack=false;
                    self.start("Transferring the experimental M10 filter…",move || Ok(Done::Upload(nad_upload::upload_candidate(&address,port,&session,slot,&name,cancel)?)));
                }
            }
        });
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        if self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("OpenRoom").color(Color32::from_rgb(103, 223, 185)));
                ui.label("Room measurement & curve design");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(self.job.is_none(), |ui| {
                        if ui.button("Save session").clicked() {
                            self.save_session();
                        }
                        if ui.button("Open session").clicked() {
                            self.open_session();
                        }
                    });
                });
            });
            ui.add_space(7.);
            ui.horizontal(|ui| {
                for (i, name) in ["1  Connect", "2  Measure", "3  Shape curve", "4  Export"]
                    .iter()
                    .enumerate()
                {
                    if ui.selectable_label(self.tab == i, *name).clicked() {
                        self.tab = i;
                    }
                }
            });
            ui.add_space(6.);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.job.is_some() {
                    ui.spinner();
                }
                ui.label(&self.status);
                if self.job.is_some()
                    && self.status == "Recording the M10 network sweep…"
                    && ui.button("Stop test").clicked()
                {
                    self.cancel.store(true, Ordering::Relaxed);
                    self.status = "Stopping the test and restoring amplifier settings…".into();
                }
                if self.job.is_some()
                    && self.status == "Transferring the experimental M10 filter…"
                    && ui.button("Cancel transfer").clicked()
                {
                    self.cancel.store(true, Ordering::Relaxed);
                    self.status = "Cancelling transfer; a partial slot may remain…".into();
                }
            });
            if let Some(e) = self.error.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(e).color(Color32::from_rgb(244, 146, 129)));
                    if ui.small_button("Dismiss").clicked() {
                        self.error = None;
                    }
                });
            }
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(10.);
                ui.add_enabled_ui(self.job.is_none(), |ui| match self.tab {
                    0 => self.draw_devices(ui),
                    1 => self.draw_measure(ui),
                    2 => self.draw_curve(ui),
                    _ => self.draw_export(ui),
                });
            });
        });
    }
}
fn selector(
    ui: &mut egui::Ui,
    label: &str,
    id: &str,
    value: &mut String,
    devices: &[audio::AudioInput],
) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(id)
            .selected_text(if value.is_empty() {
                "No device selected"
            } else {
                value.as_str()
            })
            .width(330.)
            .show_ui(ui, |ui| {
                for d in devices {
                    ui.selectable_value(
                        value,
                        d.name.clone(),
                        format!(
                            "{} · {} channels · {} Hz",
                            d.name, d.channels, d.sample_rate
                        ),
                    );
                }
            });
    });
}
fn speaker_selector(ui: &mut egui::Ui, ch: &mut usize, max: usize) {
    egui::ComboBox::from_id_salt("speaker")
        .selected_text(format!("Speaker {}", *ch + 1))
        .show_ui(ui, |ui| {
            for i in 0..=max.min(63) {
                ui.selectable_value(ch, i, format!("Speaker {}", i + 1));
            }
        });
}
