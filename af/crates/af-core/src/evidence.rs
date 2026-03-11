use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A reference to a piece of evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvidenceRef {
    Artifact(Uuid),
    ToolRun(Uuid),
    Message(Uuid),
    Plugin {
        namespace: String,
        kind: String,
        id: String,
    },
}

impl EvidenceRef {
    pub fn parse(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.strip_prefix("evidence:")?.splitn(2, ':').collect();
        match parts.as_slice() {
            ["artifact", id] => Some(Self::Artifact(Uuid::parse_str(id).ok()?)),
            ["tool_run", id] => Some(Self::ToolRun(Uuid::parse_str(id).ok()?)),
            ["message", id] => Some(Self::Message(Uuid::parse_str(id).ok()?)),
            [ns, rest] => {
                let (kind, id) = rest.split_once(':')?;
                Some(Self::Plugin {
                    namespace: ns.to_string(),
                    kind: kind.to_string(),
                    id: id.to_string(),
                })
            }
            _ => None,
        }
    }

    pub fn to_inline(&self) -> String {
        match self {
            Self::Artifact(id) => format!("evidence:artifact:{id}"),
            Self::ToolRun(id) => format!("evidence:tool_run:{id}"),
            Self::Message(id) => format!("evidence:message:{id}"),
            Self::Plugin {
                namespace,
                kind,
                id,
            } => format!("evidence:{namespace}:{kind}:{id}"),
        }
    }
}

/// Resolves plugin-defined evidence references.
pub trait EvidenceResolver: Send + Sync {
    fn namespace(&self) -> &str;
    fn resolve(&self, kind: &str, id: &str) -> Option<String>;
    /// Returns an SQL query that checks if the evidence record exists and belongs
    /// to the given project. The query must accept `$1::uuid` (record id) and
    /// `$2::uuid` (project_id), and return at least one row if the record exists.
    /// Table names must be schema-qualified (e.g. `re.iocs`).
    /// Default: None (no DB existence check — format validation only).
    fn existence_query(&self, _kind: &str) -> Option<&str> {
        None
    }
}

/// Registry of evidence resolvers.
pub struct EvidenceResolverRegistry {
    resolvers: Vec<Box<dyn EvidenceResolver>>,
}

impl EvidenceResolverRegistry {
    pub fn new() -> Self {
        Self {
            resolvers: Vec::new(),
        }
    }

    pub fn register(&mut self, resolver: Box<dyn EvidenceResolver>) {
        self.resolvers.push(resolver);
    }

    pub fn resolve(&self, namespace: &str, kind: &str, id: &str) -> Option<String> {
        self.resolvers
            .iter()
            .find(|r| r.namespace() == namespace)
            .and_then(|r| r.resolve(kind, id))
    }

    pub fn existence_query(&self, namespace: &str, kind: &str) -> Option<&str> {
        self.resolvers
            .iter()
            .find(|r| r.namespace() == namespace)
            .and_then(|r| r.existence_query(kind))
    }
}
