use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{self, Burn, Mint, MintTo, Token, TokenAccount, Transfer},
};

// Use any valid program id (this one is valid base58). Replace with yours if needed.
declare_id!("3v1bM4DLQ4VgX1wV7zvJmr41BFEdLyzoYiHjpzjBZT31");

/// -------------------------------
/// Constants / Math helpers
/// -------------------------------
const BPS_DENOM: i64 = 10_000;
const FP_SCALE: i128 = 1_000_000_000; // 1e9 fixed-point

fn now_slot() -> u64 {
    Clock::get().map(|c| c.slot).unwrap_or(0)
}

fn checked_u128(x: u64) -> u128 {
    x as u128
}

fn isqrt_u128(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) >> 1;
    while y < x {
        x = y;
        y = (x + n / x) >> 1;
    }
    x
}

fn abs_i128(x: i128) -> i128 {
    if x < 0 {
        -x
    } else {
        x
    }
}

/// Convert (u64) -> fp i128
fn to_fp_u64(x: u64) -> i128 {
    (x as i128).checked_mul(FP_SCALE).unwrap()
}

/// fixed-point multiply: (a_fp * b_fp) / FP_SCALE
fn fp_mul(a_fp: i128, b_fp: i128) -> Result<i128> {
    a_fp
        .checked_mul(b_fp)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(FP_SCALE)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))
}

/// fixed-point divide: (a_fp * FP_SCALE) / b_fp
fn fp_div(a_fp: i128, b_fp: i128) -> Result<i128> {
    require!(b_fp != 0, ErrorCode::MathOverflow);
    a_fp
        .checked_mul(FP_SCALE)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))?
        .checked_div(b_fp)
        .ok_or_else(|| error!(ErrorCode::MathOverflow))
}

/// -------------------------------
/// Program
/// -------------------------------
#[program]
pub mod delta_neutral_lp_amm {
    use super::*;

    /// Initializes global config PDA.
    pub fn initialize_global(ctx: Context<InitializeGlobal>, params: GlobalParams) -> Result<()> {
        let g = &mut ctx.accounts.global;
        g.admin = ctx.accounts.admin.key();
        g.guardian = ctx.accounts.admin.key();

        g.max_leverage_bps = params.max_leverage_bps;
        g.max_hedge_notional_quote = params.max_hedge_notional_quote;
        g.rebalance_cooldown_slots = params.rebalance_cooldown_slots;
        g.max_oracle_staleness_slots = params.max_oracle_staleness_slots;
        g.deposit_cap_base = params.deposit_cap_base;
        g.deposit_cap_quote = params.deposit_cap_quote;

        g.paused = false;
        g.bump = ctx.bumps.global;

        emit!(GlobalInitialized {
            admin: g.admin,
            guardian: g.guardian,
        });
        Ok(())
    }

    /// Initializes a market (pool) + vault authority PDA + LP mint + hedge manager + oracle state.
    pub fn initialize_market(ctx: Context<InitializeMarket>, params: MarketParams) -> Result<()> {
        let global = &ctx.accounts.global;
        require!(!global.paused, ErrorCode::Paused);

        let m = &mut ctx.accounts.market;
        m.global = global.key();
        m.base_mint = ctx.accounts.base_mint.key();
        m.quote_mint = ctx.accounts.quote_mint.key();

        m.base_vault = ctx.accounts.base_vault.key();
        m.quote_vault = ctx.accounts.quote_vault.key();

        m.lp_mint = ctx.accounts.lp_mint.key();
        m.fee_bps = params.fee_bps;
        m.protocol_fee_bps = params.protocol_fee_bps;

        m.deposit_cap_base = params.deposit_cap_base;
        m.deposit_cap_quote = params.deposit_cap_quote;

        m.hedge_enabled = true;
        m.delta_band_bps = params.delta_band_bps;
        m.target_delta_bps = 0;

        m.paused = false;
        m.bump = ctx.bumps.market;
        m.vault_authority_bump = ctx.bumps.vault_authority;

        let hedge = &mut ctx.accounts.hedge_manager;
        hedge.market = m.key();
        hedge.hedge_position_base_fp = 0;
        hedge.entry_price_fp = 0;
        hedge.pending_funding_quote_fp = 0;
        hedge.realized_pnl_quote_fp = 0;
        hedge.last_rebalance_slot = 0;
        hedge.hedge_paused = false;
        hedge.liquidation_mode = false;
        hedge.bump = ctx.bumps.hedge_manager;

        let oracle = &mut ctx.accounts.oracle;
        oracle.market = m.key();
        oracle.price_fp = 0;
        oracle.confidence_bps = 0;
        oracle.last_updated_slot = 0;
        oracle.bump = ctx.bumps.oracle;

        emit!(MarketInitialized {
            market: m.key(),
            base_mint: m.base_mint,
            quote_mint: m.quote_mint,
            lp_mint: m.lp_mint,
            base_vault: m.base_vault,
            quote_vault: m.quote_vault,
        });
        Ok(())
    }

