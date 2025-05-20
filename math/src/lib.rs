// Symmetric‑Solana ─ Math layer
// ================================================================
// Fixed‑point helpers (60.18‑dec FP) and Balancer‑style weighted‑pool
// mathematics, built for `#![no_std]` so it can run on‑chain.
// Uses the `uint` crate for 256‑bit ints and `libm` for f64 pow.
// ================================================================
#![cfg_attr(not(test), no_std)]
#![allow(clippy::many_single_char_names)]

extern crate alloc;

use libm::pow; // f64 pow from libm (works in no_std)
use uint::construct_uint;

construct_uint! {
    /// 256‑bit unsigned integer.
    pub struct U256(4);
}

// ------------------------------------------------------------
// 18‑decimal fixed‑point helpers (1e18 ≙ 1.0)
// ------------------------------------------------------------
pub mod fixed {
    use super::U256;
    use libm::pow as pow_f64;

    /// 1e18 (fixed‑point representation of 1).
    pub const ONE: U256 = U256([1_000_000_000_000_000_000u64, 0, 0, 0]);

    /// Multiply two numbers, round **down**.
    #[inline] pub fn mul_down(a: U256, b: U256) -> U256 { (a * b) / ONE }
    /// Multiply, round **up**.
    #[inline] pub fn mul_up(a: U256, b: U256) -> U256 {
        if a.is_zero() || b.is_zero() { return U256::zero(); }
        ((a * b) + ONE - U256::one()) / ONE
    }
    /// Divide, round **down**.
    #[inline] pub fn div_down(a: U256, b: U256) -> U256 { (a * ONE) / b }
    /// Divide, round **up**.
    #[inline] pub fn div_up(a: U256, b: U256) -> U256 {
        if a.is_zero() { return U256::zero(); }
        ((a * ONE) + b - U256::one()) / b
    }
    /// 1 − x.
    #[inline] pub fn complement(x: U256) -> U256 { ONE - x }

    /// Fixed‑point exponentiation using libm::pow.
    #[inline]
    pub fn pow(base: U256, exp: U256) -> U256 {
        if exp.is_zero() { return ONE; }
        if base.is_zero() { return U256::zero(); }
        let b = to_f64(base);
        let e = to_f64(exp);
        from_f64(pow_f64(b, e))
    }

    // ---------- helpers ----------
    #[inline] pub fn to_f64(x: U256) -> f64 {
        let low: u128 = x.low_u128();
        (low as f64) / 1e18
    }
    #[inline] pub fn from_f64(v: f64) -> U256 {
        if v <= 0.0 { return U256::zero(); }
        let scaled = v * 1e18;
        U256::from(scaled as u128)
    }
}

// ------------------------------------------------------------
// Weighted‑pool maths (port of Balancer V3 `WeightedMath.sol`)
// ------------------------------------------------------------
pub mod weighted_math {
    use super::{fixed, U256};

    /// Π balanceᵢ^weightᵢ (geometric mean‑like invariant).
    pub fn calculate_invariant(balances: &[U256], weights: &[U256]) -> U256 {
        assert_eq!(balances.len(), weights.len());
        let mut inv = fixed::ONE;
        for (b, w) in balances.iter().zip(weights) {
            inv = fixed::mul_down(inv, fixed::pow(*b, *w));
        }
        inv
    }

    /// Swap given exact amount in → amount out.
    pub fn calc_out_given_in(
        balance_in: U256,
        weight_in: U256,
        balance_out: U256,
        weight_out: U256,
        amount_in: U256,
        swap_fee: U256,
    ) -> U256 {
        let amount_in_after_fee = fixed::mul_down(amount_in, fixed::complement(swap_fee));
        let new_balance_in = balance_in + amount_in_after_fee;
        let base = fixed::div_down(balance_in, new_balance_in);
        let exponent = fixed::div_down(weight_in, weight_out);
        let power = fixed::pow(base, exponent);
        fixed::mul_down(balance_out, fixed::complement(power))
    }

    /// Swap for an exact amount out → required amount in.
    pub fn calc_in_given_out(
        balance_in: U256,
        weight_in: U256,
        balance_out: U256,
        weight_out: U256,
        amount_out: U256,
        swap_fee: U256,
    ) -> U256 {
        let denom = balance_out - amount_out;
        let base = fixed::div_down(balance_out, denom);
        let exponent = fixed::div_down(weight_out, weight_in);
        let power = fixed::pow(base, exponent);
        let ratio = power - fixed::ONE;
        let without_fee = fixed::mul_down(balance_in, ratio);
        fixed::div_up(without_fee, fixed::complement(swap_fee))
    }
}

// ------------------------------------------------------------
// Tests (off‑chain)
// ------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use fixed::{from_f64 as fp};

    #[test]
    fn basic_arith() {
        assert_eq!(fixed::mul_down(fp(1.5), fp(2.0)), fp(3.0));
        assert_eq!(fixed::div_down(fp(3.0), fp(2.0)), fp(1.5));
    }

    #[test]
    fn invariant_example() {
        let balances = [fp(50.0), fp(50.0)];
        let weights  = [fp(0.5), fp(0.5)];
        let inv = weighted_math::calculate_invariant(&balances, &weights);
        assert!(inv > fp(49.0) && inv < fp(51.0));
    }
}
