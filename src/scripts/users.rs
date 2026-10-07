use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;
use task_manager_shared::users::{ASSIGNEE_AI, is_ai_assignee};

use crate::app::AppContext;
use crate::board::UserModel;
use crate::postgres::UserDto;

/// A very loose check. The point is to catch a name typed where an address was meant — anything
/// stricter would eventually refuse a real address, and the real gate is Google: an email that is not
/// a Google account simply never signs in.
fn validate_email(email: &str) -> Result<String, String> {
    let email = email.trim().to_lowercase();

    if is_ai_assignee(&email) {
        return Err(format!(
            "'{ASSIGNEE_AI}' is not a person — it is the reserved assignee meaning an agent does this, and it needs no user row"
        ));
    }

    let looks_like_an_address = email.contains('@')
        && !email.starts_with('@')
        && !email.ends_with('@')
        && !email.contains(' ');

    if !looks_like_an_address {
        return Err(format!("'{email}' does not look like an email address"));
    }

    Ok(email)
}

async fn save(app: &AppContext, user: UserModel) {
    let ctx = MyTelemetryContext::create_empty();
    let dto: UserDto = (&user).into();
    app.users_repo.upsert(&dto, &ctx).await;

    app.board.upsert_user(user);
}

pub async fn create_user(
    app: &AppContext,
    email: &str,
    name: &str,
    admin: bool,
) -> Result<(), String> {
    let email = validate_email(email)?;

    if app.board.read().get_user(&email).is_some() {
        return Err(format!("{email} is already on the roster"));
    }

    save(
        app,
        UserModel {
            email,
            name: name.trim().to_string(),
            admin,
            disabled: false,
            created: DateTimeAsMicroseconds::now(),
        },
    )
    .await;

    Ok(())
}

/// Change a person's name, admin flag or disabled flag.
///
/// The email is not editable: it is the identity, and every task assignment and comment authorship
/// points at it. Someone whose address changes is a new row — their old assignments keep naming the
/// address that made them, which is a record of what happened rather than a live pointer.
pub async fn update_user(
    app: &AppContext,
    email: &str,
    name: &str,
    admin: bool,
    disabled: bool,
) -> Result<(), String> {
    let email = email.trim().to_lowercase();

    let mut user = app
        .board
        .read()
        .get_user(&email)
        .map(|itm| itm.as_ref().clone())
        .ok_or_else(|| format!("{email} is not on the roster"))?;

    user.name = name.trim().to_string();
    user.admin = admin;
    user.disabled = disabled;

    save(app, user).await;
    Ok(())
}

/// Create the row for someone signing in through Google for the first time.
///
/// Only ever called for an email the settings admin list names — that is what breaks the bootstrap
/// loop: on an empty database nobody is an admin in Postgres, so without this nobody could ever create
/// the first row. Everyone else has to be put on the roster by an admin before they can sign in.
pub async fn provision_settings_admin(app: &AppContext, email: &str, name: &str) {
    let email = email.trim().to_lowercase();

    if app.board.read().get_user(&email).is_some() {
        return;
    }

    save(
        app,
        UserModel {
            email,
            name: name.trim().to_string(),
            // Left false deliberately: the admin right comes from the settings list, and writing it
            // into the row too would leave a permanent admin behind after the address is taken off
            // that list.
            admin: false,
            disabled: false,
            created: DateTimeAsMicroseconds::now(),
        },
    )
    .await;
}
