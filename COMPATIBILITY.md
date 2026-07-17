# Compatibility

WeiAgent publishes native builds for the following production targets:

| Operating system | Architectures | Baseline |
| --- | --- | --- |
| Windows | x64 | Windows 10 22H2 or newer |
| Windows | ARM64 | Windows 11 or newer |
| macOS | Intel x64, Apple Silicon ARM64 | macOS 13 or newer |
| Linux (glibc) | x64, ARM64 | glibc 2.39 or newer |
| Linux (musl) | x64, ARM64 | musl 1.2 or newer |

The project does not publish 32-bit x86 or ARMv7 builds.

## Managed environments

- Codex Client, Node.js, uv, and Python use upstream native release assets.
  Linux Node.js and uv downloads distinguish glibc from musl.
- Git uses MinGit on Windows. On macOS, installation uses Homebrew. On Linux,
  installation uses the detected distribution package manager: apt, dnf, yum,
  zypper, pacman, or apk.
- Existing system installations are detected before WeiAgent offers an
  installation.

CI executes builds, tests, and Clippy on native x64 and ARM64 runners for all
three operating systems, plus musl release checks. Version tags produce eight
archives with SHA-256 files.
