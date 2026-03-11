use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A verified identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub subject: String,
    pub display: Option<String>,
    pub roles: Vec<String>,
    /// Database user ID (None for local CLI identities).
    #[serde(default)]
    pub user_id: Option<Uuid>,
}

impl Identity {
    /// Create a local CLI identity from the OS user.
    pub fn local_operator() -> Self {
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        Self {
            subject: format!("local:{user}"),
            display: Some(user),
            roles: vec!["operator".to_string(), "admin".to_string()],
            user_id: None,
        }
    }
}

/// Derived from Identity for authorization checks.
#[derive(Debug, Clone)]
pub struct ActorContext {
    pub identity: Identity,
    pub is_admin: bool,
}

impl ActorContext {
    pub fn from_identity(identity: Identity) -> Self {
        let is_admin = identity.roles.iter().any(|r| r == "admin");
        Self { identity, is_admin }
    }
}

/// Produces an Identity from an authentication event.
pub trait AuthnProvider: Send + Sync {
    fn authenticate(&self) -> Result<Identity, String>;
}

/// Decides whether a tool run is allowed.
pub trait ToolAuthorizer: Send + Sync {
    fn can_enqueue(
        &self,
        actor: &ActorContext,
        tool: &crate::types::ToolSpec,
    ) -> Result<(), String>;
}

/// Hardcoded local identity provider for CLI MVP.
pub struct LocalAuthnProvider;

impl AuthnProvider for LocalAuthnProvider {
    fn authenticate(&self) -> Result<Identity, String> {
        Ok(Identity::local_operator())
    }
}
