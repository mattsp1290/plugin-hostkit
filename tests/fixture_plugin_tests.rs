use plugin_hostkit::{ProcessConfig, Vst3Bundle, VstInstance};
#[test]
fn fixture_lifecycle_produces_audio() {
    let Some(path) = std::env::var_os("VST3_TEST_PLUGIN") else {
        assert_ne!(
            std::env::var("REQUIRE_TEST_PLUGIN").as_deref(),
            Ok("1"),
            "VST3_TEST_PLUGIN required"
        );
        println!("SKIPPED: VST3_TEST_PLUGIN unset");
        return;
    };
    eprintln!("fixture: loading");
    let bundle = Vst3Bundle::from_path(std::path::Path::new(&path)).expect("fixture bundle");
    let mut instance =
        VstInstance::load(&bundle.binary_path.expect("fixture binary")).expect("load");
    eprintln!("fixture: initialize");
    instance.initialize().expect("initialize");
    eprintln!("fixture: setup_processing");
    instance
        .setup_processing(ProcessConfig::default())
        .expect("setup_processing");
    eprintln!("fixture: activate");
    instance.activate().expect("activate");
    let mut channels =
        vec![vec![0.0; 512]; instance.primary_output_channels().expect("output layout")];
    assert!(!channels.is_empty(), "instrument needs output channels");
    let mut audible = false;
    eprintln!("fixture: validate buffer boundaries");
    let mut too_short = [0.0f32; 1];
    let mut normal = [0.0f32; 512];
    assert!(
        instance
            .process(&[], &mut [&mut normal, &mut too_short])
            .is_err()
    );
    assert!(instance.process(&[], &mut []).is_err());
    let mut oversized = vec![vec![0.0f32; 513]; channels.len()];
    let mut output: Vec<_> = oversized.iter_mut().map(Vec::as_mut_slice).collect();
    assert!(instance.process(&[], &mut output).is_err());
    let mut output: Vec<_> = channels.iter_mut().map(Vec::as_mut_slice).collect();
    assert!(instance.process(&[(512, 60, 100, 0)], &mut output).is_err());
    assert!(instance.process(&[(0, 60, 100, 16)], &mut output).is_err());
    eprintln!("fixture: process");
    for block in 0..16 {
        let mut output: Vec<_> = channels.iter_mut().map(Vec::as_mut_slice).collect();
        let events: &[(u32, u8, u8, u8)] = if block == 0 { &[(0, 60, 100, 0)] } else { &[] };
        instance.process(events, &mut output).expect("process");
        assert!(channels.iter().flatten().all(|v| v.is_finite()));
        audible |= channels.iter().flatten().any(|v| v.abs() > 1e-6);
    }
    eprintln!("fixture: deactivate");
    instance.deactivate().expect("deactivate");
    eprintln!("fixture: terminate");
    instance.terminate().expect("terminate");
    assert!(audible, "note-on must produce non-silent audio");
}
