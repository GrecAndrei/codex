use serde::Deserialize;
use std::path::Path;
use std::path::PathBuf;
use tracing::warn;

const PROMPT_HOOKS_FILENAME: &str = "prompt-hooks.toml";
const DOCS_SUBDIR: &str = "docs";
const PROMPT_HOOKS_DOC_FILENAME: &str = "prompt-hooks.md";
const PROMPT_HOOKS_EXAMPLE_FILENAME: &str = "prompt-hooks.example.toml";
const PROMPT_HOOKS_DOCS: &str = include_str!("../../../docs/prompt-hooks.md");
const PROMPT_HOOKS_EXAMPLE: &str = include_str!("../../../docs/prompt-hooks.example.toml");

#[derive(Debug, Clone, Copy)]
pub(crate) enum PromptHookTarget {
    BaseInstructions,
    DeveloperMessage,
    UserContext,
    CompactPrompt,
    ReviewPrompt,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PromptHooks {
    pub(crate) base_instructions: PromptHook,
    pub(crate) developer_message: PromptHook,
    pub(crate) user_context: PromptHook,
    pub(crate) compact_prompt: PromptHook,
    pub(crate) review_prompt: PromptHook,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PromptHook {
    pub(crate) enabled: bool,
    pub(crate) mode: PromptHookMergeMode,
    pub(crate) text: Option<String>,
    pub(crate) file: Option<PathBuf>,
}

impl Default for PromptHook {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: PromptHookMergeMode::Append,
            text: None,
            file: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PromptHookMergeMode {
    #[default]
    Append,
    Prepend,
    Replace,
}

impl PromptHooks {
    pub(crate) fn load(codex_home: &Path) -> Self {
        ensure_codex_home_docs(codex_home);

        let path = codex_home.join(PROMPT_HOOKS_FILENAME);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                warn!("failed to read prompt hooks {}: {err}", path.display());
                return Self::default();
            }
        };

        match toml::from_str::<Self>(&raw) {
            Ok(hooks) => hooks,
            Err(err) => {
                warn!("failed to parse prompt hooks {}: {err}", path.display());
                Self::default()
            }
        }
    }

    pub(crate) fn apply_text(
        &self,
        target: PromptHookTarget,
        codex_home: &Path,
        base: String,
    ) -> String {
        self.hook(target).apply_text(codex_home, base)
    }

    pub(crate) fn has_hook_content(&self, target: PromptHookTarget, codex_home: &Path) -> bool {
        self.hook(target).render(codex_home).is_some()
    }

    pub(crate) fn render_text(
        &self,
        target: PromptHookTarget,
        codex_home: &Path,
    ) -> Option<String> {
        self.hook(target).render(codex_home)
    }

    pub(crate) fn merge_mode(&self, target: PromptHookTarget) -> PromptHookMergeMode {
        self.hook(target).mode
    }

    fn hook(&self, target: PromptHookTarget) -> &PromptHook {
        match target {
            PromptHookTarget::BaseInstructions => &self.base_instructions,
            PromptHookTarget::DeveloperMessage => &self.developer_message,
            PromptHookTarget::UserContext => &self.user_context,
            PromptHookTarget::CompactPrompt => &self.compact_prompt,
            PromptHookTarget::ReviewPrompt => &self.review_prompt,
        }
    }
}

impl PromptHook {
    fn apply_text(&self, codex_home: &Path, base: String) -> String {
        let Some(fragment) = self.render(codex_home) else {
            return base;
        };

        merge_text(&base, &fragment, self.mode)
    }

    fn render(&self, codex_home: &Path) -> Option<String> {
        if !self.enabled {
            return None;
        }

        let file_text = self
            .file
            .as_ref()
            .and_then(|path| read_hook_file(codex_home, path));

        match (file_text, self.text.clone()) {
            (None, None) => None,
            (Some(file_text), None) => Some(file_text),
            (None, Some(text)) => Some(text),
            (Some(file_text), Some(text)) => {
                Some(merge_text(&file_text, &text, PromptHookMergeMode::Append))
            }
        }
    }
}

pub(crate) fn ensure_codex_home_docs(codex_home: &Path) {
    let docs_dir = codex_home.join(DOCS_SUBDIR);
    if let Err(err) = std::fs::create_dir_all(&docs_dir) {
        warn!(
            "failed to create prompt hook docs dir {}: {err}",
            docs_dir.display()
        );
        return;
    }

    write_doc_file(
        &docs_dir.join(PROMPT_HOOKS_DOC_FILENAME),
        PROMPT_HOOKS_DOCS,
        "prompt hook docs",
    );
    write_doc_file(
        &docs_dir.join(PROMPT_HOOKS_EXAMPLE_FILENAME),
        PROMPT_HOOKS_EXAMPLE,
        "prompt hook example",
    );
}

fn write_doc_file(path: &Path, contents: &str, label: &str) {
    if let Err(err) = std::fs::write(path, contents) {
        warn!("failed to write {label} {}: {err}", path.display());
    }
}

fn read_hook_file(codex_home: &Path, path: &Path) -> Option<String> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        codex_home.join(path)
    };

    match std::fs::read_to_string(&resolved) {
        Ok(contents) => Some(contents),
        Err(err) => {
            warn!(
                "failed to read prompt hook file {}: {err}",
                resolved.display()
            );
            None
        }
    }
}

fn merge_text(base: &str, fragment: &str, mode: PromptHookMergeMode) -> String {
    match mode {
        PromptHookMergeMode::Replace => fragment.to_string(),
        PromptHookMergeMode::Append => join_with_spacing(base, fragment),
        PromptHookMergeMode::Prepend => join_with_spacing(fragment, base),
    }
}

fn join_with_spacing(first: &str, second: &str) -> String {
    match (first.is_empty(), second.is_empty()) {
        (true, true) => String::new(),
        (false, true) => first.to_string(),
        (true, false) => second.to_string(),
        (false, false) => format!("{first}\n\n{second}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn applies_text_hooks_in_all_modes() {
        let tmp = TempDir::new().expect("tempdir");
        let append = PromptHook {
            mode: PromptHookMergeMode::Append,
            text: Some("tail".to_string()),
            ..PromptHook::default()
        };
        let prepend = PromptHook {
            mode: PromptHookMergeMode::Prepend,
            text: Some("head".to_string()),
            ..PromptHook::default()
        };
        let replace = PromptHook {
            mode: PromptHookMergeMode::Replace,
            text: Some("new".to_string()),
            ..PromptHook::default()
        };

        assert_eq!(
            append.apply_text(tmp.path(), "base".to_string()),
            "base\n\ntail"
        );
        assert_eq!(
            prepend.apply_text(tmp.path(), "base".to_string()),
            "head\n\nbase"
        );
        assert_eq!(replace.apply_text(tmp.path(), "base".to_string()), "new");
    }

    #[test]
    fn writes_prompt_hook_docs_into_codex_home() {
        let tmp = TempDir::new().expect("tempdir");

        ensure_codex_home_docs(tmp.path());

        assert!(tmp.path().join("docs/prompt-hooks.md").is_file());
        assert!(tmp.path().join("docs/prompt-hooks.example.toml").is_file());
    }
}
