//! Type-level security for database IDs
//!
//! This prevents bugs where we might use unvalidated input as database IDs.
//!
//! The flow usually goes like this:
//! 1. External input (e.g. from a user) creates an Id<Untrusted>
//! 2. Validation converts to Id<Trusted>
//! 3. Only Id<Trusted> can be used in database queries, unless explicitly allowed
//!
//! Example:
//! ```
//! let user_input = "123"; // from Slack command
//! let untrusted_id = member::Id::<Untrusted>::new(user_input);
//! // Note: system_id would have to be validated before
//! let trusted_id = untrusted_id.validate_by_system(system_id, &db).await?;
//! // Now we can safely use trusted_id in queries
//! ```

use std::fmt::Debug;

pub trait Trustability: Send + Sync + Debug {}

/// A trusted/valid ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trusted;

impl Trustability for Trusted {}

/// An untrusted ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Untrusted;

impl Trustability for Untrusted {}

#[macro_export]
/// Creates a new ID wrapper for a database ID that can be trusted or untrusted
///
/// Example:
/// ```
/// id!(
///     /// Documentation for the trigger ID wrapper
///     => Trigger
/// )
/// ```
macro_rules! id {
    ($(#[$attr:meta])* => $name:ident) => {
        #[derive(::sqlx::Type, Debug, PartialEq, Eq, Clone, Copy)]
        $(#[$attr])*
        pub struct Id<T: $crate::models::trust::Trustability> {
            pub id: i64,
            trusted: ::std::marker::PhantomData<T>,
        }

        impl ::std::str::FromStr for Id<$crate::models::trust::Untrusted> {
            type Err = ::std::num::ParseIntError;

            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                Ok(Id {
                    id: s.parse()?,
                    trusted: ::std::marker::PhantomData,
                })
            }
        }

        impl<'q, DB> Encode<'q, DB> for Id<$crate::models::trust::Trusted>
        where
            DB: ::sqlx::Database,
            i64: ::sqlx::Encode<'q, DB>,
        {
            fn encode_by_ref(
                &self,
                buf: &mut <DB as ::sqlx::Database>::ArgumentBuffer<'q>,
            ) -> ::std::result::Result<::sqlx::encode::IsNull, ::sqlx::error::BoxDynError> {
                <i64 as ::sqlx::Encode<'_, DB>>::encode_by_ref(&self.id, buf)
            }

            fn produces(&self) -> Option<<DB as ::sqlx::Database>::TypeInfo> {
                <i64 as ::sqlx::Encode<'_, DB>>::produces(&self.id)
            }
        }

        impl<'q, DB> Decode<'q, DB> for Id<$crate::models::trust::Trusted>
        where
            DB: ::sqlx::Database,
            i64: ::sqlx::Decode<'q, DB>,
        {
            fn decode(
                value: <DB as ::sqlx::Database>::ValueRef<'q>,
            ) -> ::std::result::Result<Self, ::sqlx::error::BoxDynError> {
                let id = <i64 as ::sqlx::Decode<'_, DB>>::decode(value)?;
                Ok(Id {
                    id,
                    trusted: std::marker::PhantomData,
                })
            }
        }


        impl<DB> ::sqlx::Type<DB> for Id<Trusted>
        where
            DB: ::sqlx::Database,
            i64: ::sqlx::Type<DB>,
        {
            fn type_info() -> <DB as ::sqlx::Database>::TypeInfo {
                <i64 as ::sqlx::Type<DB>>::type_info()
            }
        }

        impl ::std::fmt::Display for Id<$crate::models::trust::Trusted> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.id)
            }
        }

        impl ::std::fmt::Display for Id<$crate::models::trust::Untrusted> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.id)
            }
        }
    };
}
