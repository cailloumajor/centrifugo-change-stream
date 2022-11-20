use std::fmt::Display;

use tracing::error;

#[derive(Debug)]
pub(crate) enum TracedError {
    Centrifugo { code: u16, message: String },
    During { during: String, err: String },
    Kind { kind: String, err: String },
}

pub(crate) trait TracedErrorContext<T> {
    fn context_during(self, during: &str) -> Result<T, TracedError>;
}

impl<T, E> TracedErrorContext<T> for Result<T, E>
where
    E: Display,
{
    fn context_during(self, during: &str) -> Result<T, TracedError> {
        self.map_err(|err| TracedError::During {
            during: String::from(during),
            err: err.to_string(),
        })
    }
}

impl TracedError {
    pub(crate) fn from_kind(kind: &str, err: &str) -> Self {
        Self::Kind {
            kind: String::from(kind),
            err: String::from(err),
        }
    }

    pub(crate) fn from_centrifugo_error(code: u16, message: &str) -> Self {
        Self::Centrifugo {
            code,
            message: String::from(message),
        }
    }

    pub(crate) fn trace_error(&self) {
        match self {
            Self::Centrifugo { code, message } => {
                error!(kind = "from Centrifugo", code, message)
            }
            Self::During { during, err } => {
                error!(during, err);
            }
            Self::Kind { kind, err } => {
                error!(kind, err);
            }
        }
    }
}
