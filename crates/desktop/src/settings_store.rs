use std::fs;
use std::path::PathBuf;

const FACTAL_KEY_FILE: &str = ".1kee_factal_api_key";

pub fn load_factal_api_key() -> Option<String> {
    let path = settings_path()?;
    let value = fs::read_to_string(path).ok()?;
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub fn save_factal_api_key(api_key: &str) -> std::io::Result<()> {
    let path = settings_path()
        .ok_or_else(|| std::io::Error::other("unable to resolve workspace settings path"))?;
    if api_key.trim().is_empty() {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        fs::write(path, format!("{}\n", api_key.trim()))
    }
}

fn settings_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.ancestors().nth(2)?;
    Some(workspace_root.join(FACTAL_KEY_FILE))
}
