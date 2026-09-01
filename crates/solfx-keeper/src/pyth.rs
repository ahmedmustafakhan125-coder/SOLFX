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
use anyhow::{anyhow, bail, Context as _, Result};
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

/// Pyth's upgraded Hermes backend. The old `hermes.pyth.network` still resolves and still
/// works *with a key*, but Pyth recommend this one and will cut over to it.
pub const DEFAULT_HERMES_URL: &str = "https://pyth.dourolabs.app/hermes";

/// Build an HTTP client that authenticates to Hermes.
///
/// # Why this exists rather than a bare `reqwest::Client` at each call site
///
/// Pyth put Hermes behind authentication on **26 August 2026 at 16:00 UTC**. Before that the
/// public endpoint was open and four call sites each built their own client; after it, every
/// one of them returned `401 unauthorized` and the protocol could not price a single trade.
/// Centralising the client is what stops the next such change from having to be found in four
/// places — and it is why the token is a default header rather than a query parameter, so a
/// new request cannot forget it.
///
/// A missing token is **not** an error here: `hermes-beta`, a self-hosted instance and a node
/// provider may each want different (or no) credentials. The 401 that follows is clearer than
/// a launch-time refusal would be, because it names the endpoint that rejected us.
pub fn hermes_client(timeout: Duration, token: Option<&str>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().timeout(timeout);

    if let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) {
        let mut headers = reqwest::header::HeaderMap::new();
        let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .context("PYTH_API_KEY is not a valid HTTP header value")?;
        // The key is a credential; keep it out of any `Debug` rendering of the headers.
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
        builder = builder.default_headers(headers);
    }

    builder.build().context("building the Hermes HTTP client")
}

/// The Pyth `asset_type` a SolFX symbol must resolve to.
///
/// Derived from the symbol rather than configured, so the three operator binaries cannot
/// disagree about what BTC/USD is. Mirrors the asset-class split in `contracts.rs`.
#[must_use]
pub fn asset_class_for(symbol: &str) -> &'static str {
    let sym = symbol.to_uppercase();
    if sym.starts_with("XAU")
        || sym.starts_with("XAG")
        || sym.starts_with("XPT")
        || sym.starts_with("XPD")
        || sym.starts_with("XCU")
    {
        "metal"
    } else if sym.contains("OIL") {
        "commodities"
    } else if sym.starts_with("BTC") || sym.starts_with("ETH") || sym.starts_with("SOL") {
        "crypto"
    } else {
        "fx"
    }
}

/// One catalogue entry, resolved.
#[derive(Debug, Clone)]
pub struct ResolvedFeed {
    /// Lowercase hex, 64 chars.
    pub feed_id: String,
    /// The fully-qualified Pyth symbol this came from, e.g. `Crypto.BTC/USD`.
    pub pyth_symbol: String,
}

