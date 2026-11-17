fn main() {
    let result = one_thousand_electric_eye_cache_builder::run();
    one_thousand_electric_eye_cache_builder::gpu_terrain::shutdown();
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
