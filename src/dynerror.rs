//! An error type which is meant to encapsulate all other error types to allow
//! more weakly typed errors, which in an application's context allows for less
//! friction.
//!
//! In the context of a library, it's reccomended to use the standard error
//! patterns, as this library makes superfluous allocations and attaches
//! metadata to all errors such to enable a more convenient API and more
//! debugging-oriented error reporting.
use std::{error::Error as StdError, ops::Deref};

/// A convenience wrapper around `Result<T, dynerror::Error>`.
pub type Result<T> = ::core::result::Result<T, Error>;

/// A weakly typed wrapper for errors to allow converting other error types to.
pub struct Error {
    msg: Box<dyn core::fmt::Display>,
    wraps: Option<Box<dyn std::error::Error>>,
    file: String,
    line: u32,
    column: u32,
}

impl Error {
    /// Creates a new error with the given message printing when the error is
    /// displayed.
    #[track_caller]
    pub fn new(msg: impl core::fmt::Display) -> Error {
        let loc = std::panic::Location::caller();
        Error {
            msg: Box::new(msg.to_string()),
            wraps: None,
            file: loc.file().to_string(),
            line: loc.line(),
            column: loc.column(),
        }
    }

    /// Converts an [StdError] into an [Error].
    #[track_caller]
    pub fn from_error(err: impl StdError) -> Error {
        let loc = std::panic::Location::caller();
        Error {
            msg: Box::new(err.to_string()),
            wraps: err.source().map(|e| {
                Box::<dyn StdError>::from(Box::new(Error::from_error(e)))
            }),
            file: loc.file().to_string(),
            line: loc.line(),
            column: loc.column(),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.wraps {
            Some(err) => Some(err.deref()),
            None => None,
        }
    }
}

impl core::fmt::Debug for Error {
    /// Prints the error as an error return trace. An example of the output:
    /// ```txt
    /// error:main.rs:5:1: Initialization failed
    ///
    /// caused by:
    ///     main.rs:15:6:1: Couldn't open the config file
    ///     main.rs:20:5:1: No such file or directory (os error 2)
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "error:{}:{}:{}: {}",
            self.file, self.line, self.column, self.msg
        )?;

        if self.wraps.is_none() {
            return Ok(());
        }

        write!(f, "\n\ncaused by:\n\t")?;
        let mut current_source = self.source();
        while let Some(source) = current_source {
            if let Some(e) = source.downcast_ref::<Error>() {
                writeln!(f, "{}:{}:{}: {}", e.file, e.line, e.column, e.msg)?;
            } else {
                writeln!(f, "{}", source)?;
            }
            current_source = source.source();
        }

        Ok(())
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "{}", self.msg)
        } else {
            match &self.wraps {
                Some(err) => {
                    write!(f, "{}: {}", self.msg, err)
                }
                None => write!(f, "{}", self.msg),
            }
        }
    }
}

pub trait Context<T> {
    /// Adds some context to the error which is used in the
    /// [core::fmt::Display] implementation for [Result].
    fn context<C>(self, context: C) -> Result<T>
    where
        C: core::fmt::Display + 'static;

    /// Adds context to the error which is evaluated only if an error
    /// actually occurs.
    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: core::fmt::Display + 'static,
        F: FnOnce() -> C;

    /// Converts the type into a dynamic [Result], adding no further
    /// context.
    fn result(self) -> Result<T>;
}

impl<T> Context<T> for ::core::option::Option<T> {
    #[track_caller]
    fn context<C>(self, context: C) -> Result<T>
    where
        C: core::fmt::Display + 'static,
    {
        let loc = std::panic::Location::caller();
        match self {
            Some(t) => Ok(t),
            None => Err(Error {
                msg: Box::new(context),
                wraps: None,
                file: loc.file().to_string(),
                line: loc.line(),
                column: loc.column(),
            }),
        }
    }

    #[track_caller]
    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: core::fmt::Display + 'static,
        F: FnOnce() -> C,
    {
        let loc = std::panic::Location::caller();
        match self {
            Some(t) => Ok(t),
            None => Err(Error {
                msg: Box::new(f()),
                wraps: None,
                file: loc.file().to_string(),
                line: loc.line(),
                column: loc.column(),
            }),
        }
    }

    #[track_caller]
    fn result(self) -> Result<T> {
        let loc = std::panic::Location::caller();
        match self {
            Some(t) => Ok(t),
            None => Err(Error {
                msg: Box::new("an optional value was None"),
                wraps: None,
                file: loc.file().to_string(),
                line: loc.line(),
                column: loc.column(),
            }),
        }
    }
}

impl<T, E: std::error::Error + 'static> Context<T>
    for ::core::result::Result<T, E>
{
    #[track_caller]
    fn context<C>(self, context: C) -> Result<T>
    where
        C: core::fmt::Display + 'static,
    {
        let loc = std::panic::Location::caller();
        match self {
            Ok(t) => Ok(t),
            Err(e) => Err(Error {
                msg: Box::new(context),
                wraps: Some(Box::new(e)),
                file: loc.file().to_string(),
                line: loc.line(),
                column: loc.column(),
            }),
        }
    }

    #[track_caller]
    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: core::fmt::Display + 'static,
        F: FnOnce() -> C,
    {
        let loc = std::panic::Location::caller();
        match self {
            Ok(t) => Ok(t),
            Err(e) => Err(Error {
                msg: Box::new(f()),
                wraps: Some(Box::new(e)),
                file: loc.file().to_string(),
                line: loc.line(),
                column: loc.column(),
            }),
        }
    }

    #[track_caller]
    fn result(self) -> Result<T> {
        let loc = std::panic::Location::caller();
        match self {
            Ok(t) => Ok(t),
            Err(e) => Err(Error {
                msg: Box::new(e.to_string()),
                wraps: e.source().map(|e| {
                    Box::<dyn StdError>::from(Box::new(Error::from_error(e)))
                }),
                file: loc.file().to_string(),
                line: loc.line(),
                column: loc.column(),
            }),
        }
    }
}

#[macro_export]
macro_rules! bail {
    ($($tts:tt)*) => {return Err(crate::dynerror::Error::new(format!($($tts)*)))};
}

#[macro_export]
macro_rules! err {
    ($($tts:tt)*) => {crate::dynerror::Error::new(format!($($tts)*))};
}
