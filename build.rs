#[cfg(target_os = "windows")]
use std::env;
#[cfg(target_os = "windows")]
use std::path::Path;

#[cfg(target_os = "windows")]
use chrono::Local;
#[cfg(target_os = "windows")]
use winres::{VersionInfo, WindowsResource};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/app.ico");

    #[cfg(target_os = "windows")]
    export_build_info();

    #[cfg(target_os = "windows")]
    compile_windows_resources();
}

#[cfg(target_os = "windows")]
fn export_build_info() {
    let build_date = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let author = "WETQV";
    let built_by = resolve_build_author();

    println!("cargo:rustc-env=ZAPRET_HUB_BUILD_DATE={build_date}");
    println!("cargo:rustc-env=ZAPRET_HUB_AUTHOR={author}");
    println!("cargo:rustc-env=ZAPRET_HUB_BUILT_BY={built_by}");
}

#[cfg(target_os = "windows")]
fn compile_windows_resources() {
    if let Err(error) = try_compile_windows_resources() {
        panic!("failed to compile windows resources: {error}");
    }
}

#[cfg(target_os = "windows")]
fn try_compile_windows_resources() -> Result<(), Box<dyn std::error::Error>> {
    let package_version = env::var("CARGO_PKG_VERSION")?;
    let build_date = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let built_by = resolve_build_author();
    let numeric_version = encode_version(&package_version);
    let requested_execution_level = if cfg!(debug_assertions) {
        "asInvoker"
    } else {
        "requireAdministrator"
    };

    let mut resource = WindowsResource::new();

    resource
        .set("CompanyName", "WETQV")
        .set("ProductName", "Zapret Hub")
        .set(
            "FileDescription",
            "Windows GUI utility for managing a local Zapret bundle",
        )
        .set("OriginalFilename", "zapret-hub-rs.exe")
        .set("ProductVersion", &package_version)
        .set("FileVersion", &package_version)
        .set("Comments", &format!("Built by {built_by} on {build_date}"))
        .set("InternalName", "zapret-hub-rs.exe")
        .set("LegalCopyright", "Copyright (c) 2026 WETQV")
        .set_manifest(
            &format!(
                r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="{requested_execution_level}" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#,
            ),
        )
        .set_version_info(VersionInfo::PRODUCTVERSION, numeric_version)
        .set_version_info(VersionInfo::FILEVERSION, numeric_version);

    if let Some(icon_path) = find_icon_path() {
        resource.set_icon(icon_path.to_string_lossy().as_ref());
    }

    resource.compile()?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn resolve_build_author() -> String {
    env::var("ZAPRET_HUB_BUILT_BY")
        .or_else(|_| env::var("GITHUB_ACTOR"))
        .unwrap_or_else(|_| "WETQV".to_owned())
}

#[cfg(target_os = "windows")]
fn encode_version(version: &str) -> u64 {
    let mut parts = [0_u64; 4];

    for (index, part) in version.split(['.', '-']).take(4).enumerate() {
        parts[index] = part.parse::<u64>().unwrap_or(0);
    }

    (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16) | parts[3]
}

#[cfg(target_os = "windows")]
fn find_icon_path() -> Option<std::path::PathBuf> {
    let icons_dir = Path::new("assets").join("icons");

    Some(icons_dir.join("app.ico")).filter(|path| path.is_file())
}
