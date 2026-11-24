//! Agent host resolution for host-specific agent-facing wording.
//!
//! Contract: `AGENT_HOST` of `codex`, `claude_code`, or `antigravity` wins; otherwise
//! `CLAUDECODE=1` (set by Claude Code for MCP servers) selects Claude Code;
//! `ANTIGRAVITY_AGENT=1` or `ANTIGRAVITY_LS_ADDRESS` selects Antigravity;
//! otherwise the host is Codex. Unknown `AGENT_HOST` values are ignored.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentHost {
    Codex,
    ClaudeCode,
    Antigravity,
}

impl AgentHost {
    pub fn current() -> Self {
        let antigravity = std::env::var("ANTIGRAVITY_AGENT").ok();
        let antigravity_ls = std::env::var("ANTIGRAVITY_LS_ADDRESS").ok();
        Self::resolve(
            std::env::var("AGENT_HOST").ok().as_deref(),
            std::env::var("CLAUDECODE").ok().as_deref(),
            antigravity.as_deref().or(antigravity_ls.as_deref()),
        )
    }

    pub fn resolve(
        agent_host: Option<&str>,
        claudecode: Option<&str>,
        antigravity: Option<&str>,
    ) -> Self {
        match agent_host {
            Some("codex") => Self::Codex,
            Some("claude_code") => Self::ClaudeCode,
            Some("antigravity") => Self::Antigravity,
            _ if claudecode == Some("1") => Self::ClaudeCode,
            _ if antigravity.is_some_and(|v| !v.is_empty()) => Self::Antigravity,
            _ => Self::Codex,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AgentHost;

    #[test]
    fn resolves_host_from_agent_host_then_claudecode_then_antigravity_then_codex_default() {
        assert_eq!(
            AgentHost::resolve(Some("claude_code"), None, None),
            AgentHost::ClaudeCode
        );
        assert_eq!(
            AgentHost::resolve(Some("antigravity"), None, None),
            AgentHost::Antigravity
        );
        assert_eq!(
            AgentHost::resolve(Some("codex"), Some("1"), Some("1")),
            AgentHost::Codex
        );
        assert_eq!(
            AgentHost::resolve(Some("other"), Some("1"), None),
            AgentHost::ClaudeCode
        );
        assert_eq!(
            AgentHost::resolve(None, Some("1"), None),
            AgentHost::ClaudeCode
        );
        assert_eq!(
            AgentHost::resolve(None, None, Some("1")),
            AgentHost::Antigravity
        );
        assert_eq!(AgentHost::resolve(None, None, None), AgentHost::Codex);
        assert_eq!(
            AgentHost::resolve(Some("other"), None, None),
            AgentHost::Codex
        );
    }
}
