//! Skills: installable, frontmatter-driven agent instructions.
//!
//! A `Skill` is a markdown file (conventionally `SKILL.md`) with YAML-style
//! frontmatter carrying `name`, `description`, and an optional
//! `disable-model-invocation` flag. Skills are discovered from two scopes —
//! user (`<home>/skills/`) and project (`<cwd>/.slimcode/skills/`) — by
//! recursively scanning for `SKILL.md` at any depth (category folders such as
//! `skills/engineering/…` are walked through), and are triggered from an
//! interactive frontend (the TUI) as `/skill:name` commands. Only skills whose
//! `disable_model_invocation` is false are advertised to the model (via the
//! system prompt's `## Skills` markdown index, each bullet carrying the
//! `SKILL.md` location so the model can `read` it); a skill marked
//! `disable-model-invocation: true` is available only through an explicit
//! `/skill:name` trigger.

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
    /// reachable through an explicit `/skill:name` trigger.
    pub disable_model_invocation: bool,
    /// The markdown body (everything after the frontmatter block).
    pub body: String,
    pub scope: SkillScope,
    /// Directory this skill's files live in (or are installed into). Relative
    /// paths in the body resolve against this directory; it is injected into
    /// the skill-trigger prompt so the model can find referenced assets.
    pub dir: PathBuf,
    /// The on-disk skill file (`<dir>/SKILL.md`, or the single-file `.md`
    /// itself for root-level skills). Advertised as `[Read from <file>]` in
    /// the system prompt's skills index so the model can `read` it.
    pub file: PathBuf,
}

impl Skill {
    /// Set the on-disk location (skill directory + skill file) and return the
    /// skill. Producers that know the location (`SkillStore`) call this;
    /// `parse_skill` leaves `dir`/`file` empty because it only sees the
    /// markdown text, not where it came from.
    fn at(mut self, dir: PathBuf, file: PathBuf) -> Self {
        self.dir = dir;
        self.file = file;
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
        let file = dir.join("SKILL.md");
        Ok(skill.at(dir, file))
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

        let file = target.join("SKILL.md");
        Ok(skill.at(target, file))
    }
}

/// Resolve a skill by its canonical name or `/`-prefixed spelling, in either
/// the `/skill:name` (canonical) or bare `/name` (legacy) form — exact,
/// case-sensitive.
pub fn find_skill<'a>(skills: &'a [Skill], name: &str) -> Option<&'a Skill> {
    let name = name.strip_prefix('/').unwrap_or(name);
    let name = name.strip_prefix("skill:").unwrap_or(name);
    skills.iter().find(|s| s.name == name)
}

/// The canonical slash trigger spelling for a skill (`/skill:name`).
pub fn skill_trigger(name: &str) -> String {
    format!("/skill:{name}")
}

/// Rewrite a leading `/skill:{name}` trigger in a raw prompt to the `/{name}`
/// form the model knows (the CLI one-shot path has no command parser, so a
/// `/skill:` prefix typed there would otherwise reach the LLM literally).
/// Only a leading trigger is rewritten; `/skill:` in the middle of a prompt is
/// ordinary text.
pub fn normalize_skill_trigger(prompt: &str) -> String {
    let trimmed = prompt.trim_start();
    match trimmed.strip_prefix("/skill:") {
        Some(rest) if !rest.is_empty() && !rest.starts_with(char::is_whitespace) => {
            let indent = &prompt[..prompt.len() - trimmed.len()];
            format!("{indent}/{rest}")
        }
        _ => prompt.to_string(),
    }
}

/// Predict skills matching a partial `/` input. A non-`/` input yields none;
/// `/` alone yields every skill. The `/skill:` prefix of the canonical trigger
/// is stripped so `/skill:gr` matches skill `gr…`.
pub fn suggest_skills<'a>(skills: &'a [Skill], input: &str) -> Vec<&'a Skill> {
    let input = input.trim();
    if !input.starts_with('/') {
        return Vec::new();
    }
    let prefix = input.strip_prefix('/').unwrap_or(input);
    let prefix = prefix.strip_prefix("skill:").unwrap_or(prefix);
    skills
        .iter()
        .filter(|s| s.name.starts_with(prefix))
        .collect()
}

