fn main() {
    if let Err(error) = one_thousand_electric_eye_cache_builder::run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