    /// Admin sets oracle (stub). Replace later with Pyth parsing.
    pub fn set_oracle_price(
        ctx: Context<SetOraclePrice>,
        price_fp: i128,
        confidence_bps: u16,
    ) -> Result<()> {
        let global = &ctx.accounts.global;
        require!(!global.paused, ErrorCode::Paused);
        require!(
            ctx.accounts.admin.key() == global.admin || ctx.accounts.admin.key() == global.guardian,
            ErrorCode::Unauthorized
        );

        require!(price_fp > 0, ErrorCode::BadOraclePrice);
        require!((confidence_bps as i64) <= BPS_DENOM, ErrorCode::BadOracleConfidence);

        let oracle = &mut ctx.accounts.oracle;
        oracle.price_fp = price_fp;
        oracle.confidence_bps = confidence_bps;
        oracle.last_updated_slot = now_slot();

        emit!(OracleUpdated {
            market: ctx.accounts.market.key(),
            price_fp,
            confidence_bps,
            slot: oracle.last_updated_slot,
        });
        Ok(())
    }

    /// Pause/unpause global.
    pub fn set_global_pause(ctx: Context<SetGlobalPause>, paused: bool) -> Result<()> {
        let g = &mut ctx.accounts.global;
        require!(
            ctx.accounts.admin.key() == g.admin || ctx.accounts.admin.key() == g.guardian,
            ErrorCode::Unauthorized
        );
        g.paused = paused;
        emit!(GlobalPaused { paused });
        Ok(())
    }

