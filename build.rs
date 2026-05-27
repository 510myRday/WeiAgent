use std::process::Command;

fn main() {
    // Only rebuild frontend when dist is missing or sources changed
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/locales");
    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");

    let status = Command::new("cmd.exe")
        .args(["/d", "/c", "cd frontend && npm install --silent && npx vite build"])
        .status()
        .expect("failed to run Vite build; make sure Node.js is installed");

    if !status.success() {
        panic!("Vite build failed with exit code {:?}", status.code());
    }

    println!("cargo:rerun-if-changed=frontend/dist");
}
