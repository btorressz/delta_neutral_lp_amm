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
