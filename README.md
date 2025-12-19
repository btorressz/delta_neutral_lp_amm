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

### High-level Program Flow & Instructions 🚦

The main instructions and what they do:

- initialize_global(admin signer, params) ✅  
  Creates the global configuration PDA. Sets admin and guardian (both initially the provided admin signer), global limits (max leverage, max hedge notional in quote, rebalance cooldown, oracle staleness tolerance), and deposit caps. Emits a GlobalInitialized event.

- initialize_market(admin signer, params, base_mint, quote_mint, vault_authority PDA, vault ATAs, lp_mint, hedge_manager PDA, oracle PDA) ✅  
  Initializes a market/pool for a base/quote mint pair. Sets vault authority PDA, creates base and quote vault token accounts owned by the vault_authority, mints an LP token (mint authority = vault authority), and creates HedgeManager and OracleState PDAs. Emits MarketInitialized event.

- set_oracle_price(admin signer, price_fp, confidence_bps) 🛰️  
  Admin or guardian can set an oracle price on the OracleState PDA. Validates price > 0 and confidence within basis points. Updates last_updated_slot and emits OracleUpdated event. This is a stub for a real oracle feed (Pyth or similar should replace it in production).

- set_global_pause(admin signer, paused: bool) ⛔  
  Pause or unpause the entire protocol (global). Only admin or guardian may toggle. Emits GlobalPaused.

- set_market_pause(admin signer, paused: bool) ⛔  
  Pause or unpause a specific market. Only admin or guardian may toggle. Emits MarketPaused.

- deposit_liquidity(user signer, amount_base, amount_quote, min_lp_out) ➕  
  Transfer base and quote tokens from the user's ATAs into vaults. Computes LP mint amount:
  - If LP supply is zero (initialization), LP minted = sqrt(amount_base * amount_quote).
  - Otherwise, LP minted is min(amount_base * total_lp_supply / pre_deposit_base, amount_quote * total_lp_supply / pre_deposit_quote).
  Validates deposit caps and min LP expectation (slippage). Mints LP tokens to user via the vault authority PDA as mint authority. Emits LiquidityDeposited.

- withdraw_liquidity(user signer, lp_amount, min_base_out, min_quote_out) ➖  
  Burn user LP tokens and transfer proportional amounts of base and quote from vaults to the user. Proportions are based on total vault balances and current LP supply. Checks slippage minimums and requires lp_amount <= lp_supply. Emits LiquidityWithdrawn.

- swap_exact_in(user signer, amount_in, min_out, base_to_quote: bool) 🔁  
  Exact-input swap using constant-product curve:
  - Fee is taken as fee_bps configured in Market.
  - amount_in_after_fee = amount_in - fee.
  - new_x = x + amount_in_after_fee; new_y = k / new_x; amount_out = y - new_y.
  Transfers input tokens from user to vault and output tokens from vault to user, using vault_authority PDA for outgoing transfer. Emits SwapExecuted with fee_paid.

- rebalance_hedge(keeper signer) ⚖️  
  Keeper-triggered instruction to set the virtual hedge position equal to current base vault balance (converted to fixed point). Rebalance only allowed if:
  - market and protocol not paused,
  - hedge enabled and not paused or in liquidation,
  - oracle is available and not stale,
  - cooldown since last rebalance satisfied,
  - change in desired position exceeds a delta band threshold or a floor (0.01 base).
  Updates HedgeManager fields: hedge_position_base_fp, entry_price_fp, last_rebalance_slot. Emits HedgeRebalanced.

- settle_funding(keeper signer, funding_rate_bps: i16) 💸  
  Apply a funding rate to the hedge notional (base * oracle price) and add computed funding delta to HedgeManager.pending_funding_quote_fp. Validates oracle freshness and not in liquidation. Emits FundingSettled.

- settle_pnl_to_vaults(keeper signer) 🧾  
  Move pending funding into realized PnL accounting within HedgeManager (increment realized_pnl_quote_fp by pending, then zero pending). This records the change but does not yet move SPL tokens. Emits PnLSettled.

- enter_liquidation_mode(admin signer, enabled: bool) 🚨  
  Admin or guardian can toggle liquidation_mode on HedgeManager. When enabled, critical operations like rebalance and settle_funding are blocked. Emits LiquidationModeSet.

***  
