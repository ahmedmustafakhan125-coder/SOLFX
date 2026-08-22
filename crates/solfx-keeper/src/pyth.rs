//! Finding, reading and monitoring Pyth price accounts.
//!
//! # Why a keeper needs almost no Pyth machinery
//!
//! Pyth's pull model is usually described as "the caller posts the price". That is true of a
//! *trader* placing an order, but it is the wrong mental model for a keeper on a market with
//! sponsored feeds, and the difference matters a lot for transaction size.
//!
//! Every SolFX feed is a Pyth **price feed account**: a PDA of the push-oracle program, kept
//! current by Pyth's own publishers, that anyone may read. So the keeper's transaction only
//! has to *reference* the account, not carry a signed update into it. That keeps a
//! liquidation to one instruction well inside the 1232-byte packet, which is the whole reason
//! sub-2s liquidations are achievable at all: a transaction that needs a Wormhole VAA plus
//! guardian signatures does not fit alongside the seventeen accounts a liquidation already
//! takes, and would have to be split across two transactions with a state account between
//! them.
//!
//! The keeper still watches Hermes, but for a different purpose than posting: to notice when
//! the on-chain account has fallen behind the real market. That is the operator's early
//! warning that the sponsored publisher has stopped — the condition under which the protocol
//! correctly refuses to trade (correction C-1) and under which an operator needs to know
//! *before* the support tickets arrive.

use std::time::Duration;

use anchor_lang::AccountDeserialize;
use anyhow::{anyhow, Context as _, Result};
use pyth_solana_receiver_sdk::price_update::PriceUpdateV2;
use solana_pubkey::Pubkey;

/// Pyth's push-oracle program, which owns the sponsored price feed account PDAs.
pub const PYTH_PUSH_ORACLE: Pubkey =
    solana_pubkey::pubkey!("pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT");

/// Shard 0 is the default set of sponsored feeds. Shards exist so heavily-read feeds can be
/// split across accounts to avoid write-lock contention; we have no reason to leave shard 0.
pub const DEFAULT_SHARD: u16 = 0;

/// Where a feed's price lives on chain.
///
/// Derived rather than configured, so adding a market never means also editing a keeper's
/// address book — the market account already carries the feed id, and the address follows
/// from it.
#[must_use]
pub fn price_account(feed_id: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(&[&DEFAULT_SHARD.to_le_bytes(), feed_id], &PYTH_PUSH_ORACLE).0
}

/// A feed id of all zeroes means "unset" — a direct market has no secondary leg.
#[must_use]
pub fn is_set(feed_id: &[u8; 32]) -> bool {
    feed_id.iter().any(|b| *b != 0)
}

pub fn parse_update(data: &[u8]) -> Result<PriceUpdateV2> {
    PriceUpdateV2::try_deserialize(&mut &data[..]).map_err(|e| anyhow!("bad PriceUpdateV2: {e}"))
}

/// Hex feed id as Hermes wants it: 64 lowercase hex characters, no `0x`.
#[must_use]
pub fn feed_hex(feed_id: &[u8; 32]) -> String {
    hex::encode(feed_id)
}

/// A read-only Hermes client used to detect that an on-chain feed has stopped advancing.
pub struct Hermes {
    base: String,
    http: reqwest::Client,
}

#[derive(serde::Deserialize)]
struct HermesLatest {
    parsed: Option<Vec<HermesParsed>>,
}

#[derive(serde::Deserialize)]
struct HermesParsed {
    id: String,
    price: HermesPrice,
}

#[derive(serde::Deserialize)]
struct HermesPrice {
    price: String,
    conf: String,
    expo: i32,
    publish_time: i64,
}

/// One feed as Hermes currently sees it.
#[derive(Debug, Clone, Copy)]
pub struct OffChainPrice {
    pub price: i64,
    pub conf: u64,
    pub expo: i32,
    pub publish_time: i64,
}

