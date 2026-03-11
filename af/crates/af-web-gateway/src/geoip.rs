use maxminddb::Reader;
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

/// GeoIP checker using MaxMind GeoLite2-Country database.
pub struct GeoIpChecker {
    reader: Reader<Vec<u8>>,
    blocked_countries: HashSet<String>,
}

impl GeoIpChecker {
    pub fn new(mmdb_path: &Path, blocked: Vec<String>) -> Result<Self, maxminddb::MaxMindDBError> {
        let reader = Reader::open_readfile(mmdb_path)?;
        let blocked_countries = blocked.into_iter().map(|c| c.to_uppercase()).collect();
        Ok(Self {
            reader,
            blocked_countries,
        })
    }

    /// Returns `Some(country_code)` if the IP is in a blocked country, `None` if allowed.
    pub fn check(&self, ip: &IpAddr) -> Option<String> {
        if self.blocked_countries.is_empty() {
            return None;
        }
        let country: Result<maxminddb::geoip2::Country, _> = self.reader.lookup(*ip);
        match country {
            Ok(record) => {
                if let Some(c) = record.country.and_then(|c| c.iso_code) {
                    let code = c.to_uppercase();
                    if self.blocked_countries.contains(&code) {
                        return Some(code);
                    }
                }
                None
            }
            Err(_) => None, // Unknown IP — allow
        }
    }

}
