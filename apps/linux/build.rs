fn main() {
    println!("cargo:rustc-link-lib=fontconfig");
    println!("cargo:rustc-link-lib=pangocairo-1.0");
}
