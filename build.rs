use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

const WINDOWS_ICON: &str = "design/weiagent-logo-final/weiagent-app-icon.ico";
const BUNDLED_RIPGREP_ROOT: &str = ".tools/bundled-ripgrep";

fn main() {
    println!("cargo:rerun-if-changed={WINDOWS_ICON}");
    println!("cargo:rerun-if-changed={BUNDLED_RIPGREP_ROOT}");
    println!("cargo:rerun-if-env-changed=WEIAGENT_BUNDLED_RG_PATH");
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/public");
    println!("cargo:rerun-if-changed=frontend/locales");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/package-lock.json");
    println!("cargo:rerun-if-env-changed=WEIAGENT_FORCE_FRONTEND_BUILD");

    embed_windows_resources();
    generate_bundled_ripgrep();

    let frontend_dir = Path::new("frontend");
    let dist_index = frontend_dir.join("dist").join("index.html");
    let force_build = env::var("WEIAGENT_FORCE_FRONTEND_BUILD").as_deref() == Ok("1");

    if !force_build && frontend_dist_is_fresh(&dist_index) {
        return;
    }

    ensure_frontend_dependencies(frontend_dir);

    let build_status = Command::new(npm_command())
        .args(["run", "build"])
        .current_dir(frontend_dir)
        .status()
        .expect("failed to run frontend build; make sure Node.js is installed");

    if !build_status.success() {
        panic!(
            "frontend build failed with exit code {:?}",
            build_status.code()
        );
    }

    println!("cargo:rerun-if-changed=frontend/dist");
}

fn generate_bundled_ripgrep() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let generated = out_dir.join("bundled_ripgrep.rs");
    let target = env::var("TARGET").expect("TARGET is set by Cargo");
    let default_path =
        Path::new(BUNDLED_RIPGREP_ROOT)
            .join(&target)
            .join(if target.contains("windows") {
                "rg.exe"
            } else {
                "rg"
            });
    let path = env::var_os("WEIAGENT_BUNDLED_RG_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .unwrap_or(default_path);

    if !path.is_file() {
        println!("cargo:warning=no bundled ripgrep asset for target {target}; using the host PATH");
        fs::write(
            generated,
            "const BUNDLED_RIPGREP: Option<BundledTool> = None;\n",
        )
        .expect("write bundled ripgrep metadata");
        return;
    }

    println!("cargo:rerun-if-changed={}", path.display());
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .expect("bundled ripgrep path has a UTF-8 file name");
    let path_literal = path
        .canonicalize()
        .unwrap_or_else(|_| path.clone())
        .to_string_lossy()
        .replace('\\', "/");
    let source = format!(
        "const BUNDLED_RIPGREP: Option<BundledTool> = Some(BundledTool {{ file_name: {file_name:?}, bytes: include_bytes!(r#\"{path_literal}\"#) }});\n"
    );
    fs::write(generated, source).expect("write bundled ripgrep metadata");
}

fn embed_windows_resources() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(WINDOWS_ICON);
    resource.set("ProductName", "WeiAgent");
    resource.set("FileDescription", "WeiAgent");
    resource
        .compile()
        .expect("failed to embed the WeiAgent Windows icon");
}

fn ensure_frontend_dependencies(frontend_dir: &Path) {
    if frontend_dir.join("node_modules").exists() {
        return;
    }

    let command = if frontend_dir.join("package-lock.json").exists() {
        "ci"
    } else {
        "install"
    };
    let install_status = Command::new(npm_command())
        .arg(command)
        .arg("--silent")
        .current_dir(frontend_dir)
        .status()
        .expect("failed to install frontend dependencies; make sure Node.js is installed");

    if !install_status.success() {
        panic!(
            "npm {command} failed with exit code {:?}",
            install_status.code()
        );
    }
}

fn frontend_dist_is_fresh(dist_index: &Path) -> bool {
    let Ok(dist_time) = modified_time(dist_index) else {
        return false;
    };

    let mut newest_source = SystemTime::UNIX_EPOCH;
    for path in [
        PathBuf::from("frontend/src"),
        PathBuf::from("frontend/public"),
        PathBuf::from("frontend/locales"),
        PathBuf::from("frontend/index.html"),
        PathBuf::from("frontend/package.json"),
        PathBuf::from("frontend/package-lock.json"),
    ] {
        update_newest_mtime(&path, &mut newest_source);
    }

    dist_time >= newest_source
}

fn update_newest_mtime(path: &Path, newest: &mut SystemTime) {
    if path.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            update_newest_mtime(&entry.path(), newest);
        }
        return;
    }

    if let Ok(time) = modified_time(path) {
        *newest = (*newest).max(time);
    }
}

fn modified_time(path: &Path) -> std::io::Result<SystemTime> {
    fs::metadata(path)?.modified()
}

fn npm_command() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}
