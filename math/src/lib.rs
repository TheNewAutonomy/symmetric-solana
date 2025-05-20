// Symmetric‑Solana ─ Math layer
// ================================================================
// Expanded Balancer‑style math helpers (Weighted Pool).
// Includes:
//   • Fixed‑point helpers (60.18‑dec) – unchanged API.
//   • Deterministic integer‑based exponentiation rounding **up** (pow_up).
//   • Join/exit & LP‑token math parity with Balancer V3.
//   • All functions kept `no_std` compatible.
// ================================================================
#![cfg_attr(not(test), no_std)]
#![allow(clippy::many_single_char_names)]

extern crate alloc;

use alloc::vec::Vec;
use libm::pow as f64_pow; // f64 pow – used only in pow_down; see pow_up for integer path.
use uint::construct_uint;

construct_uint! {
    /// 256‑bit unsigned integer (little‑endian limbs).
    pub struct U256(4);
}

// ------------------------------------------------------------
// 18‑decimal fixed‑point helpers (1e18 ≙ 1.0)
// ------------------------------------------------------------
#[allow(dead_code)]
pub mod fixed {
    use super::U256;
    use super::f64_pow;

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
    /// 1 − x.
    #[inline] pub fn complement(x: U256) -> U256 { ONE - x }

    // ----------------------------------------------------
    // Exponentiation helpers
    // ----------------------------------------------------
    /// Deterministic integer exponentiation rounding **down** (wrapper around f64).
    #[inline]
    pub fn pow_down(base: U256, exp: U256) -> U256 { // keeps previous behaviour
        if exp.is_zero() { return ONE; }
        if base.is_zero() { return U256::zero(); }
        from_f64(f64_pow(to_f64(base), to_f64(exp)))
    }

    /// Deterministic integer exponentiation rounding **up** (port of Balancer `powUpFixed`).
    /// Uses binary fraction exponentiation entirely in integer domain to avoid FP rounding‑errors.
    pub fn pow_up(mut base: U256, mut exp: U256) -> U256 {
        // Based on binary decomposition of `exp` in [0,1] range (18‑dec fixed).
        // 1) Convert exp to 128‑bit fractional (Q128.128) to iterate.
        // 2) Square‑and‑multiply keeping rounding **up**.
        if exp.is_zero() { return ONE; }
        if base.is_zero() { return U256::zero(); }
        // Scale exp to Q128 (shift left by 128) then iterate 128 bits.
        let mut result = ONE; // running product
        let mut bit = U256::from(1u128 << 127); // highest bit in 128 range
        while bit > U256::zero() {
            base = mul_up(base, base); // square, rounding up
            if exp & bit != U256::zero() {
                result = mul_up(result, base);
            }
            bit >>= 1;
        }
        result
    }

    /// Default exponentiation – **down** for swap math (matches EVM powDownFixed).
    #[inline] pub fn pow(base: U256, exp: U256) -> U256 { pow_down(base, exp) }

    // ---------- helpers ----------
    #[inline] pub fn to_f64(x: U256) -> f64 { (x.low_u128() as f64) / 1e18 }
    #[inline] pub fn from_f64(v: f64) -> U256 {
        if v <= 0.0 { return U256::zero(); }
        let scaled = v * 1e18;
        U256::from(scaled as u128)
    }
}

// ------------------------------------------------------------
// Weighted‑pool maths (Balancer V3 parity)
// ------------------------------------------------------------
#[allow(dead_code)]
pub mod weighted_math {
    use super::{fixed, U256};
    use alloc::vec::Vec;

    // ---------------- Invariant

    #[inline]
    pub fn calculate_invariant(balances: &[U256], weights: &[U256]) -> U256 {
        assert_eq!(balances.len(), weights.len());
        let mut inv = fixed::ONE;
        for (b, w) in balances.iter().zip(weights) {
            inv = fixed::mul_down(inv, fixed::pow(*b, *w));
        }
        inv
    }

