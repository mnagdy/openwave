use super::*;
use egui::{Pos2, RawInput, Rect, Vec2};

#[test]
fn empty_discovery_displays_network_guidance_and_probe_errors() {
    let mut app = App::offline(None, false);
    let (tx, rx) = mpsc::channel();
    app.job = Some(rx);
    tx.send(Ok(Done::Devices(
        vec![],
        vec!["UPnP discovery failed: network unavailable".into()],
    )))
    .unwrap();
    app.poll();
    assert!(app.job.is_none());
    assert!(app.status.contains("Local Network access"));
    assert!(app.status.contains("Connect"));
    assert!(app.error.unwrap().contains("UPnP discovery failed"));
}

fn frame(
    app: &mut App,
    ctx: &egui::Context,
    size: Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| app.render(ctx),
    )
}
fn text_centre(output: &egui::FullOutput, label: &str) -> Pos2 {
    fn find(shape: &egui::Shape, label: &str) -> Option<Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == label => Some(t.pos + t.galley.size() / 2.),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|s| find(&s.shape, label))
        .unwrap_or_else(|| panic!("Missing control: {label}"))
}
fn click(app: &mut App, ctx: &egui::Context, size: Vec2, pos: Pos2) -> egui::FullOutput {
    frame(
        app,
        ctx,
        size,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
    );
    frame(
        app,
        ctx,
        size,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    )
}

#[test]
fn preset_preview_apply_and_undo_use_real_controls_and_invalidate_filters() {
    let ctx = egui::Context::default();
    design::configure(&ctx);
    let mut app = App::offline(None, true);
    let size = Vec2::new(1280., 860.);
    let before = app.session.target.clone();
    let _ = frame(&mut app, &ctx, size, vec![]);
    let output = frame(&mut app, &ctx, size, vec![]);
    let pos = text_centre(&output, "More bass");
    let output = click(&mut app, &ctx, size, pos);
    assert_eq!(app.preset_preview, Some(CurvePreset::BassLift));
    assert!(app
        .session
        .target
        .iter()
        .zip(&before)
        .all(|(a, b)| a.db == b.db));
    app.filter = vec![1.];
    let pos = text_centre(&output, "Use More bass");
    click(&mut app, &ctx, size, pos);
    assert!(CurvePreset::BassLift.matches(&app.session.target));
    assert!(app.filter.is_empty());
    assert_eq!(
        (
            app.session.low_hz,
            app.session.high_hz,
            app.session.max_boost_db
        ),
        (30., 500., 0.)
    );
    let output = frame(&mut app, &ctx, size, vec![]);
    let pos = text_centre(&output, "Undo last preset");
    click(&mut app, &ctx, size, pos);
    assert!(app
        .session
        .target
        .iter()
        .zip(before)
        .all(|(a, b)| a.db == b.db));
    assert!(app.job.is_none());
}

#[test]
fn next_point_requires_both_speakers_and_does_not_guess_imported_geometry() {
    let mut app = App::offline(None, true);
    app.select_next_placement();
    assert_eq!(app.placement, Placement::Main);
    app.session.measurements[0].placement = Some(Placement::Main);
    app.select_next_placement();
    assert_eq!(app.placement, Placement::Main);
    app.session
        .measurements
        .iter_mut()
        .find(|m| m.channel == 1)
        .unwrap()
        .placement = Some(Placement::Main);
    app.select_next_placement();
    assert_eq!(app.placement, Placement::FrontLeftHigh);
    app.channel = 1;
    app.select_next_placement();
    assert_eq!(app.placement, Placement::FrontLeftHigh);
    let ctx = egui::Context::default();
    design::configure(&ctx);
    app.tab = 1;
    app.placement = Placement::FrontLeftHigh;
    let size = Vec2::new(1280., 860.);
    frame(&mut app, &ctx, size, vec![]);
    let output = frame(&mut app, &ctx, size, vec![]);
    let pos = text_centre(&output, "Low positions");
    click(&mut app, &ctx, size, pos);
    assert_eq!(app.placement, Placement::FrontLeftLow);
}

fn simulated_response(channel: usize, placement: Placement, included: bool) -> Measurement {
    Measurement {
        channel,
        position: 0,
        placement: Some(placement),
        points: vec![],
        source: "simulated capture".into(),
        included,
        calibration_applied: false,
    }
}

#[test]
fn completed_pair_saves_both_and_advances_independently_of_review_speaker() {
    let mut app = App::offline(None, false);
    app.channel = 1;
    app.accept_pair(PairCapture {
        measurements: vec![
            simulated_response(0, Placement::Main, false),
            simulated_response(1, Placement::Main, false),
        ],
        failure: None,
    });
    assert_eq!(app.session.measurements.len(), 2);
    assert_eq!(app.placement, Placement::FrontLeftHigh);
    assert_eq!(app.channel, 1);
    assert!(app.status.contains("Review"));
    assert!(app.session.measurements.iter().all(|m| !m.included));
}

#[test]
fn incomplete_pair_preserves_completed_response_and_stays_at_same_position() {
    let mut app = App::offline(None, false);
    app.placement = Placement::BackCenterHigh;
    app.accept_pair(PairCapture {
        measurements: vec![simulated_response(0, Placement::BackCenterHigh, true)],
        failure: Some("Right speaker capture failed".into()),
    });
    assert_eq!(app.session.measurements.len(), 1);
    assert_eq!(app.placement, Placement::BackCenterHigh);
    assert!(app.status.contains("1 of 2"));
    assert!(app.error.unwrap().contains("Right speaker"));
}

