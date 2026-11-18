//! Gives the Windows program its icon and its name.
//!
//! On Windows both live in the program file as resources: icon 1 is what the
//! Explorer and the taskbar show for the file and what GPUI gives its
//! windows, and the version block is where the task manager reads "kou" from.
//! The icon file comes from `script/icons`. Nothing is done for another
//! system: a macOS bundle and a Linux desktop entry carry theirs as files.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/windows/kou.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo"));
    let version = env::var("CARGO_PKG_VERSION").expect("set by cargo");
    let icon = manifest.join("resources/windows/kou.ico");
    let script = out.join("kou.rc");
    fs::write(&script, resource_script(&icon.to_string_lossy(), &version))
        .expect("write the resource script");
    embed_resource::compile(&script, embed_resource::NONE)
        .manifest_optional()
        .expect("compile the program's resources");
}

/// The resource script: the icon under the number GPUI looks it up by, and
/// the version block.
fn resource_script(icon: &str, version: &str) -> String {
    // A resource script reads a backslash in a string as an escape.
    let icon = icon.replace('\\', "/");
    // Four numbers, whatever the version has of them: "0.1.0-rc.1" is 0,1,0,0.
    let mut numbers: Vec<u16> = version
        .split(['.', '-', '+'])
        .map_while(|part| part.parse().ok())
        .take(4)
        .collect();
    numbers.resize(4, 0);
    let numbers = numbers
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription", "kou\0"
            VALUE "ProductName", "kou\0"
            VALUE "InternalName", "kou-desktop\0"
            VALUE "OriginalFilename", "kou-desktop.exe\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}
