//! Serde support for types with a canonical text form, such as
//! identifiers and option names, which serialize as that text.

use crate::{
    Blake3Hash, Dep, LuksOption, Name, PkgRef, Provide, Sha256Hash, Soname, StoreId, Timestamp,
    Variant, Version,
};

/// Implements `serde::Serialize` and `serde::Deserialize` for each listed
/// type through its text form: serializing writes its `Display` output as a
/// string, and deserializing parses a string with its `FromStr`
/// implementation, reporting the parse error as the deserialization error.
///
/// ```
/// use std::fmt;
/// use std::str::FromStr;
///
/// #[derive(Debug, PartialEq)]
/// struct Port(u16);
///
/// impl fmt::Display for Port {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         write!(f, "port {}", self.0)
///     }
/// }
///
/// impl FromStr for Port {
///     type Err = std::num::ParseIntError;
///
///     fn from_str(s: &str) -> Result<Self, Self::Err> {
///         s.trim_start_matches("port ").parse().map(Port)
///     }
/// }
///
/// laft_core::impl_serde_as_text!(Port);
///
/// assert_eq!(serde_json::to_string(&Port(22))?, r#""port 22""#);
/// assert_eq!(serde_json::from_str::<Port>(r#""port 22""#)?, Port(22));
/// # Ok::<(), serde_json::Error>(())
/// ```
#[macro_export]
macro_rules! impl_serde_as_text {
    ($($ty:ty),* $(,)?) => {$(
        impl $crate::__private::serde::Serialize for $ty {
            fn serialize<S: $crate::__private::serde::Serializer>(
                &self,
                serializer: S,
            ) -> ::std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> $crate::__private::serde::Deserialize<'de> for $ty {
            fn deserialize<D: $crate::__private::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> ::std::result::Result<Self, D::Error> {
                let text = <::std::string::String as $crate::__private::serde::Deserialize>::deserialize(
                    deserializer,
                )?;
                text.parse()
                    .map_err(<D::Error as $crate::__private::serde::de::Error>::custom)
            }
        }
    )*};
}

crate::impl_serde_as_text!(
    Blake3Hash, Dep, LuksOption, Name, PkgRef, Provide, Sha256Hash, Soname, StoreId, Timestamp,
    Variant, Version
);

#[cfg(test)]
#[path = "text_serde_tests.rs"]
mod tests;