    /// Pause/unpause market.
    pub fn set_market_pause(ctx: Context<SetMarketPause>, paused: bool) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(
            ctx.accounts.admin.key() == g.admin || ctx.accounts.admin.key() == g.guardian,
            ErrorCode::Unauthorized
        );
        let m = &mut ctx.accounts.market;
        m.paused = paused;
        emit!(MarketPaused {
            market: m.key(),
            paused
        });
        Ok(())
    }

    /// Deposit base + quote; mint LP shares.
    pub fn deposit_liquidity(
        ctx: Context<DepositLiquidity>,
        amount_base: u64,
        amount_quote: u64,
        min_lp_out: u64,
    ) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, ErrorCode::Paused);

        let m = &ctx.accounts.market;
        require!(!m.paused, ErrorCode::Paused);

        require!(amount_base > 0 && amount_quote > 0, ErrorCode::BadAmount);

        if m.deposit_cap_base > 0 {
            require!(
                ctx.accounts.base_vault.amount.saturating_add(amount_base) <= m.deposit_cap_base,
                ErrorCode::DepositCapExceeded
            );
        }
        if m.deposit_cap_quote > 0 {
            require!(
                ctx.accounts.quote_vault.amount.saturating_add(amount_quote) <= m.deposit_cap_quote,
                ErrorCode::DepositCapExceeded
            );
        }

        token::transfer(ctx.accounts.transfer_base_ctx(), amount_base)?;
        token::transfer(ctx.accounts.transfer_quote_ctx(), amount_quote)?;

        ctx.accounts.base_vault.reload()?;
        ctx.accounts.quote_vault.reload()?;

        let total_base = ctx.accounts.base_vault.amount;
        let total_quote = ctx.accounts.quote_vault.amount;
        let lp_supply = ctx.accounts.lp_mint.supply;

        let lp_out: u64 = if lp_supply == 0 {
            let prod = checked_u128(amount_base)
                .checked_mul(checked_u128(amount_quote))
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
            let root = isqrt_u128(prod);
            require!(root > 0, ErrorCode::BadAmount);
            root as u64
        } else {
            let pre_base = total_base
                .checked_sub(amount_base)
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
            let pre_quote = total_quote
                .checked_sub(amount_quote)
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
            require!(pre_base > 0 && pre_quote > 0, ErrorCode::MathOverflow);

            let lp_from_base = (checked_u128(amount_base)
                .checked_mul(checked_u128(lp_supply))
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?
                .checked_div(checked_u128(pre_base))
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?) as u64;

            let lp_from_quote = (checked_u128(amount_quote)
                .checked_mul(checked_u128(lp_supply))
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?
                .checked_div(checked_u128(pre_quote))
                .ok_or_else(|| error!(ErrorCode::MathOverflow))?) as u64;

            lp_from_base.min(lp_from_quote)
        };

        require!(lp_out >= min_lp_out, ErrorCode::SlippageExceeded);

        // PDA signer seeds (NO temporaries returned / borrowed)
        let market_key = ctx.accounts.market.key();
        let bump = ctx.accounts.market.vault_authority_bump;
        let bump_seed = [bump];
        let seeds: &[&[u8]] = &[b"vault", market_key.as_ref(), &bump_seed];

        token::mint_to(ctx.accounts.mint_lp_ctx().with_signer(&[seeds]), lp_out)?;

        emit!(LiquidityDeposited {
            market: ctx.accounts.market.key(),
            user: ctx.accounts.user.key(),
            amount_base,
            amount_quote,
            lp_out,
        });

        Ok(())
    }

    /// Burn LP shares; withdraw proportional base/quote.
    pub fn withdraw_liquidity(
        ctx: Context<WithdrawLiquidity>,
        lp_amount: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, ErrorCode::Paused);

        let m = &ctx.accounts.market;
        require!(!m.paused, ErrorCode::Paused);

        require!(lp_amount > 0, ErrorCode::BadAmount);

        let lp_supply = ctx.accounts.lp_mint.supply;
        require!(lp_supply > 0, ErrorCode::MathOverflow);
        require!(lp_amount <= lp_supply, ErrorCode::BadAmount);

        let total_base = ctx.accounts.base_vault.amount;
        let total_quote = ctx.accounts.quote_vault.amount;

        let base_out = (checked_u128(total_base)
            .checked_mul(checked_u128(lp_amount))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?
            .checked_div(checked_u128(lp_supply))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?) as u64;

        let quote_out = (checked_u128(total_quote)
            .checked_mul(checked_u128(lp_amount))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?
            .checked_div(checked_u128(lp_supply))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?) as u64;

        require!(base_out >= min_base_out, ErrorCode::SlippageExceeded);
        require!(quote_out >= min_quote_out, ErrorCode::SlippageExceeded);

        token::burn(ctx.accounts.burn_lp_ctx(), lp_amount)?;

        // PDA signer seeds (NO temporaries returned / borrowed)
        let market_key = ctx.accounts.market.key();
        let bump = ctx.accounts.market.vault_authority_bump;
        let bump_seed = [bump];
        let seeds: &[&[u8]] = &[b"vault", market_key.as_ref(), &bump_seed];

        token::transfer(
            ctx.accounts.transfer_base_out_ctx().with_signer(&[seeds]),
            base_out,
        )?;
        token::transfer(
            ctx.accounts.transfer_quote_out_ctx().with_signer(&[seeds]),
            quote_out,
        )?;

        emit!(LiquidityWithdrawn {
            market: ctx.accounts.market.key(),
            user: ctx.accounts.user.key(),
            lp_burned: lp_amount,
            base_out,
            quote_out,
        });

        Ok(())
    }

    /// Spot swap: exact input, constant-product math.
    /// base_to_quote:
    /// - true  => base_in -> quote_out
    /// - false => quote_in -> base_out
    pub fn swap_exact_in(
        ctx: Context<SwapExactIn>,
        amount_in: u64,
        min_out: u64,
        base_to_quote: bool,
    ) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, ErrorCode::Paused);
        let m = &ctx.accounts.market;
        require!(!m.paused, ErrorCode::Paused);

        require!(amount_in > 0, ErrorCode::BadAmount);

        let fee_bps = m.fee_bps as u64;
        require!((fee_bps as i64) <= BPS_DENOM, ErrorCode::BadFeeBps);

        let (x, y) = if base_to_quote {
            (ctx.accounts.base_vault.amount, ctx.accounts.quote_vault.amount)
        } else {
            (ctx.accounts.quote_vault.amount, ctx.accounts.base_vault.amount)
        };
        require!(x > 0 && y > 0, ErrorCode::EmptyPool);

        let fee = (checked_u128(amount_in)
            .checked_mul(checked_u128(fee_bps))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?
            .checked_div(checked_u128(BPS_DENOM as u64))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?) as u64;

        let amount_in_after_fee = amount_in
            .checked_sub(fee)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        require!(amount_in_after_fee > 0, ErrorCode::BadAmount);

        let k = checked_u128(x)
            .checked_mul(checked_u128(y))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

        let new_x = checked_u128(x)
            .checked_add(checked_u128(amount_in_after_fee))
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

        let new_y = k
            .checked_div(new_x)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

        let out_u128 = checked_u128(y)
            .checked_sub(new_y)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        let amount_out = out_u128 as u64;

        require!(amount_out >= min_out, ErrorCode::SlippageExceeded);
        require!(amount_out > 0, ErrorCode::BadAmount);

        if base_to_quote {
            token::transfer(ctx.accounts.transfer_user_base_in_ctx(), amount_in)?;
        } else {
            token::transfer(ctx.accounts.transfer_user_quote_in_ctx(), amount_in)?;
        }

        // PDA signer seeds (NO temporaries returned / borrowed)
        let market_key = ctx.accounts.market.key();
        let bump = ctx.accounts.market.vault_authority_bump;
        let bump_seed = [bump];
        let seeds: &[&[u8]] = &[b"vault", market_key.as_ref(), &bump_seed];

        if base_to_quote {
            token::transfer(
                ctx.accounts
                    .transfer_vault_quote_out_ctx()
                    .with_signer(&[seeds]),
                amount_out,
            )?;
        } else {
            token::transfer(
                ctx.accounts
                    .transfer_vault_base_out_ctx()
                    .with_signer(&[seeds]),
                amount_out,
            )?;
        }

        emit!(SwapExecuted {
            market: ctx.accounts.market.key(),
            user: ctx.accounts.user.key(),
            base_to_quote,
            amount_in,
            amount_out,
            fee_paid: fee,
        });

        Ok(())
    }

    /// Rebalance hedge (virtual perp): short base equal to pool base delta.
    pub fn rebalance_hedge(ctx: Context<RebalanceHedge>) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, ErrorCode::Paused);

        let m = &ctx.accounts.market;
        require!(!m.paused, ErrorCode::Paused);
        require!(m.hedge_enabled, ErrorCode::HedgeDisabled);

        let hedge = &mut ctx.accounts.hedge_manager;
        require!(!hedge.hedge_paused, ErrorCode::HedgePaused);
        require!(!hedge.liquidation_mode, ErrorCode::LiquidationMode);

        let oracle = &ctx.accounts.oracle;
        require!(oracle.price_fp > 0, ErrorCode::BadOraclePrice);

        let slot = now_slot();
        let staleness = slot.saturating_sub(oracle.last_updated_slot);
        require!(staleness <= g.max_oracle_staleness_slots, ErrorCode::OracleStale);

        if hedge.last_rebalance_slot != 0 {
            let since = slot.saturating_sub(hedge.last_rebalance_slot);
            require!(since >= g.rebalance_cooldown_slots, ErrorCode::HedgeCooldown);
        }

        let base_balance = ctx.accounts.base_vault.amount;
        let desired_short_base_fp = to_fp_u64(base_balance);

        let diff_fp = desired_short_base_fp
            .checked_sub(hedge.hedge_position_base_fp)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        let abs_diff_fp = abs_i128(diff_fp);

        let band_bps = m.delta_band_bps as i64;
        require!(
            band_bps >= 0 && band_bps <= BPS_DENOM,
            ErrorCode::BadDeltaBandBps
        );

        let threshold_fp = desired_short_base_fp
            .checked_mul(band_bps as i128)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?
            .checked_div(BPS_DENOM as i128)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

        let floor_fp: i128 = FP_SCALE / 100; // 0.01 base
        let effective_threshold_fp = threshold_fp.max(floor_fp);

        require!(
            abs_diff_fp >= effective_threshold_fp,
            ErrorCode::NoRebalanceNeeded
        );

        let notional_quote_fp = fp_mul(desired_short_base_fp, oracle.price_fp)?;
        require!(
            notional_quote_fp <= (g.max_hedge_notional_quote as i128) * FP_SCALE,
            ErrorCode::HedgeNotionalTooLarge
        );

        hedge.entry_price_fp = oracle.price_fp;
        hedge.hedge_position_base_fp = desired_short_base_fp;
        hedge.last_rebalance_slot = slot;

        emit!(HedgeRebalanced {
            market: m.key(),
            new_short_base_fp: hedge.hedge_position_base_fp,
            entry_price_fp: hedge.entry_price_fp,
            oracle_price_fp: oracle.price_fp,
            slot,
        });

        Ok(())
    }

    /// Settle funding (stub): funding_rate_bps applied to notional each call.
    pub fn settle_funding(ctx: Context<SettleFunding>, funding_rate_bps: i16) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, ErrorCode::Paused);

        let m = &ctx.accounts.market;
        require!(!m.paused, ErrorCode::Paused);

        let hedge = &mut ctx.accounts.hedge_manager;
        require!(!hedge.liquidation_mode, ErrorCode::LiquidationMode);

        let oracle = &ctx.accounts.oracle;
        require!(oracle.price_fp > 0, ErrorCode::BadOraclePrice);

        let slot = now_slot();
        let staleness = slot.saturating_sub(oracle.last_updated_slot);
        require!(staleness <= g.max_oracle_staleness_slots, ErrorCode::OracleStale);

        let notional_quote_fp = fp_mul(hedge.hedge_position_base_fp, oracle.price_fp)?;
        let funding_fp = notional_quote_fp
            .checked_mul(funding_rate_bps as i128)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?
            .checked_div(BPS_DENOM as i128)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

        hedge.pending_funding_quote_fp = hedge
            .pending_funding_quote_fp
            .checked_add(funding_fp)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;

        emit!(FundingSettled {
            market: m.key(),
            funding_rate_bps,
            funding_delta_quote_fp: funding_fp,
            pending_funding_quote_fp: hedge.pending_funding_quote_fp,
            slot,
        });

        Ok(())
    }

    /// Move pending funding into realized PnL accounting (no SPL token movement yet).
    pub fn settle_pnl_to_vaults(ctx: Context<SettlePnlToVaults>) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(!g.paused, ErrorCode::Paused);

        let m = &ctx.accounts.market;
        require!(!m.paused, ErrorCode::Paused);

        let hedge = &mut ctx.accounts.hedge_manager;

        let delta_fp = hedge.pending_funding_quote_fp;
        hedge.realized_pnl_quote_fp = hedge
            .realized_pnl_quote_fp
            .checked_add(delta_fp)
            .ok_or_else(|| error!(ErrorCode::MathOverflow))?;
        hedge.pending_funding_quote_fp = 0;

        emit!(PnLSettled {
            market: m.key(),
            realized_pnl_quote_fp: hedge.realized_pnl_quote_fp,
            applied_delta_quote_fp: delta_fp,
            slot: now_slot(),
        });

        Ok(())
    }

    /// Safety switch: enter liquidation mode (guardian/admin).
    pub fn enter_liquidation_mode(ctx: Context<EnterLiquidationMode>, enabled: bool) -> Result<()> {
        let g = &ctx.accounts.global;
        require!(
            ctx.accounts.admin.key() == g.admin || ctx.accounts.admin.key() == g.guardian,
            ErrorCode::Unauthorized
        );

        let hedge = &mut ctx.accounts.hedge_manager;
        hedge.liquidation_mode = enabled;

        emit!(LiquidationModeSet {
            market: ctx.accounts.market.key(),
            enabled,
        });
        Ok(())
    }
}