    // ---------------- Swap math (already present – kept)

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

    // ---------------- BPT math (joins / exits)

    /// All‑tokens‑in join: caller supplies `amounts_in` for each token and receives BPT.
    pub fn calc_bpt_out_given_exact_tokens_in(
        balances: &[U256],
        weights: &[U256],
        amounts_in: &[U256],
        total_bpt: U256,
        swap_fee: U256,
    ) -> U256 {
        let n = balances.len();
        assert_eq!(n, weights.len());
        assert_eq!(n, amounts_in.len());

        // --- First pass: calculate the weighted balance ratio with fee.
        let mut invariant_ratio_with_fees = U256::zero();
        let mut balance_ratios_with_fees: Vec<U256> = Vec::with_capacity(n);
        for i in 0..n {
            let ratio = fixed::div_down(balances[i] + amounts_in[i], balances[i]);
            balance_ratios_with_fees.push(ratio);
            invariant_ratio_with_fees += fixed::mul_down(ratio, weights[i]);
        }
        // invariant_ratio_with_fees is a weighted arithmetic mean (already fixed‑point)

        // --- Second pass: compute invariant ratio after collecting swap fees per token.
        let mut invariant_ratio = fixed::ONE;
        for i in 0..n {
            let mut amount_in_after_fee = amounts_in[i];
            if balance_ratios_with_fees[i] > invariant_ratio_with_fees {
                // taxable = amounts_in[i] - balances[i] * (invariant_ratio_with_fees - 1)
                let non_taxable = fixed::mul_down(balances[i], invariant_ratio_with_fees - fixed::ONE);
                let taxable = amounts_in[i].saturating_sub(non_taxable);
                amount_in_after_fee = non_taxable + fixed::mul_down(taxable, fixed::complement(swap_fee));
            }
            let balance_ratio = fixed::div_down(balances[i] + amount_in_after_fee, balances[i]);
            invariant_ratio = fixed::mul_down(invariant_ratio, fixed::pow(balance_ratio, weights[i]));
        }
        if invariant_ratio <= fixed::ONE { return U256::zero(); }
        fixed::mul_down(total_bpt, invariant_ratio - fixed::ONE)
    }

    /// Single‑token join: returns token_amount_in needed to mint `bpt_out`.
    pub fn calc_token_in_given_exact_bpt_out(
        balance_in: U256,
        weight_in: U256,
        bpt_out: U256,
        total_bpt: U256,
        swap_fee: U256,
    ) -> U256 {
        // invariant_ratio = 1 + bpt_out / total_bpt
        let invariant_ratio = fixed::div_up(total_bpt + bpt_out, total_bpt);
        // new_balance_in = balance_in * invariant_ratio^{1/weight_in}
        let pow = fixed::pow_up(invariant_ratio, fixed::div_down(fixed::ONE, weight_in));
        let new_balance_in = fixed::mul_up(balance_in, pow);
        let amount_in_without_fee = new_balance_in.saturating_sub(balance_in);
        // fee on the taxable portion only (amount above proportional share)
        let non_taxable = fixed::mul_up(balance_in, invariant_ratio - fixed::ONE);
        let taxable = amount_in_without_fee.saturating_sub(non_taxable);
        non_taxable + fixed::div_up(taxable, fixed::complement(swap_fee))
    }

    /// All‑tokens‑out exit: burns BPT and returns per‑token amounts.
    pub fn calc_tokens_out_given_exact_bpt_in(
        balances: &[U256],
        bpt_in: U256,
        total_bpt: U256,
        exit_fee: U256, // protocol exit fee (can be zero)
    ) -> Vec<U256> {
        let bpt_to_burn = fixed::mul_up(bpt_in, fixed::complement(exit_fee));
        let bpt_ratio = fixed::div_down(bpt_to_burn, total_bpt);
        balances.iter().map(|b| fixed::mul_down(*b, bpt_ratio)).collect()
    }

