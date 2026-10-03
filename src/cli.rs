//! Out-of-process VST3 metadata extractor.
//!
//! Loads a single VST3 binary, queries IPluginFactory2 for metadata,
//! and prints JSON to stdout. Runs in isolation so a crashing plugin
//! cannot corrupt the host process.
//!
//! Usage: plugin-scanner <binary_path>
//! Output: {"vendor":"...","version":"...","subcategories":"..."}
//! Exit 0 on success, 1 on failure.

pub fn scanner_cli_main() -> ! {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: plugin-scanner <binary_path>");
        std::process::exit(1);
    }

    let binary_path = std::path::Path::new(&args[1]);
    match crate::factory::query_factory_metadata(binary_path) {
        Some(meta) => {
            let json = serde_json::json!({
                "vendor": meta.vendor,
                "version": meta.version,
                "subcategories": meta.subcategories,
            });
            println!("{json}");
        }
        None => {
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}
