//! Agent host resolution for host-specific agent-facing wording.
//!
//! Contract: `AGENT_HOST` of `codex` or `claude_code` wins; otherwise
//! `CLAUDECODE=1` (set by Claude Code for MCP servers) selects Claude Code;
//! otherwise the host is Codex. Unknown `AGENT_HOST` values are ignored.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentHost {
    Codex,
    ClaudeCode,
}

impl AgentHost {
    pub fn current() -> Self {
        Self::resolve(
            std::env::var("AGENT_HOST").ok().as_deref(),
            std::env::var("CLAUDECODE").ok().as_deref(),
        )
    }

    pub fn resolve(agent_host: Option<&str>, claudecode: Option<&str>) -> Self {
        match agent_host {
            Some("codex") => Self::Codex,
            Some("claude_code") => Self::ClaudeCode,
            _ if claudecode == Some("1") => Self::ClaudeCode,
            _ => Self::Codex,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AgentHost;

    #[test]
    fn resolves_host_from_agent_host_then_claudecode_then_codex_default() {
        assert_eq!(
            AgentHost::resolve(Some("claude_code"), None),
            AgentHost::ClaudeCode
        );
        assert_eq!(
            AgentHost::resolve(Some("codex"), Some("1")),
            AgentHost::Codex
        );
        assert_eq!(
            AgentHost::resolve(Some("other"), Some("1")),
            AgentHost::ClaudeCode
        );
        assert_eq!(AgentHost::resolve(None, Some("1")), AgentHost::ClaudeCode);
        assert_eq!(AgentHost::resolve(None, None), AgentHost::Codex);
        assert_eq!(AgentHost::resolve(Some("other"), None), AgentHost::Codex);
    }
}
