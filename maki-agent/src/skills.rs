use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use tracing::warn;

use crate::command::find_project_ancestor_dirs;

const SKILL_FILE: &str = "SKILL.md";
const PROJECT_SKILL_DIRS: &[&str] = &[
    ".maki/skills",
    ".claude/skills",
    ".opencode/skills",
    ".agents/skills",
];
const GLOBAL_SKILL_DIRS: &[&str] = &[".claude/skills", ".config/opencode/skills", ".agents/skills"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

pub fn enumerate_skills(cwd: &Path) -> Vec<SkillInfo> {
    enumerate_skills_inner(
        cwd,
        maki_config::global_config_dir().as_deref(),
        maki_storage::paths::home().as_deref(),
    )
}

fn enumerate_skills_inner(cwd: &Path, config: Option<&Path>, home: Option<&Path>) -> Vec<SkillInfo> {
    let mut skills: HashMap<String, SkillInfo> = HashMap::new();

    if let Some(config) = config {
        scan_skill_dir(&config.join("skills"), &mut skills);
    }
    if let Some(home) = home {
        for rel in GLOBAL_SKILL_DIRS {
            scan_skill_dir(&home.join(rel), &mut skills);
        }
    }
    for ancestor in find_project_ancestor_dirs(cwd) {
        for rel in PROJECT_SKILL_DIRS {
            scan_skill_dir(&ancestor.join(rel), &mut skills);
        }
    }

    let mut result: Vec<_> = skills.into_values().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

fn scan_skill_dir(dir: &Path, skills: &mut HashMap<String, SkillInfo>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_dir()) {
            continue;
        }
        let skill_dir = entry.path();
        let Ok(content) = fs::read_to_string(skill_dir.join(SKILL_FILE)) else {
            continue;
        };
        let (fm, body) = parse_frontmatter(&content);
        if body.is_empty() {
            continue;
        }
        let name = fm
            .name
            .unwrap_or_else(|| skill_name(&skill_dir));
        let info = SkillInfo {
            name: name.clone(),
            description: fm.description.unwrap_or_default(),
        };
        if let Some(existing) = skills.insert(name, info) {
            warn!(skill = existing.name, path = ?skill_dir, "skill overridden by later priority");
        }
    }
}

fn skill_name(skill_dir: &Path) -> String {
    skill_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn parse_frontmatter(content: &str) -> (SkillFrontmatter, &str) {
    let content = content.trim_start();

    let Some(rest) = content.strip_prefix("---") else {
        return (SkillFrontmatter::default(), content);
    };

    let Some(end) = rest.find("\n---") else {
        return (SkillFrontmatter::default(), content);
    };

    let yaml = &rest[1..end + 1];
    let body = rest[end + 4..].trim();

    let fm = serde_yaml::from_str(yaml).unwrap_or_default();
    (fm, body)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// Writes a skill at `root/<dir>/SKILL.md` (dir is e.g. `.maki/skills/foo`).
    fn write_skill(root: &Path, dir: &str, content: &str) {
        let dir = root.join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SKILL_FILE), content).unwrap();
    }

    #[test]
    fn scan_finds_skills() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), ".maki/skills/basic", "---\nname: basic\n---\nDo things.");

        let skills = enumerate_skills_inner(root.path(), None, None);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "basic");
        assert_eq!(skills[0].description, "");
    }

    #[test]
    fn frontmatter_name_overrides_dirname() {
        let root = TempDir::new().unwrap();
        write_skill(
            root.path(),
            ".maki/skills/my-dir-name",
            "---\nname: review\ndescription: Code review\n---\nReview code.",
        );

        let skills = enumerate_skills_inner(root.path(), None, None);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "review");
        assert_eq!(skills[0].description, "Code review");
    }

    #[test]
    fn dirname_fallback_without_frontmatter() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), ".maki/skills/my-dir", "Just content");

        let skills = enumerate_skills_inner(root.path(), None, None);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-dir");
    }

    #[test]
    fn respects_git_boundary() {
        let root = TempDir::new().unwrap();
        // A repo rooted one level up: `.git` lives at `root.parent()`... use a nested repo.
        let repo = root.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        // Skill inside the repo tree.
        write_skill(&repo, ".maki/skills/inner", "---\n---\nInner skill");
        // Skill outside the repo boundary (in cwd's own dotfile), which must NOT be seen
        // when scanning from inside the repo that has a `.git`.
        write_skill(&repo, "../.claude/skills/outer", "Outer skill");

        let skills = enumerate_skills_inner(&repo, None, None);
        let names: Vec<_> = skills.iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"inner".to_string()));
        assert!(!names.contains(&"outer".to_string()));
    }

    #[test]
    fn empty_body_skipped() {
        let root = TempDir::new().unwrap();
        write_skill(root.path(), ".maki/skills/empty", "---\nname: empty\n---\n   \n");

        let skills = enumerate_skills_inner(root.path(), None, None);
        assert!(skills.is_empty());
    }

    #[test]
    fn sorted_by_name() {
        let root = TempDir::new().unwrap();
        for (dir, name) in [("zebra", "zebra"), ("alpha", "alpha"), ("mike", "mike")] {
            write_skill(
                root.path(),
                &format!(".maki/skills/{dir}"),
                &format!("---\nname: {name}\n---\nBody"),
            );
        }

        let skills = enumerate_skills_inner(root.path(), None, None);
        let names: Vec<_> = skills.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["alpha".to_string(), "mike".to_string(), "zebra".to_string()]);
    }

    #[test]
    fn project_beats_global_by_name() {
        let root = TempDir::new().unwrap();
        let global = TempDir::new().unwrap();
        write_skill(
            root.path(),
            ".maki/skills/overlap",
            "---\nname: overlap\ndescription: Project\n---\nP",
        );
        write_skill(
            global.path(),
            "skills/overlap",
            "---\nname: overlap\ndescription: Global\n---\nG",
        );

        let skills = enumerate_skills_inner(root.path(), Some(global.path()), None);
        let overlap: Vec<_> = skills.iter().filter(|s| s.name == "overlap").collect();
        assert_eq!(overlap.len(), 1);
        assert_eq!(overlap[0].description, "Project");
    }

    #[test]
    fn scans_all_project_dirs() {
        let root = TempDir::new().unwrap();
        for (i, dir) in PROJECT_SKILL_DIRS.iter().enumerate() {
            let name = format!("skill-{i}");
            write_skill(root.path(), &format!("{dir}/{name}"), &format!("---\nname: {name}\n---\nBody"));
        }

        let skills = enumerate_skills_inner(root.path(), None, None);
        assert_eq!(skills.len(), PROJECT_SKILL_DIRS.len());
    }
}