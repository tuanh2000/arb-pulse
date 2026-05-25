use alloy::primitives::U256;

/// Uniswap V2-style `getAmountOut` using the pool's own fee (bps), in exact integer math.
/// `amountOut = (amountIn*γ*reserveOut) / (reserveIn*10000 + amountIn*γ)` with γ = 10000 - fee.
pub fn get_amount_out(amount_in: U256, reserve_in: U256, reserve_out: U256, fee_bps: u32) -> U256 {
    if amount_in.is_zero() || reserve_in.is_zero() || reserve_out.is_zero() {
        return U256::ZERO;
    }
    let fee_mult = U256::from(10_000u32.saturating_sub(fee_bps));
    let amount_in_with_fee = amount_in * fee_mult;
    let numerator = amount_in_with_fee * reserve_out;
    let denominator = reserve_in * U256::from(10_000u32) + amount_in_with_fee;
    if denominator.is_zero() {
        return U256::ZERO;
    }
    numerator / denominator
}

/// Fee-less virtual constant-product pool `(ei, eo)` such that the composite output
/// of a whole path is `out(x) = eo*x / (ei + x)`. Per-hop fees are folded into the reserves.
#[derive(Clone, Copy, Debug)]
pub struct VirtualPool {
    pub ei: f64,
    pub eo: f64,
}

impl VirtualPool {
    /// Initialize from the first hop. A single pool `out = γ*ro*x/(ri+γ*x)` rewrites
    /// to the fee-less form with `ei = ri/γ`, `eo = ro`.
    pub fn first(reserve_in: f64, reserve_out: f64, fee_bps: u32) -> Self {
        let gamma = gamma(fee_bps);
        Self {
            ei: reserve_in / gamma,
            eo: reserve_out,
        }
    }

    /// Extend the path with the next hop. Derived by composing `out=eo*x/(ei+x)` with the
    /// next pool's `g(y)=γ*ro*y/(ri+γ*y)`:
    ///   eo' = γ*ro*eo / (ri + γ*eo)
    ///   ei' = ri*ei   / (ri + γ*eo)
    pub fn extend(self, reserve_in: f64, reserve_out: f64, fee_bps: u32) -> Self {
        let gamma = gamma(fee_bps);
        let denom = reserve_in + gamma * self.eo;
        Self {
            eo: gamma * reserve_out * self.eo / denom,
            ei: reserve_in * self.ei / denom,
        }
    }

    /// Profit-maximizing input for repay factor `c` (c=1 for 0% loan / own capital).
    /// `x* = sqrt(ei*eo/c) - ei`; profitable iff `eo > c*ei` (i.e. x* > 0).
    pub fn optimal_input(&self, c: f64) -> Option<f64> {
        if self.eo <= c * self.ei {
            return None;
        }
        let x = (self.ei * self.eo / c).sqrt() - self.ei;
        if x > 0.0 {
            Some(x)
        } else {
            None
        }
    }
}

fn gamma(fee_bps: u32) -> f64 {
    (10_000i32 - fee_bps as i32) as f64 / 10_000.0
}

/// Approximate U256 -> f64 via its little-endian 64-bit limbs (no string alloc).
pub fn u256_to_f64(v: U256) -> f64 {
    let mut f = 0.0f64;
    let mut scale = 1.0f64;
    for &limb in v.as_limbs() {
        f += limb as f64 * scale;
        scale *= 18_446_744_073_709_551_616.0; // 2^64
    }
    f
}
