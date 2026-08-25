//! Reasons a stored session cannot be continued from this run.

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ResumeBlock {
    #[error("session belongs to {0}; cd there and run `maki -c <ID>` from that directory")]
    OtherCwd(String),
}

pub fn other_cwd_block(session_cwd: &str, current_cwd: &str) -> Option<ResumeBlock> {
    (session_cwd != current_cwd).then(|| ResumeBlock::OtherCwd(session_cwd.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    const HERE: &str = "/here";
    const ELSEWHERE: &str = "/elsewhere";

    #[test_case(HERE, HERE, false; "equal_cwds")]
    #[test_case(ELSEWHERE, HERE, true; "different_cwds")]
    fn other_cwd_block_matrix(session_cwd: &str, current_cwd: &str, blocked: bool) {
        let block = other_cwd_block(session_cwd, current_cwd);
        assert_eq!(block.is_some(), blocked);
        if let Some(block) = &block {
            assert_eq!(block, &ResumeBlock::OtherCwd(session_cwd.to_owned()));
        }
    }
}
