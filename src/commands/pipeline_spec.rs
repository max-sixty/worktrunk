use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Serialized specification for a background pipeline.
///
/// Serialized to JSON and piped to stdin of `wt hook run-pipeline`.
/// Contains raw templates — expansion happens at execution time in
/// the background process.
#[derive(Serialize, Deserialize)]
pub struct PipelineSpec {
    pub worktree_path: PathBuf,
    pub branch: String,
    pub hook_type: String,
    pub source: String,
    /// Base context variables for template expansion.
    pub context: HashMap<String, String>,
    pub steps: Vec<PipelineStepSpec>,
}

#[derive(Serialize, Deserialize)]
pub enum PipelineStepSpec {
    Single {
        name: Option<String>,
        template: String,
    },
    Concurrent {
        commands: Vec<PipelineCommandSpec>,
    },
}

#[derive(Serialize, Deserialize)]
pub struct PipelineCommandSpec {
    pub name: Option<String>,
    pub template: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_spec_roundtrip() {
        let spec = PipelineSpec {
            worktree_path: "/tmp/test-worktree".into(),
            branch: "feature/auth".into(),
            hook_type: "post-start".into(),
            source: "user".into(),
            context: [("branch".into(), "feature/auth".into())]
                .into_iter()
                .collect(),
            steps: vec![
                PipelineStepSpec::Single {
                    name: Some("install".into()),
                    template: "npm install".into(),
                },
                PipelineStepSpec::Concurrent {
                    commands: vec![
                        PipelineCommandSpec {
                            name: Some("build".into()),
                            template: "npm run build".into(),
                        },
                        PipelineCommandSpec {
                            name: None,
                            template: "echo {{ vars.tag }}".into(),
                        },
                    ],
                },
            ],
        };

        let json = serde_json::to_string(&spec).unwrap();
        let roundtripped: PipelineSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtripped.worktree_path, spec.worktree_path);
        assert_eq!(roundtripped.branch, spec.branch);
        assert_eq!(roundtripped.hook_type, spec.hook_type);
        assert_eq!(roundtripped.source, spec.source);
        assert_eq!(roundtripped.context, spec.context);
        assert_eq!(roundtripped.steps.len(), 2);

        // Verify step structure survives roundtrip
        match &roundtripped.steps[0] {
            PipelineStepSpec::Single { name, template } => {
                assert_eq!(name.as_deref(), Some("install"));
                assert_eq!(template, "npm install");
            }
            _ => panic!("expected Single step"),
        }
        match &roundtripped.steps[1] {
            PipelineStepSpec::Concurrent { commands } => {
                assert_eq!(commands.len(), 2);
                assert_eq!(commands[0].name.as_deref(), Some("build"));
                assert!(commands[1].template.contains("vars.tag"));
            }
            _ => panic!("expected Concurrent step"),
        }
    }
}