/// Resolve SolFX symbols to Pyth feed ids. **The only implementation — do not write another.**
///
/// # The bug this exists to prevent
///
/// Three binaries each had their own copy of this. One was fixed; the others were not, and
/// the poster went on resolving BTC/USD to `FundingRate.Deribit.8h.BTC/USD` — an 8-hour
/// funding rate near 0.0001 where BTC is near 100,000 — while `init-protocol` had already
/// been corrected to `Crypto.BTC/USD`. Two tools, two answers, same market.
///
/// # How a match is decided
///
/// Two rules, both from Pyth's own documentation:
///
/// 1. **Filter server-side by `asset_type`.** `/v2/price_feeds?asset_type=crypto` keeps the
///    decoy families (funding rates, indices, NAVs, redemption rates, equities) out of the
///    candidate set entirely.
/// 2. **Match the fully-qualified symbol, in its two-segment `AssetClass.PAIR` form.** Pyth
///    state that symbols must carry the asset-type prefix (`Crypto.BTC/USD`, not `BTC/USD`)
///    and warn that the short display name "is not guaranteed to be unique". Requiring
///    exactly two segments admits spot and rejects `FX.Index.EUR/USD`, `Equity.US.FBTC/USD`
///    and every `FundingRate.*` form, all of which share the trailing pair.
///
/// Two surviving candidates is an ambiguity to report, never to resolve by iteration order.
pub async fn resolve_feed_ids(
    hermes: &str,
    token: Option<&str>,
    wanted: &[&str],
) -> Result<std::collections::HashMap<String, ResolvedFeed>> {
    #[derive(serde::Deserialize)]
    struct Entry {
        id: String,
        attributes: Attrs,
    }
    #[derive(serde::Deserialize)]
    struct Attrs {
        #[serde(default)]
        symbol: Option<String>,
        #[serde(default)]
        asset_type: Option<String>,
    }

    let http = hermes_client(Duration::from_secs(30), token)?;
    let base = hermes.trim_end_matches('/');

    let mut classes: Vec<&'static str> = wanted.iter().map(|s| asset_class_for(s)).collect();
    classes.sort_unstable();
    classes.dedup();

    let mut catalogue: Vec<Entry> = Vec::new();
    for class in classes {
        let page: Vec<Entry> = http
            .get(format!("{base}/v2/price_feeds?asset_type={class}"))
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("Hermes rejected the {class} catalogue"))?
            .json()
            .await
            .with_context(|| format!("parsing the {class} catalogue"))?;
        catalogue.extend(page);
    }

    let want: std::collections::HashMap<String, (&str, &'static str)> = wanted
        .iter()
        .map(|s| (s.to_uppercase(), (*s, asset_class_for(s))))
        .collect();

    let mut hits: std::collections::HashMap<String, Vec<ResolvedFeed>> =
        std::collections::HashMap::new();

    for entry in catalogue {
        let asset_type = entry.attributes.asset_type.clone().unwrap_or_default();
        let Some(pyth_symbol) = entry.attributes.symbol.clone() else {
            continue;
        };
        let mut parts = pyth_symbol.split('.');
        let (Some(_class), Some(pair), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Some((original, expected)) = want.get(&pair.to_uppercase()) else {
            continue;
        };
        if !asset_type.eq_ignore_ascii_case(expected) {
            continue;
        }
        let bucket = hits.entry((*original).to_string()).or_default();
        if !bucket.iter().any(|f| f.feed_id == entry.id) {
            bucket.push(ResolvedFeed {
                feed_id: entry.id.clone(),
                pyth_symbol: pyth_symbol.clone(),
            });
        }
    }

    let mut out = std::collections::HashMap::new();
    for (symbol, mut candidates) in hits {
        if candidates.len() > 1 {
            let listed = candidates
                .iter()
                .map(|f| format!("{} ({})", f.pyth_symbol, f.feed_id))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "{symbol} matches {} feeds of asset type {}: {listed}. Refusing to guess — \
                 the wrong feed would price the market against the wrong instrument.",
                candidates.len(),
                asset_class_for(&symbol)
            );
        }
        if let Some(found) = candidates.pop() {
            out.insert(symbol, found);
        }
    }
    Ok(out)
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
    pub fn new(base: &str, token: Option<&str>) -> Result<Self> {
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            http: hermes_client(Duration::from_secs(5), token)?,
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

        let response = self.http.get(&url).send().await.context("hermes request")?;

        // A 403 here is almost never a broken client or a bad key: Hermes entitles keys
        // *per feed*, so a key that serves every listed market still refuses the feeds it
        // does not cover — measured on this cluster, where 6 of 8 feeds return 200 and the
        // two belonging to halted markets return 403 on the same key in the same second.
        // Reporting that as "hermes unreachable" sends an operator hunting a network fault
        // that does not exist, so say which feeds and why.
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            bail!(
                "hermes refused {} feed(s) with 403 — the API key is not entitled to them, \
                 not a connectivity problem. Feeds: {}",
                feed_ids.len(),
                feed_ids
                    .iter()
                    .map(|id| feed_hex(id))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        let body: HermesLatest = response
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
