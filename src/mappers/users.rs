use task_manager_shared::users::UserResponse;

use crate::board::UserModel;
use crate::postgres::UserDto;

impl From<&UserDto> for UserModel {
    fn from(src: &UserDto) -> Self {
        Self {
            email: src.email.trim().to_lowercase(),
            name: src.name.clone(),
            admin: src.admin,
            disabled: src.disabled,
            created: src.created,
        }
    }
}

impl From<&UserModel> for UserDto {
    fn from(src: &UserModel) -> Self {
        Self {
            email: src.email.clone(),
            name: src.name.clone(),
            admin: src.admin,
            disabled: src.disabled,
            created: src.created,
        }
    }
}

/// Memory -> wire.
///
/// `admin_from_settings` cannot be read off the user row — it is the settings admin list — so it is
/// passed in. The UI needs the distinction: an admin by settings has no checkbox to untick, and
/// showing one that silently does nothing would be worse than showing none.
pub fn user_to_response(src: &UserModel, admin_from_settings: bool) -> UserResponse {
    UserResponse {
        email: src.email.clone(),
        name: src.name.clone(),
        disabled: src.disabled,
        admin: src.admin || admin_from_settings,
        admin_from_settings,
    }
}