/// The user message a skill trigger submits: an XML `<skill>` block carrying
/// the skill's name, its `SKILL.md` location, and a base-dir reference line,
/// followed by the skill body — or, when `already_loaded` is true (the skill
/// was already injected into an earlier message of this conversation), a short
/// notice pointing at that earlier message instead of repeating the body — and
/// then an optional task argument. Mirrors pi's `/skill:name` expansion.
pub fn skill_prompt(skill: &Skill, arg: Option<&str>, already_loaded: bool) -> String {
    let mut prompt = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n",
        escape_xml(&skill.name),
        escape_xml(&skill.file.display().to_string()),
        escape_xml(&skill.dir.display().to_string()),
    );
    if already_loaded {
        prompt.push_str(&format!(
            "\nThis skill's instructions were already loaded in a previous message \
             (search the conversation for `<skill name=\"{}\">`). They are not repeated here; \
             refer to the earlier message.\n",
            escape_xml(&skill.name),
        ));
    } else {
        prompt.push_str(&format!("\n{}\n", skill.body.trim()));
    }
    prompt.push_str("</skill>");
    if let Some(arg) = arg.filter(|a| !a.is_empty()) {
        prompt.push_str(&format!("\n\n{arg}"));
    }
    prompt
}

/// Escape a string for safe inclusion in an XML element/attribute (Agent
/// Skills prompt format).
pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Format auto-invokable skills (those without `disable-model-invocation:
/// true`) for the system prompt, as a markdown skill index: a short "when to
/// use" header plus one bullet per skill (`- name: description [Read from
/// <file>]`). Each bullet co-locates the trigger (name + description) with
/// how to reach the full instructions (the `SKILL.md` path), so the model
/// can `read` a skill on demand. Returns "" when no skill is auto-invokable.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n## Skills\n\n\
         Use a skill when its name or description matches the task, or when the user references it \
         explicitly as /{name}. Read the file and follow its instructions. When a skill file \
         references a relative path, resolve it against the skill directory and use the absolute \
         path in tool commands.\n",
    );
    for skill in visible {
        out.push_str(&format!(
            "- {name}: {description} [Read from {path}]\n",
            name = skill.name,
            description = flatten_whitespace(&skill.description),
            path = skill.file.display(),
        ));
    }
    out
}

/// Collapse internal whitespace (including line breaks) so a one-line
/// markdown bullet stays on one line.
fn flatten_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
        file: PathBuf::new(),
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
///
/// Discovery is recursive: any `SKILL.md` under the root is a skill, so
/// category folders (`skills/engineering/…`) are walked through to find skill
/// directories at any depth. A skill directory itself is not descended into —
/// its files and subdirectories are the skill's payload. Root-level `.md`
/// files remain single-file skills; deeper `.md` files are ignored. Hidden
/// entries (dot-prefixed) are skipped at every level.
///
/// Name collisions within one scope resolve to the shallowest skill
/// directory (a root-level `/install-skill` install beats a deeper vendored
/// copy declaring the same frontmatter name), with sorted paths as a
/// deterministic tiebreak.
fn read_skill_dir(dir: &Path, scope: SkillScope) -> Result<Vec<Skill>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut skills = Vec::new();
    walk_skill_dir(dir, true, scope, &mut skills)?;
    // Stable sort shallowest-first (fewest path components), then by path, so
    // same-scope collisions resolve deterministically.
    skills.sort_by(|a, b| {
        a.dir
            .components()
            .count()
            .cmp(&b.dir.components().count())
            .then_with(|| a.dir.cmp(&b.dir))
    });
    let mut out: Vec<Skill> = Vec::new();
    for skill in skills {
        if !out.iter().any(|s| s.name == skill.name) {
            out.push(skill);
        }
    }
    Ok(out)
}

