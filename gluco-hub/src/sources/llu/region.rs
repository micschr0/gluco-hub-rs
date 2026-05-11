// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::Deserialize;

use super::error::LluError;

/// LibreLink Up regional API endpoints. LibreView publishes new regions
/// occasionally; unknown values surface as `LluError::UnknownRegion` rather
/// than a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Region {
    Ae,
    Ap,
    Au,
    Ca,
    De,
    Eu,
    Eu2,
    Fr,
    Jp,
    Us,
    La,
    Ru,
    Cn,
}

impl Region {
    /// Hostname (without scheme or path).
    pub fn host(self) -> &'static str {
        match self {
            Region::Ae => "api-ae.libreview.io",
            Region::Ap => "api-ap.libreview.io",
            Region::Au => "api-au.libreview.io",
            Region::Ca => "api-ca.libreview.io",
            Region::De => "api-de.libreview.io",
            Region::Eu => "api-eu.libreview.io",
            Region::Eu2 => "api-eu2.libreview.io",
            Region::Fr => "api-fr.libreview.io",
            Region::Jp => "api-jp.libreview.io",
            Region::Us => "api-us.libreview.io",
            Region::La => "api-la.libreview.io",
            Region::Ru => "api.libreview.ru",
            Region::Cn => "api-cn.myfreestyle.cn",
        }
    }

    /// Full HTTPS base URL ("https://{host}").
    pub fn base_url(self) -> String {
        format!("https://{}", self.host())
    }

    /// Parse the region string returned by an LLU redirect response.
    /// Case-insensitive: `"eu"` and `"EU"` both resolve.
    pub fn parse(value: &str) -> Result<Self, LluError> {
        let upper = value.to_ascii_uppercase();
        let region = match upper.as_str() {
            "AE" => Region::Ae,
            "AP" => Region::Ap,
            "AU" => Region::Au,
            "CA" => Region::Ca,
            "DE" => Region::De,
            "EU" => Region::Eu,
            "EU2" => Region::Eu2,
            "FR" => Region::Fr,
            "JP" => Region::Jp,
            "US" => Region::Us,
            "LA" => Region::La,
            "RU" => Region::Ru,
            "CN" => Region::Cn,
            other => {
                return Err(LluError::UnknownRegion {
                    value: other.to_string(),
                });
            }
        };
        Ok(region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_region_round_trips() {
        for region in [Region::Eu, Region::Us, Region::De, Region::Ru, Region::Cn] {
            assert!(region.host().starts_with("api"));
            assert!(region.base_url().starts_with("https://"));
        }
    }

    #[test]
    fn parse_handles_case() {
        assert_eq!(Region::parse("eu").unwrap(), Region::Eu);
        assert_eq!(Region::parse("EU").unwrap(), Region::Eu);
        assert_eq!(Region::parse("Eu2").unwrap(), Region::Eu2);
    }

    #[test]
    fn parse_rejects_unknown_region() {
        let err = Region::parse("MARS").unwrap_err();
        assert!(matches!(err, LluError::UnknownRegion { ref value } if value == "MARS"));
    }
}
