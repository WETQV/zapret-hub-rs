// Build scripts run on the *host*, so `cfg(target_os)` here would describe Ubuntu when
// cross-compiling. Everything target-specific goes through CARGO_CFG_TARGET_OS instead.
use std::env;
use std::path::Path;

use chrono::Local;
use winresource::{VersionInfo, WindowsResource};

const AUTHOR: &str = "WETQV";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icons/app.ico");
    println!("cargo:rerun-if-env-changed=ZAPRET_HUB_BUILT_BY");

    let build_date = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let built_by = resolve_build_author();

    println!("cargo:rustc-env=ZAPRET_HUB_BUILD_DATE={build_date}");
    println!("cargo:rustc-env=ZAPRET_HUB_AUTHOR={AUTHOR}");
    println!("cargo:rustc-env=ZAPRET_HUB_BUILT_BY={built_by}");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && let Err(error) = compile_windows_resources(&build_date, &built_by)
    {
        panic!("failed to compile windows resources: {error}");
    }
}

fn compile_windows_resources(
    build_date: &str,
    built_by: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let package_version = env::var("CARGO_PKG_VERSION")?;
    let numeric_version = encode_version(&package_version);
    let requested_execution_level = if env::var("PROFILE").as_deref() == Ok("release") {
        "requireAdministrator"
    } else {
        "asInvoker"
    };

    let mut resource = WindowsResource::new();
    // winresource picks `x86_64-w64-mingw32-windres` on its own when cross-compiling to *-gnu.
    resource
        .set("CompanyName", AUTHOR)
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
        .set_manifest(&format!(
            r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="{requested_execution_level}" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
"#,
        ))
        .set_version_info(VersionInfo::PRODUCTVERSION, numeric_version)
        .set_version_info(VersionInfo::FILEVERSION, numeric_version);

    let icon_path = Path::new("assets").join("icons").join("app.ico");
    if icon_path.is_file() {
        resource.set_icon(icon_path.to_string_lossy().as_ref());
    }

    resource.compile()?;
    Ok(())
}

fn resolve_build_author() -> String {
    env::var("ZAPRET_HUB_BUILT_BY")
        .or_else(|_| env::var("GITHUB_ACTOR"))
        .unwrap_or_else(|_| AUTHOR.to_owned())
}

fn encode_version(version: &str) -> u64 {
    let mut parts = [0_u64; 4];

    for (index, part) in version.split(['.', '-']).take(4).enumerate() {
        parts[index] = part.parse::<u64>().unwrap_or(0);
    }

    (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16) | parts[3]
}
