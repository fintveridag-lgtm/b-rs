fn main() {
    // Bygg inn appikonet i .exe-filen når vi kompilerer for Windows.
    // På andre plattformer er dette en no-op.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "b-rs børs-konsoll");
        res.set("FileDescription", "b-rs børs-konsoll");
        if let Err(e) = res.compile() {
            println!("cargo:warning=klarte ikke bygge inn ikonet: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
