use std::fmt::Debug;

pub mod member;
pub mod message;
pub mod system;
pub mod trigger;
pub mod user;

pub trait Trustability: Send + Sync + Debug {}

/// A trusted/valid ID
#[derive(Debug, Clone, Copy)]
pub struct Trusted;

impl Trustability for Trusted {}

/// An untrusted ID
#[derive(Debug, Clone, Copy)]
pub struct Untrusted;

impl Trustability for Untrusted {}

#[macro_export]
/// Creates a new ID wrapper for a database ID that can be trusted or untrusted
macro_rules! id {
    ($(#[$attr:meta])* => $name:ident) => {
        #[derive(::sqlx::Type, Debug, PartialEq, Eq, Clone, Copy)]
        $(#[$attr])*
        pub struct Id<T> {
            pub id: i64,
            trusted: ::std::marker::PhantomData<T>,
        }

        impl<'q, DB> Encode<'q, DB> for Id<$crate::models::Trusted>
        where
            DB: ::sqlx::Database,
            i64: ::sqlx::Encode<'q, DB>,
        {
            fn encode_by_ref(
                &self,
                buf: &mut <DB as ::sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<::sqlx::encode::IsNull, ::sqlx::error::BoxDynError> {
                <i64 as ::sqlx::Encode<'_, DB>>::encode_by_ref(&self.id, buf)
            }

            fn produces(&self) -> Option<<DB as ::sqlx::Database>::TypeInfo> {
                <i64 as ::sqlx::Encode<'_, DB>>::produces(&self.id)
            }
        }

        impl<'q, DB> Decode<'q, DB> for Id<$crate::models::Trusted>
        where
            DB: ::sqlx::Database,
            i64: ::sqlx::Decode<'q, DB>,
        {
            fn decode(
                value: <DB as ::sqlx::Database>::ValueRef<'q>,
            ) -> Result<Self, ::sqlx::error::BoxDynError> {
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

        impl ::std::fmt::Display for Id<$crate::models::Trusted> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.id)
            }
        }

        impl ::std::fmt::Display for Id<$crate::models::Untrusted> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.id)
            }
        }
    };
}
