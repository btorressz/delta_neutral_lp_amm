# delta_neutral_lp_amm

# Delta Neutral LP AMM 🧮⚖️

***  

### Overview 🔍
Delta Neutral LP AMM is a Solana Anchor program that implements a simple automated market maker (AMM) with liquidity provider (LP) tokens and an integrated delta-hedging subsystem (a virtual hedge manager). The program is designed to let users deposit base and quote tokens into vaults, receive LP tokens, swap between the two tokens using constant-product AMM math, and let a keepers/hedge manager rebalance a virtual short position to keep the pool delta-neutral against the base token exposure.
