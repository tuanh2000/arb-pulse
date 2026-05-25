use std::collections::HashMap;

/// Compute pool TVL in USD given on-chain reserves (u128), decimal counts,
/// and a price map (lowercase address → USD price from the token_price table).
///
/// Returns None when either token lacks a price — partial-price estimates are
/// too inaccurate (wrong pool balance, missing second side) to be useful.
pub fn compute_tvl(
    prices: &HashMap<String, f64>,
    reserve0: u128,
    reserve1: u128,
    token0_addr: &str,
    token1_addr: &str,
    dec0: u8,
    dec1: u8,
) -> Option<f64> {
    let r0 = to_float(reserve0, dec0);
    let r1 = to_float(reserve1, dec1);

    let p0 = prices.get(&token0_addr.to_lowercase()).copied()?;
    let p1 = prices.get(&token1_addr.to_lowercase()).copied()?;

    Some(r0 * p0 + r1 * p1)
}

/// Compute the spot price of the unknown token using on-chain reserves.
/// `price_known` is the USD price of the token whose reserves are `reserve_known`.
/// Returns None if either reserve is zero (empty pool).
pub fn compute_spot_price(
    reserve_known: u128,
    dec_known: u8,
    price_known: f64,
    reserve_unknown: u128,
    dec_unknown: u8,
) -> Option<f64> {
    let rk = to_float(reserve_known, dec_known);
    let ru = to_float(reserve_unknown, dec_unknown);
    if rk == 0.0 || ru == 0.0 {
        return None;
    }
    Some(price_known * rk / ru)
}

pub fn to_float(value: u128, decimals: u8) -> f64 {
    (value as f64) / 10f64.powi(decimals as i32)
}
