use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let out_dir = env::var_os("OUT_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Cargo did not set OUT_DIR"))?;
    let out = PathBuf::from(out_dir);
    fs::write(out.join("memory.x"), include_bytes!("memory.x"))?;
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    Ok(())
}