/// -------------------------------
/// Accounts / Params
/// -------------------------------
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct GlobalParams {
    pub max_leverage_bps: u16,
    pub max_hedge_notional_quote: u64, // in quote units (not fp)
    pub rebalance_cooldown_slots: u64,
    pub max_oracle_staleness_slots: u64,
    pub deposit_cap_base: u64,
    pub deposit_cap_quote: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct MarketParams {
    pub fee_bps: u16,
    pub protocol_fee_bps: u16,
    pub delta_band_bps: u16,
    pub deposit_cap_base: u64,
    pub deposit_cap_quote: u64,
}

#[account]
pub struct GlobalConfig {
    pub admin: Pubkey,
    pub guardian: Pubkey,

    pub max_leverage_bps: u16,
    pub max_hedge_notional_quote: u64,
    pub rebalance_cooldown_slots: u64,
    pub max_oracle_staleness_slots: u64,
    pub deposit_cap_base: u64,
    pub deposit_cap_quote: u64,

    pub paused: bool,
    pub bump: u8,
}

impl GlobalConfig {
    pub const LEN: usize =
        8 + // discriminator
        32 + 32 +
        2 + 8 + 8 + 8 + 8 + 8 +
        1 + 1;
}

#[account]
pub struct Market {
    pub global: Pubkey,

    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,

    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,

    pub lp_mint: Pubkey,

    pub fee_bps: u16,
    pub protocol_fee_bps: u16,

    pub deposit_cap_base: u64,
    pub deposit_cap_quote: u64,

    pub hedge_enabled: bool,
    pub delta_band_bps: u16,
    pub target_delta_bps: i16,
    pub paused: bool,

    pub bump: u8,
    pub vault_authority_bump: u8,
}

impl Market {
    pub const LEN: usize =
        8 + // discriminator
        32 + // global
        32 + 32 + // mints
        32 + 32 + // vaults
        32 + // lp mint
        2 + 2 + // fee bps
        8 + 8 + // deposit caps
        1 + // hedge_enabled
        2 + // delta_band_bps
        2 + // target_delta_bps
        1 + // paused
        1 + 1; // bumps
}

#[account]
pub struct HedgeManager {
    pub market: Pubkey,

    pub hedge_position_base_fp: i128,
    pub entry_price_fp: i128,

    pub pending_funding_quote_fp: i128,
    pub realized_pnl_quote_fp: i128,

    pub last_rebalance_slot: u64,

    pub hedge_paused: bool,
    pub liquidation_mode: bool,

    pub bump: u8,
}

impl HedgeManager {
    pub const LEN: usize =
        8 + // discriminator
        32 +
        16 + 16 +
        16 + 16 +
        8 +
        1 + 1 +
        1;
}

#[account]
pub struct OracleState {
    pub market: Pubkey,
    pub price_fp: i128,
    pub confidence_bps: u16,
    pub last_updated_slot: u64,
    pub bump: u8,
}

impl OracleState {
    pub const LEN: usize =
        8 + // discriminator
        32 +
        16 +
        2 +
        8 +
        1;
}

/// -------------------------------
/// Contexts
/// -------------------------------
#[derive(Accounts)]
pub struct InitializeGlobal<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = GlobalConfig::LEN,
        seeds = [b"global"],
        bump
    )]
    pub global: Account<'info, GlobalConfig>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitializeMarket<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    pub base_mint: Account<'info, Mint>,
    pub quote_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        space = Market::LEN,
        seeds = [b"market", base_mint.key().as_ref(), quote_mint.key().as_ref()],
        bump
    )]
    pub market: Account<'info, Market>,

    /// PDA authority that owns vault ATAs and is LP mint authority.
    #[account(seeds = [b"vault", market.key().as_ref()], bump)]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin,
        associated_token::mint = base_mint,
        associated_token::authority = vault_authority
    )]
    pub base_vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        associated_token::mint = quote_mint,
        associated_token::authority = vault_authority
    )]
    pub quote_vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        mint::decimals = 9,
        mint::authority = vault_authority,
        mint::freeze_authority = vault_authority
    )]
    pub lp_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        space = HedgeManager::LEN,
        seeds = [b"hedge", market.key().as_ref()],
        bump
    )]
    pub hedge_manager: Account<'info, HedgeManager>,

    #[account(
        init,
        payer = admin,
        space = OracleState::LEN,
        seeds = [b"oracle", market.key().as_ref()],
        bump
    )]
    pub oracle: Account<'info, OracleState>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct SetOraclePrice<'info> {
    pub admin: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(mut, seeds = [b"oracle", market.key().as_ref()], bump = oracle.bump)]
    pub oracle: Account<'info, OracleState>,
}