/// Depth-first scan of `dir`. `at_root` marks the skills root, the only level
/// at which standalone `.md` files count as single-file skills. Sorted
/// traversal keeps discovery deterministic.
fn walk_skill_dir(
    dir: &Path,
    at_root: bool,
    scope: SkillScope,
    out: &mut Vec<Skill>,
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?
            .is_dir();
        if is_dir {
            let skill_md = path.join("SKILL.md");
            if skill_md.is_file() {
                let content = fs::read_to_string(&skill_md)
                    .map_err(|e| format!("{}: {e}", skill_md.display()))?;
                out.push(parse_skill(&content, scope)?.at(path, skill_md));
            } else {
                walk_skill_dir(&path, false, scope, out)?;
            }
        } else if at_root && path.extension().is_some_and(|e| e == "md") {
            // A root-level `.md` skill has no dedicated directory; the skills
            // root is the closest base for relative references.
            let content =
                fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.push(parse_skill(&content, scope)?.at(dir.to_path_buf(), path));
        }
    }
    Ok(())
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
        out.push(skill_trigger(&s.name));
    }
    out
}

/// One selectable row in the TUI's `/` completion popup: the exact text to
/// commit to the input buffer and a short description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    /// The `/`-prefixed spelling to commit (e.g. `/save`, `/resume`, or
    /// `/skill:name`). Unlike [`combined_suggestions`], this is the bare
    /// spelling — never the usage string with its argument placeholder.
    pub value: String,
    pub description: String,
}

