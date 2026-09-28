use std::path::PathBuf;

#[derive(Debug)]
pub struct Command {
    input: PathBuf,
    out: PathBuf,
}
pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let (mut input, mut out) = (None, None);
    while let Some(flag) = args.next() {
        let value = PathBuf::from(
            args.next()
                .ok_or_else(|| format!("Missing value for {flag}"))?,
        );
        match flag.as_str() {
            "--input" => input = Some(value),
            "--out" => out = Some(value),
            _ => return Err(format!("Unknown platform option {flag}")),
        }
    }
    Ok(Command {
        input: input.ok_or("Missing --input")?,
        out: out.ok_or("Missing --out")?,
    })
}
pub fn run(command: Command) -> Result<(), String> {
    let count = tile_archive::platforms::build(&command.input, &command.out)?;
    println!(
        "Published {count} offshore installations to {}",
        command.out.display()
    );
    Ok(())
}
