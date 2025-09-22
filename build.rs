#[cfg(windows)]
fn link_system_libs() {
    println!("cargo:rustc-link-lib=Gdi32");
    println!("cargo:rustc-link-lib=OleAut32");
    println!("cargo:rustc-link-lib=Shlwapi");
    println!("cargo:rustc-link-lib=Mfuuid");
    println!("cargo:rustc-link-lib=Strmiids");
    println!("cargo:rustc-link-lib=Vfw32");
}

fn main() {
    link_system_libs();
}
