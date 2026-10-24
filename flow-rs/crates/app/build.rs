//! Embeds the app icon, version information and the application manifest
//! (per-monitor DPI awareness, UTF-8, long paths) into Flow.exe.

use std::path::PathBuf;

fn rc_path(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("../../../resources/icons/icon.ico");
    let manifest = manifest_dir.join("resources/Flow.manifest");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", manifest.display());

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let parts: Vec<u16> = version.split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    let (major, minor, patch) = (parts[0], parts.get(1).copied().unwrap_or(0), parts.get(2).copied().unwrap_or(0));

    let rc = format!(
        r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Flow contributors"
      VALUE "FileDescription", "Flow"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "Flow"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "Flow.exe"
      VALUE "ProductName", "Flow"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = rc_path(&icon),
        manifest = rc_path(&manifest),
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("flow.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
}
