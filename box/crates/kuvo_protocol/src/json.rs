//! Serde adapters for the parts of the shape language JSON represents literally.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// An explicit JSON null. An optional field can independently be absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Null;

pub fn present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// A required nullable value. Use Option<Nullable<T>> when it may also be omitted.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Nullable<T> {
    Null(Null),
    Value(T),
}

// Serde normally accepts arrays as structs. Decode via a map at this JSON boundary
// so an object variant cannot accidentally accept a list from a union.
macro_rules! wire_object {
    ($name:ident, $remote:literal, { $($(#[$attr:meta])* pub $field:ident: $ty:ty,)* }) => {
        #[derive(Clone, Debug, PartialEq, serde::Serialize)]
        pub struct $name {
            $($(#[$attr])* pub $field: $ty,)*
        }

        const _: () = {
            #[derive(serde::Deserialize)]
            #[serde(remote = $remote, deny_unknown_fields)]
            struct Wire {
                $($(#[$attr])* $field: $ty,)*
            }

            impl<'de> serde::Deserialize<'de> for $name {
                fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                    struct Object;
                    impl<'de> serde::de::Visitor<'de> for Object {
                        type Value = $name;

                        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            formatter.write_str("a JSON object")
                        }

                        fn visit_map<M: serde::de::MapAccess<'de>>(self, map: M) -> Result<$name, M::Error> {
                            Wire::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                        }
                    }
                    deserializer.deserialize_map(Object)
                }
            }
        };
    };
}

pub(crate) use wire_object;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoolLiteral<const VALUE: bool>;

impl<'de, const VALUE: bool> Deserialize<'de> for BoolLiteral<VALUE> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? == VALUE {
            Ok(Self)
        } else {
            Err(de::Error::custom(format_args!("expected {VALUE}")))
        }
    }
}

impl<const VALUE: bool> Serialize for BoolLiteral<VALUE> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(VALUE)
    }
}

/// A numeric literal, with its f64 bits as a const parameter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NumberLiteral<const BITS: u64>;

impl<'de, const BITS: u64> Deserialize<'de> for NumberLiteral<BITS> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let expected = f64::from_bits(BITS);
        if f64::deserialize(deserializer)? == expected {
            Ok(Self)
        } else {
            Err(de::Error::custom(format_args!("expected {expected}")))
        }
    }
}

impl<const BITS: u64> Serialize for NumberLiteral<BITS> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(f64::from_bits(BITS))
    }
}