#[derive(Accounts)]
pub struct SetGlobalPause<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,
}

#[derive(Accounts)]
pub struct SetMarketPause<'info> {
    pub admin: Signer<'info>,
    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,
    #[account(
        mut,
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,
}

#[derive(Accounts)]
pub struct DepositLiquidity<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        seeds = [b"vault", market.key().as_ref()],
        bump = market.vault_authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(mut, address = market.base_vault)]
    pub base_vault: Account<'info, TokenAccount>,
    #[account(mut, address = market.quote_vault)]
    pub quote_vault: Account<'info, TokenAccount>,

    #[account(mut, address = market.lp_mint)]
    pub lp_mint: Account<'info, Mint>,

    #[account(mut, constraint = user_base_ata.mint == market.base_mint)]
    pub user_base_ata: Account<'info, TokenAccount>,
    #[account(mut, constraint = user_quote_ata.mint == market.quote_mint)]
    pub user_quote_ata: Account<'info, TokenAccount>,

    #[account(mut, constraint = user_lp_ata.mint == market.lp_mint)]
    pub user_lp_ata: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

impl<'info> DepositLiquidity<'info> {
    fn transfer_base_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.user_base_ata.to_account_info(),
                to: self.base_vault.to_account_info(),
                authority: self.user.to_account_info(),
            },
        )
    }
    fn transfer_quote_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.user_quote_ata.to_account_info(),
                to: self.quote_vault.to_account_info(),
                authority: self.user.to_account_info(),
            },
        )
    }
    fn mint_lp_ctx(&self) -> CpiContext<'_, '_, '_, 'info, MintTo<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            MintTo {
                mint: self.lp_mint.to_account_info(),
                to: self.user_lp_ata.to_account_info(),
                authority: self.vault_authority.to_account_info(),
            },
        )
    }
}

