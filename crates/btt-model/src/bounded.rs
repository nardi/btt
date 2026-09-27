//! Integer newtypes whose constructor checks a range.

use thiserror::Error;

/// Returned when a value falls outside the range its type allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{type_name} must be between {minimum} and {maximum}, got {value}")]
pub struct ValueOutOfRangeError {
    /// Name of the type that rejected the value.
    pub type_name: &'static str,
    /// The rejected value.
    pub value: i64,
    /// Smallest allowed value.
    pub minimum: i64,
    /// Largest allowed value.
    pub maximum: i64,
}

/// Defines a newtype over an integer with a single named field and a checked range.
///
/// The field stays private, so the only ways to build a value are `new` and `TryFrom`.
/// Deserialization goes through `new` as well. The type serializes and exports to TypeScript
/// as a plain number.
macro_rules! bounded_integer {
    (
        $(#[$type_meta:meta])*
        $type_name:ident { $field:ident: $integer:ty } in $minimum:expr, $maximum:expr
    ) => {
        $(#[$type_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, specta::Type)]
        #[specta(type = $integer)]
        pub struct $type_name {
            $field: $integer,
        }

        impl $type_name {
            /// Smallest allowed value.
            pub const MINIMUM: $integer = $minimum;
            /// Largest allowed value.
            pub const MAXIMUM: $integer = $maximum;

            /// Checks the range and wraps the value.
            ///
            /// # Errors
            ///
            /// Returns an error when the value is outside `MINIMUM..=MAXIMUM`.
            #[allow(unused_comparisons, clippy::cast_lossless)]
            pub const fn new(
                $field: $integer,
            ) -> Result<Self, $crate::bounded::ValueOutOfRangeError> {
                if $field < Self::MINIMUM || $field > Self::MAXIMUM {
                    return Err($crate::bounded::ValueOutOfRangeError {
                        type_name: stringify!($type_name),
                        value: $field as i64,
                        minimum: Self::MINIMUM as i64,
                        maximum: Self::MAXIMUM as i64,
                    });
                }
                Ok(Self { $field })
            }

            /// Returns the wrapped value.
            pub const fn $field(self) -> $integer {
                self.$field
            }
        }

        impl TryFrom<$integer> for $type_name {
            type Error = $crate::bounded::ValueOutOfRangeError;

            fn try_from(value: $integer) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$type_name> for $integer {
            fn from(value: $type_name) -> Self {
                value.$field
            }
        }

        impl serde::Serialize for $type_name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serde::Serialize::serialize(&self.$field, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $type_name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = <$integer as serde::Deserialize>::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

pub(crate) use bounded_integer;