impl OffChainPrice {
    /// Confidence as a fraction of price, in basis points — the same quantity § 7.2 gates
    /// trading on.
    ///
    /// Worth reporting alongside divergence because the two failure modes look identical
    /// from inside the protocol and are not: a widening band is the market telling the truth
    /// about its own uncertainty, whereas a diverging price is the feed being wrong. Only the
    /// first is a reason to stop trading, and an operator staring at rejected orders needs to
    /// know which one they are looking at.
    #[must_use]
    pub fn conf_bps(self) -> Option<u64> {
        let price = self.price.unsigned_abs();
        if price == 0 {
            return None;
        }
        self.conf.checked_mul(10_000)?.checked_div(price)
    }
}

impl Hermes {
    pub fn new(base: &str) -> Result<Self> {
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .context("building HTTP client")?,
        })
    }

    /// Latest price for each requested feed, keyed by lowercase hex id.
    ///
    /// Feeds Hermes does not know about are simply absent from the result rather than an
    /// error, so one retired feed cannot blind the operator to the state of the others.
    pub async fn latest(
        &self,
        feed_ids: &[[u8; 32]],
    ) -> Result<std::collections::HashMap<String, OffChainPrice>> {
        let mut out = std::collections::HashMap::new();
        if feed_ids.is_empty() {
            return Ok(out);
        }
        let query = feed_ids
            .iter()
            .map(|id| format!("ids[]={}", feed_hex(id)))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{}/v2/updates/price/latest?{query}&parsed=true", self.base);

        let body: HermesLatest = self
            .http
            .get(&url)
            .send()
            .await
            .context("hermes request")?
            .error_for_status()
            .context("hermes status")?
            .json()
            .await
            .context("hermes body")?;

        for entry in body.parsed.unwrap_or_default() {
            let (Ok(price), Ok(conf)) = (entry.price.price.parse(), entry.price.conf.parse())
            else {
                tracing::warn!(id = %entry.id, "unparseable hermes price");
                continue;
            };
            out.insert(
                entry.id.trim_start_matches("0x").to_lowercase(),
                OffChainPrice {
                    price,
                    conf,
                    expo: entry.price.expo,
                    publish_time: entry.price.publish_time,
                },
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unset feed id is all zeroes, and a market must not go looking for a price account
    /// that does not exist — a direct market has no second leg.
    #[test]
    fn an_unset_feed_is_recognised() {
        assert!(!is_set(&[0u8; 32]));
        assert!(is_set(&[0x11; 32]));
        let mut mostly_zero = [0u8; 32];
        mostly_zero[31] = 1;
        assert!(is_set(&mostly_zero), "one non-zero byte is enough");
    }

    /// The address must be derived, not configured — otherwise listing a market means also
    /// editing every keeper's address book, and a keeper that missed the edit goes quiet on
    /// exactly the market nobody is watching yet.
    #[test]
    fn a_feed_id_determines_its_price_account() {
        let a = price_account(&[0x11; 32]);
        let b = price_account(&[0x11; 32]);
        let c = price_account(&[0x22; 32]);
        assert_eq!(a, b, "derivation must be stable");
        assert_ne!(a, c, "different feeds must not collide");
    }

    #[test]
    fn feed_ids_are_hex_encoded_the_way_hermes_wants_them() {
        let hex = feed_hex(&[0xab; 32]);
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, "ab".repeat(32));
        assert!(!hex.starts_with("0x"), "hermes takes bare hex");
    }

    #[test]
    fn confidence_is_reported_as_a_fraction_of_price() {
        // 1.08543 with a 0.00014 band is ~1.3bps — a normal FX major.
        let p = OffChainPrice {
            price: 108_543,
            conf: 14,
            expo: -5,
            publish_time: 0,
        };
        assert_eq!(p.conf_bps(), Some(1));

        // A depeg: the band widens to 5% of price. § 7.2 stops trading here, and the
        // liquidation gate (correction C-4) deliberately does not.
        let wide = OffChainPrice { conf: 5_427, ..p };
        assert_eq!(wide.conf_bps(), Some(499));
    }

    #[test]
    fn a_zero_price_has_no_meaningful_confidence() {
        let p = OffChainPrice {
            price: 0,
            conf: 14,
            expo: -5,
            publish_time: 0,
        };
        assert_eq!(p.conf_bps(), None);
    }
}
