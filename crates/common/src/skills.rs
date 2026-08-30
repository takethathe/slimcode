//! Skills: installable, frontmatter-driven agent instructions.
//!
//! A `Skill` is a markdown file (conventionally `SKILL.md`) with YAML-style
//! frontmatter carrying `name`, `description`, and an optional
//! `disable-model-invocation` flag. Skills are discovered from two scopes —
//! user (`<home>/skills/`) and project (`<cwd>/.slimcode/skills/`) — and are
//! triggered from the REPL as `/name` commands, just like the built-in
//! commands. Only skills whose `disable_model_invocation` is false have their
//! description advertised to the model (via the system prompt); a skill marked
//! `disable-model-invocation: true` is available only through an explicit
//! `/name` trigger.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Where a skill was discovered from (or installed into).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillScope {
    /// `~/.slimcode/skills` (shared across every project).
    User,
    /// `<cwd>/.slimcode/skills` (project-local, wins over user on name clash).
    Project,
}

/// A parsed skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// When true the skill is NOT advertised in the system prompt and is only
    /// reachable through an explicit `/name` trigger.
    pub disable_model_invocation: bool,
    /// The markdown body (everything after the frontmatter block).
    pub body: String,
    pub scope: SkillScope,
}

/// Parsed frontmatter (a tiny, line-based subset of YAML).
#[derive(Debug, Default, PartialEq, Eq)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    disable_model_invocation: Option<bool>,
}

/// A frontend-agnostic skill registry rooted at the user and project dirs.
pub struct SkillStore {
    user_dir: PathBuf,
    project_dir: PathBuf,
}

impl SkillStore {
    /// Build a store rooted at the user skills dir (`<home>/skills`) and the
    /// project skills dir (`<cwd>/.slimcode/skills`).
    pub fn new(home: &Path, cwd: &Path) -> Self {
        Self {
            user_dir: home.join("skills"),
            project_dir: cwd.join(".slimcode").join("skills"),
        }
    }

    /// The skills directory for a scope.
    pub fn dir_for(&self, scope: SkillScope) -> &Path {
        match scope {
            SkillScope::User => &self.user_dir,
            SkillScope::Project => &self.project_dir,
        }
    }

    /// The directory a skill of `name` would live in for a scope.
    pub fn skill_dir(&self, scope: SkillScope, name: &str) -> PathBuf {
        self.dir_for(scope).join(name)
    }

    /// Load every installed skill, project scope overriding user scope on a
    /// name clash, sorted by name.
    pub fn list(&self) -> Result<Vec<Skill>, String> {
        let mut by_name: BTreeMap<String, Skill> = BTreeMap::new();
        for (scope, dir) in [
            (SkillScope::User, &self.user_dir),
            (SkillScope::Project, &self.project_dir),
        ] {
            for skill in read_skill_dir(dir, scope)? {
                // Project wins over user: insert first (later user entries with
                // the same name are dropped).
                if by_name.contains_key(&skill.name) && scope == SkillScope::User {
                    continue;
                }
                by_name.insert(skill.name.clone(), skill);
            }
        }
        Ok(by_name.into_values().collect())
    }

    /// Parse and validate `source` without installing it. Used to reject
    /// bad sources (or name collisions) before anything is written.
    pub fn inspect(&self, source: &Path, scope: SkillScope) -> Result<Skill, String> {
        let (content, _) = read_source(source)?;
        parse_skill(&content, scope)
    }

    /// Install `source` into a scope, returning the parsed skill. `source` is
    /// either a directory containing a `SKILL.md`, or a markdown file treated
    /// as the skill body. An existing skill of the same name is overwritten.
    pub fn install(&self, source: &Path, scope: SkillScope) -> Result<Skill, String> {
        let (content, source_is_dir) = read_source(source)?;
        let skill = parse_skill(&content, scope)?;
        let target = self.skill_dir(scope, &skill.name);

        fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        if source_is_dir {
            copy_dir_contents(source, &target)?;
        }
        // Write the parsed content as SKILL.md so a file-source install also
        // lands in the canonical layout.
        fs::write(target.join("SKILL.md"), content)
            .map_err(|e| format!("{}: {e}", target.join("SKILL.md").display()))?;

        Ok(skill)
    }
}

