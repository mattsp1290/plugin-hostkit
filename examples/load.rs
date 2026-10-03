use plugin_hostkit::{ProcessConfig, Vst3Bundle, VstInstance};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Usage: load <bundle.vst3>")?;
    let bundle = Vst3Bundle::from_path(std::path::Path::new(&path)).ok_or("Invalid bundle")?;
    let binary = bundle.binary_path.ok_or("Bundle contains no binary")?;
    let mut instance = VstInstance::load(&binary)?;
    instance.initialize()?;
    instance.setup_processing(ProcessConfig::default())?;
    instance.activate()?;
    let mut channels = vec![vec![0.0; 512]; instance.primary_output_channels()?];
    let mut output: Vec<_> = channels.iter_mut().map(Vec::as_mut_slice).collect();
    instance.process(&[(0, 60, 100, 0)], &mut output)?;
    let peak = channels
        .iter()
        .flatten()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    eprintln!("First block peak: {peak}");
    println!("{}", instance.name());
    instance.deactivate()?;
    instance.terminate()?;
    Ok(())
}
