use std::fmt;

/// What every API call in `api/` returns on failure.
///
/// One type rather than a per-endpoint enum: the server already writes its business errors as prose
/// meant for whoever caused them, so the useful thing to do with one is show it.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestError {
    pub message: String,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<flurl::FlUrlError> for RequestError {
    fn from(err: flurl::FlUrlError) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for RequestError {
    fn from(err: serde_json::Error) -> Self {
        Self {
            message: err.to_string(),
        }
    }
}
