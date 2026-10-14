//! The source-scalar portion of native session description. Usage identities,
//! busy intervals and other output-sized facts are reduced by their own indexed
//! owners; this cursor never retains transcript events or historical usage.
use serde::{Deserialize, Serialize};

pub(crate) struct SourceDescription<'a> {
    pub(crate) first: Option<i64>,
    pub(crate) last: Option<i64>,
    pub(crate) model: Option<&'a str>,
    pub(crate) title: Option<&'a str>,
    pub(crate) agent_name: Option<&'a str>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DescriptionFacts {
    pub(crate) first: i64,
    pub(crate) last: i64,
    pub(crate) model: Option<String>,
    pub(crate) title: Option<String>,
}

impl Default for DescriptionFacts {
    fn default() -> Self {
        Self {
            first: i64::MAX,
            last: 0,
            model: None,
            title: None,
        }
    }
}

impl DescriptionFacts {
    /// Sources arrive in the native lineage's order, not path or SQL order.
    /// A later named model or title replaces the earlier observation; an agent
    /// name supplies a title only while no explicit title has been observed.
    pub(crate) fn push(&mut self, source: SourceDescription<'_>) {
        if let Some(model) = source.model {
            self.model = Some(model.to_owned());
        }
        self.first = self.first.min(source.first.unwrap_or(i64::MAX));
        self.last = self.last.max(source.last.unwrap_or(0));
        if let Some(title) = source.title {
            self.title = Some(title.to_owned());
        } else if self.title.is_none()
            && let Some(name) = source.agent_name
        {
            self.title = Some(name.to_owned());
        }
    }

    pub(crate) fn model(&self, fallback: Option<&str>) -> String {
        self.model
            .as_deref()
            .and_then(super::pretty_model)
            .or_else(|| fallback.and_then(super::pretty_model))
            .unwrap_or_else(|| "—".into())
    }
}

pub(crate) fn name(
    codex_task: Option<String>,
    native_session: bool,
    title: Option<String>,
    fallback: String,
    branch: Option<&str>,
    repo: Option<&str>,
) -> String {
    codex_task.unwrap_or_else(|| {
        if native_session {
            title.unwrap_or(fallback)
        } else {
            branch.or(repo).unwrap_or("Codex run").to_owned()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_description_keeps_lineage_source_order_across_resume() {
        let mut facts = DescriptionFacts::default();
        facts.push(SourceDescription {
            first: Some(30),
            last: Some(40),
            model: None,
            title: None,
            agent_name: Some("agent"),
        });
        let mut facts: DescriptionFacts =
            serde_json::from_str(&serde_json::to_string(&facts).unwrap()).unwrap();
        facts.push(SourceDescription {
            first: Some(10),
            last: Some(20),
            model: Some("unknown"),
            title: Some("explicit"),
            agent_name: Some("later agent"),
        });
        facts.push(SourceDescription {
            first: None,
            last: None,
            model: None,
            title: None,
            agent_name: Some("ignored"),
        });
        assert_eq!((facts.first, facts.last), (10, 40));
        assert_eq!(facts.title.as_deref(), Some("explicit"));
        assert_eq!(facts.model.as_deref(), Some("unknown"));
        assert_eq!(
            name(
                None,
                true,
                facts.title,
                "fallback".into(),
                Some("branch"),
                Some("repo")
            ),
            "explicit"
        );
        assert_eq!(
            name(
                None,
                false,
                Some("ignored".into()),
                "fallback".into(),
                Some("branch"),
                Some("repo")
            ),
            "branch"
        );
    }
}
