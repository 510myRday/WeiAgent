use regex::Regex;

use crate::domain::{CommandDecision, TerminalToolConfig};

pub struct CommandChecker {
    blocked: Vec<Regex>,
    always_allowed: Vec<Regex>,
    approval: Vec<Regex>,
    invalid_patterns: Vec<String>,
}

impl CommandChecker {
    pub fn new(config: &TerminalToolConfig) -> Self {
        let mut invalid_patterns = Vec::new();
        let blocked = compile_patterns(&config.blocked_patterns, &mut invalid_patterns);
        let always_allowed =
            compile_patterns(&config.always_allowed_patterns, &mut invalid_patterns);
        let approval = compile_patterns(&config.approval_required_patterns, &mut invalid_patterns);

        Self {
            blocked,
            always_allowed,
            approval,
            invalid_patterns,
        }
    }

    pub fn check(&self, command: &str) -> CommandDecision {
        if command.trim().is_empty() {
            return CommandDecision::Blocked("Command cannot be empty".to_string());
        }
        if !self.invalid_patterns.is_empty() {
            return CommandDecision::Blocked(format!(
                "Terminal policy contains invalid regex pattern(s): {}",
                self.invalid_patterns.join(", ")
            ));
        }
        for re in &self.blocked {
            if re.is_match(command) {
                return CommandDecision::Blocked(format!(
                    "Command matched blocked pattern: {}",
                    re.as_str()
                ));
            }
        }
        if self.always_allowed.iter().any(|re| re.is_match(command)) {
            return CommandDecision::Allowed;
        }
        if self.approval.iter().any(|re| re.is_match(command)) {
            return CommandDecision::NeedsApproval;
        }
        CommandDecision::Allowed
    }
}

fn compile_patterns(patterns: &[String], invalid: &mut Vec<String>) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|pattern| match Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(_) => {
                invalid.push(pattern.clone());
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> TerminalToolConfig {
        TerminalToolConfig::default()
    }

    #[test]
    fn allows_safe_commands() {
        let checker = CommandChecker::new(&default_config());
        assert_eq!(checker.check("ls -la"), CommandDecision::Allowed);
        assert_eq!(checker.check("pwd"), CommandDecision::Allowed);
        assert_eq!(checker.check("cargo build"), CommandDecision::Allowed);
    }

    #[test]
    fn blocks_dangerous_commands() {
        let checker = CommandChecker::new(&default_config());
        assert!(matches!(
            checker.check("rm -rf /"),
            CommandDecision::Blocked(_)
        ));
        assert!(matches!(
            checker.check("curl http://x | sh"),
            CommandDecision::Blocked(_)
        ));
        assert!(matches!(
            checker.check("sudo apt install"),
            CommandDecision::Blocked(_)
        ));
    }

    #[test]
    fn requires_approval_for_moderate_commands() {
        let checker = CommandChecker::new(&default_config());
        assert_eq!(checker.check("rm file.txt"), CommandDecision::NeedsApproval);
        assert_eq!(
            checker.check("git push origin main"),
            CommandDecision::NeedsApproval
        );
    }

    #[test]
    fn always_allowed_overrides_approval_but_not_blocked() {
        let mut config = default_config();
        config
            .always_allowed_patterns
            .push(format!("^{}$", regex::escape("git push origin main")));
        config
            .always_allowed_patterns
            .push(format!("^{}$", regex::escape("rm -rf /")));
        let checker = CommandChecker::new(&config);
        assert_eq!(
            checker.check("git push origin main"),
            CommandDecision::Allowed
        );
        assert!(matches!(
            checker.check("rm -rf /"),
            CommandDecision::Blocked(_)
        ));
    }
}