#[derive(Accounts)]
pub struct WithdrawLiquidity<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        seeds = [b"vault", market.key().as_ref()],
        bump = market.vault_authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(mut, address = market.base_vault)]
    pub base_vault: Account<'info, TokenAccount>,
    #[account(mut, address = market.quote_vault)]
    pub quote_vault: Account<'info, TokenAccount>,

    #[account(mut, address = market.lp_mint)]
    pub lp_mint: Account<'info, Mint>,

    #[account(mut, constraint = user_lp_ata.mint == market.lp_mint)]
    pub user_lp_ata: Account<'info, TokenAccount>,

    #[account(mut, constraint = user_base_ata.mint == market.base_mint)]
    pub user_base_ata: Account<'info, TokenAccount>,
    #[account(mut, constraint = user_quote_ata.mint == market.quote_mint)]
    pub user_quote_ata: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

impl<'info> WithdrawLiquidity<'info> {
    fn burn_lp_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Burn<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Burn {
                mint: self.lp_mint.to_account_info(),
                from: self.user_lp_ata.to_account_info(),
                authority: self.user.to_account_info(),
            },
        )
    }

    fn transfer_base_out_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.base_vault.to_account_info(),
                to: self.user_base_ata.to_account_info(),
                authority: self.vault_authority.to_account_info(),
            },
        )
    }

    fn transfer_quote_out_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.quote_vault.to_account_info(),
                to: self.user_quote_ata.to_account_info(),
                authority: self.vault_authority.to_account_info(),
            },
        )
    }
}

