fn main() {
    let schema = "src/transit/gtfs-realtime.proto";
    println!("cargo:rerun-if-changed={schema}");
    protobuf_codegen_pure::Codegen::new()
        .out_dir(std::env::var_os("OUT_DIR").expect("Cargo output directory"))
        .include("src/transit")
        .input(schema)
        .customize(protobuf_codegen_pure::Customize {
            gen_mod_rs: Some(true),
            ..Default::default()
        })
        .run()
        .expect("generate GTFS Realtime bindings");
}