/// Resolve a skill by its canonical or `/`-prefixed name (exact, case-sensitive).
pub fn find_skill<'a>(skills: &'a [Skill], name: &str) -> Option<&'a Skill> {
    let name = name.strip_prefix('/').unwrap_or(name);
    skills.iter().find(|s| s.name == name)
}

/// Predict skills matching a partial `/` input. A non-`/` input yields none;
/// `/` alone yields every skill.
pub fn suggest_skills<'a>(skills: &'a [Skill], input: &str) -> Vec<&'a Skill> {
    let input = input.trim();
    if !input.starts_with('/') {
        return Vec::new();
    }
    let prefix = input.strip_prefix('/').unwrap_or(input);
    skills
        .iter()
        .filter(|s| s.name.starts_with(prefix))
        .collect()
}

/// The markdown content a skill trigger submits as the user message.
pub fn skill_prompt(skill: &Skill, arg: Option<&str>) -> String {
    let mut prompt = format!(
        "Use the following skill instructions to complete the task.\n\n# {}\n\n{}",
        skill.name,
        skill.body.trim()
    );
    if let Some(arg) = arg.filter(|a| !a.is_empty()) {
        prompt.push_str("\n\nTask: ");
        prompt.push_str(arg);
    }
    prompt
}

/// Parse a `SKILL.md` file's frontmatter + body.
fn parse_skill(content: &str, scope: SkillScope) -> Result<Skill, String> {
    let (fm, body) = split_frontmatter(content)?;
    let name = fm
        .name
        .ok_or_else(|| "skill is missing required frontmatter field `name`".to_string())?;
    validate_name(&name)?;
    let description = fm.description.ok_or_else(|| {
        format!("skill {name:?} is missing required frontmatter field `description`")
    })?;
    if description.trim().is_empty() {
        return Err(format!("skill {name:?} has an empty description"));
    }
    Ok(Skill {
        name,
        description,
        disable_model_invocation: fm.disable_model_invocation.unwrap_or(false),
        body: body.to_string(),
        scope,
    })
}

/// Does `content` have a frontmatter block? Returns `(frontmatter, body)`.
fn split_frontmatter(content: &str) -> Result<(Frontmatter, String), String> {
    let mut lines = content.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Err("skill must start with a `---` frontmatter block".to_string());
    }
    let mut fm_lines: Vec<String> = Vec::new();
    let mut body = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        fm_lines.push(line.to_string());
    }
    if !closed {
        return Err("skill frontmatter is not closed with `---`".to_string());
    }
    // Remaining lines are the body.
    for line in lines {
        body.push_str(line);
    }
    let fm = parse_frontmatter_lines(&fm_lines)?;
    Ok((fm, body))
}

/// Parse `key: value` lines into a `Frontmatter`.
fn parse_frontmatter_lines(lines: &[String]) -> Result<Frontmatter, String> {
    let mut fm = Frontmatter::default();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let (key, value) = t
            .split_once(':')
            .ok_or_else(|| format!("invalid frontmatter line {}: {t:?}", i + 2))?;
        let value = unquote(value.trim());
        match key.trim() {
            "name" => fm.name = Some(value),
            "description" => fm.description = Some(value),
            "disable-model-invocation" => {
                let v = match value.as_str() {
                    "true" => true,
                    "false" => false,
                    other => {
                        return Err(format!(
                            "invalid `disable-model-invocation` value {other:?} (expected true or false)"
                        ));
                    }
                };
                fm.disable_model_invocation = Some(v);
            }
            _ => {} // tolerate unknown keys (serde-style)
        }
    }
    Ok(fm)
}

