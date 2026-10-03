use plugin_hostkit::PresetOrigin;
#[test]
fn preset_manager_factory_presets_have_correct_source() {
    let mut mgr = plugin_hostkit::preset::PresetManager::new("test".into(), "Test Plugin".into());
    mgr.add_factory_presets(vec!["Init".into(), "Pad".into()]);

    let presets = mgr.presets();
    assert_eq!(presets.len(), 2);
    assert!(presets.iter().all(|p| p.origin == PresetOrigin::Factory));
}

#[test]
fn query_factory_presets_returns_empty_stub() {
    let mut mgr = plugin_hostkit::preset::PresetManager::new("test".into(), "Test Plugin".into());
    let presets = mgr.query_factory_presets();
    assert!(presets.is_empty());
}
