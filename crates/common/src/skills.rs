//! Skills: installable, frontmatter-driven agent instructions.
//!
//! A `Skill` is a markdown file (conventionally `SKILL.md`) with YAML-style
//! frontmatter carrying `name`, `description`, and an optional
//! `disable-model-invocation` flag. Skills are discovered from two scopes —
//! user (`<home>/skills/`) and project (`<cwd>/.slimcode/skills/`) — and are
//! triggered from an interactive frontend (the TUI) as `/name` commands, just
//! like the built-in commands. Only skills whose `disable_model_invocation` is
//! false have their description advertised to the model (via the system
//! prompt); a skill marked `disable-model-invocation: true` is available only
//! through an explicit
//! `/name` trigger.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use slimcode_commands::{COMMANDS, fuzzy::fuzzy_match, suggest};

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
    /// Directory this skill's files live in (or are installed into). Relative
    /// paths in the body resolve against this directory; it is injected into
    /// the skill-trigger prompt so the model can find referenced assets.
    pub dir: PathBuf,
}

impl Skill {
    /// Set the on-disk directory and return the skill. Producers that know the
    /// location (`SkillStore`) call this; `parse_skill` leaves `dir` empty
    /// because it only sees the markdown text, not where it came from.
    fn with_dir(mut self, dir: PathBuf) -> Self {
        self.dir = dir;
        self
    }
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
        let skill = parse_skill(&content, scope)?;
        let dir = self.skill_dir(scope, &skill.name);
        Ok(skill.with_dir(dir))
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

        Ok(skill.with_dir(target))
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

/// The markdown content a skill trigger submits as the user message: an
/// instruction header (plus the skill's directory when known, so the model can
/// resolve relative paths in the body), the skill body, and an optional task.
pub fn skill_prompt(skill: &Skill, arg: Option<&str>) -> String {
    let mut prompt = String::from("Use the following skill instructions to complete the task.");
    if !skill.dir.as_os_str().is_empty() {
        prompt.push_str(&format!("\nSkill directory: {}", skill.dir.display()));
    }
    prompt.push_str(&format!("\n\n# {}\n\n{}", skill.name, skill.body.trim()));
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
        dir: PathBuf::new(),
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
            out.push(parse_skill(&content, scope)?.with_dir(path.clone()));
        } else if path.is_file() && path.extension().is_some_and(|e| e == "md") {
            let content =
                fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            // A root-level `.md` skill has no dedicated directory; its parent
            // (the skills root) is the closest base for relative references.
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            out.push(parse_skill(&content, scope)?.with_dir(dir));
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

/// Does a skill name collide with a built-in command spelling?
pub fn is_builtin_command(name: &str) -> bool {
    COMMANDS
        .iter()
        .any(|c| c.spellings().any(|s| s.trim_start_matches('/') == name))
}

/// Parse the argument of `/install-skill <path> --user|--project`.
pub fn parse_install_args(arg: Option<&str>) -> Result<(PathBuf, SkillScope), String> {
    const USAGE: &str = "usage: /install-skill <path> --user|--project";
    let arg = arg
        .filter(|a| !a.is_empty())
        .ok_or_else(|| USAGE.to_string())?;
    let mut path = None;
    let mut scope = None;
    for token in arg.split_whitespace() {
        match token {
            "--user" => set_once(&mut scope, SkillScope::User, "scope")?,
            "--project" => set_once(&mut scope, SkillScope::Project, "scope")?,
            _ => {
                if path.is_some() {
                    return Err(format!("too many arguments; {USAGE}"));
                }
                path = Some(PathBuf::from(token));
            }
        }
    }
    let path = path.ok_or_else(|| USAGE.to_string())?;
    let scope = scope.ok_or_else(|| format!("choose a scope: --user or --project; {USAGE}"))?;
    Ok((path, scope))
}

/// Set `slot` to `value`, erroring if it was already set.
fn set_once<T>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{what} specified more than once"));
    }
    *slot = Some(value);
    Ok(())
}

/// Combine built-in command suggestions with skill suggestions (both triggered
/// via `/`) for a partial `/` input.
pub fn combined_suggestions(skills: &[Skill], input: &str) -> Vec<String> {
    let mut out: Vec<String> = suggest(input).iter().map(|c| c.usage.to_string()).collect();
    for s in suggest_skills(skills, input) {
        out.push(format!("/{}", s.name));
    }
    out
}

