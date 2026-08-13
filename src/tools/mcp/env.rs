// Internal MCP transport/auth helpers split out of the manager module.
pub(super) fn safe_baseline_environment(
    inherit_env: bool,
    allowlist: &[String],
) -> std::collections::BTreeMap<String, String> {
    let mut result = std::collections::BTreeMap::new();
    let baseline = [
        "HOME",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "SYSTEMROOT",
        "COMSPEC",
        "WINDIR",
        "PATHEXT",
        "HOMEDRIVE",
        "HOMEPATH",
        "ALLUSERSPROFILE",
        "PROGRAMDATA",
        "APPDATA",
        "LOCALAPPDATA",
        // Package managers need the host's network settings even when a server
        // opts out of inheriting the complete process environment.
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "UV_CACHE_DIR",
        "UV_TOOL_DIR",
        "UV_TOOL_BIN_DIR",
        "UV_HTTP_TIMEOUT",
        "UV_PYTHON_INSTALL_MIRROR",
        "NPM_CONFIG_REGISTRY",
    ];
    let inherited = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
    for key in baseline
        .iter()
        .copied()
        .chain(allowlist.iter().map(String::as_str))
    {
        if let Some((_, value)) = inherited
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
        {
            result.insert(key.to_string(), value.clone());
        }
    }
    if inherit_env {
        result.extend(inherited);
    }
    result
}

pub(super) fn expand_mcp_value(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let Some(end) = rest[start + 2..].find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let key = &rest[start + 2..start + 2 + end];
        let replacement = match key {
            "userHome" => crate::fsutil::home_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "pathSeparator" | "/" => std::path::MAIN_SEPARATOR.to_string(),
            _ => std::env::var(key).unwrap_or_else(|_| format!("${{{key}}}")),
        };
        output.push_str(&replacement);
        rest = &rest[start + 3 + end..];
    }
    output.push_str(rest);
    output
}