/// Strip one layer of matching single or double quotes.
fn unquote(s: &str) -> String {
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if (bytes[0] == b'"' && bytes[s.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[s.len() - 1] == b'\'')
        {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// A skill name must be a safe `/` trigger: `[A-Za-z0-9_-]+`.
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "invalid skill name {name:?}: use only letters, digits, `_` and `-`"
        ));
    }
    Ok(())
}

/// Read a source path into `(markdown content, is_dir)`.
fn read_source(source: &Path) -> Result<(String, bool), String> {
    if source.is_dir() {
        let skill_md = source.join("SKILL.md");
        if !skill_md.is_file() {
            return Err(format!(
                "skill source directory {} has no SKILL.md",
                source.display()
            ));
        }
        let content =
            fs::read_to_string(&skill_md).map_err(|e| format!("{}: {e}", skill_md.display()))?;
        Ok((content, true))
    } else if source.is_file() {
        let content =
            fs::read_to_string(source).map_err(|e| format!("{}: {e}", source.display()))?;
        Ok((content, false))
    } else {
        Err(format!("skill source {} not found", source.display()))
    }
}

/// Read all skills from a skills directory (user or project scope).
fn read_skill_dir(dir: &Path, scope: SkillScope) -> Result<Vec<Skill>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let skill_md = path.join("SKILL.md");
            if !skill_md.is_file() {
                continue; // not a skill directory
            }
            let content = fs::read_to_string(&skill_md)
                .map_err(|e| format!("{}: {e}", skill_md.display()))?;
            out.push(parse_skill(&content, scope)?);
        } else if path.is_file() && path.extension().is_some_and(|e| e == "md") {
            let content =
                fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.push(parse_skill(&content, scope)?);
        }
    }
    Ok(out)
}