#[derive(Accounts)]
pub struct SwapExactIn<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(
        seeds = [b"vault", market.key().as_ref()],
        bump = market.vault_authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    #[account(mut, address = market.base_vault)]
    pub base_vault: Account<'info, TokenAccount>,
    #[account(mut, address = market.quote_vault)]
    pub quote_vault: Account<'info, TokenAccount>,

    #[account(mut, constraint = user_base_ata.mint == market.base_mint)]
    pub user_base_ata: Account<'info, TokenAccount>,
    #[account(mut, constraint = user_quote_ata.mint == market.quote_mint)]
    pub user_quote_ata: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

impl<'info> SwapExactIn<'info> {
    fn transfer_user_base_in_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.user_base_ata.to_account_info(),
                to: self.base_vault.to_account_info(),
                authority: self.user.to_account_info(),
            },
        )
    }

    fn transfer_user_quote_in_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.user_quote_ata.to_account_info(),
                to: self.quote_vault.to_account_info(),
                authority: self.user.to_account_info(),
            },
        )
    }

    fn transfer_vault_base_out_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.base_vault.to_account_info(),
                to: self.user_base_ata.to_account_info(),
                authority: self.vault_authority.to_account_info(),
            },
        )
    }

    fn transfer_vault_quote_out_ctx(&self) -> CpiContext<'_, '_, '_, 'info, Transfer<'info>> {
        CpiContext::new(
            self.token_program.to_account_info(),
            Transfer {
                from: self.quote_vault.to_account_info(),
                to: self.user_quote_ata.to_account_info(),
                authority: self.vault_authority.to_account_info(),
            },
        )
    }
}

#[derive(Accounts)]
pub struct RebalanceHedge<'info> {
    pub keeper: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(mut, address = market.base_vault)]
    pub base_vault: Account<'info, TokenAccount>,

    #[account(mut, seeds = [b"hedge", market.key().as_ref()], bump = hedge_manager.bump)]
    pub hedge_manager: Account<'info, HedgeManager>,

    #[account(seeds = [b"oracle", market.key().as_ref()], bump = oracle.bump)]
    pub oracle: Account<'info, OracleState>,
}