/// One selectable row in the TUI's `/` completion popup: the exact text to
/// commit to the input buffer and a short description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    /// The `/`-prefixed spelling to commit (e.g. `/save`, `/resume`, or
    /// `/skill-name`). Unlike [`combined_suggestions`], this is the bare
    /// spelling — never the usage string with its argument placeholder.
    pub value: String,
    pub description: String,
}

/// Ranked completion candidates for a partial `/` input: every command
/// spelling (canonical name and aliases) plus every installed skill, fuzzy
/// matched and sorted best-first. A bare `/` yields every candidate in
/// registry order (commands first, then skills); a non-`/` input yields none.
pub fn complete(input: &str, skills: &[Skill]) -> Vec<CompletionItem> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Vec::new();
    }
    let query = trimmed.strip_prefix('/').unwrap_or(trimmed);

    // Candidate pool: every command spelling (canonical + alias) then every
    // skill, as `/name`. Commands first keeps the registry order for the
    // bare-`/` case; aliases are real spellings, so `/res` completes to
    // `/resume` which `find` resolves back to `/load`.
    let mut pool: Vec<(&str, &'static str)> = Vec::new();
    for command in COMMANDS {
        for spelling in command.spellings() {
            // Command spellings already carry the leading `/`; store the bare
            // name so the final `/name` value is not doubled.
            let bare = spelling.strip_prefix('/').unwrap_or(spelling);
            pool.push((bare, command.description));
        }
    }
    let skill_pool: Vec<(&str, String)> = skills
        .iter()
        .map(|s| (s.name.as_str(), s.description.clone()))
        .collect();

    let mut scored: Vec<(i64, usize, CompletionItem)> = Vec::new();
    if query.is_empty() {
        // Bare `/`: everything, in pool order, unsorted (registry first).
        let mut out: Vec<CompletionItem> = pool
            .into_iter()
            .map(|(spelling, desc)| CompletionItem {
                value: format!("/{spelling}"),
                description: desc.to_string(),
            })
            .collect();
        out.extend(skill_pool.into_iter().map(|(name, desc)| CompletionItem {
            value: format!("/{name}"),
            description: desc,
        }));
        return out;
    }

    for (spelling, desc) in pool {
        if let Some(score) = fuzzy_match(query, spelling) {
            // Skip the leading `/` in the value; the pool stores bare names.
            let idx = scored.len();
            scored.push((
                score,
                idx,
                CompletionItem {
                    value: format!("/{spelling}"),
                    description: desc.to_string(),
                },
            ));
        }
    }
    for (name, desc) in skill_pool {
        if let Some(score) = fuzzy_match(query, name) {
            let idx = scored.len();
            scored.push((
                score,
                idx,
                CompletionItem {
                    value: format!("/{name}"),
                    description: desc,
                },
            ));
        }
    }
    // Best score first; ties keep pool order (commands before skills,
    // registry order within commands) via the insertion index.
    scored.sort_by_key(|(score, idx, _)| (*score, *idx));
    scored.into_iter().map(|(_, _, item)| item).collect()
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
        assert!(!p.contains("Skill directory"), "got: {p}");
        let p = skill_prompt(&s, Some("run it now"));
        assert!(p.ends_with("Task: run it now"));
    }

    #[test]
    fn skill_prompt_embeds_directory_when_set() {
        let s = parse_skill(&skill_md(""), SkillScope::User)
            .unwrap()
            .with_dir(PathBuf::from("/home/u/skills/demo"));
        let p = skill_prompt(&s, None);
        assert!(
            p.contains("Skill directory: /home/u/skills/demo"),
            "got: {p}"
        );
        assert!(p.contains("Do the demo."));
    }

    #[test]
    fn parse_skill_dir_defaults_empty() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap();
        assert!(s.dir.as_os_str().is_empty());
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
        assert_eq!(
            shared.dir,
            cwd.join(".slimcode").join("skills").join("shared")
        );
        let useronly = find_skill(&skills, "useronly").unwrap();
        assert_eq!(useronly.dir, home.join("skills").join("useronly"));
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
        assert_eq!(skill.dir, target);
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
        assert_eq!(skill.dir, cwd.join(".slimcode").join("skills").join("demo"));
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

    #[test]
    fn parse_install_args_reads_path_and_scope() {
        let (path, scope) = parse_install_args(Some("/tmp/skill --project")).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/skill"));
        assert_eq!(scope, SkillScope::Project);

        let (path, scope) = parse_install_args(Some("/tmp/skill --user")).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/skill"));
        assert_eq!(scope, SkillScope::User);
    }

    #[test]
    fn parse_install_args_requires_path_and_scope() {
        assert!(parse_install_args(None).is_err());
        assert!(parse_install_args(Some(" --project")).is_err());
        assert!(parse_install_args(Some("/tmp/skill")).is_err());
    }

    #[test]
    fn parse_install_args_rejects_conflicting_or_duplicate_scope() {
        assert!(parse_install_args(Some("/tmp/skill --user --project")).is_err());
        assert!(parse_install_args(Some("/tmp/skill --user --user")).is_err());
        assert!(parse_install_args(Some("/tmp/a /tmp/b --project")).is_err());
    }

    #[test]
    fn is_builtin_command_detects_collision() {
        assert!(is_builtin_command("help"));
        assert!(is_builtin_command("load"));
        assert!(!is_builtin_command("my-skill"));
    }

    #[test]
    fn combined_suggestions_includes_skills_and_commands() {
        let s = parse_skill(
            "---\nname: histo\ndescription: history-ish\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let got = combined_suggestions(&[s], "/hist");
        assert!(got.contains(&"/history".to_string()), "got: {got:?}");
        assert!(got.contains(&"/histo".to_string()), "got: {got:?}");
    }

    // --- completion popup candidates --------------------------------------

    #[test]
    fn complete_bare_slash_yields_every_command_then_skills() {
        let s = parse_skill(
            "---\nname: grill\ndescription: stress-test a plan\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let items = complete("/", &[s]);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        // Every command spelling (canonical + alias) precedes skills.
        assert!(values.contains(&"/help"));
        assert!(values.contains(&"/resume")); // alias of /load
        assert!(values.contains(&"/grill"));
        assert_eq!(*values.last().unwrap(), "/grill");
        let help = items.iter().find(|i| i.value == "/help").unwrap();
        assert_eq!(help.description, "list commands");
    }

    #[test]
    fn complete_non_slash_input_is_empty() {
        assert!(complete("hist", &[]).is_empty());
        assert!(complete("", &[]).is_empty());
    }

    #[test]
    fn complete_prefix_matches_command_name() {
        let items = complete("/sav", &[]);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, vec!["/save"]);
        assert_eq!(items[0].description, "save the current session");
    }

    #[test]
    fn complete_matches_command_alias() {
        let items = complete("/res", &[]);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"/resume"), "got: {values:?}");
    }

    #[test]
    fn complete_matches_skill_name() {
        let s = parse_skill(
            "---\nname: grill\ndescription: stress-test a plan\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let items = complete("/gr", &[s]);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"/grill"), "got: {values:?}");
    }

    #[test]
    fn complete_ranks_fuzzy_matches_best_first() {
        let s = parse_skill(
            "---\nname: save-notes\ndescription: save loose notes\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        // Query "sav" matches /save (contiguous) far better than the skill
        // "save-notes" (gappy s..a..v spread over the word).
        let items = complete("/sav", &[s]);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(*values.first().unwrap(), "/save");
        assert!(values.contains(&"/save-notes"), "got: {values:?}");
        // The fuzzy order places the contiguous match first.
        let save = items.iter().position(|i| i.value == "/save").unwrap();
        let notes = items.iter().position(|i| i.value == "/save-notes").unwrap();
        assert!(save < notes, "{values:?}");
    }

    #[test]
    fn complete_unknown_is_empty() {
        assert!(complete("/zzz", &[]).is_empty());
    }

    #[test]
    fn complete_values_are_bare_spellings_not_usages() {
        // The value must be commit-able (no `<id>` placeholder), unlike the
        // usage strings combined_suggestions surfaces. Querying the exact
        // canonical name yields its bare spelling, not `/load <id>`.
        let items = complete("/load", &[]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].value, "/load");
        assert_eq!(items[0].description, "load a saved session");
    }
}