/// Recursively copy the contents of `from` into `to`.
fn copy_dir_contents(from: &Path, to: &Path) -> Result<(), String> {
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            fs::create_dir_all(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            copy_dir_contents(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::unique_temp_dir;

    fn skill_md(extra: &str) -> String {
        format!("---\nname: demo\n{extra}description: A demo skill\n---\n\nDo the demo.\n")
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap();
        assert_eq!(s.name, "demo");
        assert_eq!(s.description, "A demo skill");
        assert!(!s.disable_model_invocation);
        assert_eq!(s.body, "\nDo the demo.\n");
    }

    #[test]
    fn disable_model_invocation_defaults_false_and_parses_true() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap();
        assert!(!s.disable_model_invocation);
        let s = parse_skill(
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\nbody\n",
            SkillScope::User,
        )
        .unwrap();
        assert!(s.disable_model_invocation);
    }

    #[test]
    fn quoted_description_is_unquoted() {
        let s = parse_skill(
            "---\nname: demo\ndescription: \"Use when: it helps\"\n---\nbody\n",
            SkillScope::User,
        )
        .unwrap();
        assert_eq!(s.description, "Use when: it helps");
    }

    #[test]
    fn missing_name_or_description_errors() {
        let err = parse_skill("---\ndescription: d\n---\nbody\n", SkillScope::User).unwrap_err();
        assert!(err.contains("name"), "err: {err}");
        let err = parse_skill("---\nname: demo\n---\nbody\n", SkillScope::User).unwrap_err();
        assert!(err.contains("description"), "err: {err}");
    }

    #[test]
    fn invalid_name_chars_error() {
        let err = parse_skill(
            "---\nname: bad/name\ndescription: d\n---\nbody\n",
            SkillScope::User,
        )
        .unwrap_err();
        assert!(err.contains("invalid skill name"), "err: {err}");
    }

    #[test]
    fn unclosed_frontmatter_errors() {
        let err = parse_skill("---\nname: demo\n", SkillScope::User).unwrap_err();
        assert!(err.contains("not closed"), "err: {err}");
    }

    #[test]
    fn find_skill_matches_exact_and_prefixed() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap();
        assert_eq!(
            find_skill(std::slice::from_ref(&s), "demo").unwrap().name,
            "demo"
        );
        assert_eq!(
            find_skill(std::slice::from_ref(&s), "/demo").unwrap().name,
            "demo"
        );
        assert!(find_skill(&[s], "/nope").is_none());
    }

    #[test]
    fn suggest_skills_prefix_matches_and_handles_bare_slash() {
        let a = parse_skill(
            "---\nname: alpha\ndescription: a\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let b = parse_skill(
            "---\nname: beta\ndescription: b\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let skills = [a, b];
        assert_eq!(suggest_skills(&skills, "/").len(), 2);
        assert_eq!(suggest_skills(&skills, "/al").len(), 1);
        assert_eq!(suggest_skills(&skills, "/al")[0].name, "alpha");
        assert!(suggest_skills(&skills, "al").is_empty());
    }

    #[test]
    fn skill_prompt_embeds_body_and_optional_task() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap();
        let p = skill_prompt(&s, None);
        assert!(p.contains("Do the demo."));
        assert!(!p.contains("Task:"));
        let p = skill_prompt(&s, Some("run it now"));
        assert!(p.ends_with("Task: run it now"));
    }

    #[test]
    fn store_lists_user_and_project_with_project_precedence() {
        let dir = unique_temp_dir("slimcode-skills-list");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        fs::create_dir_all(home.join("skills").join("shared")).unwrap();
        fs::create_dir_all(home.join("skills").join("useronly")).unwrap();
        fs::create_dir_all(cwd.join(".slimcode").join("skills").join("shared")).unwrap();
        fs::write(
            home.join("skills").join("shared").join("SKILL.md"),
            "---\nname: shared\ndescription: user version\n---\nuser\n",
        )
        .unwrap();
        fs::write(
            home.join("skills").join("useronly").join("SKILL.md"),
            "---\nname: useronly\ndescription: user only\n---\nu\n",
        )
        .unwrap();
        fs::write(
            cwd.join(".slimcode")
                .join("skills")
                .join("shared")
                .join("SKILL.md"),
            "---\nname: shared\ndescription: project version\n---\nproj\n",
        )
        .unwrap();

        let store = SkillStore::new(&home, &cwd);
        let skills = store.list().unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["shared", "useronly"]);
        let shared = find_skill(&skills, "shared").unwrap();
        assert_eq!(shared.description, "project version");
        assert_eq!(shared.scope, SkillScope::Project);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_list_missing_dirs_is_empty() {
        let dir = unique_temp_dir("slimcode-skills-empty");
        let store = SkillStore::new(&dir.join("home"), &dir.join("proj"));
        assert!(store.list().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_from_directory_copies_and_parses() {
        let dir = unique_temp_dir("slimcode-skills-install");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("SKILL.md"), skill_md("")).unwrap();
        fs::write(src.join("notes.txt"), "supporting file").unwrap();

        let store = SkillStore::new(&home, &cwd);
        let skill = store.install(&src, SkillScope::User).unwrap();
        assert_eq!(skill.name, "demo");
        assert_eq!(skill.scope, SkillScope::User);

        let target = home.join("skills").join("demo");
        assert!(target.join("SKILL.md").is_file());
        assert_eq!(
            fs::read_to_string(target.join("notes.txt")).unwrap(),
            "supporting file"
        );
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_from_file_writes_skll_md() {
        let dir = unique_temp_dir("slimcode-skills-file");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("whatever.md");
        fs::write(&src, skill_md("")).unwrap();

        let store = SkillStore::new(&home, &cwd);
        let skill = store.install(&src, SkillScope::Project).unwrap();
        assert_eq!(skill.name, "demo");
        let target = cwd
            .join(".slimcode")
            .join("skills")
            .join("demo")
            .join("SKILL.md");
        assert!(target.is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_requires_skill_md_for_directory() {
        let dir = unique_temp_dir("slimcode-skills-baddir");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        let store = SkillStore::new(&home, &cwd);
        let err = store.install(&src, SkillScope::User).unwrap_err();
        assert!(err.contains("SKILL.md"), "err: {err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
