use std::fs;
use std::path::Path;

fn main() {
    // Ensure web/dist exists so `rust-embed` compiles even before `npm run build`.
    let dist_dir = Path::new("web/dist");
    if !dist_dir.exists()
        && let Err(err) = fs::create_dir_all(dist_dir)
    {
        panic!("failed to create web/dist directory: {err}");
    }
    println!("cargo:rerun-if-changed=web/dist");
}
