//! Sampling-temperature setting.
//!
//! Most providers accept `temperature`, and chronikl asks for `0.0` so
//! classification and prose are as repeatable as the model allows. Some
//! current models reject the parameter outright (Anthropic's Claude 5.5
//! family: "temperature is deprecated for this model"; OpenAI's
//! reasoning models only take their default). [`Temperature::Omit`]
//! leaves it out of every request.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A configured sampling temperature, or an explicit request to send
/// none at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Temperature {
    /// Send this value with every request.
    Value(f64),
    /// Send no `temperature`; the provider's default applies.
    Omit,
}

impl Temperature {
    /// The value to send, or `None` to leave the parameter out.
    pub fn as_option(self) -> Option<f64> {
        match self {
            Self::Value(v) => Some(v),
            Self::Omit => None,
        }
    }
}

/// Error for an unparseable temperature.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected a number between 0 and 2, or `none`; got `{0}`")]
pub struct ParseTemperatureError(pub String);

impl FromStr for Temperature {
    type Err = ParseTemperatureError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let s = raw.trim();
        match s.to_ascii_lowercase().as_str() {
            "none" | "omit" | "off" | "default" | "unset" => return Ok(Self::Omit),
            _ => {}
        }
        let v: f64 = s
            .parse()
            .map_err(|_| ParseTemperatureError(raw.to_string()))?;
        Self::try_from(v)
    }
}

impl TryFrom<f64> for Temperature {
    type Error = ParseTemperatureError;

    fn try_from(v: f64) -> Result<Self, Self::Error> {
        if v.is_finite() && (0.0..=2.0).contains(&v) {
            Ok(Self::Value(v))
        } else {
            Err(ParseTemperatureError(v.to_string()))
        }
    }
}

impl fmt::Display for Temperature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(v) => write!(f, "{v}"),
            Self::Omit => f.write_str("none"),
        }
    }
}

impl Serialize for Temperature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(v) => serializer.serialize_f64(*v),
            Self::Omit => serializer.serialize_str("none"),
        }
    }
}

impl<'de> Deserialize<'de> for Temperature {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(f64),
            Int(i64),
            Str(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Num(v) => Temperature::try_from(v).map_err(serde::de::Error::custom),
            Raw::Int(v) => Temperature::try_from(v as f64).map_err(serde::de::Error::custom),
            Raw::Str(s) => s.parse().map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbers_and_none() {
        assert_eq!("0".parse::<Temperature>().unwrap(), Temperature::Value(0.0));
        assert_eq!(
            "0.7".parse::<Temperature>().unwrap(),
            Temperature::Value(0.7)
        );
        assert_eq!("none".parse::<Temperature>().unwrap(), Temperature::Omit);
        assert_eq!("None".parse::<Temperature>().unwrap(), Temperature::Omit);
        assert!("hot".parse::<Temperature>().is_err());
        assert!("3".parse::<Temperature>().is_err());
        assert!("-1".parse::<Temperature>().is_err());
    }

    #[test]
    fn toml_accepts_number_or_none_string() {
        #[derive(Deserialize)]
        struct W {
            t: Temperature,
        }
        let w: W = toml::from_str("t = 0.2").unwrap();
        assert_eq!(w.t, Temperature::Value(0.2));
        let w: W = toml::from_str("t = 1").unwrap();
        assert_eq!(w.t, Temperature::Value(1.0));
        let w: W = toml::from_str("t = \"none\"").unwrap();
        assert_eq!(w.t, Temperature::Omit);
        assert!(toml::from_str::<W>("t = \"warm\"").is_err());
    }

    #[test]
    fn as_option_maps_omit_to_none() {
        assert_eq!(Temperature::Omit.as_option(), None);
        assert_eq!(Temperature::Value(0.0).as_option(), Some(0.0));
    }
}
