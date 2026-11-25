fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/dblocal.ico");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icons/dblocal.ico");
        res.set("FileDescription", "DBLocal");
        res.set("ProductName", "DBLocal");
        if let Err(err) = res.compile() {
            println!("cargo:warning=failed to compile Windows icon resource: {err}");
            std::process::exit(1);
        }
    }
}
