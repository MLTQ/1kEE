use std::path::PathBuf;

#[derive(Debug)]
pub struct Command {
    input: PathBuf,
    manifest: PathBuf,
    out: PathBuf,
}

pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let (mut input, mut manifest, mut out) = (None, None, None);
    while let Some(flag) = args.next() {
        let value = PathBuf::from(
            args.next()
                .ok_or_else(|| format!("Missing value for {flag}"))?,
        );
        match flag.as_str() {
            "--input" => input = Some(value),
            "--manifest" => manifest = Some(value),
            "--out" => out = Some(value),
            _ => return Err(format!("Unknown pipeline option {flag}")),
        }
    }
    Ok(Command {
        input: input.ok_or("Missing --input")?,
        manifest: manifest.ok_or("Missing --manifest")?,
        out: out.ok_or("Missing --out")?,
    })
}

pub fn run(command: Command) -> Result<(), String> {
    let result = tile_archive::pipelines::build::run(
        &command.input,
        &command.manifest,
        &command.out,
        &mut |s| println!("{s}"),
    )?;
    println!("{result}");
    Ok(())
}
