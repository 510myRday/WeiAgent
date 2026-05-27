use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

fn main() {
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/locales");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/package-lock.json");
    println!("cargo:rerun-if-env-changed=WEIAGENT_FORCE_FRONTEND_BUILD");

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
