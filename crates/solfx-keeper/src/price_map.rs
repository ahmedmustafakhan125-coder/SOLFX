//! The feed-id → price-account map written by `price-poster`.
//!
//! # Why this exists at all
//!
//! Pyth documents two different shapes of on-chain price, and SolFX uses both depending on the
//! cluster:
//!
//! - A **price feed account** is a PDA of the push-oracle program, `[shard, feed_id]`, kept
//!   current by a publisher. Its address is *derived*, so a consumer needs no configuration —
//!   [`crate::pyth::price_account`] does exactly this, and it is right on mainnet.
//! - A **price update account** is an ordinary account the poster creates and writes through
//!   `post_update`. Its address is whatever the poster chose, so it cannot be derived by
//!   anyone, and there is no discovery mechanism for it.
//!
//! On devnet only 8 of 33 markets have a live sponsored feed, so `price-poster` publishes the
//! rest itself into per-feed keypair accounts — the second shape. The protocol accepts them
//! because `price_update` is a plain `Account<'info, PriceUpdateV2>` carrying no seeds
//! constraint: receiver ownership and a `feed_id` match are what is enforced, and a
//! self-posted update is still guardian-signed. We are the courier, not the source.
//!
//! The consequence is the trap this module closes: **a consumer that derives the sponsored
//! address looks in the wrong place on a self-posted cluster**, finds nothing, and concludes
//! there is no price. For the keeper that meant the liquidator, the trigger executor and the
//! funding crank were all watching accounts that do not exist — running, logging, healthy on
//! every dashboard, and doing nothing.
//!
//! So: consult the map first, derive second. A cluster with sponsored feeds needs no map and
//! behaves exactly as before; a self-posted cluster supplies one and everything resolves.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use solana_pubkey::Pubkey;

/// One row of `price-accounts.json`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Entry {
    pub symbol: String,
    #[serde(deserialize_with = "feed_id_hex")]
    pub feed_id: [u8; 32],
    #[serde(deserialize_with = "pubkey_str")]
    pub price_account: Pubkey,
}

/// Read the map, or an empty one when the file is absent.
///
/// A missing file is not an error: it is the normal state on a cluster whose feeds are
/// sponsored, where every address derives. Callers decide whether an empty map is a problem —
/// see the startup check in `main.rs`.
pub fn load(path: &Path) -> Result<Vec<Entry>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Keyed the way the keeper looks things up: it holds `Market::pyth_feed_id`, not a symbol.
#[must_use]
pub fn by_feed_id(entries: &[Entry]) -> HashMap<[u8; 32], Pubkey> {
    entries
        .iter()
        .map(|e| (e.feed_id, e.price_account))
        .collect()
}

/// Keyed the way the `trade` CLI looks things up: the user names a market.
#[must_use]
pub fn by_symbol(entries: &[Entry]) -> HashMap<String, Pubkey> {
    entries
        .iter()
        .map(|e| (e.symbol.to_uppercase(), e.price_account))
        .collect()
}

fn feed_id_hex<'de, D>(d: D) -> Result<[u8; 32], D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let text = String::deserialize(d)?;
    let bytes = hex::decode(text.trim_start_matches("0x")).map_err(serde::de::Error::custom)?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| serde::de::Error::custom("feed id must be 32 bytes"))
}

fn pubkey_str<'de, D>(d: D) -> Result<Pubkey, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let text = String::deserialize(d)?;
    text.parse().map_err(serde::de::Error::custom)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"feed_id":"a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b",
       "price_account":"1rqo3w8a8X8MkHMavtzxJARwXYUtms2FpYvNEq3MwZ2",
       "symbol":"EUR/USD"}
    ]"#;

    #[test]
    fn parses_the_shape_the_poster_writes() {
        let entries: Vec<Entry> = serde_json::from_str(SAMPLE).expect("sample parses");
        let [entry] = entries.as_slice() else {
            panic!("expected exactly one entry")
        };
        assert_eq!(entry.symbol, "EUR/USD");
        assert_eq!(entry.feed_id[0], 0xa9);
        assert_eq!(
            entry.price_account.to_string(),
            "1rqo3w8a8X8MkHMavtzxJARwXYUtms2FpYvNEq3MwZ2"
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_map_not_an_error() {
        let entries = load(Path::new("definitely-not-a-real-file.json")).expect("absent is ok");
        assert!(entries.is_empty());
    }

    /// The whole point of the module: the map's address must win over the derived one.
    #[test]
    fn the_map_overrides_the_derived_address() {
        let entries: Vec<Entry> = serde_json::from_str(SAMPLE).expect("sample parses");
        let by_feed = by_feed_id(&entries);
        let feed_id = entries[0].feed_id;
        let mapped = by_feed.get(&feed_id).copied().expect("mapped");
        assert_ne!(
            mapped,
            crate::pyth::price_account(&feed_id),
            "the self-posted account must differ from the sponsored PDA, or this module is pointless"
        );
    }
}
