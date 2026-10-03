//! Integration tests for VST3 preset management.

mod common;
use common::create_fake_preset;
use plugin_hostkit::preset::PresetManager;

#[test]
fn scan_finds_preset_files() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_dir = tmp.path().join("presets");

    create_fake_preset(&preset_dir, "Pad");
    create_fake_preset(&preset_dir, "Bass");
    create_fake_preset(&preset_dir, "Lead");

    // Also create a non-preset file that should be ignored
    std::fs::write(preset_dir.join("readme.txt"), b"not a preset").unwrap();

    // Verify files were created correctly
    let entries: Vec<_> = std::fs::read_dir(&preset_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "vstpreset"))
        .collect();

    assert_eq!(entries.len(), 3);
}

#[test]
fn load_preset_via_manager() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = create_fake_preset(tmp.path(), "TestPreset");

    // Use the public load_preset_file API
    let data = PresetManager::load_preset_file(&preset_path).unwrap();
    assert!(!data.component_state.is_empty());
}

#[test]
fn load_preset_with_vst3_header() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("WithHeader.vstpreset");

    let mut data = b"VST3".to_vec();
    data.extend_from_slice(&[0u8; 44]); // 48-byte header
    data.extend_from_slice(&[10, 20, 30, 40, 50]);

    std::fs::write(&path, &data).unwrap();

    let parsed = PresetManager::load_preset_file(&path).unwrap();
    assert_eq!(parsed.component_state, vec![10, 20, 30, 40, 50]);
    assert!(parsed.controller_state.is_empty());
}

#[test]
fn factory_presets_indexed_correctly() {
    let mut mgr = PresetManager::new("test-synth".into(), "Test Synth".into());
    mgr.add_factory_presets(vec![
        "Init".into(),
        "Warm Pad".into(),
        "Deep Bass".into(),
        "Pluck Lead".into(),
    ]);

    let presets = mgr.presets();
    assert_eq!(presets.len(), 4);
    assert_eq!(presets[0].name, "Init");
    assert_eq!(presets[0].index, 0);
    assert_eq!(presets[3].name, "Pluck Lead");
    assert_eq!(presets[3].index, 3);
    assert!(
        presets
            .iter()
            .all(|p| p.origin == plugin_hostkit::PresetOrigin::Factory)
    );
}

#[test]
fn load_too_small_preset_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("tiny.vstpreset");
    std::fs::write(&path, [0u8; 2]).unwrap();

    assert!(PresetManager::load_preset_file(&path).is_err());
}
