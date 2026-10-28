//! Serde support for identifiers, which serialize as their text form.

use crate::{Blake3Hash, Dep, Name, Provide, Soname, StoreId, Timestamp, Variant, Version};

macro_rules! serde_as_text {
    ($($ty:ty),* $(,)?) => {$(
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    )*};
}

serde_as_text!(
    Blake3Hash, Dep, Name, Provide, Soname, StoreId, Timestamp, Variant, Version
);

#[cfg(test)]
#[path = "text_serde_tests.rs"]
mod tests;
