use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LatexProject {
    pub preamble_name: String,
    pub preamble: String,
    pub bibliography_name: String,
    pub bibliography: String,
}

impl Default for LatexProject {
    fn default() -> Self {
        Self {
            preamble_name: "preamble.tex".into(),
            preamble: String::new(),
            bibliography_name: "references.bib".into(),
            bibliography: String::new(),
        }
    }
}

impl LatexProject {
    pub fn validate(&self) -> Result<()> {
        for (name, extensions) in [
            (&self.preamble_name, &["tex", "cls"][..]),
            (&self.bibliography_name, &["bib"][..]),
        ] {
            ensure!(
                !name.is_empty()
                    && !name.starts_with('.')
                    && !name
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '/' | '\\'))
                    && name
                        .rsplit_once('.')
                        .is_some_and(|(_, ext)| extensions.contains(&ext)),
                "LaTeX project files must be named .tex/.cls and .bib files without paths"
            );
        }
        ensure!(
            self.preamble.len() <= 2 * 1024 * 1024,
            "LaTeX preamble exceeds 2 MiB"
        );
        ensure!(
            self.bibliography.len() <= 16 * 1024 * 1024,
            "bibliography exceeds 16 MiB"
        );
        Ok(())
    }

    pub fn key(&self) -> String {
        crate::store::digest(&serde_json::to_vec(self).expect("serializable LaTeX project"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_files_are_versioned_content_not_local_paths() {
        let mut project = LatexProject::default();
        project.preamble = "\\newcommand{\\cA}{\\mathcal{A}}".into();
        let original_key = project.key();
        project.validate().unwrap();
        project.bibliography = "@book{sample,title={Example},year={2026}}".into();
        assert_ne!(original_key, project.key());
        assert_eq!(
            serde_json::from_str::<LatexProject>(&serde_json::to_string(&project).unwrap())
                .unwrap(),
            project
        );
        for name in ["../macros.tex", "/tmp/macros.tex", "macros.sty", "a\\b.tex"] {
            project.preamble_name = name.into();
            assert!(project.validate().is_err(), "{name}");
        }
    }
}
