use sqlx::PgConnection;
use uuid::Uuid;

/// Well-known sentinel UUID for the @all pseudo-member.
pub const ALL_USERS_SENTINEL: Uuid = Uuid::from_u128(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectRole {
    Owner,
    Manager,
    Collaborator,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Read,
    Write,
    Delete,
    ManageMembers,
}

#[derive(Debug)]
pub enum AuthzError {
    Forbidden(String),
    NotMember,
    DbError(String),
}

impl std::fmt::Display for AuthzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthzError::Forbidden(msg) => write!(f, "forbidden: {msg}"),
            AuthzError::NotMember => write!(f, "not a project member"),
            AuthzError::DbError(msg) => write!(f, "database error: {msg}"),
        }
    }
}

impl From<sqlx::Error> for AuthzError {
    fn from(e: sqlx::Error) -> Self {
        AuthzError::DbError(e.to_string())
    }
}

impl ProjectRole {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "manager" => Some(Self::Manager),
            "collaborator" => Some(Self::Collaborator),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub fn allows(&self, action: Action) -> bool {
        match self {
            ProjectRole::Owner => true,
            ProjectRole::Manager => matches!(action, Action::Read | Action::Write | Action::ManageMembers),
            ProjectRole::Collaborator => matches!(action, Action::Read | Action::Write),
            ProjectRole::Viewer => matches!(action, Action::Read),
        }
    }

    /// Return a numeric priority for picking the higher role.
    fn priority(&self) -> u8 {
        match self {
            ProjectRole::Owner => 4,
            ProjectRole::Manager => 3,
            ProjectRole::Collaborator => 2,
            ProjectRole::Viewer => 1,
        }
    }
}

/// Check that `user_id` has permission to perform `required` action on `project_id`.
///
/// Takes `&mut PgConnection` so it can be called inside a scoped transaction
/// (via reborrow `&mut *tx`). Two sequential queries use reborrows.
///
/// 1. If `projects.owner_id = user_id` → Owner role.
/// 2. Check direct membership in `project_members`.
/// 3. Check `@all` sentinel membership (public access).
/// 4. Pick the higher of direct vs @all role.
/// 5. No membership → `AuthzError::NotMember`.
/// 6. Insufficient role → `AuthzError::Forbidden`.
pub async fn check_project_access(
    db: &mut PgConnection,
    user_id: Uuid,
    project_id: Uuid,
    required: Action,
) -> Result<(), AuthzError> {
    // Check direct ownership
    let owner_row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT owner_id FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .fetch_optional(&mut *db)
    .await?;

    let owner_id = match owner_row {
        Some((oid,)) => oid,
        None => return Err(AuthzError::Forbidden("project not found".into())),
    };

    if owner_id == Some(user_id) {
        return Ok(()); // Owner can do everything
    }

    // Check direct membership
    let direct_role: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(&mut *db)
    .await?;

    // Check @all sentinel membership
    let all_role: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(project_id)
    .bind(ALL_USERS_SENTINEL)
    .fetch_optional(&mut *db)
    .await?;

    // Pick the higher of direct membership vs @all
    let direct = direct_role.and_then(|r| ProjectRole::from_str(&r.0));
    let all = all_role.and_then(|r| ProjectRole::from_str(&r.0));

    let role = match (direct, all) {
        (Some(d), Some(a)) => {
            if d.priority() >= a.priority() { Some(d) } else { Some(a) }
        }
        (Some(d), None) => Some(d),
        (None, Some(a)) => Some(a),
        (None, None) => None,
    };

    let role = match role {
        Some(r) => r,
        None => return Err(AuthzError::NotMember),
    };

    if role.allows(required) {
        Ok(())
    } else {
        Err(AuthzError::Forbidden(format!(
            "{:?} role cannot perform {:?}",
            role, required
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_permissions() {
        // Owner can do everything
        assert!(ProjectRole::Owner.allows(Action::Read));
        assert!(ProjectRole::Owner.allows(Action::Write));
        assert!(ProjectRole::Owner.allows(Action::Delete));
        assert!(ProjectRole::Owner.allows(Action::ManageMembers));

        // Manager: Read + Write + ManageMembers
        assert!(ProjectRole::Manager.allows(Action::Read));
        assert!(ProjectRole::Manager.allows(Action::Write));
        assert!(!ProjectRole::Manager.allows(Action::Delete));
        assert!(ProjectRole::Manager.allows(Action::ManageMembers));

        // Collaborator: Read + Write
        assert!(ProjectRole::Collaborator.allows(Action::Read));
        assert!(ProjectRole::Collaborator.allows(Action::Write));
        assert!(!ProjectRole::Collaborator.allows(Action::Delete));
        assert!(!ProjectRole::Collaborator.allows(Action::ManageMembers));

        // Viewer: Read only
        assert!(ProjectRole::Viewer.allows(Action::Read));
        assert!(!ProjectRole::Viewer.allows(Action::Write));
        assert!(!ProjectRole::Viewer.allows(Action::Delete));
        assert!(!ProjectRole::Viewer.allows(Action::ManageMembers));
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!(ProjectRole::from_str("owner"), Some(ProjectRole::Owner));
        assert_eq!(ProjectRole::from_str("manager"), Some(ProjectRole::Manager));
        assert_eq!(ProjectRole::from_str("collaborator"), Some(ProjectRole::Collaborator));
        assert_eq!(ProjectRole::from_str("viewer"), Some(ProjectRole::Viewer));
        assert_eq!(ProjectRole::from_str("editor"), None);
        assert_eq!(ProjectRole::from_str("admin"), None);
        assert_eq!(ProjectRole::from_str(""), None);
    }

    #[test]
    fn test_role_priority() {
        assert!(ProjectRole::Owner.priority() > ProjectRole::Manager.priority());
        assert!(ProjectRole::Manager.priority() > ProjectRole::Collaborator.priority());
        assert!(ProjectRole::Collaborator.priority() > ProjectRole::Viewer.priority());
    }

    #[test]
    fn test_all_users_sentinel() {
        assert_eq!(ALL_USERS_SENTINEL, Uuid::from_u128(0));
        assert_eq!(ALL_USERS_SENTINEL.to_string(), "00000000-0000-0000-0000-000000000000");
    }
}
