use std::process::Command;

fn main() {
    // Only rebuild frontend when dist is missing or sources changed
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/locales");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");

    let install_status = Command::new(npm_command())
        .args(["install", "--silent"])
        .current_dir("frontend")
        .status()
        .expect("failed to run npm install; make sure Node.js is installed");

    if !install_status.success() {
        panic!(
            "npm install failed with exit code {:?}",
            install_status.code()
        );
    }

    let build_status = Command::new(npm_command())
        .args(["run", "build"])
        .current_dir("frontend")
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

fn npm_command() -> &'static str {
    if cfg!(windows) { "npm.cmd" } else { "npm" }
}
