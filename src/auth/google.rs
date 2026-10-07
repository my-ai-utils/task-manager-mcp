use flurl::FlUrl;
use flurl::body::UrlEncodedBody;
use serde::Deserialize;

/// Who Google says the person is.
pub struct GoogleIdentity {
    pub email: String,
    pub name: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct UserInfoResponse {
    email: Option<String>,
    name: Option<String>,
    verified_email: Option<bool>,
}

/// Where to send the browser to sign in.
///
/// `prompt=select_account` rather than the default: on a shared machine, or for anyone with a work and
/// a personal Google account, silently reusing whichever one the browser happens to be signed into is
/// how you end up looking at "you have no projects" and not knowing why.
pub fn build_auth_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&prompt=select_account",
        urlencode(client_id),
        urlencode(redirect_uri),
        urlencode("openid email profile"),
        urlencode(state),
    )
}

/// Percent-encode a query value.
///
/// Hand-rolled because the only characters that matter here are the ones a client id, a redirect URI
/// and a UUID can contain, and pulling a dependency in for that is not worth it. Unreserved characters
/// per RFC 3986 pass through; everything else is escaped.
fn urlencode(src: &str) -> String {
    let mut result = String::with_capacity(src.len());

    for byte in src.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char)
            }
            _ => result.push_str(&format!("%{byte:02X}")),
        }
    }

    result
}

/// Turn the one-time `code` Google handed the browser into the person's identity.
///
/// Two calls, both of which have to succeed: exchange the code for an access token, then read the
/// userinfo endpoint with it. Errors come back as text — the caller turns them into a 401, because from
/// the browser's point of view every failure here means "you are not signed in".
pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<GoogleIdentity, String> {
    let body = UrlEncodedBody::new()
        .append("code", code)
        .append("client_id", client_id)
        .append("client_secret", client_secret)
        .append("redirect_uri", redirect_uri)
        .append("grant_type", "authorization_code");

    let mut response = FlUrl::new("https://oauth2.googleapis.com")
        .append_path_segment("token")
        .post(body)
        .await
        .map_err(|err| format!("google: token request failed: {err:?}"))?;

    let token: TokenResponse = response
        .get_json()
        .await
        .map_err(|err| format!("google: token response was not the JSON we expected: {err:?}"))?;

    if let Some(error) = token.error {
        let detail = token.error_description.unwrap_or_default();
        return Err(format!("google refused the sign-in: {error} {detail}"));
    }

    let access_token = token
        .access_token
        .ok_or_else(|| "google: token response carried no access_token".to_string())?;

    let mut response = FlUrl::new("https://www.googleapis.com")
        .append_path_segment("oauth2")
        .append_path_segment("v2")
        .append_path_segment("userinfo")
        .with_header("Authorization", format!("Bearer {access_token}"))
        .get()
        .await
        .map_err(|err| format!("google: userinfo request failed: {err:?}"))?;

    let info: UserInfoResponse = response.get_json().await.map_err(|err| {
        format!("google: userinfo response was not the JSON we expected: {err:?}")
    })?;

    let email = info
        .email
        .map(|itm| itm.trim().to_lowercase())
        .filter(|itm| !itm.is_empty())
        .ok_or_else(|| "google: this account has no email address".to_string())?;

    // An unverified address is refused: the whole access model keys off the email, so accepting one
    // Google itself has not confirmed would let anyone claim a colleague's account.
    if info.verified_email == Some(false) {
        return Err(format!("google has not verified {email}"));
    }

    Ok(GoogleIdentity {
        email,
        name: info.name.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreserved_characters_pass_through_and_the_rest_is_escaped() {
        assert_eq!(urlencode("abcXYZ019-_.~"), "abcXYZ019-_.~");
        assert_eq!(
            urlencode("openid email profile"),
            "openid%20email%20profile"
        );
        // The real redirect URI, so the case that actually matters is the one pinned. `-` and `.` are
        // unreserved and pass through; only the scheme colon and the slashes are escaped.
        assert_eq!(
            urlencode("https://task-manager.jetdev.eu/authorized"),
            "https%3A%2F%2Ftask-manager.jetdev.eu%2Fauthorized"
        );
    }

    /// The auth URL is what a browser is redirected to — a wrong separator or an unescaped redirect
    /// URI shows up as a Google error page, so it is worth pinning the exact shape.
    #[test]
    fn the_auth_url_escapes_every_value_it_carries() {
        let url = build_auth_url("id.apps.googleusercontent.com", "https://tm/cb", "st-1");

        assert!(url.contains("client_id=id.apps.googleusercontent.com"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Ftm%2Fcb"));
        assert!(url.contains("scope=openid%20email%20profile"));
        assert!(url.contains("state=st-1"));
        assert!(url.contains("prompt=select_account"));
    }
}
