use std::env;

fn main() {
    println!("cargo:rerun-if-changed=wix/flow-state.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("wix/flow-state.ico")
        .set("ProductName", "Flow State")
        .set("FileDescription", "Flow State writing application")
        .set("CompanyName", "Rohan S")
        .set("InternalName", "flow-state")
        .set("OriginalFilename", "flow-state.exe")
        .set("LegalCopyright", "Copyright (C) 2026 Rohan S");
    resource
        .compile()
        .expect("Windows application resources compile");
}