#[derive(Accounts)]
pub struct SettleFunding<'info> {
    pub keeper: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(mut, seeds = [b"hedge", market.key().as_ref()], bump = hedge_manager.bump)]
    pub hedge_manager: Account<'info, HedgeManager>,

    #[account(seeds = [b"oracle", market.key().as_ref()], bump = oracle.bump)]
    pub oracle: Account<'info, OracleState>,
}

#[derive(Accounts)]
pub struct SettlePnlToVaults<'info> {
    pub keeper: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(mut, seeds = [b"hedge", market.key().as_ref()], bump = hedge_manager.bump)]
    pub hedge_manager: Account<'info, HedgeManager>,
}

#[derive(Accounts)]
pub struct EnterLiquidationMode<'info> {
    pub admin: Signer<'info>,

    #[account(seeds = [b"global"], bump = global.bump)]
    pub global: Account<'info, GlobalConfig>,

    #[account(
        seeds = [b"market", market.base_mint.as_ref(), market.quote_mint.as_ref()],
        bump = market.bump
    )]
    pub market: Account<'info, Market>,

    #[account(mut, seeds = [b"hedge", market.key().as_ref()], bump = hedge_manager.bump)]
    pub hedge_manager: Account<'info, HedgeManager>,
}

/// -------------------------------
/// Events
/// -------------------------------
#[event]
pub struct GlobalInitialized {
    pub admin: Pubkey,
    pub guardian: Pubkey,
}

#[event]
pub struct GlobalPaused {
    pub paused: bool,
}

#[event]
pub struct MarketPaused {
    pub market: Pubkey,
    pub paused: bool,
}

#[event]
pub struct MarketInitialized {
    pub market: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub lp_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
}

#[event]
pub struct OracleUpdated {
    pub market: Pubkey,
    pub price_fp: i128,
    pub confidence_bps: u16,
    pub slot: u64,
}

#[event]
pub struct LiquidityDeposited {
    pub market: Pubkey,
    pub user: Pubkey,
    pub amount_base: u64,
    pub amount_quote: u64,
    pub lp_out: u64,
}

#[event]
pub struct LiquidityWithdrawn {
    pub market: Pubkey,
    pub user: Pubkey,
    pub lp_burned: u64,
    pub base_out: u64,
    pub quote_out: u64,
}

#[event]
pub struct SwapExecuted {
    pub market: Pubkey,
    pub user: Pubkey,
    pub base_to_quote: bool,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_paid: u64,
}

#[event]
pub struct HedgeRebalanced {
    pub market: Pubkey,
    pub new_short_base_fp: i128,
    pub entry_price_fp: i128,
    pub oracle_price_fp: i128,
    pub slot: u64,
}

#[event]
pub struct FundingSettled {
    pub market: Pubkey,
    pub funding_rate_bps: i16,
    pub funding_delta_quote_fp: i128,
    pub pending_funding_quote_fp: i128,
    pub slot: u64,
}

#[event]
pub struct PnLSettled {
    pub market: Pubkey,
    pub realized_pnl_quote_fp: i128,
    pub applied_delta_quote_fp: i128,
    pub slot: u64,
}

#[event]
pub struct LiquidationModeSet {
    pub market: Pubkey,
    pub enabled: bool,
}

/// -------------------------------
/// Errors
/// -------------------------------
#[error_code]
pub enum ErrorCode {
    #[msg("Unauthorized.")]
    Unauthorized,
    #[msg("Protocol or market is paused.")]
    Paused,
    #[msg("Bad amount.")]
    BadAmount,
    #[msg("Math overflow / underflow.")]
    MathOverflow,
    #[msg("Slippage exceeded.")]
    SlippageExceeded,
    #[msg("Deposit cap exceeded.")]
    DepositCapExceeded,
    #[msg("Pool is empty.")]
    EmptyPool,
    #[msg("Bad oracle price.")]
    BadOraclePrice,
    #[msg("Bad oracle confidence.")]
    BadOracleConfidence,
    #[msg("Oracle is stale.")]
    OracleStale,
    #[msg("Bad fee bps.")]
    BadFeeBps,
    #[msg("Hedge is disabled.")]
    HedgeDisabled,
    #[msg("Hedge is paused.")]
    HedgePaused,
    #[msg("Liquidation mode enabled.")]
    LiquidationMode,
    #[msg("Hedge cooldown not satisfied.")]
    HedgeCooldown,
    #[msg("No rebalance needed (within delta band).")]
    NoRebalanceNeeded,
    #[msg("Hedge notional too large.")]
    HedgeNotionalTooLarge,
    #[msg("Bad delta band bps.")]
    BadDeltaBandBps,
}
