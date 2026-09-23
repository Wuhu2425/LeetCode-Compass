//! 构建脚本：把应用图标与版本信息嵌入 Windows 可执行文件。
//!
//! 实现说明：**刻意不使用 `winresource`/`embed-resource` 等构建依赖**——
//! 它们靠 `reg.exe` 查注册表定位资源编译器，在受限环境下会被拦截。
//! 这里直接在 Windows Kits 标准安装路径下搜索最新版 `rc.exe`，
//! 手动编译 `.rc` 为 `.res`，再经 `cargo:rustc-link-arg-bins` 交给链接器。

use std::path::{Path, PathBuf};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // 允许用环境变量显式指定 rc.exe；否则在 Windows Kits 标准路径下
    // 搜索版本号最高的一个。
    let rc = std::env::var("RC_PATH").map(PathBuf::from).ok().or_else(find_rc);
    let Some(rc) = rc else {
        println!("cargo:warning=未找到 rc.exe，跳过图标/版本信息嵌入");
        return;
    };

    let out = std::env::var("OUT_DIR").expect("OUT_DIR 未设置");
    let rc_file = Path::new(&out).join("app.rc");
    let res_file = Path::new(&out).join("app.res");

    // 版本号来自 Cargo.toml（0.1.0 → 0,1,0,0）。
    let v: Vec<&str> = env!("CARGO_PKG_VERSION").split('.').collect();
    let (maj, min, pat) = (v[0], v[1], v[2]);

    std::fs::write(
        &rc_file,
        format!(
            r#"1 ICON "assets\\icon.ico"

1 VERSIONINFO
FILEVERSION {maj},{min},{pat},0
PRODUCTVERSION {maj},{min},{pat},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "080404B0"
    BEGIN
      VALUE "FileDescription", "LeetCode Compass"
      VALUE "ProductName", "LeetCode Compass"
      VALUE "FileVersion", "{maj}.{min}.{pat}.0"
      VALUE "ProductVersion", "{maj}.{min}.{pat}.0"
      VALUE "OriginalFilename", "leetcode-compass.exe"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0804, 1200
  END
END
"#
        ),
    )
    .expect("写入 app.rc 失败");

    let status = std::process::Command::new(&rc)
        .args([
            "/nologo",
            &format!("/fo{}", res_file.display()),
            &rc_file.display().to_string(),
        ])
        .current_dir(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 未设置"))
        .status()
        .expect("rc.exe 启动失败");
    assert!(status.success(), "rc.exe 编译资源失败");

    println!("cargo:rustc-link-arg-bins={}", res_file.display());
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
}

/// 在 Windows Kits 标准安装路径下搜索版本号最高的 `rc.exe`。
fn find_rc() -> Option<PathBuf> {
    let roots = [
        r"C:/Program Files (x86)/Windows Kits/10/bin",
        r"C:/Program Files/Windows Kits/10/bin",
    ];
    let mut best: Option<PathBuf> = None;
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for ver in entries.flatten() {
            let ver_path = ver.path();
            let Some(name) = ver_path.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                continue; // 只要版本号目录
            }
            for arch in ["x64", "x86"] {
                let candidate = ver_path.join(arch).join("rc.exe");
                if candidate.is_file() {
                    let better = best
                        .as_ref()
                        .map(|b| candidate > *b)
                        .unwrap_or(true);
                    if better {
                        best = Some(candidate);
                    }
                }
            }
        }
    }
    best
}
