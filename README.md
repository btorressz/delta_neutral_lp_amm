# delta_neutral_lp_amm

# Delta Neutral LP AMM 🧮⚖️

***  

### Overview 🔍
Delta Neutral LP AMM is a Solana Anchor program that implements a simple automated market maker (AMM) with liquidity provider (LP) tokens and an integrated delta-hedging subsystem (a virtual hedge manager). The program is designed to let users deposit base and quote tokens into vaults, receive LP tokens, swap between the two tokens using constant-product AMM math, and let a keepers/hedge manager rebalance a virtual short position to keep the pool delta-neutral against the base token exposure.

**  

### Key Concepts 🧠
- Pool divides assets into two vaults: base and quote. Liquidity providers deposit both tokens and mint LP shares.  
- Swaps use constant-product (x * y = k) math with a configurable fee.  
- A HedgeManager keeps a virtual short base position equal to the pool's base token balance (in fixed point) to achieve delta neutrality. Rebalance operations and funding settlement are off-chain triggered by keepers but recorded on-chain.  
- OracleState is an on-chain, admin-settable oracle stub (meant to be replaced by a real oracle provider).  
- GlobalConfig governs global limits, pausing, cooldowns, and deposit caps.

***  

### Important Constants & Math Helpers 🧮
- BPS_DENOM = 10_000 — basis points denominator used for fees and bands (100% = 10,000 bps).  
- FP_SCALE = 1_000_000_000 (1e9) — fixed-point scaling factor used for price and base position representation.  
- Helper functions included:
  - now_slot(): returns the current Clock slot (or 0 on failure).
  - checked_u128(u64) -> u128: safe cast helper.
  - isqrt_u128(n: u128) -> u128: integer square root for u128 (used for initial LP mint logic).
  - abs_i128(x: i128) -> i128: absolute value helper.
  - to_fp_u64(x: u64) -> i128: convert u64 to fixed-point (i128) using FP_SCALE.
  - fp_mul(a_fp, b_fp) / fp_div(a_fp, b_fp): fixed-point multiply and divide utilities that guard overflow and division by zero.

These helpers ensure numeric stability and consistent fixed-point arithmetic across the program. ⚠️ Overflow checks are enforced and will return program-defined errors on arithmetic failures.

***  