/// Ranked completion candidates for a partial `/` input: every command
/// spelling (canonical name and aliases) plus every installed skill, fuzzy
/// matched and sorted best-first. A bare `/` yields every candidate in
/// registry order (commands first, then skills); a non-`/` input yields none.
/// Skills are matched on their bare name only — the `/skill:` trigger prefix
/// is never scored, so its letters cannot match every skill — and advertised
/// under their canonical `/skill:name` spelling (a typed `/skill:name`
/// trigger is likewise matched by its name part).
pub fn complete(input: &str, skills: &[Skill]) -> Vec<CompletionItem> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Vec::new();
    }
    let query = trimmed.strip_prefix('/').unwrap_or(trimmed);

    // Candidate pool: every command spelling (canonical + alias) then every
    // skill, as `skill:name`. Commands first keeps the registry order for the
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
    // Skills pool keyed on the bare skill *name*: the `/skill:` trigger is
    // pure spelling, so matching against it would let any letter of "skill"
    // (s, k, i, l) match every installed skill and would drown the name's own
    // boundary bonuses. Only the name is scored; the committed value is
    // rebuilt as the canonical `/skill:name` trigger below.
    let skill_pool: Vec<(String, String)> = skills
        .iter()
        .map(|s| (s.name.clone(), s.description.clone()))
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
            value: skill_trigger(&name),
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
    // A typed `/skill:name` trigger matches by its name part only: strip the
    // spelling prefix before scoring so it cannot distort the ranking, and a
    // bare `/skill:` lists every skill (score 0) exactly like a bare `/`.
    let skill_query = query.strip_prefix("skill:").unwrap_or(query);
    for (name, desc) in skill_pool {
        if let Some(score) = fuzzy_match(skill_query, &name) {
            let idx = scored.len();
            scored.push((
                score,
                idx,
                CompletionItem {
                    value: skill_trigger(&name),
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
    fn find_skill_matches_exact_prefixed_and_skill_trigger() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap();
        assert_eq!(
            find_skill(std::slice::from_ref(&s), "demo").unwrap().name,
            "demo"
        );
        assert_eq!(
            find_skill(std::slice::from_ref(&s), "/demo").unwrap().name,
            "demo"
        );
        assert_eq!(
            find_skill(std::slice::from_ref(&s), "/skill:demo")
                .unwrap()
                .name,
            "demo"
        );
        assert_eq!(
            find_skill(std::slice::from_ref(&s), "skill:demo")
                .unwrap()
                .name,
            "demo"
        );
        assert!(find_skill(&[s], "/nope").is_none());
    }

    #[test]
    fn normalize_skill_trigger_rewrites_leading_trigger_only() {
        // A leading `/skill:{name}` trigger becomes the `/{name}` form.
        assert_eq!(normalize_skill_trigger("/skill:grill"), "/grill");
        assert_eq!(
            normalize_skill_trigger("/skill:grill do it now"),
            "/grill do it now"
        );
        // Leading whitespace is preserved, only the trigger is rewritten.
        assert_eq!(normalize_skill_trigger("  /skill:grill hi"), "  /grill hi");
        // Bare `/{name}` and plain prompts pass through unchanged.
        assert_eq!(normalize_skill_trigger("/grill"), "/grill");
        assert_eq!(
            normalize_skill_trigger("review this diff"),
            "review this diff"
        );
        // `/skill:` in the middle of a prompt is ordinary text, not a trigger.
        assert_eq!(
            normalize_skill_trigger("run /skill:grill after this"),
            "run /skill:grill after this"
        );
        // A bare `/skill:` with nothing after it is not a trigger.
        assert_eq!(normalize_skill_trigger("/skill:"), "/skill:");
    }

    #[test]
    fn suggest_skills_prefix_matches_and_handles_bare_slash_and_skill_prefix() {
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
        // The canonical `/skill:` prefix is stripped before matching.
        assert_eq!(suggest_skills(&skills, "/skill:al").len(), 1);
        assert_eq!(suggest_skills(&skills, "/skill:al")[0].name, "alpha");
        assert!(suggest_skills(&skills, "al").is_empty());
    }

    #[test]
    fn skill_prompt_uses_xml_skill_block_and_optional_task() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap().at(
            PathBuf::from("/home/u/skills/demo"),
            PathBuf::from("/home/u/skills/demo/SKILL.md"),
        );
        let p = skill_prompt(&s, None, false);
        assert!(
            p.starts_with(
                "<skill name=\"demo\" location=\"/home/u/skills/demo/SKILL.md\">\nReferences are relative to /home/u/skills/demo.\n"
            ),
            "got: {p}"
        );
        assert!(p.contains("Do the demo."));
        assert!(p.ends_with("</skill>"));
        assert!(!p.contains("Task:"), "got: {p}");

        let p = skill_prompt(&s, Some("run it now"), false);
        assert!(p.ends_with("</skill>\n\nrun it now"), "got: {p}");
    }

    #[test]
    fn skill_prompt_replaces_body_with_already_loaded_notice() {
        let s = parse_skill(&skill_md(""), SkillScope::User).unwrap().at(
            PathBuf::from("/home/u/skills/demo"),
            PathBuf::from("/home/u/skills/demo/SKILL.md"),
        );
        let p = skill_prompt(&s, None, true);
        // The base-dir reference line is kept.
        assert!(
            p.contains("References are relative to /home/u/skills/demo."),
            "got: {p}"
        );
        // The body is replaced by an already-loaded notice.
        assert!(!p.contains("Do the demo."), "got: {p}");
        assert!(p.contains("already loaded"), "got: {p}");
        assert!(p.contains("previous message"), "got: {p}");
        assert!(p.ends_with("</skill>"));
    }

    #[test]
    fn escape_xml_escapes_five_entities() {
        assert_eq!(
            escape_xml("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
    }

    #[test]
    fn format_skills_for_prompt_filters_and_lists_location() {
        let auto = parse_skill(
            "---\nname: tdd\ndescription: test first\n---\nb\n",
            SkillScope::User,
        )
        .unwrap()
        .at(PathBuf::from("/s/tdd"), PathBuf::from("/s/tdd/SKILL.md"));
        let manual = parse_skill(
            "---\nname: grill\ndescription: stress-test a plan\ndisable-model-invocation: true\n---\nb\n",
            SkillScope::User,
        )
        .unwrap()
        .at(
            PathBuf::from("/s/grill"),
            PathBuf::from("/s/grill/SKILL.md"),
        );
        let out = format_skills_for_prompt(&[auto, manual]);
        assert!(out.contains("## Skills"), "got: {out}");
        assert!(
            out.contains("- tdd: test first [Read from /s/tdd/SKILL.md]"),
            "got: {out}"
        );
        assert!(!out.contains("grill"), "got: {out}");
        assert!(
            out.contains(
                "Use a skill when its name or description matches the task, or when the user \
                 references it explicitly as /{name}."
            ),
            "got: {out}"
        );
        assert!(
            out.contains("Read the file and follow its instructions."),
            "got: {out}"
        );
        assert!(
            out.contains(
                "When a skill file references a relative path, resolve it against the skill directory \
                 and use the absolute path in tool commands."
            ),
            "got: {out}"
        );
    }

    #[test]
    fn format_skills_for_prompt_empty_when_no_auto_skills() {
        let manual = parse_skill(
            "---\nname: grill\ndescription: d\ndisable-model-invocation: true\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        assert!(format_skills_for_prompt(&[manual]).is_empty());
        assert!(format_skills_for_prompt(&[]).is_empty());
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
    fn store_lists_nested_skill_dirs_recursively() {
        let dir = unique_temp_dir("slimcode-skills-deep");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        let root = home.join("skills");

        // Category folders: skills/engineering/tdd and skills/design/grill.
        fs::create_dir_all(root.join("engineering").join("tdd").join("references")).unwrap();
        fs::create_dir_all(root.join("design").join("grill")).unwrap();
        // Skill payload must NOT become a skill: a skill directory is not
        // descended into, even when its files carry frontmatter.
        fs::write(
            root.join("engineering")
                .join("tdd")
                .join("references")
                .join("REFERENCE.md"),
            "---\nname: reference\ndescription: payload doc\n---\nref\n",
        )
        .unwrap();
        fs::write(
            root.join("engineering").join("tdd").join("SKILL.md"),
            "---\nname: tdd\ndescription: test first\n---\nred green refactor\n",
        )
        .unwrap();
        fs::write(
            root.join("design").join("grill").join("SKILL.md"),
            "---\nname: grill\ndescription: stress-test a plan\n---\ninterview\n",
        )
        .unwrap();
        // Root-level single-file skill (backward compat).
        fs::write(
            root.join("plain.md"),
            "---\nname: plain\ndescription: a root file skill\n---\nbody\n",
        )
        .unwrap();
        // Hidden directory is skipped, even though it contains a SKILL.md.
        fs::create_dir_all(root.join(".hidden").join("secret")).unwrap();
        fs::write(
            root.join(".hidden").join("secret").join("SKILL.md"),
            "---\nname: hidden\ndescription: must not load\n---\nno\n",
        )
        .unwrap();
        // A deeper `.md` without an owning SKILL.md dir is not a skill.
        fs::write(
            root.join("engineering").join("notes.md"),
            "---\nname: notes\ndescription: grouping note\n---\nno\n",
        )
        .unwrap();

        let store = SkillStore::new(&home, &cwd);
        let skills = store.list().unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["grill", "plain", "tdd"]);

        // The nested skill resolves relative payload references against its
        // own directory, not the skills root.
        let tdd = find_skill(&skills, "tdd").unwrap();
        assert_eq!(tdd.scope, SkillScope::User);
        assert_eq!(tdd.dir, root.join("engineering").join("tdd"));
        assert_eq!(
            tdd.file,
            root.join("engineering").join("tdd").join("SKILL.md")
        );
        assert_eq!(tdd.body, "red green refactor\n");

        // Root-level single-file skills keep the skills root as their base and
        // the `.md` file itself as the skill file.
        let plain = find_skill(&skills, "plain").unwrap();
        assert_eq!(plain.dir, root);
        assert_eq!(plain.file, root.join("plain.md"));

        // The nested skill is triggerable via the `/` completion popup under
        // its canonical `/skill:name` spelling.
        let items = complete("/tdd", &skills);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"/skill:tdd"), "got: {values:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_scope_name_collision_prefers_shallowest_skill() {
        let dir = unique_temp_dir("slimcode-skills-dup");
        let home = dir.join("home");
        let cwd = dir.join("proj");
        // A root-level install (shallowest) beats a deeper vendored copy that
        // declares the same frontmatter name.
        fs::create_dir_all(home.join("skills").join("dup")).unwrap();
        fs::create_dir_all(home.join("skills").join("engineering").join("dup")).unwrap();
        fs::write(
            home.join("skills").join("dup").join("SKILL.md"),
            "---\nname: dup\ndescription: installed copy\n---\nroot\n",
        )
        .unwrap();
        fs::write(
            home.join("skills")
                .join("engineering")
                .join("dup")
                .join("SKILL.md"),
            "---\nname: dup\ndescription: vendored copy\n---\nnested\n",
        )
        .unwrap();

        let store = SkillStore::new(&home, &cwd);
        let skills = store.list().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "installed copy");
        assert_eq!(skills[0].dir, home.join("skills").join("dup"));
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
        // Skills surface under their canonical `/skill:name` spelling.
        assert!(got.contains(&"/skill:histo".to_string()), "got: {got:?}");
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
        // Every command spelling (canonical + alias) precedes skills, which
        // appear under their canonical `/skill:name` spelling.
        assert!(values.contains(&"/help"));
        assert!(values.contains(&"/resume")); // alias of /load
        assert!(values.contains(&"/skill:grill"));
        assert_eq!(*values.last().unwrap(), "/skill:grill");
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
        let items = complete("/gr", std::slice::from_ref(&s));
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"/skill:grill"), "got: {values:?}");
        // The canonical `/skill:` prefix also completes.
        let items = complete("/skill:gr", std::slice::from_ref(&s));
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert!(values.contains(&"/skill:grill"), "got: {values:?}");
    }

    #[test]
    fn complete_matches_skill_by_bare_name_not_trigger_prefix() {
        // The `/skill:` trigger is pure spelling: matching against it would
        // let any letter of "skill" (k, i, l, s) match every installed skill.
        // A query containing only such a letter must find nothing.
        let alpha = parse_skill(
            "---\nname: alpha\ndescription: first\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let beta = parse_skill(
            "---\nname: beta\ndescription: second\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let skills = [alpha, beta];
        // 'k' is in the literal trigger prefix but in neither name.
        let items = complete("/k", &skills);
        assert!(
            items.iter().all(|i| !i.value.starts_with("/skill:")),
            "trigger-prefix letters must not match skills: {items:?}"
        );
        // Name letters still match, under the canonical trigger value.
        let items = complete("/bet", &skills);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, vec!["/skill:beta"]);
        assert_eq!(items[0].description, "second");
        // The exact canonical name alone (not a skill candidate) does not
        // leak: `/skill` matches only commands, never every skill.
        let items = complete("/skill", &skills);
        assert!(
            items.iter().all(|i| !i.value.starts_with("/skill:")),
            "trigger spelling without a name must not match skills: {items:?}"
        );
    }

    #[test]
    fn complete_typed_trigger_prefix_matches_name_part_and_lists_all_when_bare() {
        let alpha = parse_skill(
            "---\nname: alpha\ndescription: first\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let beta = parse_skill(
            "---\nname: beta\ndescription: second\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let skills = [alpha, beta];
        // `/skill:bet` matches by the name part after the trigger prefix.
        let items = complete("/skill:bet", &skills);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, vec!["/skill:beta"]);
        // A bare `/skill:` lists every skill, like a bare `/` does.
        let items = complete("/skill:", &skills);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, vec!["/skill:alpha", "/skill:beta"]);
    }

    #[test]
    fn complete_ranks_fuzzy_matches_best_first() {
        // A skill whose bare name exactly equals the query scores the exact
        // match (best) and outranks the /save command that only starts with
        // the same letters: matching happens on the name, not the trigger.
        let s = parse_skill(
            "---\nname: sav\ndescription: save quick\n---\nb\n",
            SkillScope::User,
        )
        .unwrap();
        let items = complete("/sav", &[s]);
        let values: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(*values.first().unwrap(), "/skill:sav", "{values:?}");
        assert!(values.contains(&"/save"), "got: {values:?}");
        // The exact-name match is ranked strictly above the fuzzy command hit.
        let sav = items.iter().position(|i| i.value == "/skill:sav").unwrap();
        let save = items.iter().position(|i| i.value == "/save").unwrap();
        assert!(sav < save, "{values:?}");
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