#[test]
fn reviewing_other_speaker_does_not_change_position_or_output_routing() {
    let ctx = egui::Context::default();
    design::configure(&ctx);
    let mut app = App::offline(None, false);
    app.tab = 1;
    app.placement = Placement::FrontRightHigh;
    app.output_channels = [3, 5];
    let size = Vec2::new(1280., 1000.);
    frame(&mut app, &ctx, size, vec![]);
    let output = frame(&mut app, &ctx, size, vec![]);
    let pos = text_centre(&output, "Left speaker");
    click(&mut app, &ctx, size, pos);
    let output = frame(&mut app, &ctx, size, vec![]);
    let pos = text_centre(&output, "Right speaker");
    click(&mut app, &ctx, size, pos);
    assert_eq!(app.channel, 1);
    assert_eq!(app.placement, Placement::FrontRightHigh);
    assert_eq!(app.output_channels, [3, 5]);
    assert!(app.job.is_none());
}

#[test]
fn measurement_progress_keeps_job_open_until_pair_result() {
    let mut app = App::offline(None, false);
    let (tx, rx) = mpsc::channel();
    app.job = Some(rx);
    app.measuring_pair = true;
    tx.send(Ok(Done::Progress("Measuring left · 1 of 2".into())))
        .unwrap();
    tx.send(Ok(Done::Measurements(PairCapture {
        measurements: vec![
            simulated_response(0, Placement::Main, true),
            simulated_response(1, Placement::Main, true),
        ],
        failure: None,
    })))
    .unwrap();
    app.poll();
    assert!(app.job.is_some());
    assert!(app.measuring_pair);
    assert!(app.status.contains("1 of 2"));
    assert!(app.session.measurements.is_empty());
    app.poll();
    assert!(app.job.is_none());
    assert!(!app.measuring_pair);
    assert_eq!(app.session.measurements.len(), 2);
    assert_eq!(app.placement, Placement::FrontLeftHigh);
}

#[test]
fn stop_test_control_cancels_pair_without_starting_another_job() {
    let ctx = egui::Context::default();
    design::configure(&ctx);
    let mut app = App::offline(None, false);
    let (_tx, rx) = mpsc::channel();
    app.job = Some(rx);
    app.measuring_pair = true;
    app.status = "Measuring left · 1 of 2".into();
    let size = Vec2::new(1280., 860.);
    frame(&mut app, &ctx, size, vec![]);
    let output = frame(&mut app, &ctx, size, vec![]);
    let pos = text_centre(&output, "Stop test");
    click(&mut app, &ctx, size, pos);
    assert!(app.cancel.load(Ordering::Relaxed));
    assert!(app.job.is_some());
    assert!(app.status.contains("Stopping"));
}

/// Optional deterministic visual snapshots of the actual egui paint output.
/// No window, microphone, device discovery, or amplifier connection is opened.
#[test]
fn render_all_pages_for_visual_review() {
    let destination = std::env::var_os("OPENWAVE_UI_SNAPSHOTS").map(std::path::PathBuf::from);
    if let Some(ref path) = destination {
        std::fs::create_dir_all(path).unwrap();
    }
    for (name, theme, size) in [
        ("light", egui::Theme::Light, Vec2::new(1280., 1000.)),
        ("dark", egui::Theme::Dark, Vec2::new(980., 720.)),
    ] {
        for tab in 0..4 {
            let ctx = egui::Context::default();
            design::configure(&ctx);
            ctx.set_theme(theme);
            let mut app = App::offline(None, true);
            app.tab = tab;
            if tab == 1 {
                app.placement = Placement::BackRightLow;
                app.high_plane = false;
            }
            let mut textures = Vec::new();
            let mut output = None;
            for _ in 0..3 {
                let out = frame(&mut app, &ctx, size, vec![]);
                if destination.is_some() {
                    for (id, delta) in &out.textures_delta.set {
                        let pixels: Vec<[u8; 4]> = match &delta.image {
                            egui::ImageData::Color(c) => {
                                c.pixels.iter().map(Color32::to_array).collect()
                            }
                            egui::ImageData::Font(f) => {
                                f.srgba_pixels(None).map(|c| c.to_array()).collect()
                            }
                        };
                        textures.push(serde_json::json!({ "id": format!("{id:?}"), "size": delta.image.size(), "pos": delta.pos, "pixels": pixels }));
                    }
                }
                output = Some(out);
            }
            let output = output.unwrap();
            assert!(!output.shapes.is_empty());
            assert!(app.job.is_none());
            if tab == 1 {
                let review = text_centre(&output, "Review speaker");
                let progress = text_centre(&output, "0 / 13 positions captured for both speakers");
                assert!(
                    review.x < size.x && progress.x < size.x,
                    "Measurement header overflows {name} window"
                );
            }
            if let Some(ref path) = destination {
                let meshes: Vec<_> = ctx.tessellate(output.shapes, output.pixels_per_point).into_iter().filter_map(|p| {
                    if let egui::epaint::Primitive::Mesh(m) = p.primitive {
                        Some(serde_json::json!({ "clip": [p.clip_rect.min.x, p.clip_rect.min.y, p.clip_rect.max.x, p.clip_rect.max.y], "texture": format!("{:?}", m.texture_id), "indices": m.indices, "vertices": m.vertices.iter().map(|v| serde_json::json!([v.pos.x,v.pos.y,v.uv.x,v.uv.y,v.color.to_array()])).collect::<Vec<_>>() }))
                    } else { None }
                }).collect();
                std::fs::write(path.join(format!("{name}-{tab}.json")), serde_json::to_vec(&serde_json::json!({ "size": [size.x,size.y], "textures": textures, "meshes": meshes })).unwrap()).unwrap();
            }
        }
    }
}
