use plugin_hostkit::PluginScanner;
fn main() {
    for bundle in PluginScanner::new().discover_bundles() {
        println!("{}: {}", bundle.name, bundle.path.display());
    }
}