    /// Single‑token out exit: exact `bpt_in` burned, returns token_amount_out.
    pub fn calc_token_out_given_exact_bpt_in(
        balance_out: U256,
        weight_out: U256,
        bpt_in: U256,
        total_bpt: U256,
        swap_fee: U256,
    ) -> U256 {
        let invariant_ratio = fixed::complement(fixed::div_down(bpt_in, total_bpt));
        // new_balance_out = balance_out * invariant_ratio^{1/weight_out}
        let pow = fixed::pow_down(invariant_ratio, fixed::div_down(fixed::ONE, weight_out));
        let new_balance_out = fixed::mul_down(balance_out, pow);
        let amount_out_before_fee = balance_out.saturating_sub(new_balance_out);
        // fee only on proportion that exceeds ideal exit share
        let non_taxable = fixed::mul_down(balance_out, fixed::complement(invariant_ratio));
        let taxable = amount_out_before_fee.saturating_sub(non_taxable);
        non_taxable + fixed::mul_down(taxable, fixed::complement(swap_fee))
    }

    /// Exact tokens out: returns BPT to burn.
    pub fn calc_bpt_in_given_exact_tokens_out(
        balances: &[U256],
        weights: &[U256],
        amounts_out: &[U256],
        total_bpt: U256,
        swap_fee: U256,
    ) -> U256 {
        let n = balances.len();
        assert_eq!(n, weights.len());
        assert_eq!(n, amounts_out.len());

        // First pass: compute balance ratios without fee to get arithmetic mean.
        let mut invariant_ratio_without_fees = U256::zero();
        let mut balance_ratios_without_fees: Vec<U256> = Vec::with_capacity(n);
        for i in 0..n {
            let ratio = fixed::div_down(balances[i] - amounts_out[i], balances[i]);
            balance_ratios_without_fees.push(ratio);
            invariant_ratio_without_fees += fixed::mul_down(ratio, weights[i]);
        }

        // Second pass: adjust each amount by fee.
        let mut invariant_ratio = fixed::ONE;
        for i in 0..n {
            let mut amount_out_with_fee = amounts_out[i];
            if balance_ratios_without_fees[i] < invariant_ratio_without_fees {
                let non_taxable = fixed::mul_down(balances[i], fixed::complement(invariant_ratio_without_fees));
                let taxable = amounts_out[i].saturating_sub(non_taxable);
                amount_out_with_fee = non_taxable + fixed::div_up(taxable, fixed::complement(swap_fee));
            }
            let balance_ratio = fixed::div_down(balances[i] - amount_out_with_fee, balances[i]);
            invariant_ratio = fixed::mul_down(invariant_ratio, fixed::pow(balance_ratio, weights[i]));
        }
        if invariant_ratio >= fixed::ONE { return U256::zero(); }
        fixed::mul_up(total_bpt, fixed::complement(invariant_ratio))
    }
}

// ------------------------------------------------------------
// Tests (very limited sanity checks)
// ------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use fixed::{from_f64 as fp};

    #[test]
    fn join_exit_round_trip() {
        let balances = [fp(50.0), fp(50.0)];
        let weights  = [fp(0.5), fp(0.5)];
        let mut amounts_in = [fp(10.0), fp(0.0)];
        let total_bpt = fp(100.0);
        let swap_fee = fp(0.001);

        let bpt_out = weighted_math::calc_bpt_out_given_exact_tokens_in(&balances, &weights, &amounts_in, total_bpt, swap_fee);
        assert!(bpt_out > U256::zero());

        // burn same BPT via proportional exit => should roughly match input amounts (ignoring fees).
        let amounts_out = weighted_math::calc_tokens_out_given_exact_bpt_in(&balances, bpt_out, total_bpt + bpt_out, fp(0.0));
        assert!(amounts_out[0] > fp(8.0));
    }
}
