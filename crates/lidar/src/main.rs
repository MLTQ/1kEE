fn main() {
    if let Err(error) = lidar_pipeline::builder::run(std::env::args().skip(1)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
