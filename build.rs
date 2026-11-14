fn main() {
    println!("cargo:rerun-if-env-changed=GARAGE_DISTILLER_UPDATE_PUBLIC_KEY_HEX");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .set("ProductName", "Garage Distiller")
            .set("FileDescription", "Garage Distiller — annonces des garages")
            .set("InternalName", "garage-distiller.exe")
            .compile()
            .expect("Unable to compile the Windows application icon and version resources");
    }
}
